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

/// Camera descriptor returned by `ListCameras`.
#[derive(Debug, Serialize, Deserialize)]
pub struct CameraDescriptor {
    pub index: u32,
    pub name: String,
    /// Suitability score: 0 = colour webcam, 1 = possible IR, 2 = IR (recommended).
    pub suitability: u8,
}

/// A single entry in the merged service opt list.
#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceEntry {
    pub name: String,
    pub allowed: bool,
}

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

    /// Enumerate all available V4L2 camera capture devices.
    ///
    /// Returns `Response::Cameras` with an array of `CameraDescriptor`s sorted
    /// by descending suitability (most IR-like first). No authorization required.
    ListCameras,

    /// Return the merged service opt list for a user.
    ///
    /// Combines `defaults.opt` and the user's `<uid>.opt` file.
    /// Authorization: the caller must own `username` or be root.
    GetServices { username: String },

    /// Write a `+` or `-` entry for `service` in `<uid>.opt`.
    ///
    /// `allowed: true` writes `+service`; `false` writes `-service`.
    /// Authorization: the caller must own `username` or be root.
    SetOpt {
        username: String,
        service: String,
        allowed: bool,
    },

    /// Check whether `service` is opted in for `username`.
    ///
    /// Returns `Response::Ok` if the service is opted in (`+` entry in the
    /// user's or global opt file), or `Response::Err` if it is not opted in
    /// or the user is unknown. The daemon reads the opt files as root, so this
    /// works regardless of the calling process's privileges.
    /// No user authorization required.
    CheckService {
        username: String,
        service: String,
    },

    /// Record that `service` has invoked face authentication.
    ///
    /// Called by the PAM module on every authentication attempt. The daemon
    /// writes `-service` to the global `.opt` if the service is not already
    /// listed, so it appears in every user's `faceauth services` output.
    ///
    /// The daemon verifies that `service` corresponds to a real PAM service
    /// file (`/etc/pam.d/<service>` or `/usr/lib/pam.d/<service>`).
    /// No user authorization required; the caller UID is obtained via
    /// `SO_PEERCRED` but not used for access control here.
    RecordCaller { service: String },
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
    /// Response to `ListCameras`. Contains all available capture devices.
    Cameras { cameras: Vec<CameraDescriptor> },
    /// Response to `GetServices`. Contains the merged opt list for a user.
    Services { services: Vec<ServiceEntry> },
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

    #[test]
    fn request_list_cameras_roundtrip() {
        let json = serde_json::to_string(&Request::ListCameras).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["op"], "ListCameras");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::ListCameras));
    }

    #[test]
    fn request_get_services_roundtrip() {
        let req = Request::GetServices { username: "alice".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::GetServices { username } = back else { panic!("wrong variant") };
        assert_eq!(username, "alice");
    }

    #[test]
    fn request_set_opt_roundtrip() {
        let req = Request::SetOpt {
            username: "bob".to_string(),
            service: "sudo".to_string(),
            allowed: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["op"], "SetOpt");
        assert_eq!(v["allowed"], false);
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::SetOpt { username, service, allowed } = back else { panic!("wrong variant") };
        assert_eq!(username, "bob");
        assert_eq!(service, "sudo");
        assert!(!allowed);
    }

    #[test]
    fn request_record_caller_roundtrip() {
        let req = Request::RecordCaller { service: "login".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::RecordCaller { service } = back else { panic!("wrong variant") };
        assert_eq!(service, "login");
    }

    #[test]
    fn response_cameras_roundtrip() {
        let resp = Response::Cameras {
            cameras: vec![
                CameraDescriptor { index: 0, name: "FaceTime HD".to_string(), suitability: 2 },
                CameraDescriptor { index: 1, name: "USB Cam".to_string(), suitability: 0 },
            ],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        let Response::Cameras { cameras } = back else { panic!("wrong variant") };
        assert_eq!(cameras.len(), 2);
        assert_eq!(cameras[0].index, 0);
        assert_eq!(cameras[0].suitability, 2);
        assert_eq!(cameras[1].name, "USB Cam");
    }

    #[test]
    fn response_services_roundtrip() {
        let resp = Response::Services {
            services: vec![
                ServiceEntry { name: "sudo".to_string(), allowed: false },
                ServiceEntry { name: "sddm".to_string(), allowed: true },
            ],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        let Response::Services { services } = back else { panic!("wrong variant") };
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].name, "sudo");
        assert!(!services[0].allowed);
        assert!(services[1].allowed);
    }
}
