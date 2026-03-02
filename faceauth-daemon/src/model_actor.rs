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
    /// Update the per-user authentication configuration (disabled flag, ignore list).
    SetConfig {
        username: String,
        disabled: Option<bool>,
        ignore_add: Option<String>,
        ignore_remove: Option<String>,
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
            ModelMsg::SetConfig { username, disabled, ignore_add, ignore_remove, reply } => {
                let _ = reply.send(do_set_config(&username, disabled, ignore_add, ignore_remove));
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
    let mut face_model = model::load_or_create_model(username, camera_id).map_err(|e| e.to_string())?;
    if face_model.camera.index != camera_index {
        return Err(format!(
            "model already uses /dev/video{}; use 'faceauth clear' first to change cameras",
            face_model.camera.index
        ));
    }
    face_model.add_batch(batch);
    model::save_model(uid, &face_model).map_err(|e| e.to_string())
}

fn do_set_config(
    username: &str,
    disabled: Option<bool>,
    ignore_add: Option<String>,
    ignore_remove: Option<String>,
) -> Result<(), String> {
    let uid = faceauth_core::uid_for_username(username)
        .ok_or_else(|| format!("unknown user '{}'", username))?;
    let mut face_model = model::load_model(username).map_err(|e| e.to_string())?;
    if let Some(d) = disabled {
        face_model.disabled = d;
    }
    if let Some(app) = ignore_add {
        if !face_model.ignore.contains(&app) {
            face_model.ignore.push(app);
        }
    }
    if let Some(app) = ignore_remove {
        face_model.ignore.retain(|s| *s != app);
    }
    model::save_model(uid, &face_model).map_err(|e| e.to_string())
}

fn do_clear(username: &str, index: Option<usize>) -> Result<(), String> {
    match index {
        None => {
            let uid = faceauth_core::uid_for_username(username)
                .ok_or_else(|| format!("unknown user '{}'", username))?;
            let path = model::model_path(uid);
            if !path.exists() {
                return Ok(());
            }
            std::fs::remove_file(&path).map_err(|e| e.to_string())
        }
        Some(i) => {
            let uid = faceauth_core::uid_for_username(username)
                .ok_or_else(|| format!("unknown user '{}'", username))?;
            let mut face_model = model::load_model(username).map_err(|e| e.to_string())?;
            if i >= face_model.encodings.len() {
                return Err(format!(
                    "batch index {} out of range (have {})",
                    i, face_model.encodings.len()
                ));
            }
            face_model.encodings.remove(i);
            if face_model.encodings.is_empty() {
                std::fs::remove_file(&model::model_path(uid)).map_err(|e| e.to_string())
            } else {
                model::save_model(uid, &face_model).map_err(|e| e.to_string())
            }
        }
    }
}
