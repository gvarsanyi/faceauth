use std::sync::mpsc;

use faceauth_core::model;

pub enum ModelMsg {
    /// Read a user's model file and return its raw JSON.
    Load {
        username: String,
        reply: mpsc::Sender<Result<String, String>>,
    },
    /// Append a batch of face encodings to a user's model.
    Enroll {
        username: String,
        camera_index: u32,
        batch: Vec<[f32; 128]>,
        reply: mpsc::Sender<Result<(), String>>,
    },
    /// Remove a user's model entirely, or remove a single batch by index.
    Clear {
        username: String,
        index: Option<usize>,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

pub fn start_model_actor() -> mpsc::Sender<ModelMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("model-actor".to_string())
        .spawn(move || model_actor(rx))
        .expect("failed to spawn model actor thread");
    tx
}

/// Model actor: processes all model file I/O sequentially so that concurrent
/// clients queue rather than race on read-modify-write operations.
fn model_actor(rx: mpsc::Receiver<ModelMsg>) {
    for msg in rx {
        match msg {
            ModelMsg::Load { username, reply } => {
                let _ = reply.send(do_load(&username));
            }
            ModelMsg::Enroll { username, camera_index, batch, reply } => {
                let _ = reply.send(do_enroll(&username, camera_index, batch));
            }
            ModelMsg::Clear { username, index, reply } => {
                let _ = reply.send(do_clear(&username, index));
            }
        }
    }
}

fn do_load(username: &str) -> Result<String, String> {
    let uid = faceauth_core::uid_for_username(username)
        .ok_or_else(|| format!("unknown user '{}'", username))?;
    let path = model::model_path(uid);
    match std::fs::read_to_string(&path) {
        Ok(json) => Ok(json),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("no model enrolled for '{}'", username))
        }
        Err(e) => Err(e.to_string()),
    }
}

fn do_enroll(username: &str, camera_index: u32, batch: Vec<[f32; 128]>) -> Result<(), String> {
    let uid = faceauth_core::uid_for_username(username)
        .ok_or_else(|| format!("unknown user '{}'", username))?;
    let camera_id = faceauth_core::camera::camera_id_for_index(camera_index);
    let mut face_model = model::load_or_create_model(username, camera_id.clone()).map_err(|e| e.to_string())?;
    if face_model.encodings.is_empty() {
        // No encodings — camera cleared or brand-new model; accept whichever
        // camera is being used and update the stored camera metadata.
        face_model.camera = camera_id;
    } else if face_model.camera.index != camera_index {
        return Err(format!(
            "model already uses /dev/video{}; use 'faceauth clear' first to change cameras",
            face_model.camera.index
        ));
    }
    face_model.add_batch(batch);
    model::save_model(uid, &face_model).map_err(|e| e.to_string())
}

fn do_clear(username: &str, index: Option<usize>) -> Result<(), String> {
    let uid = faceauth_core::uid_for_username(username)
        .ok_or_else(|| format!("unknown user '{}'", username))?;

    // If no model file exists there is nothing to clear.
    if !model::model_path(uid).exists() {
        return Ok(());
    }

    let mut face_model = model::load_model(username).map_err(|e| e.to_string())?;

    match index {
        None => {
            face_model.encodings.clear();
        }
        Some(i) => {
            if i >= face_model.encodings.len() {
                return Err(format!(
                    "batch index {} out of range (have {})",
                    i, face_model.encodings.len()
                ));
            }
            face_model.encodings.remove(i);
        }
    }

    // Save back — preserves disabled flag and camera metadata.
    model::save_model(uid, &face_model).map_err(|e| e.to_string())
}
