//! Persistent state: face models and per-user service opt-outs.
//!
//! Layout under the state directory (default `/var/lib/faceauth`, owned by
//! the daemon user, mode 0700):
//! - `<uid>.enc.json`: enrolled camera and encoding batches
//! - `<uid>.optout`: PAM services the user disabled, one per line
//!
//! Files are named by numeric UID, so usernames never reach the filesystem.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::encoding::Encoding;
use crate::error::{FaceAuthError, Result};

/// Default state directory (systemd `StateDirectory=faceauth`).
pub const STATE_DIR: &str = "/var/lib/faceauth";

/// An enrolled face model.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceModel {
    /// Stable camera identifier (udev by-path/by-id name or /dev/videoN).
    pub camera: String,
    /// Enrollment batches; each holds the encodings of one enrollment run.
    pub batches: Vec<Vec<Encoding>>,
}

impl FaceModel {
    /// All encodings across batches.
    pub fn all_encodings(&self) -> Vec<Encoding> {
        self.batches.iter().flatten().copied().collect()
    }
}

/// On-disk format; encodings are stored as 1024-char hex strings (exact f32 bits).
#[derive(Serialize, Deserialize)]
struct ModelFile {
    camera: String,
    encodings: Vec<Vec<String>>,
}

fn encoding_to_hex(enc: &Encoding) -> String {
    enc.iter().map(|v| format!("{:08x}", v.to_bits())).collect()
}

fn encoding_from_hex(s: &str) -> Result<Encoding> {
    if s.len() != 128 * 8 || !s.is_ascii() {
        return Err(FaceAuthError::InvalidModel(format!("encoding must be 1024 hex chars, got {}", s.len())));
    }
    let mut out = [0f32; 128];
    for (slot, chunk) in out.iter_mut().zip(s.as_bytes().chunks(8)) {
        let hex = std::str::from_utf8(chunk).map_err(|e| FaceAuthError::InvalidModel(e.to_string()))?;
        let bits = u32::from_str_radix(hex, 16).map_err(|e| FaceAuthError::InvalidModel(e.to_string()))?;
        *slot = f32::from_bits(bits);
    }
    Ok(out)
}

/// Parse model JSON.
pub fn model_from_json(json: &str) -> Result<FaceModel> {
    let file: ModelFile = serde_json::from_str(json)?;
    let batches = file
        .encodings
        .iter()
        .map(|b| b.iter().map(|s| encoding_from_hex(s)).collect::<Result<Vec<_>>>())
        .collect::<Result<Vec<_>>>()?;
    Ok(FaceModel { camera: file.camera, batches })
}

/// Serialize a model to JSON.
pub fn model_to_json(model: &FaceModel) -> String {
    let file = ModelFile {
        camera: model.camera.clone(),
        encodings: model.batches.iter().map(|b| b.iter().map(encoding_to_hex).collect()).collect(),
    };
    serde_json::to_string_pretty(&file).expect("model serialization cannot fail")
}

/// Merge the admin allow-list with a user's opt-outs:
/// `(service, enabled)` for every allowed service, in allow-list order.
pub fn service_states(allowed: &[String], disabled: &[String]) -> Vec<(String, bool)> {
    allowed.iter().map(|s| (s.clone(), !disabled.contains(s))).collect()
}

