use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use faceauth_core::{authenticate_face_with_model, camera_name_for_index, error::FaceAuthError, load_model_via_daemon, AuthConfig};
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
fn notify_start(uid: u32, gid: u32, service: &str, caller: &str) -> Option<u32> {
    let mut cmd = Command::new(concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-notify"));
    cmd.args(["start", &uid.to_string(), service, caller])
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
fn notify_result(uid: u32, gid: u32, notif_id: u32, success: bool, reason: &str) {
    let subcommand = if success { "success" } else { "failure" };
    let mut cmd = Command::new(concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-notify"));
    cmd.args([subcommand, &uid.to_string(), &notif_id.to_string(), reason])
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

    let parent = parent_process_name();
    // `caller` is used in syslog; `parent` is passed to faceauth-notify separately
    // so it can format "Requested by: {service} via {parent}" without double "via".
    let caller = parent.as_deref()
        .map(|name| format!("{service} via {name}"))
        .unwrap_or_else(|| service.clone());

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

    // Show a desktop notification so the user knows to look at the camera.
    let notif_id = match (uid, gid) {
        (Some(u), Some(g)) => notify_start(u, g, &service, parent.as_deref().unwrap_or("")),
        _ => None,
    };

    // The camera index is stored in the model from enrollment time; no config needed.
    let result = authenticate_face_with_model(face_model, &AuthConfig::default());

    // Replace the in-progress notification with the outcome.
    if let (Some(u), Some(g), Some(id)) = (uid, gid, notif_id) {
        match &result {
            Ok(())  => notify_result(u, g, id, true,  ""),
            Err(e)  => notify_result(u, g, id, false, &e.to_string()),
        }
    }

    match result {
        Ok(()) => {
            let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication succeeded for {user_desc} using {camera_desc}"));
            PamError::SUCCESS
        }

        // Any failure (timeout, camera error, dlib error, etc.): pass through
        // to the next PAM module (e.g. pam_unix for password prompt).
        Err(e) => {
            let _ = pamh.syslog(LogLvl::NOTICE, &format!("[{caller}] face authentication failed for {user_desc} using {camera_desc}, skipping: {e}"));
            PamError::AUTHINFO_UNAVAIL
        }
    }
}

pamsm::pam_module!(FaceAuthPam);
