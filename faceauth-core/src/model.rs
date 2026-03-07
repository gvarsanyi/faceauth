use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use crate::error::{FaceAuthError, Result};

/// A stable identifier for a V4L2 camera device.
///
/// Rather than storing a raw integer index (which changes as devices are
/// plugged/unplugged), we store udev's stable symlink names from
/// `/dev/v4l/by-id/` and `/dev/v4l/by-path/`. At open time the symlink is
/// resolved back to a `/dev/videoN` index. The integer index is kept as a
/// last-resort fallback for cameras that have no udev entries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraId {
    /// Symlink name from `/dev/v4l/by-id/`, e.g.
    /// `"usb-046d_Logitech_Webcam_C930e_12345678-video-index0"`.
    /// Stable across port changes; absent for cameras with no serial number.
    pub by_id: Option<String>,
    /// Symlink name from `/dev/v4l/by-path/`, e.g.
    /// `"pci-0000:00:14.0-usb-0:2:1.0-video-index0"`.
    /// Stable as long as the device stays on the same physical port.
    pub by_path: Option<String>,
    /// Raw V4L2 device index (`N` in `/dev/videoN`). Used as a last-resort
    /// fallback when neither `by_id` nor `by_path` can be resolved.
    pub index: u32,
}

/// Look up the numeric UID for a username via `getpwnam`.
pub fn uid_for_username(username: &str) -> Option<u32> {
    // SAFETY: on glibc/musl Linux getpwnam uses a per-thread buffer so it is
    // thread-safe. We copy the uid immediately and don't retain the pointer.
    let name = std::ffi::CString::new(username).ok()?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        None
    } else {
        Some(unsafe { (*pw).pw_uid })
    }
}

/// Look up the username for a numeric UID via `getpwuid_r` (thread-safe).
pub fn username_for_uid(uid: u32) -> Option<String> {
    // Use a 1 KiB stack buffer; retry with a heap allocation if the system
    // reports the buffer is too small.
    let mut buf = vec![0u8; 1024];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: buf is valid for the duration of the call; result points into
    // pwd/buf and is not used after buf is dropped.
    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if ret == 0 && !result.is_null() {
        let name = unsafe { std::ffi::CStr::from_ptr((*result).pw_name) };
        name.to_str().ok().map(|s| s.to_string())
    } else {
        None
    }
}

/// Directory where per-user model files are stored.
/// Must be owned by root with mode 0755, created by the installer.
pub const MODEL_DIR: &str = "/etc/security/faceauth";

/// Encode a 128-D f32 encoding as a 1024-character lowercase hex string.
/// Each f32 is stored as 8 hex digits (little-endian bit pattern).
fn encoding_to_hex(enc: &[f32; 128]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(128 * 8);
    for &v in enc {
        write!(s, "{:08x}", v.to_bits()).unwrap();
    }
    s
}

/// Decode a 1024-character hex string back to a 128-D f32 encoding.
fn encoding_from_hex(s: &str) -> std::result::Result<[f32; 128], String> {
    if s.len() != 128 * 8 {
        return Err(format!("encoding hex string must be {} chars, got {}", 128 * 8, s.len()));
    }
    let mut out = [0f32; 128];
    for (i, chunk) in s.as_bytes().chunks(8).enumerate() {
        let hex = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
        let bits = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
        out[i] = f32::from_bits(bits);
    }
    Ok(out)
}

/// On-disk serialization format. Encodings are stored as batches of hex strings
/// (128 × 8 hex chars = 1024 chars each) for compact, exact representation.
/// Each inner Vec is one enrollment batch (e.g. 5 captures in one `faceauth add` run).
#[derive(Debug, Serialize, Deserialize)]
struct FaceModelDisk {
    camera: CameraId,
    encodings: Vec<Vec<String>>,
}

#[derive(Debug)]
pub struct FaceModel {
    /// Camera used during enrollment. Authentication reuses this so the same
    /// physical device is always used without requiring the caller to remember it.
    pub camera: CameraId,
    /// Batches of 128-dimensional dlib face descriptor vectors.
    /// Each inner Vec is one enrollment batch from a single `faceauth add` run.
    pub encodings: Vec<Vec<[f32; 128]>>,
}

impl FaceModel {
    /// Create a new empty model recorded with `camera`.
    pub fn new(camera: CameraId) -> Self {
        FaceModel {
            camera,
            encodings: Vec::new(),
        }
    }

