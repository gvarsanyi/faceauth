use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;

use faceauth_core::ipc::{Request, Response, SOCKET_PATH};
use faceauth_core::model;

fn main() {
    let socket_dir = Path::new(SOCKET_PATH).parent().unwrap();
    if let Err(e) = std::fs::create_dir_all(socket_dir) {
        eprintln!("faceauth-daemon: failed to create socket directory: {}", e);
        std::process::exit(1);
    }

    // Remove a stale socket from a previous run, if present.
    let _ = std::fs::remove_file(SOCKET_PATH);

    let listener = match UnixListener::bind(SOCKET_PATH) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("faceauth-daemon: failed to bind {}: {}", SOCKET_PATH, e);
            std::process::exit(1);
        }
    };

    // Set socket permissions to 0666 so any user can connect.
    // The daemon authenticates callers via SO_PEERCRED, not filesystem permissions.
    if let Err(e) = std::fs::set_permissions(
        SOCKET_PATH,
        std::os::unix::fs::PermissionsExt::from_mode(0o666),
    ) {
        eprintln!("faceauth-daemon: failed to set socket permissions: {}", e);
        std::process::exit(1);
    }

    eprintln!("faceauth-daemon: listening on {}", SOCKET_PATH);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || handle_client(stream));
            }
            Err(e) => {
                eprintln!("faceauth-daemon: accept error: {}", e);
            }
        }
    }
}

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

/// Read one request from `stream`, authorise it, dispatch it, and write the response.
fn handle_client(stream: std::os::unix::net::UnixStream) {
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

    let response = handle_request(request, peer_uid);
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

/// Dispatch an authorised request and return the response.
fn handle_request(request: Request, peer_uid: u32) -> Response {
    match request {
        Request::Enroll { username, camera_index, encoding } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            let encoding_arr: [f32; 128] = match encoding.try_into() {
                Ok(a) => a,
                Err(_) => {
                    return Response::Err {
                        message: "encoding must be exactly 128 floats".to_string(),
                    };
                }
            };

            let camera_id = faceauth_core::camera::camera_id_for_index(camera_index);

            let mut face_model = match model::load_or_create_model(&username, camera_id) {
                Ok(m) => m,
                Err(e) => return Response::Err { message: e.to_string() },
            };

            face_model.add_encoding(encoding_arr);

            match model::save_model(&face_model) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Err { message: e.to_string() },
            }
        }

        Request::LoadModel { username } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            let uid = match faceauth_core::uid_for_username(&username) {
                Some(u) => u,
                None => return Response::Err { message: format!("unknown user '{}'", username) },
            };
            let path = model::model_path(uid);

            match std::fs::read_to_string(&path) {
                Ok(json) => Response::Model { json },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Response::Err {
                    message: format!("no model enrolled for '{}'", username),
                },
                Err(e) => Response::Err { message: e.to_string() },
            }
        }

        Request::Clear { username } => {
            if let Some(err) = authorize(&username, peer_uid) { return err; }

            let uid = match faceauth_core::uid_for_username(&username) {
                Some(u) => u,
                None => return Response::Err { message: format!("unknown user '{}'", username) },
            };
            let path = model::model_path(uid);

            if !path.exists() {
                return Response::Ok; // nothing to do
            }

            match std::fs::remove_file(&path) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Err { message: e.to_string() },
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

