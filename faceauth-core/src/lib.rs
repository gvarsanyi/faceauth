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
pub use encoding::FaceEncoder;
use error::{FaceAuthError, Result};
pub use encoding::DISTANCE_THRESHOLD;
pub use model::{load_model_from_json, uid_for_username, username_for_uid};
use model::FaceModel;

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

/// Frame interval used between capture attempts in the enrollment and auth loops.
const FRAME_INTERVAL: Duration = Duration::from_millis(100);

/// Capture a single face encoding using a provided encoder.
///
/// Opens `camera_index`, loops until exactly one face is visible within
/// `timeout`, and returns the 128-D descriptor. Allows long-running processes
/// (e.g. the daemon) to create the encoder once and reuse it across requests.
///
/// # Errors
/// - [`FaceAuthError::Timeout`] if no single face is detected within `timeout`
/// - [`FaceAuthError::Camera`] if the camera cannot be opened or read
pub fn capture_with_encoder(
    encoder: &FaceEncoder,
    camera_index: u32,
    timeout: Duration,
) -> Result<[f32; 128]> {
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
            Err(_) => { std::thread::sleep(FRAME_INTERVAL); continue; }
        };

        let faces = match encoder.encode_faces(&frame.data, frame.width, frame.height) {
            Ok(f) => f,
            Err(_) => { std::thread::sleep(FRAME_INTERVAL); continue; }
        };

        match faces.len() {
            1 => break faces.into_iter().next().unwrap(),
            _ => { std::thread::sleep(FRAME_INTERVAL); continue; }
        }
    };

    let _ = camera.stop_stream();
    Ok(encoding)
}

/// Capture a single face encoding by asking the faceauth daemon.
///
/// The daemon opens `camera_index`, waits for exactly one face within
/// `timeout_secs`, and returns the 128-D descriptor. The caller needs no
/// camera access or dlib.
///
/// # Errors
/// - [`FaceAuthError::Storage`] if the daemon socket is unreachable or I/O fails
/// - [`FaceAuthError::Camera`] if the daemon reports a capture failure
///   (timeout, camera error, multiple faces, etc.)
/// - [`FaceAuthError::Dlib`] if the returned encoding has unexpected length
pub fn capture_encoding_via_daemon(camera_index: u32, timeout_secs: u64) -> Result<[f32; 128]> {
    use crate::ipc::{Request, Response, SOCKET_PATH};

    let stream = UnixStream::connect(SOCKET_PATH)
        .map_err(FaceAuthError::Storage)?;

    let read_timeout = Duration::from_secs(timeout_secs.saturating_add(10));
    stream.set_read_timeout(Some(read_timeout)).map_err(FaceAuthError::Storage)?;

    let req = Request::CaptureEncoding { camera_index, timeout_secs };
    let mut line = serde_json::to_string(&req)?;
    line.push('\n');
    (&stream).write_all(line.as_bytes()).map_err(FaceAuthError::Storage)?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(FaceAuthError::Storage)?;

    let response: Response = serde_json::from_str(line.trim())?;
    match response {
        Response::Encoding { data } => {
            data.try_into().map_err(|_| FaceAuthError::Dlib(
                "encoding from daemon must be exactly 128 floats".to_string(),
            ))
        }
        Response::Err { message } => Err(FaceAuthError::Camera(message)),
        _ => Err(FaceAuthError::Storage(std::io::Error::new(
            std::io::ErrorKind::Other,
            "unexpected response from faceauth daemon",
        ))),
    }
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
        _ => Err(FaceAuthError::Storage(std::io::Error::new(
            std::io::ErrorKind::Other,
            "unexpected response from faceauth daemon",
        ))),
    }
}

/// Authenticate using a pre-loaded `FaceModel` and an already-initialised encoder.
///
/// This is the core authentication loop, separated from encoder creation so
/// long-running processes (e.g. the daemon) can create the encoder once and
/// reuse it across multiple authentication requests.
///
/// # Errors
/// - [`FaceAuthError::ModelNotFound`] if the model contains no encodings
/// - [`FaceAuthError::Timeout`] if no matching face is found within `config.timeout`
/// - [`FaceAuthError::Camera`] if the camera cannot be opened or read
/// - [`FaceAuthError::Dlib`] if encoding fails unexpectedly
pub fn authenticate_with_encoder(
    encoder: &FaceEncoder,
    face_model: FaceModel,
    config: &AuthConfig,
) -> Result<()> {
    // Flatten batches into a single slice for matching.
    let flat_encodings: Vec<[f32; 128]> = face_model.encodings.iter().flatten().copied().collect();
    if flat_encodings.is_empty() {
        return Err(FaceAuthError::Camera("model has no face encodings".to_string()));
    }

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
            if FaceEncoder::matches_model(face_encoding, &flat_encodings) {
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

/// Authenticate using a pre-loaded `FaceModel`.
///
/// Creates a new `FaceEncoder`, then delegates to [`authenticate_with_encoder`].
/// Prefer [`authenticate_with_encoder`] in long-running processes that want to
/// initialise the encoder once and reuse it.
///
/// # Errors
/// - [`FaceAuthError::ModelNotFound`] if the model contains no encodings
/// - [`FaceAuthError::Timeout`] if no matching face is found within `config.timeout`
/// - [`FaceAuthError::Camera`] if the camera cannot be opened or read
/// - [`FaceAuthError::Dlib`] if the face encoder fails to initialise
pub fn authenticate_face_with_model(face_model: FaceModel, config: &AuthConfig) -> Result<()> {
    let encoder = FaceEncoder::new()?;
    authenticate_with_encoder(&encoder, face_model, config)
}

/// Authenticate `username` by sending a request to the faceauth daemon.
///
/// The daemon opens the camera, runs the capture loop, and returns the result.
/// The caller does not need camera access or dlib; the daemon handles everything.
///
/// `timeout_secs` is passed to the daemon as the capture deadline.
///
/// # Errors
/// - [`FaceAuthError::Storage`] if the daemon socket is unreachable or I/O fails
/// - [`FaceAuthError::Camera`] if the daemon reports an authentication failure
///   (timeout, camera error, no match, etc.)
pub fn authenticate_via_daemon(username: &str, timeout_secs: u64) -> Result<()> {
    use crate::ipc::{Request, Response, SOCKET_PATH};

    let stream = UnixStream::connect(SOCKET_PATH)
        .map_err(FaceAuthError::Storage)?;

    // Set a generous read timeout: authentication may queue behind another
    // in-flight request in the camera actor, so allow queue time on top of
    // the auth timeout itself.
    let read_timeout = Duration::from_secs(timeout_secs.saturating_add(60));
    stream.set_read_timeout(Some(read_timeout)).map_err(FaceAuthError::Storage)?;

    let req = Request::Authenticate { username: username.to_string(), timeout_secs };
    let mut line = serde_json::to_string(&req)?;
    line.push('\n');
    (&stream).write_all(line.as_bytes()).map_err(FaceAuthError::Storage)?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(FaceAuthError::Storage)?;

    let response: Response = serde_json::from_str(line.trim())?;
    match response {
        Response::Ok => Ok(()),
        Response::Err { message } => Err(FaceAuthError::Camera(message)),
        _ => Err(FaceAuthError::Storage(std::io::Error::new(
            std::io::ErrorKind::Other,
            "unexpected response from faceauth daemon",
        ))),
    }
}