/// Access to the state directory.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Store { dir: dir.into() }
    }

    fn model_path(&self, uid: u32) -> PathBuf {
        self.dir.join(format!("{uid}.enc.json"))
    }

    fn optout_path(&self, uid: u32) -> PathBuf {
        self.dir.join(format!("{uid}.optout"))
    }

    /// Load a user's model; [`FaceAuthError::NoModel`] if none is enrolled.
    pub fn load(&self, uid: u32) -> Result<FaceModel> {
        match fs::read_to_string(self.model_path(uid)) {
            Ok(json) => model_from_json(&json),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(FaceAuthError::NoModel),
            Err(e) => Err(e.into()),
        }
    }

    /// Save a model atomically; an empty model deletes the file.
    pub fn save(&self, uid: u32, model: &FaceModel) -> Result<()> {
        if model.batches.is_empty() {
            return self.delete(uid);
        }
        write_atomic(&self.model_path(uid), model_to_json(model).as_bytes())
    }

    /// Delete a user's model. Succeeds if none exists.
    pub fn delete(&self, uid: u32) -> Result<()> {
        remove_if_exists(&self.model_path(uid))
    }

    /// Services the user has disabled.
    pub fn disabled_services(&self, uid: u32) -> Vec<String> {
        fs::read_to_string(self.optout_path(uid))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| faceauth_common::is_valid_service_name(l))
            .map(str::to_string)
            .collect()
    }

    /// Enable or disable `service` for a user.
    pub fn set_service_enabled(&self, uid: u32, service: &str, enabled: bool) -> Result<()> {
        let mut disabled = self.disabled_services(uid);
        disabled.retain(|s| s != service);
        if !enabled {
            disabled.push(service.to_string());
        }
        if disabled.is_empty() {
            return remove_if_exists(&self.optout_path(uid));
        }
        let text: String = disabled.iter().map(|s| format!("{s}\n")).collect();
        write_atomic(&self.optout_path(uid), text.as_bytes())
    }
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Write `data` to `path` via a 0600 temp file, fsync, rename, and fsync the
/// directory so the update survives a crash.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn sample() -> FaceModel {
        FaceModel { camera: "pci-0000:00:14.0-usb-0:2:1.0-video-index0".into(), batches: vec![vec![[0.25; 128]]] }
    }

    #[test]
    fn hex_roundtrip_including_special_values() {
        let mut enc = [0.1f32; 128];
        enc[0] = f32::NAN;
        enc[1] = f32::NEG_INFINITY;
        enc[2] = -0.0;
        let back = encoding_from_hex(&encoding_to_hex(&enc)).unwrap();
        assert!(back[0].is_nan());
        assert_eq!(back[1], f32::NEG_INFINITY);
        assert!(back[2].is_sign_negative());
        assert_eq!(back[3..], enc[3..]);
    }

    #[test]
    fn bad_hex_is_invalid_model() {
        assert!(matches!(encoding_from_hex("abc"), Err(FaceAuthError::InvalidModel(_))));
        assert!(matches!(encoding_from_hex(&"z".repeat(1024)), Err(FaceAuthError::InvalidModel(_))));
    }

    #[test]
    fn json_roundtrip() {
        assert_eq!(model_from_json(&model_to_json(&sample())).unwrap(), sample());
    }

    #[test]
    fn malformed_json_is_invalid_model() {
        assert!(matches!(model_from_json("not json"), Err(FaceAuthError::InvalidModel(_))));
    }

    #[test]
    fn store_save_load_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(matches!(store.load(1000), Err(FaceAuthError::NoModel)));
        store.save(1000, &sample()).unwrap();
        assert_eq!(store.load(1000).unwrap(), sample());
        let mode = fs::metadata(dir.path().join("1000.enc.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!dir.path().join("1000.enc.tmp").exists());
        store.delete(1000).unwrap();
        store.delete(1000).unwrap();
        assert!(matches!(store.load(1000), Err(FaceAuthError::NoModel)));
    }

    #[test]
    fn saving_empty_model_deletes_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(1, &sample()).unwrap();
        store.save(1, &FaceModel { camera: "x".into(), batches: vec![] }).unwrap();
        assert!(matches!(store.load(1), Err(FaceAuthError::NoModel)));
    }

    #[test]
    fn optout_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.set_service_enabled(5, "sudo", false).unwrap();
        store.set_service_enabled(5, "polkit-1", false).unwrap();
        store.set_service_enabled(5, "sudo", false).unwrap();
        assert_eq!(store.disabled_services(5), vec!["polkit-1", "sudo"]);
        store.set_service_enabled(5, "sudo", true).unwrap();
        store.set_service_enabled(5, "polkit-1", true).unwrap();
        assert!(store.disabled_services(5).is_empty());
        assert!(!dir.path().join("5.optout").exists());
    }

    #[test]
    fn optout_ignores_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("7.optout"), "sudo\n# c\nbad/name\n\n").unwrap();
        assert_eq!(Store::new(dir.path()).disabled_services(7), vec!["sudo"]);
    }

    #[test]
    fn service_states_merge() {
        let allowed = vec!["sudo".to_string(), "polkit-1".to_string()];
        let disabled = vec!["polkit-1".to_string(), "login".to_string()];
        assert_eq!(
            service_states(&allowed, &disabled),
            vec![("sudo".to_string(), true), ("polkit-1".to_string(), false)]
        );
    }
}
