use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process;
use std::time::Duration;

use clap::{Parser, Subcommand};
use faceauth_core::ipc::{Request, Response, SOCKET_PATH};
use faceauth_core::model::FaceModel;
use faceauth_core::{
    camera_name_for_index, encoding_distance, load_model_from_json, username_for_uid,
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
    /// Asks the faceauth daemon to capture a face encoding from the camera,
    /// then sends it to the daemon for storage. Requires the daemon to be running.
    ///
    /// Encodings are appended to any already stored. Use `clear --index N` to
    /// remove a specific batch, or `clear` to remove all stored batches.
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
    /// Without `--index`, removes the entire face model. With `--index N`,
    /// removes only enrollment batch N (0-based); run `faceauth info` to see
    /// how many batches are enrolled.
    Clear {
        /// Remove only this enrollment batch (0-based index) instead of all batches.
        #[arg(short, long)]
        index: Option<usize>,
    },

    /// Show information about a user's stored face model.
    Info,

    /// List available camera devices and their suitability for face authentication.
    Cameras,

    /// Opt out a PAM service from face authentication for this user.
    ///
    /// Writes a `-service` entry to the user's opt file. Face auth is skipped
    /// when this service requests it. Use `unignore` to re-enable.
    Ignore {
        /// PAM service name to opt out (e.g. "sudo").
        requestor: String,
    },

    /// Re-enable face authentication for a PAM service for this user.
    ///
    /// Writes a `+service` entry to the user's opt file, overriding any
    /// previous opt-out. Use `ignore` to opt back out.
    Unignore {
        /// PAM service name to re-enable (e.g. "sudo").
        requestor: String,
    },

    /// List known PAM services and their opt-in status for this user.
    ///
    /// Shows all services recorded in the global opt file and any user
    /// overrides. A `+` prefix means face auth is active for that service;
    /// `-` means it is skipped.
    Services,
}

/// Send a request to the daemon and return the response.
/// Returns `None` if the daemon socket is not available.
fn daemon_request(req: &Request) -> Option<Response> {
    let stream = UnixStream::connect(SOCKET_PATH).ok()?;
    // Set a generous read timeout so long-running requests (e.g. Authenticate)
    // don't block forever if the daemon hangs or the client is killed.
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .ok()?;
    let mut json = serde_json::to_string(req).ok()?;
    json.push('\n');
    (&stream).write_all(json.as_bytes()).ok()?;

    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    serde_json::from_str(line.trim()).ok()
}

