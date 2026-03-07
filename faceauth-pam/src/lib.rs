use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use faceauth_core::{
    authenticate_via_daemon, camera_name_for_index, check_service_via_daemon, error::FaceAuthError,
    load_model_via_daemon, record_caller_via_daemon,
};
use pamsm::{Pam, PamError, PamFlags, PamLibExt, PamMsgStyle, PamServiceModule, LogLvl};

struct FaceAuthPam;

impl PamServiceModule for FaceAuthPam {
    fn authenticate(pamh: Pam, _flags: PamFlags, _args: Vec<String>) -> PamError {
        // Wrap the entire body in catch_unwind: a Rust panic unwinding through
        // the C PAM stack is undefined behaviour and will crash the calling process.
        let result = std::panic::catch_unwind(|| do_authenticate(&pamh));

        match result {
            Ok(err) => err,
            Err(_) => PamError::AUTH_ERR,
        }
    }
}

/// True if `PAM_RHOST` is set and non-empty, meaning the authentication
/// request came from a remote host (e.g. SSH, rlogin).
fn is_remote_session(pamh: &Pam) -> bool {
    pamh.get_rhost()
        .ok()
        .flatten()
        .and_then(|c| c.to_str().ok())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}


/// Return the name of the calling process's parent by reading /proc.
fn parent_process_name() -> Option<String> {
    let ppid = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("PPid:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<u32>()
                .ok()
        })?;
    std::fs::read_to_string(format!("/proc/{}/comm", ppid))
        .ok()
        .map(|s| s.trim().to_string())
}


/// Read the PPid field from /proc/<pid>/status, or None if unreadable.
fn ppid_of(pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{}/status", pid))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("PPid:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<u32>()
                .ok()
        })
}

/// True if any ancestor process in the chain is named "sshd".
///
/// PAM_RHOST is only set for the direct SSH login; when the user runs `sudo`
/// inside an SSH session, PAM_RHOST is unset and `has_controlling_tty()`
/// returns true (SSH allocates a real PTY). Walking the process tree and
/// checking for an sshd parent catches this case reliably.
fn has_sshd_ancestor() -> bool {
    let mut pid = ppid_of(std::process::id());
    while let Some(p) = pid {
        if p <= 1 {
            break;
        }
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", p))
            .ok()
            .map(|s| s.trim().to_string());
        if comm.as_deref() == Some("sshd") {
            return true;
        }
        pid = ppid_of(p);
    }
    false
}

/// Look up the primary GID for a UID via getpwuid_r.
fn gid_for_uid(uid: u32) -> Option<u32> {
    let mut buf = vec![0u8; 1024];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: buf is valid for the call duration; result points into pwd/buf
    // and is not used after buf is dropped.
    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if ret == 0 && !result.is_null() {
        Some(unsafe { (*result).pw_gid })
    } else {
        None
    }
}

