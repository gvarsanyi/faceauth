//! `faceauth`: command-line front end for faceauth-daemon.

use std::io::{BufRead, Write};
use std::process;
use std::time::Duration;

use clap::{Parser, Subcommand};
use faceauth_common::client::Client;
use faceauth_common::config::{ENROLL_CAPTURES, ENROLL_CAPTURE_TIMEOUT_SECS};
use faceauth_common::dbus::{
    preferred_camera, CameraTuple, Error, ENROLLED_CAMERA, STATUS_TOO_SIMILAR, SUITABILITY_IR,
    SUITABILITY_POSSIBLE_IR,
};
use faceauth_common::{users, v4l};
use gettextrs::gettext;
use serde_json::json;

mod agent;
mod doctor;

/// Covers polkit prompts plus every enrollment capture.
const METHOD_TIMEOUT: Duration =
    Duration::from_secs(120 + (ENROLL_CAPTURES * ENROLL_CAPTURE_TIMEOUT_SECS) as u64);

#[derive(Parser)]
#[command(
    name = "faceauth",
    about = "Face authentication for Linux",
    version,
    arg_required_else_help = true
)]
struct Cli {
    /// Account to operate on (default: the current user). Managing another
    /// account requires administrator authorization.
    #[arg(long, short = 'u', global = true)]
    user: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Enroll a new batch of face captures.
    ///
    /// The daemon captures 5 encodings from different angles. The first
    /// enrollment selects the camera; later ones reuse it.
    Add {
        /// Camera: index (2), device path (/dev/video2), or udev name from
        /// /dev/v4l/by-id or /dev/v4l/by-path. Prompted for if omitted on a
        /// first enrollment in a terminal.
        #[arg(short, long)]
        camera: Option<String>,
    },

    /// Test face authentication (exit status 0 on success).
    Test {
        /// Seconds to try before failing (default: configured, max 30).
        #[arg(short, long)]
        timeout: Option<u32>,
    },

    /// Delete enrolled face data.
    Clear {
        /// Delete only this enrollment batch (0-based; see `info`).
        #[arg(short, long)]
        batch: Option<u32>,
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },

    /// Show the enrolled model.
    Info {
        #[arg(long)]
        json: bool,
    },

    /// List cameras and their suitability.
    Cameras {
        #[arg(long)]
        json: bool,
    },

    /// Show or change which services use face authentication for you.
    ///
    /// The administrator decides which PAM services may use face
    /// authentication (/etc/faceauth/faceauth.conf); you can disable any of
    /// them for your account.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

    /// Check the installation and report problems.
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    /// List services and whether face authentication is enabled.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Use face authentication for SERVICE.
    Enable { service: String },
    /// Stop using face authentication for SERVICE.
    Disable { service: String },
}

/// Print `faceauth: <msg>` to stderr and exit 1.
fn die(msg: impl AsRef<str>) -> ! {
    eprintln!("faceauth: {}", msg.as_ref());
    process::exit(1);
}

fn connect() -> Client {
    Client::from_env(METHOD_TIMEOUT).unwrap_or_else(|e| die(e.message()))
}

/// Unwrap a daemon result or exit with its message.
fn ok<T>(result: Result<T, Error>) -> T {
    result.unwrap_or_else(|e| die(e.message()))
}

fn suitability_label(s: u8) -> String {
    match s {
        SUITABILITY_IR => format!(" [{}]", gettext("IR, recommended")),
        SUITABILITY_POSSIBLE_IR => format!(" [{}]", gettext("possible IR")),
        _ => String::new(),
    }
}

fn print_cameras(cameras: &[CameraTuple], mark: Option<u32>) {
    for (index, name, suit) in cameras {
        let marker = if Some(*index) == mark { "*" } else { " " };
        let name = if name.is_empty() {
            gettext("(unknown)")
        } else {
            name.clone()
        };
        println!(
            "{marker} /dev/video{index}  {name}{}",
            suitability_label(*suit)
        );
    }
}

fn stdin_is_tty() -> bool {
    // SAFETY: trivial libc query.
    unsafe { libc::isatty(libc::STDIN_FILENO) != 0 }
}