/// Return true if the error string means the user has no enrolled model.
///
/// Two distinct messages are possible: the daemon's own "no model enrolled for"
/// and `FaceAuthError::ModelNotFound`'s display when the direct-disk fallback is used.
fn is_model_not_found(e: &str) -> bool {
    e.contains("no model enrolled") || e.contains("No face model found")
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
fn notify_result(uid: u32, notif_id: u32, success: bool) {
    let sub = if success { "success" } else { "failure" };
    let mut cmd = process::Command::new(concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-notify"));
    cmd.args([sub, &uid.to_string(), &notif_id.to_string(), "faceauth"])
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null());
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

    match cli.command {
        Commands::Add { timeout, camera } => {
            const COUNT: u32 = 5;

            // If a model already exists, its stored camera is authoritative.
            // A --camera argument is validated against it; if absent, the stored
            // camera is used directly (no interactive prompt on subsequent adds).
            let camera_index = match load_model(&username) {
                Ok(existing) => {
                    let stored = existing.camera.index;
                    if let Some(cam_arg) = camera {
                        let given = camera::resolve_add_camera(Some(cam_arg));
                        if given != stored {
                            eprintln!(
                                "faceauth: {}",
                                gettext("'{user}' is already enrolled with /dev/video{camera}; omit --camera to add another batch, or run 'faceauth clear' first to change cameras")
                                    .replace("{user}", &username)
                                    .replace("{camera}", &stored.to_string())
                            );
                            process::exit(1);
                        }
                    }
                    stored
                }
                Err(ref e) if is_model_not_found(e) => camera::resolve_add_camera(camera),
                Err(e) => {
                    eprintln!("faceauth: {}", e);
                    process::exit(1);
                }
            };

            eprintln!(
                "faceauth: {}",
                gettext("enrolling face for '{user}'").replace("{user}", &username)
            );

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
                let _ = writeln!(h, "\rfaceauth: {:<72}", msg);
            };

            while i < COUNT {
                if COUNT == 1 {
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
                            .replace("{total}", &COUNT.to_string())
                            .replace("{hint}", &hint),
                    );
                }

                let encoding: [f32; 128] = {
                    let req = Request::CaptureEncoding {
                        camera_index,
                        timeout_secs: timeout,
                    };
                    match daemon_request(&req) {
                        Some(Response::Encoding { data }) => match data.try_into() {
                            Ok(arr) => arr,
                            Err(_) => {
                                status_done(&gettext(
                                    "capture failed: invalid encoding from daemon",
                                ));
                                process::exit(1);
                            }
                        },
                        Some(Response::Err { message }) => {
                            status_done(
                                &gettext("capture failed: {error}").replace("{error}", &message),
                            );
                            process::exit(1);
                        }
                        Some(_) => {
                            status_done(&gettext(
                                "capture failed: unexpected response from daemon",
                            ));
                            process::exit(1);
                        }
                        None => {
                            status_done("daemon not available; cannot capture without daemon");
                            process::exit(1);
                        }
                    }
                };

                // Reject if too similar to any encoding already captured this session.
                if COUNT > 1 {
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
                            .replace("{total}", &COUNT.to_string())
                            .replace("{dist}", &format!("{:.2}", min_dist)));
                            continue;
                        }
                    }
                }

                captured.push(encoding);
                i += 1;
                let captured_msg = gettext("[{done}/{total}] captured")
                    .replace("{done}", &i.to_string())
                    .replace("{total}", &COUNT.to_string());
                // Only commit the line permanently on the last encoding.
                if i == COUNT {
                    status_done(&captured_msg);
                } else {
                    status(&captured_msg);
                }
            }

            // Enroll the entire batch in one request.
            let req = Request::Enroll {
                username: username.clone(),
                camera_index,
                encodings: captured.iter().map(|enc| enc.to_vec()).collect(),
            };
            match daemon_request(&req) {
                Some(Response::Ok) => {}
                Some(Response::Err { message }) => {
                    eprintln!(
                        "faceauth: {}",
                        gettext("enrollment failed: {error}").replace("{error}", &message)
                    );
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    eprintln!("faceauth: daemon not available");
                    process::exit(1);
                }
            }

            println!(
                "faceauth: {}",
                gettext("Enrollment successful for '{user}'.").replace("{user}", &username)
            );
        }

        Commands::Test { timeout } => {
            // Load model via daemon to get camera info for the user-facing message.
            // Authentication also requires the daemon, so if it is unavailable we
            // fail immediately rather than falling back to a direct disk read (which
            // would produce a misleading "Permission denied" error).
            let face_model = match daemon_request(&Request::LoadModel {
                username: username.clone(),
            }) {
                Some(Response::Model { json }) => match load_model_from_json(&json) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("faceauth: {}", e);
                        process::exit(1);
                    }
                },
                Some(Response::Err { message }) => {
                    eprintln!("faceauth: {}", message);
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    eprintln!("faceauth: daemon not available");
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
                "faceauth: {}",
                gettext("'{user}' please look at the camera ({camera}) ...")
                    .replace("{user}", &username)
                    .replace("{camera}", &camera_display)
            );

            let uid = unsafe { libc::getuid() };
            let notif_id = notify_start(uid);

            let req = Request::Authenticate {
                username: username.clone(),
                timeout_secs: timeout,
            };

            match daemon_request(&req) {
                Some(Response::Ok) => {
                    if let Some(id) = notif_id {
                        notify_result(uid, id, true);
                    }
                    println!("faceauth: {}", gettext("authentication successful"));
                    process::exit(0);
                }
                Some(Response::Err { message }) => {
                    if let Some(id) = notif_id {
                        notify_result(uid, id, false);
                    }
                    eprintln!(
                        "faceauth: {}",
                        gettext("authentication failed: {error}")
                            .replace("{error}", &message)
                    );
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    eprintln!("faceauth: daemon not available");
                    process::exit(1);
                }
            }
        }

        Commands::Clear { index } => {
            let req = Request::Clear {
                username: username.clone(),
                index,
            };

            match daemon_request(&req) {
                Some(Response::Ok) => {
                    println!(
                        "faceauth: {}",
                        gettext("Model for '{user}' removed.").replace("{user}", &username)
                    );
                }
                Some(Response::Err { message }) => {
                    eprintln!(
                        "faceauth: {}",
                        gettext("clear failed: {error}").replace("{error}", &message)
                    );
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    eprintln!("faceauth: daemon not available");
                    process::exit(1);
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

            println!("{}: {}", gettext("User"), username);
            if let Some(uid) = faceauth_core::uid_for_username(&username) {
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
            println!("{}: {}", gettext("Batches"), model.encodings.len());
        }

        Commands::Cameras => {
            let cameras = camera::print_camera_list();
            if cameras.is_empty() {
                eprintln!("faceauth: {}", gettext("no camera devices found"));
                process::exit(1);
            }
        }

        Commands::Ignore { requestor } => {
            let req = Request::SetOpt {
                username: username.clone(),
                service: requestor.clone(),
                allowed: false,
            };
            match daemon_request(&req) {
                Some(Response::Ok) => {
                    println!(
                        "faceauth: {}",
                        gettext("face authentication disabled for '{requestor}' (user '{user}').")
                            .replace("{requestor}", &requestor)
                            .replace("{user}", &username)
                    );
                }
                Some(Response::Err { message }) => {
                    eprintln!("faceauth: {}", message);
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    eprintln!("faceauth: daemon not available");
                    process::exit(1);
                }
            }
        }

        Commands::Unignore { requestor } => {
            let req = Request::SetOpt {
                username: username.clone(),
                service: requestor.clone(),
                allowed: true,
            };
            match daemon_request(&req) {
                Some(Response::Ok) => {
                    println!(
                        "faceauth: {}",
                        gettext("face authentication enabled for '{requestor}' (user '{user}').")
                            .replace("{requestor}", &requestor)
                            .replace("{user}", &username)
                    );
                }
                Some(Response::Err { message }) => {
                    eprintln!("faceauth: {}", message);
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    eprintln!("faceauth: daemon not available");
                    process::exit(1);
                }
            }
        }

        Commands::Services => {
            let req = Request::GetServices { username: username.clone() };
            match daemon_request(&req) {
                Some(Response::Services { services }) => {
                    if services.is_empty() {
                        println!("faceauth: {}", gettext("no services recorded yet"));
                    } else {
                        for entry in &services {
                            let prefix = if entry.allowed { '+' } else { '-' };
                            println!("{}{}", prefix, entry.name);
                        }
                    }
                }
                Some(Response::Err { message }) => {
                    eprintln!("faceauth: {}", message);
                    process::exit(1);
                }
                Some(_) => {
                    eprintln!("faceauth: {}", gettext("unexpected response from daemon"));
                    process::exit(1);
                }
                None => {
                    eprintln!("faceauth: daemon not available");
                    process::exit(1);
                }
            }
        }
    }
}
