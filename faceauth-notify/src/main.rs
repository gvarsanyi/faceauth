//! Short-lived helper that sends a desktop notification on behalf of the
//! faceauth PAM module.
//!
//! The PAM module (running as root) spawns this binary after dropping
//! privileges to the authenticating user's UID/GID, with
//! `DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/<uid>/bus` set so that
//! notify-rust reaches the user's session bus.
//!
//! Usage:
//!   faceauth-notify start   <uid> <service> [<caller>]  → stdout: <notif_id>
//!   faceauth-notify success <uid> <notif_id> <service> [<caller>]
//!   faceauth-notify failure <uid> <notif_id> <service> [<caller>]
//!
//! Exits 0 on success, 1 on any error (the PAM module ignores errors).

use gettextrs::gettext;
use notify_rust::{Hint, Notification, Timeout};
use std::process;

const ICON_SVG: &[u8] = include_bytes!("../../assets/icons/faceauth.svg");
const ICON_SUCCESS_SVG: &[u8] = include_bytes!("../../assets/icons/faceauth-success.svg");
const ICON_FAIL_SVG: &[u8] = include_bytes!("../../assets/icons/faceauth-fail.svg");

/// Write an embedded icon to the user's runtime directory on first use;
/// return the path. Uses $XDG_RUNTIME_DIR if set, otherwise /run/user/{uid}.
/// Returns None on write error — caller falls back to a theme icon.
fn icon_path(data: &[u8], name: &str, uid: u32) -> Option<String> {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", uid));
    let path = format!("{}/{}", dir, name);
    if !std::path::Path::new(&path).exists() {
        std::fs::write(&path, data).ok()?;
    }
    Some(path)
}

fn requested_by_body(service: &str, caller: &str) -> String {
    if caller.is_empty() || caller == service {
        gettext("Requested by: {service}").replace("{service}", service)
    } else {
        gettext("Requested by: {service} via {caller}")
            .replace("{service}", service)
            .replace("{caller}", caller)
    }
}

fn main() {
    gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain("faceauth", "/usr/share/locale").expect("failed to bind text domain");
    gettextrs::textdomain("faceauth").expect("failed to set text domain");

    let args: Vec<String> = std::env::args().collect();

    if args.len() < 3 {
        process::exit(1);
    }

    let subcommand = args[1].as_str();

    let uid: u32 = args[2].parse().unwrap_or(0);

    match subcommand {
        "start" => {
            let service = args.get(3).map(|s| s.as_str()).unwrap_or("unknown");
            let caller = args.get(4).map(|s| s.as_str()).unwrap_or("");
            // Optional timeout_ms argument: lets the caller set an expiry that
            // matches the auth timeout. Defaults to 8 s, which covers the
            // standard 5 s PAM auth timeout plus buffer for the replacement
            // notification to arrive. Using Never would leave a stale popup
            // when the screen is locked (KDE holds notifications until unlock,
            // by which time the replacement may have already been discarded).
            let timeout_ms: u32 = args.get(5)
                .and_then(|s| s.parse().ok())
                .unwrap_or(8000);

            let mut notif = Notification::new();
            notif
                .summary(&gettext("Face Authentication ..."))
                .body(&requested_by_body(service, caller))
                .timeout(Timeout::Milliseconds(timeout_ms));
            if let Some(p) = icon_path(ICON_SVG, "faceauth-icon.svg", uid) {
                notif.hint(Hint::ImagePath(p));
            }
            let handle = match notif.show() {
                Ok(h) => h,
                Err(_) => process::exit(1),
            };

            // Print the notification ID so the PAM module can replace it later.
            println!("{}", handle.id());
        }

        "success" => {
            let notif_id: u32 = match args.get(3).and_then(|s| s.parse().ok()) {
                Some(id) => id,
                None => process::exit(1),
            };
            let service = args.get(4).map(|s| s.as_str()).unwrap_or("unknown");
            let caller = args.get(5).map(|s| s.as_str()).unwrap_or("");

            let mut notif = Notification::new();
            notif
                .id(notif_id)
                .summary(&gettext("Face Authentication Successful"))
                .body(&requested_by_body(service, caller))
                .timeout(Timeout::Milliseconds(5000));
            if let Some(p) = icon_path(ICON_SUCCESS_SVG, "faceauth-icon-success.svg", uid) {
                notif.hint(Hint::ImagePath(p));
            }
            let _ = notif.show();
        }

        "failure" => {
            let notif_id: u32 = match args.get(3).and_then(|s| s.parse().ok()) {
                Some(id) => id,
                None => process::exit(1),
            };
            let service = args.get(4).map(|s| s.as_str()).unwrap_or("unknown");
            let caller = args.get(5).map(|s| s.as_str()).unwrap_or("");

            let mut notif = Notification::new();
            notif
                .id(notif_id)
                .summary(&gettext("Face Authentication Failed"))
                .body(&requested_by_body(service, caller))
                .timeout(Timeout::Milliseconds(5000));
            if let Some(p) = icon_path(ICON_FAIL_SVG, "faceauth-icon-fail.svg", uid) {
                notif.hint(Hint::ImagePath(p));
            }
            let _ = notif.show();
        }

        _ => process::exit(1),
    }
}
