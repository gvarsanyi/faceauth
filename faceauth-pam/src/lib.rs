//! pam_faceauth: PAM module that asks faceauth-daemon to match the face of
//! `PAM_USER`.
//!
//! Every non-success outcome returns `PAM_AUTHINFO_UNAVAIL`, so the stack
//! falls through to the next module (normally the password prompt).
//!
//! Module arguments:
//! - `timeout=N`: authentication timeout in seconds (default: configured)

use std::time::Duration;

use faceauth_common::client::Client;
use faceauth_common::config::{Config, MAX_AUTH_TIMEOUT_SECS};
use faceauth_common::dbus::Error;
use faceauth_common::logind::{self, SessionInfo};
use faceauth_common::users;
use pamsm::{LogLvl, Pam, PamError, PamFlags, PamLibExt, PamMsgStyle, PamServiceModule};

struct FaceAuthPam;

impl PamServiceModule for FaceAuthPam {
    fn authenticate(pamh: Pam, _flags: PamFlags, args: Vec<String>) -> PamError {
        // A panic unwinding into the C caller is undefined behaviour.
        std::panic::catch_unwind(|| do_authenticate(&pamh, &args)).unwrap_or(PamError::AUTHINFO_UNAVAIL)
    }

    /// Face authentication establishes no credentials.
    fn setcred(_: Pam, _: PamFlags, _: Vec<String>) -> PamError {
        PamError::SUCCESS
    }
}

/// Parsed module arguments.
#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    timeout_secs: Option<u32>,
    unknown: Vec<String>,
}

fn parse_args(args: &[String]) -> Args {
    let mut parsed = Args::default();
    for arg in args {
        match arg.split_once('=') {
            Some(("timeout", v)) => match v.parse::<u32>() {
                Ok(t) if (1..=MAX_AUTH_TIMEOUT_SECS).contains(&t) => parsed.timeout_secs = Some(t),
                _ => parsed.unknown.push(arg.clone()),
            },
            _ => parsed.unknown.push(arg.clone()),
        }
    }
    parsed
}

/// Facts about the calling context used to decide whether a person is
/// sitting at this machine.
#[derive(Debug)]
struct Locality {
    rhost: Option<String>,
    /// The session that decides locality (see `gather_locality`).
    session: Option<SessionInfo>,
}

/// `Ok` if face authentication may run in this context, else the reason.
///
/// - Remote requests (`PAM_RHOST` set to anything) are refused.
/// - The session must be local, attached to a seat, and in the foreground.
///   This excludes SSH (also `sudo` inside SSH), remote desktop sessions,
///   and users who have switched away.
/// - Callers without any session (cron without pam_systemd, system services)
///   are refused.
fn check_locality(l: &Locality) -> Result<(), String> {
    if let Some(host) = l.rhost.as_deref().filter(|h| !h.is_empty()) {
        return Err(format!("remote host '{host}'"));
    }
    match &l.session {
        Some(s) => s.check_local_active(),
        None => Err("no login session".into()),
    }
}

/// PID and UID of the process at the other end of stdin, if stdin is a
/// Unix socket. polkit's socket-activated agent helper (polkit 126+) runs
/// as a system service whose stdin is the connection from the user's polkit
/// agent.
fn stdin_peer() -> Option<(u32, u32)> {
    socket_peer(0)
}

fn socket_peer(fd: libc::c_int) -> Option<(u32, u32)> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len are valid for writes of the sizes given. On a
    // non-socket the call fails; on a non-Unix socket the PID is 0.
    let rc = unsafe {
        libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len)
    };
    (rc == 0 && cred.pid > 0).then_some((cred.pid as u32, cred.uid))
}

/// The session that decides locality, in order: this process's own logind
/// session; for a process outside any session whose stdin is a Unix socket,
/// the peer's session (polkit agent); otherwise the graphical session of the
/// real UID, which setuid callers (sudo, su) keep and `PAM_USER` does not
/// influence.
fn gather_locality(pamh: &Pam, client: &Client) -> Result<Locality, String> {
    let rhost = pamh.get_rhost().ok().flatten().and_then(|c| c.to_str().ok()).map(str::to_string);
    let conn = client.connection();
    let logind_err = |e| format!("logind: {e}");
    let session = match logind::session_for_pid(conn, std::process::id()).map_err(logind_err)? {
        Some(s) => Some(s),
        None => match stdin_peer() {
            Some((pid, uid)) => logind::caller_session(conn, pid, uid).map_err(logind_err)?,
            None => logind::display_session(conn, users::current_uid()).map_err(logind_err)?,
        },
    };
    Ok(Locality { rhost, session })
}

fn log(pamh: &Pam, level: LogLvl, msg: &str) {
    let _ = pamh.syslog(level, &format!("pam_faceauth: {msg}"));
}

