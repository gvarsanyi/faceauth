use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process;
use std::time::Duration;

use clap::{Parser, Subcommand};
use faceauth_core::ipc::{Request, Response, SOCKET_PATH};
use faceauth_core::model::FaceModel;
use faceauth_core::{
    authenticate_face_with_model, camera_name_for_index, capture_face_encoding, encoding_distance,
    load_model_from_json, username_for_uid, AuthConfig,
};
use gettextrs::gettext;

mod camera;

#[derive(Parser)]
#[command(
    name = "faceauth",
    about = "Facial recognition authentication tool",
    version,
    arg_required_else_help = true
)]
struct Cli {
    /// User to operate on. Defaults to the current user.
    /// Root may specify any username to manage another user's model.
    #[arg(long, short = 'u', global = true)]
    user: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Enroll a face model for a user.
    ///
    /// Captures a face encoding from the camera locally and sends it to the
    /// faceauth daemon for storage. Falls back to direct file write if the
    /// daemon socket is not available (requires root).
    ///
    /// Encodings are appended to any already stored. Use `set` to replace
    /// all existing encodings in one operation.
    Add {
        /// Seconds to wait for a single-face frame before giving up.
        #[arg(short, long, default_value_t = 30)]
        timeout: u64,

        /// Camera to use. Accepts a device index (e.g. 2), a full device path
        /// (/dev/video2), or a stable udev name from /dev/v4l/by-id/ or
        /// /dev/v4l/by-path/. Required in non-interactive mode (e.g. scripts);
        /// omit to be prompted interactively. `cameras` shows available options.
        #[arg(short, long)]
        camera: Option<String>,

        /// Number of encodings to capture. Multiple encodings from different
        /// angles or lighting conditions improve recognition accuracy.
        #[arg(short, long, default_value_t = 5)]
        count: u32,
    },

    /// Replace all stored encodings with a fresh set.
    ///
    /// Identical to `add` except that all previously stored encodings are
    /// removed just before the first new encoding is saved. If capture fails
    /// mid-way the old encodings are already gone, so prefer `add` when you
    /// want to keep existing encodings as a fallback.
    Set {
        /// Seconds to wait for a single-face frame before giving up.
        #[arg(short, long, default_value_t = 30)]
        timeout: u64,

        /// Camera to use. Accepts a device index (e.g. 2), a full device path
        /// (/dev/video2), or a stable udev name from /dev/v4l/by-id/ or
        /// /dev/v4l/by-path/. Required in non-interactive mode (e.g. scripts);
        /// omit to be prompted interactively. `cameras` shows available options.
        #[arg(short, long)]
        camera: Option<String>,

        /// Number of encodings to capture.
        #[arg(short, long, default_value_t = 5)]
        count: u32,
    },

    /// Test face authentication for a user.
    ///
    /// Exits 0 on success, 1 on failure (no match or timeout).
    /// The camera used during enrollment is stored in the model and reused automatically.
    Test {
        /// Seconds to attempt authentication before failing.
        #[arg(short, long, default_value_t = 5)]
        timeout: u64,
    },

    /// Remove all stored face encodings for a user.
    ///
    /// Sends a clear request to the faceauth daemon. Falls back to direct
    /// file removal if the daemon socket is not available (requires root).
    Clear,

    /// Show information about a user's stored face model.
    ///
    /// Prints the number of enrolled encodings, the camera index, and
    /// pairwise Euclidean distances between encodings (threshold is 0.6).
    Info,

    /// List available camera devices and their suitability for face authentication.
    Cameras,
}

/// Send a request to the daemon and return the response.
/// Returns `None` if the daemon socket is not available.
fn daemon_request(req: &Request) -> Option<Response> {
    let stream = UnixStream::connect(SOCKET_PATH).ok()?;
    let mut json = serde_json::to_string(req).ok()?;
    json.push('\n');
    (&stream).write_all(json.as_bytes()).ok()?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    serde_json::from_str(line.trim()).ok()
}

