use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::AsRawFd;
use std::sync::mpsc;
use std::time::Duration;

use faceauth_core::ipc::{Request, Response};

use crate::camera_actor::{AuthRequest, CameraMsg, CaptureRequest};
use crate::model_actor::ModelMsg;

/// Return the UID of the process on the other end of a Unix socket using
/// `SO_PEERCRED`. The kernel fills this in — the client cannot forge it.
fn peer_uid(fd: std::os::unix::io::RawFd) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: fd is a valid connected Unix socket; cred and len are correctly
    // sized and aligned for the SOL_SOCKET / SO_PEERCRED getsockopt call.
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if ret == 0 { Some(cred.uid) } else { None }
}

pub fn handle_client(
    stream: std::os::unix::net::UnixStream,
    camera_tx: mpsc::Sender<CameraMsg>,
    model_tx: mpsc::Sender<ModelMsg>,
) {
    let peer_uid = match peer_uid(stream.as_raw_fd()) {
        Some(uid) => uid,
        None => {
            send_err(&stream, "failed to determine caller identity");
            return;
        }
    };

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();

    if reader.read_line(&mut line).is_err() || line.is_empty() {
        send_err(&stream, "failed to read request");
        return;
    }

    let request: Request = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            send_err(&stream, &format!("malformed request: {}", e));
            return;
        }
    };

    let response = handle_request(request, peer_uid, &camera_tx, &model_tx);
    let mut json = serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"status":"Err","message":"serialization error"}"#.to_string()
    });
    json.push('\n');
    let _ = (&stream).write_all(json.as_bytes());
}

/// Authorise a request: caller must own `username` or be root (uid 0).
/// Returns `Some(Response::Err)` if denied, `None` if authorised.
fn authorize(username: &str, peer_uid: u32) -> Option<Response> {
    if peer_uid == 0 {
        return None;
    }
    match faceauth_core::uid_for_username(username) {
        Some(uid) if uid == peer_uid => None,
        Some(_) => Some(Response::Err {
            message: format!("permission denied: you are not '{}'", username),
        }),
        None => Some(Response::Err {
            message: format!("unknown user '{}'", username),
        }),
    }
}

/// Send a request to the model actor and wait for its reply.
///
/// Returns `Ok(T)` on success, or `Err(Response::Err)` if the actor is
/// unavailable or returns an error, ready to be returned directly to the client.
fn model_call<T: Send + 'static>(
    model_tx: &mpsc::Sender<ModelMsg>,
    make_msg: impl FnOnce(mpsc::Sender<Result<T, String>>) -> ModelMsg,
) -> Result<T, Response> {
    let (reply_tx, reply_rx) = mpsc::channel();
    if model_tx.send(make_msg(reply_tx)).is_err() {
        return Err(Response::Err { message: "model actor unavailable".to_string() });
    }
    match reply_rx.recv() {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(Response::Err { message: e }),
        Err(_) => Err(Response::Err { message: "model actor unavailable".to_string() }),
    }
}

/// Dispatch an authorised request and return the response.
fn handle_request(
    request: Request,
    peer_uid: u32,
    camera_tx: &mpsc::Sender<CameraMsg>,
    model_tx: &mpsc::Sender<ModelMsg>,
) -> Response {
    match request {
        Request::Enroll { username, camera_index, encodings } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            let batch: Result<Vec<[f32; 128]>, _> = encodings
                .into_iter()
                .map(|enc| enc.try_into().map_err(|_| ()))
                .collect();
            let batch = match batch {
                Ok(b) => b,
                Err(()) => return Response::Err {
                    message: "each encoding must be exactly 128 floats".to_string(),
                },
            };

            match model_call(model_tx, |reply| ModelMsg::Enroll { username, camera_index, batch, reply }) {
                Ok(()) => Response::Ok,
                Err(e) => e,
            }
        }

        Request::LoadModel { username } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            match model_call(model_tx, |reply| ModelMsg::Load { username, reply }) {
                Ok(json) => Response::Model { json },
                Err(e) => e,
            }
        }

        Request::Clear { username, index } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            match model_call(model_tx, |reply| ModelMsg::Clear { username, index, reply }) {
                Ok(()) => Response::Ok,
                Err(e) => e,
            }
        }

        Request::Authenticate { username, timeout_secs } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            // Step 1: load the model via the model actor (no direct disk I/O here).
            let json = match model_call(model_tx, |reply| ModelMsg::Load { username, reply }) {
                Ok(j) => j,
                Err(e) => return e,
            };
            let face_model = match faceauth_core::load_model_from_json(&json) {
                Ok(m) => m,
                Err(e) => return Response::Err { message: e.to_string() },
            };

            // Step 2: run the capture/match loop in the camera actor.
            let (reply_tx, reply_rx) = mpsc::channel();
            let msg = CameraMsg::Authenticate(AuthRequest { face_model, timeout_secs, reply: reply_tx });
            if camera_tx.send(msg).is_err() {
                return Response::Err { message: "camera actor unavailable".to_string() };
            }

            let wait = Duration::from_secs(timeout_secs.saturating_add(60));
            match reply_rx.recv_timeout(wait) {
                Ok(Ok(())) => Response::Ok,
                Ok(Err(msg)) => Response::Err { message: msg },
                Err(_) => Response::Err {
                    message: "authentication timed out waiting for camera".to_string(),
                },
            }
        }

        Request::CaptureEncoding { camera_index, timeout_secs } => {
            // No per-user authorisation: any local user may capture a frame.
            let (reply_tx, reply_rx) = mpsc::channel();
            let msg = CameraMsg::Capture(CaptureRequest { camera_index, timeout_secs, reply: reply_tx });
            if camera_tx.send(msg).is_err() {
                return Response::Err { message: "camera actor unavailable".to_string() };
            }

            let wait = Duration::from_secs(timeout_secs.saturating_add(60));
            match reply_rx.recv_timeout(wait) {
                Ok(Ok(data)) => Response::Encoding { data },
                Ok(Err(msg)) => Response::Err { message: msg },
                Err(_) => Response::Err {
                    message: "capture timed out waiting for camera".to_string(),
                },
            }
        }

        Request::SetConfig { username, disabled, ignore_add, ignore_remove } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            match model_call(model_tx, |reply| ModelMsg::SetConfig {
                username, disabled, ignore_add, ignore_remove, reply,
            }) {
                Ok(()) => Response::Ok,
                Err(e) => e,
            }
        }
    }
}

/// Serialize a `Response::Err` and write it to `stream`; errors are silently ignored.
fn send_err(mut stream: &std::os::unix::net::UnixStream, message: &str) {
    let resp = Response::Err { message: message.to_string() };
    if let Ok(mut json) = serde_json::to_string(&resp) {
        json.push('\n');
        let _ = stream.write_all(json.as_bytes());
    }
}
