use std::sync::mpsc;
use std::time::Duration;

use faceauth_core::model::FaceModel;
use faceauth_core::{authenticate_with_encoder, capture_with_encoder, AuthConfig, FaceEncoder};

pub struct AuthRequest {
    pub face_model: FaceModel,
    pub timeout_secs: u64,
    pub reply: mpsc::Sender<Result<(), String>>,
}

pub struct CaptureRequest {
    pub camera_index: u32,
    pub timeout_secs: u64,
    pub reply: mpsc::Sender<Result<Vec<f32>, String>>,
}

pub enum CameraMsg {
    Authenticate(AuthRequest),
    Capture(CaptureRequest),
}

pub fn start_camera_actor() -> mpsc::Sender<CameraMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("camera-actor".to_string())
        .spawn(move || camera_actor(rx))
        .expect("failed to spawn camera actor thread");
    tx
}

fn camera_actor(rx: mpsc::Receiver<CameraMsg>) {
    let encoder = match FaceEncoder::new() {
        Ok(e) => {
            eprintln!("faceauth-daemon: face encoder initialised");
            e
        }
        Err(e) => {
            eprintln!("faceauth-daemon: failed to initialise face encoder: {e}");
            for msg in rx {
                match msg {
                    CameraMsg::Authenticate(req) => {
                        let _ = req.reply.send(Err(format!("face encoder unavailable: {e}")));
                    }
                    CameraMsg::Capture(req) => {
                        let _ = req.reply.send(Err(format!("face encoder unavailable: {e}")));
                    }
                }
            }
            return;
        }
    };

    for msg in rx {
        match msg {
            CameraMsg::Authenticate(req) => {
                let result = perform_auth(&encoder, req.face_model, req.timeout_secs);
                let _ = req.reply.send(result);
            }
            CameraMsg::Capture(req) => {
                let result = perform_capture(&encoder, req.camera_index, req.timeout_secs);
                let _ = req.reply.send(result);
            }
        }
    }
}

/// Run the capture/match loop using a pre-loaded model and encoder.
fn perform_auth(encoder: &FaceEncoder, face_model: FaceModel, timeout_secs: u64) -> Result<(), String> {
    let config = AuthConfig {
        timeout: Duration::from_secs(timeout_secs),
        ..AuthConfig::default()
    };
    authenticate_with_encoder(encoder, face_model, &config).map_err(|e| e.to_string())
}

/// Capture a single face encoding from `camera_index` using `encoder`.
fn perform_capture(encoder: &FaceEncoder, camera_index: u32, timeout_secs: u64) -> Result<Vec<f32>, String> {
    let timeout = Duration::from_secs(timeout_secs);
    capture_with_encoder(encoder, camera_index, timeout)
        .map(|enc| enc.to_vec())
        .map_err(|e| e.to_string())
}