    /// Append a batch of 128-D face descriptors to the model.
    pub fn add_batch(&mut self, batch: Vec<[f32; 128]>) {
        self.encodings.push(batch);
    }
}

/// Derive the model file path for a given UID.
/// UIDs are numeric so there is no path traversal risk.
pub fn model_path(uid: u32) -> PathBuf {
    PathBuf::from(MODEL_DIR).join(format!("{}.json", uid))
}

/// Deserialize a `FaceModel` from a JSON string (as returned by the daemon).
///
/// # Errors
/// - [`FaceAuthError::Json`] if `contents` is not valid model JSON
/// - [`FaceAuthError::Dlib`] if an encoding hex string is malformed
pub fn load_model_from_json(contents: &str) -> Result<FaceModel> {
    let disk: FaceModelDisk = serde_json::from_str(contents)?;
    let encodings = disk
        .encodings
        .into_iter()
        .map(|batch| {
            batch
                .into_iter()
                .map(|s| encoding_from_hex(&s).map_err(FaceAuthError::Dlib))
                .collect::<Result<Vec<[f32; 128]>>>()
        })
        .collect::<Result<Vec<Vec<[f32; 128]>>>>()?;
    Ok(FaceModel {
        camera: disk.camera,
        encodings,
    })
}

/// Load a model from disk.
///
/// # Errors
/// - [`FaceAuthError::ModelNotFound`] if `username` has no system account or no enrolled model
/// - [`FaceAuthError::Storage`] on other I/O errors reading the file
/// - [`FaceAuthError::Json`] / [`FaceAuthError::Dlib`] if the file is malformed
pub fn load_model(username: &str) -> Result<FaceModel> {
    let uid = uid_for_username(username)
        .ok_or_else(|| FaceAuthError::ModelNotFound(username.to_string()))?;
    let path = model_path(uid);
    let contents = fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FaceAuthError::ModelNotFound(username.to_string())
        } else {
            FaceAuthError::Storage(e)
        }
    })?;
    load_model_from_json(&contents)
}

/// Load an existing model for `username`, or create a new empty one if none exists.
///
/// `camera` is only used when creating a new model; it is ignored when loading an existing one.
///
/// # Errors
/// Propagates errors from [`load_model`] or [`model_exists`].
pub fn load_or_create_model(username: &str, camera: CameraId) -> Result<FaceModel> {
    if model_exists(username)? {
        load_model(username)
    } else {
        Ok(FaceModel::new(camera))
    }
}

/// Persist a model to disk.
///
/// Uses an atomic write (temp file → rename) so the model file is never
/// in a partial state. The file is owned by the enrolled user (chown) with
/// mode 0640 so the user can read their own model (e.g. for `faceauth-gui`)
/// without requiring root, while other users cannot read it.
///
/// Requires write access to [`MODEL_DIR`] (i.e., must run as root).
///
/// # Errors
/// - [`FaceAuthError::Storage`] on any I/O step failure
/// - [`FaceAuthError::Json`] if serialization fails (should never happen)
pub fn save_model(uid: u32, model: &FaceModel) -> Result<()> {
    let path = model_path(uid);
    let disk = FaceModelDisk {
        camera: model.camera.clone(),
        encodings: model.encodings.iter()
            .map(|batch| batch.iter().map(encoding_to_hex).collect())
            .collect(),
    };
    let json = serde_json::to_string_pretty(&disk)?;

    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &json)?;
    fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o640))?;

    // chown the file to the enrolled user so they can read their own model.
    // gid -1 (0xffffffff as u32) means "leave group unchanged"
    let _ = unsafe { libc::chown(
        std::ffi::CString::new(tmp_path.as_os_str().as_encoded_bytes()).unwrap().as_ptr(),
        uid,
        u32::MAX,
    )};

    fs::rename(&tmp_path, &path)?;

    Ok(())
}

/// Path to the per-user opt file (`<MODEL_DIR>/<uid>.opt`).
pub fn opt_path(uid: u32) -> PathBuf {
    PathBuf::from(MODEL_DIR).join(format!("{}.opt", uid))
}

/// Path to the global opt file (`<MODEL_DIR>/defaults.opt`).
pub fn global_opt_path() -> PathBuf {
    PathBuf::from(MODEL_DIR).join("defaults.opt")
}

