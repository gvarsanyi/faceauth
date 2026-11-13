//! Authorization policy for daemon methods (pure; no I/O).

use faceauth_common::dbus::{ACTION_MANAGE_OTHER, ACTION_MANAGE_OWN};

/// Class of operation being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Read model metadata or service settings.
    Read,
    /// Enroll, delete, or change service settings.
    Manage,
    /// Open the camera and match against the enrolled model.
    Authenticate,
}

/// What must hold before the operation may proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
    /// The caller must be authorized for this polkit action.
    Polkit(&'static str),
    /// The caller must be in a local, active seat session (logind).
    LocalSession,
}

/// Decide how `caller_uid` may perform `op` on `target_uid`'s data.
///
/// Root is always allowed (PAM modules run as root inside sudo, su, and
/// display managers). Authentication by a non-root caller is limited to the
/// caller's own account and to callers physically at the machine, so remote
/// users cannot switch on the camera.
pub fn decide(op: Op, caller_uid: u32, target_uid: u32) -> Decision {
    if caller_uid == 0 {
        return Decision::Allow;
    }
    let own = caller_uid == target_uid;
    match (op, own) {
        (Op::Read, true) => Decision::Allow,
        (Op::Manage, true) => Decision::Polkit(ACTION_MANAGE_OWN),
        (Op::Manage, false) => Decision::Polkit(ACTION_MANAGE_OTHER),
        (Op::Authenticate, true) => Decision::LocalSession,
        (Op::Read | Op::Authenticate, false) => Decision::Deny("permission denied: not your account".into()),
    }
}

/// PAM services of graphical login screens. They also unlock the screen
/// (GDM, COSMIC), but a face login gives the session no password, so the
/// keyring and encrypted home directories stay locked: these services may
/// only unlock an existing, locked session.
const LOGIN_SCREENS: [&str; 5] = ["gdm-password", "gdm-autologin", "sddm", "lightdm", "cosmic-greeter"];

/// `Err` with the reason if `service` may not be used now: login screens
/// only while the target user's session is locked.
pub fn check_unlock_only(service: &str, locked: bool) -> Result<(), String> {
    if LOGIN_SCREENS.contains(&service) && !locked {
        return Err(format!("'{service}' is a login screen; face authentication only unlocks a locked session"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 1000;
    const OTHER: u32 = 1001;

    #[test]
    fn root_is_always_allowed() {
        for op in [Op::Read, Op::Manage, Op::Authenticate] {
            assert_eq!(decide(op, 0, ME), Decision::Allow);
        }
    }

    #[test]
    fn own_account() {
        assert_eq!(decide(Op::Read, ME, ME), Decision::Allow);
        assert_eq!(decide(Op::Manage, ME, ME), Decision::Polkit(ACTION_MANAGE_OWN));
        assert_eq!(decide(Op::Authenticate, ME, ME), Decision::LocalSession);
    }

    #[test]
    fn other_account() {
        assert!(matches!(decide(Op::Read, ME, OTHER), Decision::Deny(_)));
        assert!(matches!(decide(Op::Authenticate, ME, OTHER), Decision::Deny(_)));
        assert_eq!(decide(Op::Manage, ME, OTHER), Decision::Polkit(ACTION_MANAGE_OTHER));
        // Managing root's model is "other", never "own".
        assert_eq!(decide(Op::Manage, ME, 0), Decision::Polkit(ACTION_MANAGE_OTHER));
    }

    #[test]
    fn login_screens_only_unlock() {
        assert!(check_unlock_only("gdm-password", false).is_err());
        assert!(check_unlock_only("gdm-password", true).is_ok());
        assert!(check_unlock_only("cosmic-greeter", false).is_err());
        assert!(check_unlock_only("sudo", false).is_ok());
        assert!(check_unlock_only("kde-fingerprint", true).is_ok());
    }
}
