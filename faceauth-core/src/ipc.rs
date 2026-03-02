//! Wire protocol for the faceauth daemon Unix socket.
//!
//! Requests and responses are serialized as newline-delimited JSON
//! (one JSON object per line). The client sends one request and reads
//! one response, then closes the connection.
//!
//! The socket is at `/run/faceauth/faceauth.sock`. The daemon uses
//! `SO_PEERCRED` to obtain the connecting process's UID from the kernel;
//! clients do not need to authenticate themselves.

use serde::{Deserialize, Serialize};

/// Path to the daemon's Unix domain socket.
pub const SOCKET_PATH: &str = "/run/faceauth/faceauth.sock";

/// A request from a client to the daemon.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    /// Append an enrollment batch to a user's model.
    ///
    /// `encodings` is a batch of 128-dimensional dlib face descriptors captured
    /// in a single `faceauth add` run. The daemon appends it as one entry to
    /// `/etc/security/faceauth/<uid>.json`.
    Enroll {
        username: String,
        camera_index: u32,
        /// Batch of 128-dimensional dlib face descriptors, each flattened to Vec<f32>.
        encodings: Vec<Vec<f32>>,
    },

    /// Delete a user's model, or remove a single batch by index.
    ///
    /// `index: None` removes the entire model file.
    /// `index: Some(i)` removes only batch `i` (0-based); the model file is
    /// deleted if no batches remain.
    Clear { username: String, index: Option<usize> },

    /// Read a user's model file and return its raw JSON contents.
    ///
    /// Authorization: the caller must own the username or be root.
    /// The client deserializes the JSON locally; the daemon only reads the file.
    LoadModel { username: String },

    /// Authenticate `username` using the enrolled face model.
    ///
    /// The daemon opens the camera, captures frames for up to `timeout_secs`
    /// seconds, and checks each frame against the enrolled encodings.
    /// Authorization: the caller must own `username` or be root.
    ///
    /// Returns `Response::Ok` on success, or `Response::Err` on failure
    /// (timeout, no model enrolled, camera error, etc.).
    Authenticate {
        username: String,
        timeout_secs: u64,
    },

    /// Capture a single face encoding from `camera_index`.
    ///
    /// The daemon opens the camera, waits until exactly one face is visible,
    /// and returns the 128-D descriptor as `Response::Encoding`. Any local
    /// user may call this; no username authorisation is required.
    CaptureEncoding {
        camera_index: u32,
        timeout_secs: u64,
    },

    /// Update per-user authentication configuration.
    ///
    /// All fields except `username` are optional; only supplied fields are
    /// changed. Fields omitted from the request are left at their current value.
    ///
    /// - `disabled`: `Some(true)` to disable face auth globally for this user;
    ///   `Some(false)` to re-enable it.
    /// - `ignore_add`: add one service/caller name to the per-user ignore list.
    /// - `ignore_remove`: remove one name from the ignore list (no-op if absent).
    SetConfig {
        username: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ignore_add: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ignore_remove: Option<String>,
    },
}