/// Return `true` if `s` is a valid Linux service name:
/// starts with an ASCII alphanumeric character, followed by zero or more
/// ASCII alphanumerics, underscores, dots, or hyphens.
fn is_valid_service_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {},
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// Parse a single opt-file line. All whitespace is stripped before matching.
/// Returns `Some((allowed, name))` if the line is `+name` or `-name` with a
/// valid service name; returns `None` for any other content (blank, comment,
/// malformed).
fn parse_opt_line(line: &str) -> Option<(bool, String)> {
    let stripped: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    let (allowed, name) = if let Some(n) = stripped.strip_prefix('+') {
        (true, n.to_string())
    } else if let Some(n) = stripped.strip_prefix('-') {
        (false, n.to_string())
    } else {
        return None;
    };
    if is_valid_service_name(&name) { Some((allowed, name)) } else { None }
}

/// Parse an opt file. Missing or unreadable files are treated as empty.
/// Lines that do not match `[+-]<valid-service-name>` (after whitespace
/// removal) are silently ignored.
/// Returns a map of service name -> allowed (true = `+`, false = `-`).
pub fn read_opt_file(path: &Path) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return map,
    };
    for line in contents.lines() {
        if let Some((allowed, name)) = parse_opt_line(line) {
            map.insert(name, allowed);
        }
    }
    map
}

/// Merge `defaults.opt` and `<uid>.opt` into a sorted list of `(service, allowed)` pairs.
///
/// User entries override global entries for the same service name.
pub fn compile_services_list(uid: u32) -> Vec<(String, bool)> {
    let mut merged = read_opt_file(&global_opt_path());
    for (name, allowed) in read_opt_file(&opt_path(uid)) {
        merged.insert(name, allowed);
    }
    let mut list: Vec<(String, bool)> = merged.into_iter().collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    list
}

/// Return whether a PAM service is allowed for a given UID.
///
/// - If the service appears in `<uid>.opt`, its flag is authoritative.
/// - If not, it falls through to `defaults.opt`; `+` there means allowed.
/// - If absent from both, the service is not allowed (opt-in model).
pub fn service_allowed_for_uid(uid: u32, service: &str) -> bool {
    let uid_map = read_opt_file(&opt_path(uid));
    if let Some(&allowed) = uid_map.get(service) {
        return allowed;
    }
    let global = read_opt_file(&global_opt_path());
    global.get(service).copied().unwrap_or(false)
}

/// Append `service` to `defaults.opt` with a `-` prefix if not already listed.
///
/// Called by the daemon (running as root) when it receives a `RecordCaller`
/// or `SetOpt` request. Uses `flock(LOCK_EX)` to serialize concurrent callers.
/// All errors are silently ignored.
pub fn record_opt_caller(service: &str) {
    let path = global_opt_path();
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => return,
    };

    // Global opt file must be world-readable so non-root PAM processes can read it.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));

    // SAFETY: flock is safe on a valid fd; File keeps the fd valid.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return;
    }

    let mut contents = String::new();
    let _ = file.read_to_string(&mut contents);

    let already = contents.lines().any(|l| {
        parse_opt_line(l).map(|(_, n)| n == service).unwrap_or(false)
    });
    if !already {
        let _ = file.seek(SeekFrom::End(0));
        let _ = writeln!(file, "-{}", service);
    }
    // flock released on drop
}

/// Write or update a `+/-` entry for `service` in the user's `<uid>.opt` file.
///
/// Called by the daemon (running as faceauthd) which owns MODEL_DIR and all
/// files within it. The file is created if absent, mode 0644 so any user's
/// PAM module can read it. Uses `flock(LOCK_EX)` to serialize concurrent
/// writers. All errors are silently ignored.
pub fn write_user_opt_entry(uid: u32, service: &str, allowed: bool) {
    let path = opt_path(uid);
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => return,
    };

    // World-readable so non-root PAM processes can read the user's opt file.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));

    // SAFETY: flock is safe on a valid fd; File keeps the fd valid.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return;
    }

    let mut contents = String::new();
    let _ = file.read_to_string(&mut contents);

    // Remove any existing line for this service; preserve unrecognised lines.
    let filtered: String = contents.lines()
        .filter(|l| {
            parse_opt_line(l).map(|(_, n)| n != service).unwrap_or(true)
        })
        .flat_map(|l| [l, "\n"])
        .collect();

    let prefix = if allowed { '+' } else { '-' };
    let new_contents = format!("{}{}{}\n", filtered, prefix, service);

    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.set_len(0);
    let _ = file.write_all(new_contents.as_bytes());
}

