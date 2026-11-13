//! faceauth-agent: per-session agent that shows a desktop notification while
//! faceauth-daemon checks this user's face (sudo, polkit, lock screen, tests).
//!
//! Started by XDG autostart in graphical sessions. It registers with the
//! daemon (`RegisterAgent`) and turns the daemon's unicast
//! `AuthenticationStarted`/`AuthenticationFinished` signals into
//! notifications. Only one instance runs per session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use faceauth_common::client::Client;
use faceauth_common::dbus::{AGENT_BUS_NAME, BUS_NAME};
use faceauth_common::{logind, users};
use gettextrs::gettext;
use notify_rust::{Hint, Notification, Timeout};
use zbus::blocking::fdo::DBusProxy;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

const ICON_SVG: &[u8] = include_bytes!("../../assets/icons/faceauth.svg");
const ICON_SUCCESS_SVG: &[u8] = include_bytes!("../../assets/icons/faceauth-success.svg");
const ICON_FAIL_SVG: &[u8] = include_bytes!("../../assets/icons/faceauth-fail.svg");

// Breeze icon-theme names used in place of the bundled SVGs when present,
// so the notification matches the user's icon theme (light/dark, size).
const THEME_ICON: &str = "edit-image-face-show-symbolic";
const THEME_ICON_SUCCESS: &str = "edit-image-face-detect-symbolic";
const THEME_ICON_FAIL: &str = "edit-image-face-recognize-symbolic";

/// Write an embedded icon to $XDG_RUNTIME_DIR on first use and return the
/// path. Returns None if the directory is unset or not writable, in which
/// case no icon is shown.
fn icon_path(data: &[u8], name: &str) -> Option<String> {
    let dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let path = format!("{dir}/{name}");
    if !std::path::Path::new(&path).exists() {
        std::fs::write(&path, data).ok()?;
    }
    Some(path)
}

/// Base directories searched for icon themes, per the freedesktop icon
/// theme spec (XDG_DATA_DIRS/icons, ~/.local/share/icons, ~/.icons).
fn icon_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/icons"));
        dirs.push(PathBuf::from(&home).join(".icons"));
    }
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("icons")),
    );
    dirs
}

/// Icon-theme subdirectories that hold action icons, per the freedesktop
/// icon theme layout used by Breeze and Adwaita.
const ACTION_SUBDIRS: [&str; 6] = ["actions/symbolic", "actions/16", "actions/22", "actions/24", "actions/32", "symbolic/actions"];

/// True if an installed icon theme provides `name`. Probes a fixed set of
/// paths per theme instead of walking the tree, so it stays fast.
fn theme_icon_exists(name: &str) -> bool {
    icon_theme_dirs().iter().any(|base| {
        let Ok(themes) = std::fs::read_dir(base) else { return false };
        themes.flatten().any(|theme| {
            ACTION_SUBDIRS.iter().any(|sub| {
                ["svg", "png"].iter().any(|ext| theme.path().join(sub).join(format!("{name}.{ext}")).is_file())
            })
        })
    })
}

/// Set the notification icon, preferring the system icon-theme entry
/// `theme_name` if installed and falling back to the bundled `data` SVG
/// (written to the user's runtime dir as `fallback_name`).
fn set_icon(notif: &mut Notification, theme_name: &str, data: &[u8], fallback_name: &str) {
    if theme_icon_exists(theme_name) {
        notif.icon(theme_name);
    } else if let Some(p) = icon_path(data, fallback_name) {
        notif.hint(Hint::ImagePath(p));
    }
}

/// How long the result notification stays visible.
const RESULT_TIMEOUT_MS: u32 = 5000;

/// Name of the parent of `pid` (e.g. the shell that ran sudo); `None` if
/// unknown or if the parent is the service manager.
fn parent_name(pid: u32) -> Option<String> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let ppid: u32 = status.lines().find_map(|l| l.strip_prefix("PPid:"))?.trim().parse().ok()?;
    if ppid <= 1 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{ppid}/comm")).ok()?;
    Some(comm.trim().to_string()).filter(|c| !c.is_empty() && c != "systemd")
}

fn requested_by_body(service: &str, caller: Option<&str>) -> String {
    match caller {
        Some(caller) if caller != service => gettext("Requested by: {service} via {caller}")
            .replace("{service}", service)
            .replace("{caller}", caller),
        _ => gettext("Requested by: {service}").replace("{service}", service),
    }
}

/// A running face check: its service, and the notification (ID, body) if
/// one is shown.
struct Check {
    service: String,
    notification: Option<(u32, String)>,
}

type Running = Arc<Mutex<HashMap<u32, Check>>>;

/// True if this user's graphical session is locked. Lock screens (KDE
/// Plasma) do not show notifications; they would only pile up in the
/// history after unlocking.
fn session_locked(system: &zbus::blocking::Connection) -> bool {
    logind::locked_display_session(system, users::current_uid()).is_ok_and(|s| s.is_some())
}

