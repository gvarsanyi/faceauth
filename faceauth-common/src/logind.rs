//! Minimal systemd-logind client used to decide whether a caller is sitting
//! at a local seat (as opposed to SSH, remote desktop, or background jobs).

use zbus::blocking::{proxy::Builder, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";
const USER_IFACE: &str = "org.freedesktop.login1.User";

/// The logind properties of a session that matter for the locality decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    /// Session was created for a remote login (SSH, remote desktop, ...).
    pub remote: bool,
    /// Session is the foreground session on its seat.
    pub active: bool,
    /// Seat name; empty for seatless sessions (SSH, machinectl shell, cron).
    pub seat: String,
}

impl SessionInfo {
    /// `Ok` if the session is local, attached to a seat, and in the
    /// foreground; otherwise the reason it is not.
    pub fn check_local_active(&self) -> Result<(), String> {
        if self.remote {
            return Err(format!("session {} is remote", self.id));
        }
        if self.seat.is_empty() {
            return Err(format!("session {} has no seat", self.id));
        }
        if !self.active {
            return Err(format!("session {} is not the active session on {}", self.id, self.seat));
        }
        Ok(())
    }
}

fn proxy<'a>(conn: &'a Connection, path: &'a str, iface: &'a str) -> zbus::Result<Proxy<'a>> {
    Builder::new(conn)
        .destination(LOGIN1)?
        .path(path)?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
}

fn session_at(conn: &Connection, path: &str) -> zbus::Result<SessionInfo> {
    let p = proxy(conn, path, SESSION_IFACE)?;
    let (seat, _): (String, OwnedObjectPath) = p.get_property("Seat")?;
    Ok(SessionInfo {
        id: p.get_property("Id")?,
        remote: p.get_property("Remote")?,
        active: p.get_property("Active")?,
        seat,
    })
}

/// The logind session that `pid` belongs to, or `None` if it has none.
pub fn session_for_pid(conn: &Connection, pid: u32) -> zbus::Result<Option<SessionInfo>> {
    let manager = proxy(conn, LOGIN1_PATH, MANAGER_IFACE)?;
    match manager.call::<_, _, OwnedObjectPath>("GetSessionByPID", &(pid,)) {
        Ok(path) => session_at(conn, path.as_str()).map(Some),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.login1.NoSessionForPID" => Ok(None),
        Err(e) => Err(e),
    }
}

/// Object path of the session logind designates as the graphical session
/// of `uid`, or `None` if the user is not logged in or has no such session.
fn display_session_path(conn: &Connection, uid: u32) -> zbus::Result<Option<OwnedObjectPath>> {
    let manager = proxy(conn, LOGIN1_PATH, MANAGER_IFACE)?;
    let user_path: OwnedObjectPath = match manager.call("GetUser", &(uid,)) {
        Ok(p) => p,
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.login1.NoSuchUser" => return Ok(None),
        Err(e) => return Err(e),
    };
    let (id, path): (String, OwnedObjectPath) = proxy(conn, user_path.as_str(), USER_IFACE)?.get_property("Display")?;
    Ok((!id.is_empty() && path.as_str() != "/").then_some(path))
}

/// The graphical session of `uid`, or `None` if it has none.
pub fn display_session(conn: &Connection, uid: u32) -> zbus::Result<Option<SessionInfo>> {
    match display_session_path(conn, uid)? {
        Some(path) => session_at(conn, path.as_str()).map(Some),
        None => Ok(None),
    }
}

/// Whether the session at `path` is locked, as reported by its screen
/// locker through `LockedHint` (KDE Plasma and GNOME set it).
pub fn locked_hint(conn: &Connection, path: &str) -> zbus::Result<bool> {
    proxy(conn, path, SESSION_IFACE)?.get_property("LockedHint")
}

/// Object path of the graphical session of `uid` if it is locked.
pub fn locked_display_session(conn: &Connection, uid: u32) -> zbus::Result<Option<String>> {
    match display_session_path(conn, uid)? {
        Some(path) if locked_hint(conn, path.as_str())? => Ok(Some(path.to_string())),
        _ => Ok(None),
    }
}

/// The session that decides whether process `pid` of user `uid` is local:
/// its own logind session, or, for processes outside any session (desktop
/// apps started by the systemd user manager, such as terminal emulators
/// under Plasma or GNOME), the user's graphical session. polkit uses the
/// same fallback.
pub fn caller_session(conn: &Connection, pid: u32, uid: u32) -> zbus::Result<Option<SessionInfo>> {
    match session_for_pid(conn, pid)? {
        Some(s) => Ok(Some(s)),
        None => display_session(conn, uid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(remote: bool, active: bool, seat: &str) -> SessionInfo {
        SessionInfo { id: "3".into(), remote, active, seat: seat.into() }
    }

    #[test]
    fn local_active_seat_session_passes() {
        assert!(session(false, true, "seat0").check_local_active().is_ok());
    }

    #[test]
    fn remote_seatless_or_inactive_sessions_fail() {
        assert!(session(true, true, "seat0").check_local_active().is_err());
        assert!(session(false, true, "").check_local_active().is_err());
        assert!(session(false, false, "seat0").check_local_active().is_err());
    }
}