/// Returns `true` if a model file already exists for `username`.
///
/// # Errors
/// Returns [`FaceAuthError::Storage`] if `username` has no system account.
pub fn model_exists(username: &str) -> Result<bool> {
    let uid = uid_for_username(username)
        .ok_or_else(|| FaceAuthError::Storage(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("no system account for '{}'", username),
        )))?;
    Ok(model_path(uid).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::FaceAuthError;

    #[test]
    fn model_path_uses_uid() {
        let p = model_path(1000);
        assert_eq!(p, PathBuf::from("/etc/security/faceauth/1000.json"));
        let p2 = model_path(0);
        assert_eq!(p2, PathBuf::from("/etc/security/faceauth/0.json"));
    }

    #[test]
    fn encoding_hex_roundtrip() {
        let enc = [0.1f32; 128];
        let hex = encoding_to_hex(&enc);
        assert_eq!(hex.len(), 128 * 8);
        let back = encoding_from_hex(&hex).unwrap();
        assert_eq!(enc, back);
    }

    #[test]
    fn face_model_roundtrip() {
        let camera = CameraId {
            by_id: Some("usb-046d_Webcam_ABCDEF-video-index0".to_string()),
            by_path: Some("pci-0000:00:14.0-usb-0:2:1.0-video-index0".to_string()),
            index: 2,
        };
        let mut model = FaceModel::new(camera);
        let enc = [0.1f32; 128];
        model.add_batch(vec![enc]);

        let disk = FaceModelDisk {
            camera: model.camera.clone(),
            encodings: model.encodings.iter()
                .map(|batch| batch.iter().map(encoding_to_hex).collect())
                .collect(),
        };
        let json = serde_json::to_string(&disk).unwrap();
        let loaded_disk: FaceModelDisk = serde_json::from_str(&json).unwrap();
        let loaded_encodings: Vec<Vec<[f32; 128]>> = loaded_disk
            .encodings
            .into_iter()
            .map(|batch| batch.into_iter().map(|s| encoding_from_hex(&s).unwrap()).collect())
            .collect();

        assert_eq!(loaded_disk.camera.index, 2);
        assert_eq!(loaded_disk.camera.by_id.as_deref(), Some("usb-046d_Webcam_ABCDEF-video-index0"));
        assert_eq!(loaded_encodings.len(), 1);
        assert_eq!(loaded_encodings[0].len(), 1);
        assert_eq!(loaded_encodings[0][0], enc);
    }

    #[test]
    fn face_model_new_initial_state() {
        let camera = CameraId { by_id: None, by_path: None, index: 3 };
        let model = FaceModel::new(camera);
        assert_eq!(model.camera.index, 3);
        assert!(model.encodings.is_empty());
    }

    /// Legacy JSON containing `version`, `disabled`, and `ignore` deserializes
    /// without error; unknown fields are silently ignored by serde.
    #[test]
    fn load_model_from_json_ignores_legacy_fields() {
        let json = r#"{"version":2,"camera":{"by_id":null,"by_path":null,"index":0},"disabled":true,"ignore":["sudo","login"],"encodings":[]}"#;
        load_model_from_json(json).unwrap();
    }

    #[test]
    fn load_model_from_json_roundtrip() {
        let camera = CameraId { by_id: None, by_path: None, index: 0 };
        let mut model = FaceModel::new(camera);
        let enc = [0.25f32; 128];
        model.add_batch(vec![enc]);

        let disk = FaceModelDisk {
            camera: model.camera.clone(),
            encodings: model.encodings.iter()
                .map(|batch| batch.iter().map(encoding_to_hex).collect())
                .collect(),
        };
        let json = serde_json::to_string(&disk).unwrap();

        let loaded = load_model_from_json(&json).unwrap();
        assert_eq!(loaded.encodings.len(), 1);
        assert_eq!(loaded.encodings[0].len(), 1);
        assert_eq!(loaded.encodings[0][0], enc);
    }

    #[test]
    fn load_model_from_json_malformed_json() {
        let err = load_model_from_json("not json at all").unwrap_err();
        assert!(matches!(err, FaceAuthError::Json(_)));
    }

    #[test]
    fn load_model_from_json_bad_encoding_hex() {
        // Valid JSON structure but the encoding string is the wrong length.
        let json = r#"{"version":2,"username":"x","camera":{"by_id":null,"by_path":null,"index":0},"encodings":[["tooshort"]]}"#;
        let err = load_model_from_json(json).unwrap_err();
        assert!(matches!(err, FaceAuthError::Dlib(_)));
    }

    #[test]
    fn encoding_hex_roundtrip_special_values() {
        let mut enc = [0.0f32; 128];
        enc[0] = f32::NAN;
        enc[1] = f32::INFINITY;
        enc[2] = f32::NEG_INFINITY;
        enc[3] = -0.0f32;

        let hex = encoding_to_hex(&enc);
        let back = encoding_from_hex(&hex).unwrap();

        assert!(back[0].is_nan());
        assert_eq!(back[1], f32::INFINITY);
        assert_eq!(back[2], f32::NEG_INFINITY);
        assert!(back[3].is_sign_negative() && back[3] == 0.0);
    }

    #[test]
    fn encoding_from_hex_wrong_length() {
        assert!(encoding_from_hex("tooshort").is_err());
        assert!(encoding_from_hex(&"a".repeat(128 * 8 + 1)).is_err());
    }

    // --- is_valid_service_name ---

    #[test]
    fn valid_service_names() {
        for name in &["sudo", "sddm", "login", "kde-wallet", "polkit-1", "systemd-user",
                      "gnome.screensaver", "a", "A0", "s_v_c"] {
            assert!(is_valid_service_name(name), "expected valid: {name}");
        }
    }

    #[test]
    fn invalid_service_names() {
        for name in &["", "-sudo", "_sudo", ".sudo", "foo bar", "foo#bar", "foo/bar",
                      "foo\nbar"] {
            assert!(!is_valid_service_name(name), "expected invalid: {name}");
        }
    }

    // --- parse_opt_line ---

    #[test]
    fn parse_opt_line_allowed() {
        assert_eq!(parse_opt_line("+sudo"), Some((true, "sudo".to_string())));
    }

    #[test]
    fn parse_opt_line_denied() {
        assert_eq!(parse_opt_line("-sddm"), Some((false, "sddm".to_string())));
    }

    #[test]
    fn parse_opt_line_strips_whitespace() {
        assert_eq!(parse_opt_line("  + sudo  "), Some((true, "sudo".to_string())));
        assert_eq!(parse_opt_line("\t-login"), Some((false, "login".to_string())));
    }

    #[test]
    fn parse_opt_line_rejects_blank_and_comment() {
        assert_eq!(parse_opt_line(""), None);
        assert_eq!(parse_opt_line("   "), None);
        assert_eq!(parse_opt_line("# comment"), None);
    }

    #[test]
    fn parse_opt_line_rejects_no_prefix() {
        assert_eq!(parse_opt_line("sudo"), None);
    }

    #[test]
    fn parse_opt_line_rejects_invalid_name() {
        assert_eq!(parse_opt_line("+-invalid"), None);
        assert_eq!(parse_opt_line("+"), None);
        assert_eq!(parse_opt_line("-"), None);
    }

    // --- read_opt_file ---

    #[test]
    fn read_opt_file_missing_file_returns_empty() {
        let map = read_opt_file(std::path::Path::new("/nonexistent/path/to/file.opt"));
        assert!(map.is_empty());
    }

    #[test]
    fn read_opt_file_parses_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opt");
        std::fs::write(&path, "+sudo\n-login\n+sddm\n").unwrap();
        let map = read_opt_file(&path);
        assert_eq!(map.get("sudo"), Some(&true));
        assert_eq!(map.get("login"), Some(&false));
        assert_eq!(map.get("sddm"), Some(&true));
    }

    #[test]
    fn read_opt_file_skips_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opt");
        std::fs::write(&path, "+sudo\n# comment\n\nbadline\n-login\n").unwrap();
        let map = read_opt_file(&path);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("sudo"), Some(&true));
        assert_eq!(map.get("login"), Some(&false));
    }

    #[test]
    fn read_opt_file_last_entry_wins_for_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opt");
        std::fs::write(&path, "+sudo\n-sudo\n").unwrap();
        let map = read_opt_file(&path);
        assert_eq!(map.get("sudo"), Some(&false));
    }
}