fn on_started(running: &Running, system: &zbus::blocking::Connection, id: u32, service: &str, caller_pid: u32, timeout_secs: u32) {
    let service = if service.is_empty() { "faceauth" } else { service };
    let caller = (caller_pid != 0).then(|| parent_name(caller_pid)).flatten();
    let locked = session_locked(system);
    // Logged at the journal's default (info) level for later analysis.
    eprintln!(
        "faceauth-agent: face check {id} started for {service}{}{}",
        caller.as_deref().map(|c| format!(" via {c}")).unwrap_or_default(),
        if locked { " (screen locked; no notification)" } else { "" }
    );
    // Record the check first: showing the notification can block, and the
    // result must still find it.
    running
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(id, Check { service: service.to_string(), notification: None });
    if locked {
        return;
    }
    let body = requested_by_body(service, caller.as_deref());
    // Expire shortly after the timeout in case the result never comes.
    let mut notif = Notification::new();
    notif
        .summary(&gettext("Face Authentication ..."))
        .body(&body)
        .timeout(Timeout::Milliseconds((timeout_secs + 3) * 1000));
    set_icon(&mut notif, THEME_ICON, ICON_SVG, "faceauth-icon.svg");
    if let Ok(handle) = notif.show() {
        if let Some(check) = running.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&id) {
            check.notification = Some((handle.id(), body));
        }
    }
}

fn on_finished(running: &Running, id: u32, success: bool) {
    let Some(check) = running.lock().unwrap_or_else(|p| p.into_inner()).remove(&id) else { return };
    eprintln!(
        "faceauth-agent: face check {id} for {} {}",
        check.service,
        if success { "succeeded" } else { "failed" }
    );
    let Some((notif_id, body)) = check.notification else { return };
    let (summary, theme_icon, svg, fallback) = if success {
        (gettext("Face Authentication Successful"), THEME_ICON_SUCCESS, ICON_SUCCESS_SVG, "faceauth-icon-success.svg")
    } else {
        (gettext("Face Authentication Failed"), THEME_ICON_FAIL, ICON_FAIL_SVG, "faceauth-icon-fail.svg")
    };
    let mut notif = Notification::new();
    notif.id(notif_id).summary(&summary).body(&body).timeout(Timeout::Milliseconds(RESULT_TIMEOUT_MS));
    set_icon(&mut notif, theme_icon, svg, fallback);
    let _ = notif.show();
}

fn register(client: &Client) {
    if let Err(e) = client.register_agent() {
        eprintln!("faceauth-agent: cannot register with the daemon: {e}");
    }
}

fn main() {
    gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    let _ = gettextrs::bindtextdomain("faceauth", faceauth_common::LOCALE_DIR);
    let _ = gettextrs::textdomain("faceauth");

    let fail = |msg: String| -> ! {
        eprintln!("faceauth-agent: {msg}");
        process::exit(1)
    };
    let session = zbus::blocking::Connection::session().unwrap_or_else(|e| fail(format!("session bus: {e}")));
    match DBusProxy::new(&session).and_then(|p| Ok(p.request_name(AGENT_BUS_NAME.try_into()?, RequestNameFlags::DoNotQueue.into())?)) {
        Ok(RequestNameReply::PrimaryOwner) => {}
        Ok(_) => process::exit(0), // already running in this session
        Err(e) => fail(format!("cannot claim {AGENT_BUS_NAME}: {e}")),
    }
    let client = Arc::new(Client::from_env(Duration::from_secs(25)).unwrap_or_else(|e| fail(e.to_string())));
    // logind is on the system bus even when the daemon runs on the session bus.
    let system = zbus::blocking::Connection::system().unwrap_or_else(|e| fail(format!("system bus: {e}")));
    let running = Running::default();

    // Subscribe before registering so no signal is missed.
    let started = client.proxy().receive_signal("AuthenticationStarted").unwrap_or_else(|e| fail(e.to_string()));
    let finished = client.proxy().receive_signal("AuthenticationFinished").unwrap_or_else(|e| fail(e.to_string()));
    let owners = DBusProxy::new(client.connection())
        .and_then(|p| p.receive_name_owner_changed_with_args(&[(0, BUS_NAME)]))
        .unwrap_or_else(|e| fail(e.to_string()));
    register(&client);

    let r = running.clone();
    let t1 = thread::spawn(move || {
        for msg in started {
            if let Ok((id, service, pid, secs)) = msg.body().deserialize::<(u32, String, u32, u32)>() {
                on_started(&r, &system, id, &service, pid, secs);
            }
        }
    });
    let r = running.clone();
    let t2 = thread::spawn(move || {
        for msg in finished {
            if let Ok((id, success)) = msg.body().deserialize::<(u32, bool)>() {
                on_finished(&r, id, success);
            }
        }
    });
    // A restarted daemon has no registrations: register with each new
    // instance. If it stopped, activate it again so the next check is seen.
    for change in owners {
        let Ok(args) = change.args() else { continue };
        if args.new_owner().is_none() {
            thread::sleep(Duration::from_secs(2));
        }
        register(&client);
    }
    let _ = (t1.join(), t2.join());
}