fn do_authenticate(pamh: &Pam, raw_args: &[String]) -> PamError {
    let args = parse_args(raw_args);
    for a in &args.unknown {
        log(pamh, LogLvl::WARNING, &format!("ignoring invalid argument '{a}'"));
    }

    // Without a usable user or service there is nothing to check; the
    // daemon treats an empty service as test mode, so it must never be sent.
    let item = |v: Result<Option<&std::ffi::CStr>, PamError>| v.ok().flatten().and_then(|c| c.to_str().ok()).map(str::to_string);
    let (Some(username), Some(service)) = (item(pamh.get_user(None)), item(pamh.get_service())) else {
        log(pamh, LogLvl::NOTICE, "skipped: PAM user or service unavailable");
        return PamError::AUTHINFO_UNAVAIL;
    };
    if username.is_empty() || service.is_empty() {
        log(pamh, LogLvl::NOTICE, "skipped: empty PAM user or service");
        return PamError::AUTHINFO_UNAVAIL;
    }
    let skip = |reason: &str| {
        log(pamh, LogLvl::NOTICE, &format!("[{service}] skipped for {username}: {reason}"));
        PamError::AUTHINFO_UNAVAIL
    };

    let timeout = args.timeout_secs.unwrap_or_else(|| Config::load_system().0.timeout_secs);
    let client = match Client::system(Duration::from_secs(u64::from(timeout) + 30)) {
        Ok(c) => c,
        Err(e) => return skip(&format!("system bus unavailable: {e}")),
    };
    match gather_locality(pamh, &client).and_then(|l| check_locality(&l)) {
        Ok(()) => {}
        Err(reason) => return skip(&format!("not a local interactive session ({reason})")),
    }

    let camera = match client.check_authentication(&username, &service) {
        Ok(c) => c,
        Err(e) => return skip(&e.message()),
    };

    log(pamh, LogLvl::NOTICE, &format!("[{service}] starting for {username} using {camera}"));
    let _ = pamh.conv(Some(&format!("Look at the camera ({camera}) to authenticate...")), PamMsgStyle::TEXT_INFO);

    match client.authenticate(&username, &service, timeout) {
        Ok(()) => {
            log(pamh, LogLvl::NOTICE, &format!("[{service}] succeeded for {username}"));
            PamError::SUCCESS
        }
        Err(e) => {
            let level = if matches!(e, Error::NoMatch(_)) { LogLvl::NOTICE } else { LogLvl::WARNING };
            log(pamh, level, &format!("[{service}] failed for {username}: {e}"));
            let _ = pamh.conv(Some(&format!("Face authentication failed: {e}")), PamMsgStyle::ERROR_MSG);
            PamError::AUTHINFO_UNAVAIL
        }
    }
}

pamsm::pam_module!(FaceAuthPam);

#[cfg(test)]
mod tests {
    use super::*;

    fn session(remote: bool, active: bool, seat: &str) -> SessionInfo {
        SessionInfo { id: "2".into(), remote, active, seat: seat.into() }
    }

    fn locality(session: Option<SessionInfo>) -> Locality {
        Locality { rhost: None, session }
    }

    #[test]
    fn args_parse_timeout_and_reject_garbage() {
        let a = parse_args(&["timeout=7".into(), "debug".into(), "timeout=999".into()]);
        assert_eq!(a.timeout_secs, Some(7));
        assert_eq!(a.unknown, vec!["debug", "timeout=999"]);
    }

    #[test]
    fn remote_host_is_refused() {
        let mut l = locality(Some(session(false, true, "seat0")));
        l.rhost = Some("10.0.0.5".into());
        assert!(check_locality(&l).is_err());
        l.rhost = Some("  ".into());
        assert!(check_locality(&l).is_err());
        l.rhost = Some(String::new());
        assert!(check_locality(&l).is_ok());
    }

    #[test]
    fn local_foreground_session_is_accepted() {
        assert!(check_locality(&locality(Some(session(false, true, "seat0")))).is_ok());
    }

    #[test]
    fn ssh_style_sessions_are_refused() {
        // SSH: remote flag set and no seat; holds for sudo inside SSH too.
        assert!(check_locality(&locality(Some(session(true, true, "")))).is_err());
        assert!(check_locality(&locality(Some(session(false, true, "")))).is_err());
    }

    #[test]
    fn background_session_is_refused() {
        assert!(check_locality(&locality(Some(session(false, false, "seat0")))).is_err());
    }

    #[test]
    fn socket_peer_reads_unix_socket_credentials() {
        use std::os::fd::AsRawFd;
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(socket_peer(a.as_raw_fd()), Some((std::process::id(), users::current_uid())));
        let file = std::fs::File::open("/proc/self/status").unwrap();
        assert_eq!(socket_peer(file.as_raw_fd()), None);
    }

    #[test]
    fn sessionless_caller_is_refused() {
        assert!(check_locality(&locality(None)).is_err());
    }
}