/// Spawn faceauth-notify as the given user and send a "start" notification.
///
/// Returns the notification ID printed to stdout by the helper, or None if
/// the helper is unavailable or the user has no graphical session. All errors
/// are silently swallowed — authentication must never depend on notifications.
fn notify_start(uid: u32, gid: u32, service: &str, caller: &str, auth_timeout_secs: u32) -> Option<u32> {
    // Give the notification a timeout slightly longer than the auth attempt so
    // it auto-dismisses if the replacement notification is never delivered
    // (e.g. when KDE holds notifications while the screen is locked).
    let notif_timeout_ms = (auth_timeout_secs + 3) * 1000;
    let mut cmd = Command::new(concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-notify"));
    cmd.args(["start", &uid.to_string(), service, caller, &notif_timeout_ms.to_string()])
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path=/run/user/{uid}/bus"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // Drop to the authenticating user's UID/GID before exec so the helper
    // can connect to their D-Bus session bus.
    // SAFETY: setgid/setuid are async-signal-safe; this runs in the child
    // between fork and exec where only async-signal-safe calls are allowed.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setgid(gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setuid(uid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd.spawn().ok()?;
    let mut output = String::new();
    child.stdout.as_mut()?.read_to_string(&mut output).ok()?;
    child.wait().ok()?;
    output.trim().parse().ok()
}

/// Spawn faceauth-notify to replace the in-progress notification with the result.
/// Fire-and-forget: the child is not waited on.
fn notify_result(uid: u32, gid: u32, notif_id: u32, success: bool, service: &str, caller: &str) {
    let subcommand = if success { "success" } else { "failure" };
    let mut cmd = Command::new(concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-notify"));
    cmd.args([subcommand, &uid.to_string(), &notif_id.to_string(), service, caller])
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path=/run/user/{uid}/bus"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: same as notify_start.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setgid(gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setuid(uid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let _ = cmd.spawn(); // fire-and-forget: don't wait
}

/// Core authentication logic: load model, print status, run face match, send notification.
///
/// Returns `PamError::SUCCESS` on match, `PamError::AUTHINFO_UNAVAIL` on any
/// failure so the PAM stack falls through to the next module (e.g. password).
fn do_authenticate(pamh: &Pam) -> PamError {
    // For privilege-escalation services (sudo, su), PAM_RUSER is the invoking
    // user — the one whose face we should check. PAM_USER is the target (root),
    // which has no enrolled model. Fall back to PAM_USER if RUSER is unset.
    let username: String = {
        let ruser = pamh.get_ruser().ok().flatten()
            .and_then(|c| c.to_str().ok())
            .map(|s| s.to_string());
        match ruser {
            Some(u) if !u.is_empty() => u,
            _ => {
                let user_cstr = match pamh.get_user(None) {
                    Ok(Some(u)) => u,
                    Ok(None) => return PamError::USER_UNKNOWN,
                    Err(_) => return PamError::AUTH_ERR,
                };
                match user_cstr.to_str() {
                    Ok(s) => s.to_string(),
                    Err(_) => return PamError::USER_UNKNOWN,
                }
            }
        }
    };
    let username = username.as_str();

    let uid = faceauth_core::uid_for_username(username);
    let gid = uid.and_then(gid_for_uid);
    let user_desc = match uid {
        Some(uid) => format!("{username} (uid: {uid})"),
        None => username.to_string(),
    };

    let service = pamh.get_service().ok().flatten()
        .and_then(|c| c.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    record_caller_via_daemon(&service);

    let parent = parent_process_name();
    // `caller` is used in syslog; `parent` is passed to faceauth-notify separately
    // so it can format "Requested by: {service} via {parent}" without double "via".
    let caller = parent.as_deref()
        .map(|name| format!("{service} via {name}"))
        .unwrap_or_else(|| service.clone());

    // Refuse to attempt face authentication for remote or unattended sessions.
    // These checks happen before touching the daemon to fail fast.
    if is_remote_session(pamh) {
        let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication skipped for {user_desc}: remote session"));
        return PamError::AUTHINFO_UNAVAIL;
    }
    if has_sshd_ancestor() {
        let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication skipped for {user_desc}: running inside SSH session"));
        return PamError::AUTHINFO_UNAVAIL;
    }

    // Check per-user and global opt files via the daemon (which runs as root and
    // can read /etc/security/faceauth/ regardless of directory permissions).
    // Service must be opted in (+) to proceed.
    if !check_service_via_daemon(username, &service) {
        let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication skipped for {user_desc}: service not opted in"));
        return PamError::AUTHINFO_UNAVAIL;
    }

    // Load the model via the daemon so the PAM module does not need direct
    // access to /etc/security/faceauth/ (which is only readable by faceauthd).
    let face_model = match load_model_via_daemon(username) {
        Ok(m) => m,
        Err(FaceAuthError::ModelNotFound(_)) => {
            let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] no face model enrolled for {user_desc}, skipping"));
            return PamError::AUTHINFO_UNAVAIL;
        }
        Err(e) => {
            // Storage or other infrastructure error: don't block login.
            let _ = pamh.syslog(LogLvl::ERR, &format!("[{caller}] could not load face model for {user_desc}, skipping: {e}"));
            return PamError::AUTHINFO_UNAVAIL;
        }
    };

    let camera_index = face_model.camera.index;
    let camera_desc = match &face_model.camera.by_id {
        Some(id) => format!("/dev/video{camera_index} ({id})"),
        None => format!("/dev/video{camera_index}"),
    };
    let camera_name = camera_name_for_index(camera_index);
    let camera_display = if camera_name.is_empty() {
        format!("/dev/video{camera_index}")
    } else {
        format!("{camera_name} at /dev/video{camera_index}")
    };

    let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication starting for {user_desc} using {camera_desc}"));

    // Inform the user via the PAM conversation function (appears on the
    // terminal for sudo/su/ssh; graphical PAM agents may show it in their UI).
    let _ = pamh.conv(
        Some(&format!("faceauth: '{username}' please look at the camera ({camera_display}) ...")),
        PamMsgStyle::TEXT_INFO,
    );

    const AUTH_TIMEOUT_SECS: u32 = 5;

    // Show a desktop notification so the user knows to look at the camera.
    let notif_id = match (uid, gid) {
        (Some(u), Some(g)) => notify_start(u, g, &service, parent.as_deref().unwrap_or(""), AUTH_TIMEOUT_SECS),
        _ => None,
    };

    // Delegate authentication to the daemon: it owns the encoder and camera.
    let result = authenticate_via_daemon(username, AUTH_TIMEOUT_SECS as u64);

    // Replace the in-progress notification with the outcome.
    if let (Some(u), Some(g), Some(id)) = (uid, gid, notif_id) {
        let caller_str = parent.as_deref().unwrap_or("");
        match &result {
            Ok(())  => notify_result(u, g, id, true,  &service, caller_str),
            Err(_)  => notify_result(u, g, id, false, &service, caller_str),
        }
    }

    match result {
        Ok(()) => {
            let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication succeeded for {user_desc} using {camera_desc}"));
            let _ = pamh.conv(Some("faceauth: authentication successful"), PamMsgStyle::TEXT_INFO);
            PamError::SUCCESS
        }

        // Any failure (timeout, camera error, dlib error, etc.): pass through
        // to the next PAM module (e.g. pam_unix for password prompt).
        Err(e) => {
            let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication failed for {user_desc} using {camera_desc}, skipping: {e}"));
            let _ = pamh.conv(Some(&format!("faceauth: authentication failed: {e}")), PamMsgStyle::ERROR_MSG);
            PamError::AUTHINFO_UNAVAIL
        }
    }
}

pamsm::pam_module!(FaceAuthPam);

#[cfg(test)]
mod tests {
    use super::*;

    /// ppid_of(current pid) must return our own parent's PID (which is always > 0).
    #[test]
    fn ppid_of_current_process_is_nonzero() {
        let ppid = ppid_of(std::process::id());
        assert!(ppid.is_some(), "could not read /proc/self/status");
        assert!(ppid.unwrap() > 0);
    }

    /// The test runner always has a parent process with a non-empty name.
    #[test]
    fn parent_process_name_is_nonempty() {
        let name = parent_process_name();
        assert!(name.is_some(), "could not read parent process name from /proc");
        assert!(!name.unwrap().is_empty());
    }

    /// has_sshd_ancestor must complete without panicking regardless of environment.
    #[test]
    fn has_sshd_ancestor_does_not_panic() {
        let _ = has_sshd_ancestor();
    }
}
