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
    /// Append a face encoding to a user's model.
    ///
    /// The encoding was captured by the client; the daemon writes it to
    /// `/etc/security/faceauth/<username>.json`.
    Enroll {
        username: String,
        camera_index: u32,
        /// 128-dimensional dlib face descriptor, flattened.
        encoding: Vec<f32>,
    },

    /// Delete a user's model file.
    Clear { username: String },

    /// Read a user's model file and return its raw JSON contents.
    ///
    /// Authorization: the caller must own the username or be root.
    /// The client deserializes the JSON locally; the daemon only reads the file.
    LoadModel { username: String },
}

/// A response from the daemon to a client.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum Response {
    Ok,
    Err { message: String },
    /// Response to `LoadModel`. Contains the raw JSON of the model file.
    Model { json: String },
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
            encoding: vec![0.1, 0.2, 0.3],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::Enroll { username, camera_index, encoding } = back else { panic!("wrong variant") };
        assert_eq!(username, "alice");
        assert_eq!(camera_index, 2);
        assert_eq!(encoding, vec![0.1f32, 0.2, 0.3]);
    }

    #[test]
    fn request_clear_roundtrip() {
        let req = Request::Clear { username: "bob".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::Clear { username } = back else { panic!("wrong variant") };
        assert_eq!(username, "bob");
    }

    #[test]
    fn request_load_model_roundtrip() {
        let req = Request::LoadModel { username: "carol".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::LoadModel { username } = back else { panic!("wrong variant") };
        assert_eq!(username, "carol");
    }

    /// The `#[serde(tag = "op")]` attribute should produce `"op": "..."` in the JSON.
    #[test]
    fn request_uses_op_tag() {
        let req = Request::Clear { username: "x".to_string() };
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
        let resp = Response::Model { json: r#"{"version":1}"#.to_string() };
        let outer = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&outer).unwrap();
        let Response::Model { json } = back else { panic!("wrong variant") };
        assert_eq!(json, r#"{"version":1}"#);
    }

    /// The `#[serde(tag = "status")]` attribute should produce `"status": "..."` in the JSON.
    #[test]
    fn response_uses_status_tag() {
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&Response::Ok).unwrap()).unwrap();
        assert_eq!(v["status"], "Ok");
    }
}