/// Fetch a user's model via the daemon.
///
/// Returns `Ok(model)` on success, or an error string on failure.
/// Falls back to a direct disk read if the daemon socket is unavailable.
fn load_model(username: &str) -> Result<FaceModel, String> {
    let req = Request::LoadModel {
        username: username.to_string(),
    };
    match daemon_request(&req) {
        Some(Response::Model { json }) => load_model_from_json(&json).map_err(|e| e.to_string()),
        Some(Response::Err { message }) => Err(message),
        Some(_) => Err("unexpected response from daemon".to_string()),
        None => {
            // Daemon not available — try direct read (works if root or 751 dir).
            faceauth_core::model::load_model(username).map_err(|e| e.to_string())
        }
    }
}

/// Send a "start" notification via faceauth-notify; returns the notification ID.
fn notify_start(uid: u32) -> Option<u32> {
    let mut child =
        process::Command::new(concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-notify"))
            .args(["start", &uid.to_string(), "faceauth"])
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::null())
            .spawn()
            .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?)
        .read_line(&mut line)
        .ok()?;
    child.wait().ok()?;
    line.trim().parse().ok()
}

/// Send a result notification via faceauth-notify; fire-and-forget.
fn notify_result(uid: u32, notif_id: u32, success: bool, reason: &str) {
    let sub = if success { "success" } else { "failure" };
    let mut cmd = process::Command::new(concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-notify"));
    cmd.args([sub, &uid.to_string(), &notif_id.to_string()])
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null());
    if !success {
        cmd.arg(reason);
    }
    let _ = cmd.spawn();
}

