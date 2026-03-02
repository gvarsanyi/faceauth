use std::os::unix::net::UnixListener;
use std::path::Path;

use faceauth_core::ipc::SOCKET_PATH;

mod camera_actor;
mod handler;
mod model_actor;

fn main() {
    let socket_dir = Path::new(SOCKET_PATH).parent().unwrap();
    if let Err(e) = std::fs::create_dir_all(socket_dir) {
        eprintln!("faceauth-daemon: failed to create socket directory: {}", e);
        std::process::exit(1);
    }

    let _ = std::fs::remove_file(SOCKET_PATH);

    let listener = match UnixListener::bind(SOCKET_PATH) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("faceauth-daemon: failed to bind {}: {}", SOCKET_PATH, e);
            std::process::exit(1);
        }
    };

    if let Err(e) = std::fs::set_permissions(
        SOCKET_PATH,
        std::os::unix::fs::PermissionsExt::from_mode(0o666),
    ) {
        eprintln!("faceauth-daemon: failed to set socket permissions: {}", e);
        std::process::exit(1);
    }

    let camera_tx = camera_actor::start_camera_actor();
    let model_tx = model_actor::start_model_actor();

    eprintln!("faceauth-daemon: listening on {}", SOCKET_PATH);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let camera_tx = camera_tx.clone();
                let model_tx = model_tx.clone();
                std::thread::spawn(move || handler::handle_client(stream, camera_tx, model_tx));
            }
            Err(e) => {
                eprintln!("faceauth-daemon: accept error: {}", e);
            }
        }
    }
}
