pub mod camera;
pub mod encoding;
pub mod error;
pub mod ipc;
pub mod model;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub use camera::{camera_id_for_index, camera_name_for_index, list_cameras, CameraInfo};
use camera::{capture_frame, open_camera};
use encoding::FaceEncoder;
use error::{FaceAuthError, Result};
pub use encoding::DISTANCE_THRESHOLD;
pub use model::{load_model_from_json, uid_for_username, username_for_uid};
use model::{load_or_create_model, save_model, FaceModel};

/// Euclidean distance between two 128-D face encodings.
pub fn encoding_distance(a: &[f32; 128], b: &[f32; 128]) -> f64 {
    FaceEncoder::distance(a, b)
}

/// Configuration for the authentication capture loop.
pub struct AuthConfig {
    /// Maximum time to spend attempting a match before giving up.
    pub timeout: Duration,
    /// Time to sleep between frame captures. Reduces CPU load.
    pub frame_interval: Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            timeout: Duration::from_secs(5),
            frame_interval: Duration::from_millis(100),
        }
    }
}

/// Frame interval used in the enrollment capture loop.
const ENROLL_FRAME_INTERVAL: Duration = Duration::from_millis(100);

/// Capture a single face encoding from the camera.
///
/// Opens the camera and loops until a frame containing exactly one face
/// is captured, or until `timeout` elapses. Returns the 128-D encoding
/// and the resolved `CameraId`.
///
/// This is the capture half of enrollment, separated so the CLI can
/// capture locally and hand the encoding to the daemon for storage.
///
/// # Errors
/// - [`FaceAuthError::Timeout`] if no single face is detected within `timeout`
/// - [`FaceAuthError::Camera`] if the camera cannot be opened or read
/// - [`FaceAuthError::Dlib`] if the face encoder fails to initialise
pub fn capture_face_encoding(
    timeout: Duration,
    camera_index: u32,
) -> Result<([f32; 128], model::CameraId)> {
    let encoder = FaceEncoder::new()?;

    let camera_id = camera_id_for_index(camera_index);
    let mut camera = open_camera(&camera_id)?;
    camera
        .open_stream()
        .map_err(|e| FaceAuthError::Camera(e.to_string()))?;

    let deadline = Instant::now() + timeout;

    let encoding = loop {
        if Instant::now() > deadline {
            let _ = camera.stop_stream();
            return Err(FaceAuthError::Timeout(timeout.as_secs()));
        }

        let frame = match capture_frame(&mut camera) {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(ENROLL_FRAME_INTERVAL);
                continue;
            }
        };

        let faces = match encoder.encode_faces(&frame.data, frame.width, frame.height) {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(ENROLL_FRAME_INTERVAL);
                continue;
            }
        };

        match faces.len() {
            0 => {
                std::thread::sleep(ENROLL_FRAME_INTERVAL);
                continue;
            }
            1 => {
                break faces.into_iter().next().unwrap();
            }
            _ => {
                eprintln!(
                    "faceauth: multiple faces detected ({}) — \
                     please ensure only one face is visible",
                    faces.len()
                );
                std::thread::sleep(ENROLL_FRAME_INTERVAL);
                continue;
            }
        }
    };

    let _ = camera.stop_stream();
    Ok((encoding, camera_id))
}

/// Enroll a new face for `username`.
///
/// Captures a face encoding from the camera and writes it directly to the
/// model file. **Requires write access to `/etc/security/faceauth/`.**
///
/// When running without elevated privileges, use the daemon instead:
/// capture the encoding with `capture_face_encoding` and send it via
/// `ipc::Request::Enroll` to the daemon socket.
///
/// Multiple calls accumulate encodings, allowing the user to register
/// several angles or lighting conditions.
///
/// # Errors
/// Propagates any error from [`capture_face_encoding`] or the model save.
pub fn enroll_face(username: &str, timeout: Duration, camera_index: u32) -> Result<()> {
    let (encoding, camera_id) = capture_face_encoding(timeout, camera_index)?;

    let mut face_model = load_or_create_model(username, camera_id)?;

    face_model.add_encoding(encoding);
    save_model(&face_model)?;

    Ok(())
}

/// Load a user's face model by asking the faceauth daemon via the IPC socket.
///
/// This is the right way to load a model from a process that does not have
/// read access to `/etc/security/faceauth/` (e.g. a PAM module running as the
/// authenticating user). The daemon runs as `faceauthd` and authorises the
/// request via `SO_PEERCRED`: the caller must own `username` or be root.
///
/// # Errors
/// - [`FaceAuthError::ModelNotFound`] if the daemon reports no model enrolled for `username`
/// - [`FaceAuthError::Storage`] if the daemon socket is unreachable or the I/O fails
/// - [`FaceAuthError::Json`] if the model JSON is malformed
pub fn load_model_via_daemon(username: &str) -> Result<FaceModel> {
    use crate::ipc::{Request, Response, SOCKET_PATH};

    let stream = UnixStream::connect(SOCKET_PATH)
        .map_err(FaceAuthError::Storage)?;

    let req = Request::LoadModel { username: username.to_string() };
    let mut line = serde_json::to_string(&req)?;
    line.push('\n');
    (&stream).write_all(line.as_bytes()).map_err(FaceAuthError::Storage)?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(FaceAuthError::Storage)?;

    let response: Response = serde_json::from_str(line.trim())?;
    match response {
        Response::Model { json } => load_model_from_json(&json),
        Response::Err { message } if message.starts_with("no model") => {
            Err(FaceAuthError::ModelNotFound(username.to_string()))
        }
        Response::Err { message } => Err(FaceAuthError::Storage(std::io::Error::new(
            std::io::ErrorKind::Other,
            message,
        ))),
        Response::Ok => Err(FaceAuthError::Storage(std::io::Error::new(
            std::io::ErrorKind::Other,
            "unexpected Ok response from faceauth daemon",
        ))),
    }
}

/// Authenticate using a pre-loaded `FaceModel`.
///
/// Opens the camera recorded at enrollment and loops until a matching face is
/// found or `config.timeout` elapses.
///
/// # Errors
/// - [`FaceAuthError::ModelNotFound`] if the model contains no encodings
/// - [`FaceAuthError::Timeout`] if no matching face is found within `config.timeout`
/// - [`FaceAuthError::Camera`] if the camera cannot be opened or read
/// - [`FaceAuthError::Dlib`] if the face encoder fails to initialise
pub fn authenticate_face_with_model(face_model: FaceModel, config: &AuthConfig) -> Result<()> {
    if face_model.encodings.is_empty() {
        return Err(FaceAuthError::ModelNotFound(face_model.username.clone()));
    }

    let encoder = FaceEncoder::new()?;

    let mut camera = open_camera(&face_model.camera)?;
    camera
        .open_stream()
        .map_err(|e| FaceAuthError::Camera(e.to_string()))?;

    let deadline = Instant::now() + config.timeout;
    let mut matched = false;

    'capture: loop {
        if Instant::now() > deadline {
            break 'capture;
        }

        let frame = match capture_frame(&mut camera) {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(config.frame_interval);
                continue;
            }
        };

        let faces = match encoder.encode_faces(&frame.data, frame.width, frame.height) {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(config.frame_interval);
                continue;
            }
        };

        for face_encoding in &faces {
            if FaceEncoder::matches_model(face_encoding, &face_model.encodings) {
                matched = true;
                break 'capture;
            }
        }

        std::thread::sleep(config.frame_interval);
    }

    let _ = camera.stop_stream();

    if matched {
        Ok(())
    } else {
        Err(FaceAuthError::Timeout(config.timeout.as_secs()))
    }
}