/// A response from the daemon to a client.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum Response {
    Ok,
    Err { message: String },
    /// Response to `LoadModel`. Contains the raw JSON of the model file.
    Model { json: String },
    /// Response to `CaptureEncoding`. Contains the 128-D face descriptor.
    Encoding { data: Vec<f32> },
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Request round-trips ---

    #[test]
    fn request_enroll_roundtrip() {
        let req = Request::Enroll {
            username: "alice".to_string(),
            camera_index: 2,
            encodings: vec![vec![0.1, 0.2, 0.3]],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::Enroll { username, camera_index, encodings } = back else { panic!("wrong variant") };
        assert_eq!(username, "alice");
        assert_eq!(camera_index, 2);
        assert_eq!(encodings, vec![vec![0.1f32, 0.2, 0.3]]);
    }

    #[test]
    fn request_clear_roundtrip() {
        let req = Request::Clear { username: "bob".to_string(), index: None };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::Clear { username, index } = back else { panic!("wrong variant") };
        assert_eq!(username, "bob");
        assert_eq!(index, None);
    }

    #[test]
    fn request_clear_with_index_roundtrip() {
        let req = Request::Clear { username: "bob".to_string(), index: Some(2) };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::Clear { username, index } = back else { panic!("wrong variant") };
        assert_eq!(username, "bob");
        assert_eq!(index, Some(2));
    }

    #[test]
    fn request_load_model_roundtrip() {
        let req = Request::LoadModel { username: "carol".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::LoadModel { username } = back else { panic!("wrong variant") };
        assert_eq!(username, "carol");
    }

    #[test]
    fn request_authenticate_roundtrip() {
        let req = Request::Authenticate { username: "dave".to_string(), timeout_secs: 10 };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::Authenticate { username, timeout_secs } = back else { panic!("wrong variant") };
        assert_eq!(username, "dave");
        assert_eq!(timeout_secs, 10);
    }

    #[test]
    fn request_authenticate_uses_op_tag() {
        let req = Request::Authenticate { username: "dave".to_string(), timeout_secs: 3 };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(v["op"], "Authenticate");
        assert_eq!(v["username"], "dave");
        assert_eq!(v["timeout_secs"], 3);
    }

    /// The `#[serde(tag = "op")]` attribute should produce `"op": "..."` in the JSON.
    #[test]
    fn request_uses_op_tag() {
        let req = Request::Clear { username: "x".to_string(), index: None };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(v["op"], "Clear");
        assert_eq!(v["username"], "x");
    }

    #[test]
    fn request_set_config_roundtrip() {
        let req = Request::SetConfig {
            username: "eve".to_string(),
            disabled: Some(true),
            ignore_add: Some("sudo".to_string()),
            ignore_remove: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["op"], "SetConfig");
        assert_eq!(v["username"], "eve");
        assert_eq!(v["disabled"], true);
        assert_eq!(v["ignore_add"], "sudo");
        // skip_serializing_if = None means ignore_remove is absent from JSON
        assert!(v.get("ignore_remove").is_none() || v["ignore_remove"].is_null());

        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::SetConfig { username, disabled, ignore_add, ignore_remove } = back
            else { panic!("wrong variant") };
        assert_eq!(username, "eve");
        assert_eq!(disabled, Some(true));
        assert_eq!(ignore_add, Some("sudo".to_string()));
        assert_eq!(ignore_remove, None);
    }

    // --- Response round-trips ---

    #[test]
    fn response_ok_roundtrip() {
        let json = serde_json::to_string(&Response::Ok).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Response::Ok));
    }

    #[test]
    fn response_err_roundtrip() {
        let resp = Response::Err { message: "something went wrong".to_string() };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        let Response::Err { message } = back else { panic!("wrong variant") };
        assert_eq!(message, "something went wrong");
    }

    #[test]
    fn response_model_roundtrip() {
        let resp = Response::Model { json: r#"{"version":2}"#.to_string() };
        let outer = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&outer).unwrap();
        let Response::Model { json } = back else { panic!("wrong variant") };
        assert_eq!(json, r#"{"version":2}"#);
    }

    #[test]
    fn request_capture_encoding_roundtrip() {
        let req = Request::CaptureEncoding { camera_index: 1, timeout_secs: 20 };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::CaptureEncoding { camera_index, timeout_secs } = back else { panic!("wrong variant") };
        assert_eq!(camera_index, 1);
        assert_eq!(timeout_secs, 20);
    }

    #[test]
    fn response_encoding_roundtrip() {
        let data: Vec<f32> = (0..128).map(|i| i as f32 * 0.01).collect();
        let resp = Response::Encoding { data: data.clone() };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        let Response::Encoding { data: back_data } = back else { panic!("wrong variant") };
        assert_eq!(back_data.len(), 128);
        assert!((back_data[1] - 0.01f32).abs() < 1e-6);
    }

    /// The `#[serde(tag = "status")]` attribute should produce `"status": "..."` in the JSON.
    #[test]
    fn response_uses_status_tag() {
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&Response::Ok).unwrap()).unwrap();
        assert_eq!(v["status"], "Ok");
    }
}