/// Ask a yes/no question on the terminal; false on EOF or non-terminal.
fn confirm(question: &str) -> bool {
    if !stdin_is_tty() {
        return false;
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes")
}

/// Resolve a `--camera` argument to a device index.
fn parse_camera(spec: &str) -> u32 {
    if let Ok(n) = spec.parse::<u32>() {
        return n;
    }
    let resolved = if spec.starts_with('/') {
        std::fs::canonicalize(spec)
            .ok()
            .and_then(|p| p.file_name()?.to_str()?.strip_prefix("video")?.parse().ok())
    } else {
        v4l::resolve_camera(spec)
    };
    resolved.unwrap_or_else(|| die(gettext("unrecognised camera '{spec}'").replace("{spec}", spec)))
}

/// Interactively choose a camera, defaulting to the most IR-like one.
fn prompt_camera(cameras: &[CameraTuple]) -> u32 {
    let default = preferred_camera(cameras)
        .map(|i| cameras[i].0)
        .unwrap_or_else(|| die(gettext("no camera devices found")));
    if !stdin_is_tty() {
        print_cameras(cameras, None);
        die(gettext("--camera is required in non-interactive mode"));
    }
    println!("{}", gettext("Available cameras:"));
    print_cameras(cameras, Some(default));
    eprint!(
        "{}",
        gettext("Select camera (default {index}): ").replace("{index}", &default.to_string())
    );
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    match line.trim() {
        "" => default,
        s => s
            .parse()
            .ok()
            .filter(|n| cameras.iter().any(|c| c.0 == *n))
            .unwrap_or_else(|| die(gettext("invalid selection '{input}'").replace("{input}", s))),
    }
}

fn cmd_add(client: &Client, user: &str, camera: Option<String>) {
    // Only root may read another account's model; an administrator adding
    // a batch for someone else chooses the camera (the daemon refuses a
    // camera other than the enrolled one).
    let enrolled = match client.get_model(user) {
        Ok(m) => Some(m),
        Err(Error::NoModel(_) | Error::PermissionDenied(_)) => None,
        Err(e) => die(e.message()),
    };
    let index: i32 = match (camera, &enrolled) {
        (None, Some(_)) => ENROLLED_CAMERA,
        (Some(spec), _) => parse_camera(&spec) as i32,
        (None, None) => prompt_camera(&ok(client.list_cameras())) as i32,
    };
    if index >= 0 && !v4l::is_greyscale_only(index as u32) {
        eprintln!(
            "faceauth: {}",
            gettext("warning: this is not an IR camera; a photo of you may be able to unlock it")
        );
    }

    eprintln!(
        "faceauth: {}",
        gettext("enrolling face for '{user}'; look at the camera and vary your angle")
            .replace("{user}", user)
    );
    let _agent = agent::start();
    let result = client.enroll(user, index, |done, total, status| {
        let msg = if status == STATUS_TOO_SIMILAR {
            gettext("[{done}/{total}] too similar to a previous capture; turn your head slightly")
        } else {
            gettext("[{done}/{total}] captured")
        };
        eprintln!(
            "  {}",
            msg.replace("{done}", &done.to_string())
                .replace("{total}", &total.to_string())
        );
    });
    ok(result);
    println!(
        "faceauth: {}",
        gettext("Enrollment successful for '{user}'.").replace("{user}", user)
    );
}

fn cmd_test(client: &Client, user: &str, timeout: Option<u32>) {
    let camera = ok(client.check_authentication(user, ""));
    eprintln!(
        "faceauth: {}",
        gettext("look at the camera ({camera}) ...").replace("{camera}", &camera)
    );
    match client.authenticate(user, "", timeout.unwrap_or(0)) {
        Ok(()) => println!("faceauth: {}", gettext("authentication successful")),
        Err(e) => die(gettext("authentication failed: {error}").replace("{error}", &e.message())),
    }
}

fn cmd_clear(client: &Client, user: &str, batch: Option<u32>, yes: bool) {
    let question = match batch {
        Some(b) => gettext("Delete enrollment batch {batch} of '{user}'?")
            .replace("{batch}", &b.to_string()),
        None => gettext("Delete all face data of '{user}'?"),
    }
    .replace("{user}", user);
    if !yes && !confirm(&question) {
        die(gettext("not confirmed; use --yes to skip the question"));
    }
    let _agent = agent::start();
    ok(match batch {
        Some(b) => client.delete_batch(user, b),
        None => client.delete_model(user),
    });
    println!(
        "faceauth: {}",
        gettext("Face data of '{user}' removed.").replace("{user}", user)
    );
}

fn cmd_info(client: &Client, user: &str, json: bool) {
    let model = ok(client.get_model(user));
    let uid = users::by_name(user).map(|u| u.uid);
    if json {
        println!(
            "{}",
            json!({"user": user, "uid": uid, "camera": model.camera_id, "camera_index": model.camera_index, "batches": model.batches})
        );
        return;
    }
    println!("{}: {user}", gettext("User"));
    if let Some(uid) = uid {
        println!("{}: {uid}", gettext("UID"));
    }
    let device = match model.camera_index {
        Some(i) => format!("/dev/video{i}"),
        None => gettext("not connected"),
    };
    println!("{}: {} ({device})", gettext("Camera"), model.camera_id);
    println!("{}: {}", gettext("Batches"), model.batches);
}

fn cmd_cameras(client: &Client, json: bool) {
    let cameras = ok(client.list_cameras());
    if json {
        let list: Vec<_> = cameras
            .iter()
            .map(|(i, n, s)| json!({"index": i, "name": n, "suitability": s}))
            .collect();
        println!("{}", serde_json::Value::Array(list));
    } else if cameras.is_empty() {
        die(gettext("no camera devices found"));
    } else {
        print_cameras(&cameras, None);
    }
}

fn cmd_service(client: &Client, user: &str, action: ServiceAction) {
    let (service, enabled) = match action {
        ServiceAction::List { json } => {
            let services = ok(client.get_services(user));
            if json {
                let list: Vec<_> = services
                    .iter()
                    .map(|(s, on)| json!({"service": s, "enabled": on}))
                    .collect();
                println!("{}", serde_json::Value::Array(list));
            } else if services.is_empty() {
                println!("{}", gettext("No services are enabled by the administrator (/etc/faceauth/faceauth.conf)."));
            } else {
                for (s, on) in &services {
                    println!(
                        "{} {s}",
                        if *on {
                            gettext("enabled ")
                        } else {
                            gettext("disabled")
                        }
                    );
                }
            }
            return;
        }
        ServiceAction::Enable { service } => (service, true),
        ServiceAction::Disable { service } => (service, false),
    };
    let _agent = agent::start();
    ok(client.set_service_enabled(user, &service, enabled));
    let msg = if enabled {
        gettext("face authentication enabled for '{service}' (user '{user}').")
    } else {
        gettext("face authentication disabled for '{service}' (user '{user}').")
    };
    println!(
        "faceauth: {}",
        msg.replace("{service}", &service).replace("{user}", user)
    );
}

fn cmd_doctor(user: &str, json: bool) {
    let client = Client::from_env(Duration::from_secs(10));
    let checks = doctor::run(client.as_ref(), user);
    let failed = checks.iter().any(|c| c.status == doctor::Status::Fail);
    if json {
        println!("{}", serde_json::to_string(&checks).expect("serializable"));
    } else {
        for c in &checks {
            let tag = match c.status {
                doctor::Status::Ok => "ok  ",
                doctor::Status::Warn => "WARN",
                doctor::Status::Fail => "FAIL",
            };
            println!("[{tag}] {:<12} {}", c.name, c.detail);
        }
    }
    if failed {
        process::exit(1);
    }
}

fn main() {
    gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    let _ = gettextrs::bindtextdomain("faceauth", faceauth_common::LOCALE_DIR);
    let _ = gettextrs::textdomain("faceauth");

    let cli = Cli::parse();
    let user = cli.user.unwrap_or_else(|| {
        users::by_uid(users::current_uid())
            .map(|u| u.name)
            .unwrap_or_else(|| die(gettext("cannot determine username for current UID")))
    });

    match cli.command {
        Commands::Doctor { json } => cmd_doctor(&user, json),
        Commands::Add { camera } => cmd_add(&connect(), &user, camera),
        Commands::Test { timeout } => cmd_test(&connect(), &user, timeout),
        Commands::Clear { batch, yes } => cmd_clear(&connect(), &user, batch, yes),
        Commands::Info { json } => cmd_info(&connect(), &user, json),
        Commands::Cameras { json } => cmd_cameras(&connect(), json),
        Commands::Service { action } => cmd_service(&connect(), &user, action),
    }
}