fn main() {
    gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain("faceauth", "/usr/share/locale").expect("failed to bind text domain");
    gettextrs::textdomain("faceauth").expect("failed to set text domain");

    let cli = Cli::parse();
    let username = match cli.user {
        Some(u) => u,
        None => {
            let uid = unsafe { libc::getuid() };
            match username_for_uid(uid) {
                Some(u) => u,
                None => {
                    eprintln!(
                        "faceauth: {}",
                        gettext("cannot determine username for current UID")
                    );
                    process::exit(1);
                }
            }
        }
    };

    let is_set = matches!(cli.command, Commands::Set { .. });

    match cli.command {
        Commands::Add {
            timeout,
            camera,
            count,
        }
        | Commands::Set {
            timeout,
            camera,
            count,
        } => {
            if count == 0 || count > 20 {
                eprintln!("faceauth: {}", gettext("--count must be between 1 and 20"));
                process::exit(1);
            }
            let camera_index = camera::resolve_add_camera(camera);

            if is_set {
                eprintln!(
                    "{}",
                    gettext("faceauth: replacing face model for '{user}' ({count} encodings)")
                        .replace("{user}", &username)
                        .replace("{count}", &count.to_string())
                );
            } else {
                eprintln!(
                    "{}",
                    gettext("faceauth: enrolling face for '{user}' ({count} encodings)")
                        .replace("{user}", &username)
                        .replace("{count}", &count.to_string())
                );
            }

            // For `set`: clear existing encodings just before saving the first
            // new one. We defer the clear until after all captures succeed so
            // that if capture fails the old model is still intact.
            let mut cleared = false;

            // Encodings captured so far this session, used to reject poses
            // that are too similar to one already captured (distance < 0.3).
            let mut captured: Vec<[f32; 128]> = Vec::new();
            let mut i = 0u32;

            // status(msg) overwrites the current line; status_done() commits it
            // with a newline so it stays visible. We write to stderr directly to
            // avoid buffering delays.
            let stderr = std::io::stderr();
            let status = |msg: &str| {
                let mut h = stderr.lock();
                // Overwrite the current line and pad to 72 chars to erase leftovers.
                let _ = write!(h, "\r  {:<72}", msg);
                let _ = h.flush();
            };
            let status_done = |msg: &str| {
                let mut h = stderr.lock();
                let _ = writeln!(h, "\r  {:<72}", msg);
            };

            while i < count {
                if count == 1 {
                    status(&gettext("look directly at the camera..."));
                } else {
                    let hint = if i == 0 {
                        String::new()
                    } else {
                        format!(" ({})", gettext("different angle"))
                    };
                    status(
                        &gettext("[{done}/{total}] capturing...{hint}")
                            .replace("{done}", &(i + 1).to_string())
                            .replace("{total}", &count.to_string())
                            .replace("{hint}", &hint),
                    );
                }

                let (encoding, _camera_id) =
                    match capture_face_encoding(Duration::from_secs(timeout), camera_index) {
                        Ok(r) => r,
                        Err(e) => {
                            status_done(
                                &gettext("capture failed: {error}")
                                    .replace("{error}", &e.to_string()),
                            );
                            process::exit(1);
                        }
                    };

                // Reject if too similar to any encoding already captured this session.
                if count > 1 {
                    if let Some(min_dist) = captured
                        .iter()
                        .map(|prev| encoding_distance(prev, &encoding))
                        .reduce(f64::min)
                    {
                        if min_dist < 0.3 {
                            status(&gettext(
                                "[{done}/{total}] too similar ({dist}) - adjust angle and hold still"
                            )
                            .replace("{done}", &(i + 1).to_string())
                            .replace("{total}", &count.to_string())
                            .replace("{dist}", &format!("{:.2}", min_dist)));
                            continue;
                        }
                    }
                }

                // `set`: clear existing encodings on the first save, after all
                // captures for this batch have succeeded.
                if is_set && !cleared {
                    let clear_req = Request::Clear {
                        username: username.clone(),
                    };
                    match daemon_request(&clear_req) {
                        Some(Response::Ok) | None => {}
                        Some(Response::Err { message }) => {
                            // A missing model is fine; any other error is fatal.
                            if !message.contains("no model") {
                                status_done(
                                    &gettext("clear failed: {error}").replace("{error}", &message),
                                );
                                process::exit(1);
                            }
                        }
                        Some(_) => {
                            status_done(&gettext("unexpected response from daemon during clear"));
                            process::exit(1);
                        }
                    }
                    cleared = true;
                }

                let req = Request::Enroll {
                    username: username.clone(),
                    camera_index,
                    encoding: encoding.to_vec(),
                };

                let enroll_ok = match daemon_request(&req) {
                    Some(Response::Ok) => true,
                    Some(Response::Err { message }) => {
                        status_done(
                            &gettext("enrollment failed: {error}").replace("{error}", &message),
                        );
                        process::exit(1);
                    }
                    Some(_) => {
                        status_done(&gettext("unexpected response from daemon"));
                        process::exit(1);
                    }
                    None => {
                        // Daemon not running — fall back to direct write (requires root).
                        status_done(&format!(
                            "daemon not available ({}); attempting direct write (requires root)",
                            SOCKET_PATH
                        ));
                        match faceauth_core::enroll_face(
                            &username,
                            Duration::from_secs(timeout),
                            camera_index,
                        ) {
                            Ok(()) => true,
                            Err(e) => {
                                status_done(
                                    &gettext("enrollment failed: {error}")
                                        .replace("{error}", &e.to_string()),
                                );
                                process::exit(1);
                            }
                        }
                    }
                };

                if enroll_ok {
                    captured.push(encoding);
                    i += 1;
                    let captured_msg = gettext("[{done}/{total}] captured")
                        .replace("{done}", &i.to_string())
                        .replace("{total}", &count.to_string());
                    // Only commit the line permanently on the last encoding.
                    if i == count {
                        status_done(&captured_msg);
                    } else {
                        status(&captured_msg);
                    }
                }
            }

            if is_set {
                println!(
                    "{}",
                    gettext("Face model replaced for '{user}'.").replace("{user}", &username)
                );
            } else {
                println!(
                    "{}",
                    gettext("Enrollment successful for '{user}'.").replace("{user}", &username)
                );
            }
        }

        Commands::Test { timeout } => {
            let face_model = match load_model(&username) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("faceauth: {}", e);
                    process::exit(1);
                }
            };

            let camera_index = face_model.camera.index;
            let camera_name = camera_name_for_index(camera_index);
            let camera_display = if camera_name.is_empty() {
                format!("/dev/video{camera_index}")
            } else {
                format!("{camera_name} at /dev/video{camera_index}")
            };
            eprintln!(
                "{}",
                gettext("faceauth: '{user}' please look at the camera ({camera}) ...")
                    .replace("{user}", &username)
                    .replace("{camera}", &camera_display)
            );

            let uid = unsafe { libc::getuid() };
            let notif_id = notify_start(uid);

            let config = AuthConfig {
                timeout: Duration::from_secs(timeout),
                ..Default::default()
            };

            match authenticate_face_with_model(face_model, &config) {
                Ok(()) => {
                    if let Some(id) = notif_id {
                        notify_result(uid, id, true, "");
                    }
                    println!(
                        "{}",
                        gettext("Authentication successful for '{user}'.")
                            .replace("{user}", &username)
                    );
                    process::exit(0);
                }
                Err(e) => {
                    if let Some(id) = notif_id {
                        notify_result(uid, id, false, &e.to_string());
                    }
                    eprintln!(
                        "{}",
                        gettext("faceauth: authentication failed: {error}")
                            .replace("{error}", &e.to_string())
                    );
                    process::exit(1);
                }
            }
        }

        Commands::Clear => {
            let req = Request::Clear {
                username: username.clone(),
            };

            match daemon_request(&req) {
                Some(Response::Ok) => {
                    println!(
                        "{}",
                        gettext("Model for '{user}' removed.").replace("{user}", &username)
                    );
                }
                Some(Response::Err { message }) => {
                    eprintln!(
                        "{}",
                        gettext("faceauth: clear failed: {error}").replace("{error}", &message)
                    );
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    // Daemon not running — fall back to direct removal (requires root).
                    eprintln!(
                        "faceauth: daemon not available ({}); \
                         attempting direct removal (requires root)",
                        SOCKET_PATH
                    );
                    let uid = match faceauth_core::uid_for_username(&username) {
                        Some(u) => u,
                        None => {
                            eprintln!(
                                "{}",
                                gettext("faceauth: no system account for '{user}'")
                                    .replace("{user}", &username)
                            );
                            process::exit(1);
                        }
                    };
                    let path = faceauth_core::model::model_path(uid);

                    if !path.exists() {
                        eprintln!(
                            "{}",
                            gettext("faceauth: no model found for '{user}'; nothing to remove")
                                .replace("{user}", &username)
                        );
                        process::exit(0);
                    }

                    match std::fs::remove_file(&path) {
                        Ok(()) => println!(
                            "{}",
                            gettext("Model for '{user}' removed.").replace("{user}", &username)
                        ),
                        Err(e) => {
                            eprintln!(
                                "{}",
                                gettext("faceauth: failed to remove model: {error}")
                                    .replace("{error}", &e.to_string())
                            );
                            process::exit(1);
                        }
                    }
                }
            }
        }

        Commands::Info => {
            let model = match load_model(&username) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("faceauth: {}", e);
                    process::exit(1);
                }
            };

            println!("{}: {}", gettext("User"), model.username);
            if let Some(uid) = faceauth_core::uid_for_username(&model.username) {
                println!("{}: {}", gettext("UID"), uid);
            }
            println!(
                "{}: {} (/dev/video{})",
                gettext("Camera index"),
                model.camera.index,
                model.camera.index
            );
            if let Some(ref by_id) = model.camera.by_id {
                println!("{}: {}", gettext("Camera by-id"), by_id);
            }
            if let Some(ref by_path) = model.camera.by_path {
                println!("{}: {}", gettext("Camera by-path"), by_path);
            }
            println!("{}: {}", gettext("Encodings"), model.encodings.len());

            for (i, enc) in model.encodings.iter().enumerate() {
                let norm: f32 = enc.iter().map(|x| x * x).sum::<f32>().sqrt();
                println!("  enc[{}]: norm = {:.4}", i, norm);
            }
        }

        Commands::Cameras => {
            let cameras = camera::print_camera_list();
            if cameras.is_empty() {
                eprintln!("faceauth: {}", gettext("no camera devices found"));
                process::exit(1);
            }
        }
    }
}
