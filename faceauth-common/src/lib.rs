//! Code shared by all faceauth components.
//!
//! This crate deliberately has no computer-vision dependencies so that the
//! PAM module (loaded into every authenticating process) and the clients stay
//! small.

/// Where translations are installed (the Makefile's LOCALEDIR).
pub const LOCALE_DIR: &str = match option_env!("FACEAUTH_LOCALE_DIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};

pub mod client;
pub mod config;
pub mod dbus;
pub mod logind;
pub mod users;
pub mod v4l;

/// Return `true` if `s` is a valid PAM service name: an ASCII alphanumeric
/// first character followed by ASCII alphanumerics, `_`, `.`, or `-`.
pub fn is_valid_service_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    s.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_service_names() {
        for name in ["sudo", "sddm", "login", "kde-fingerprint", "polkit-1", "sudo-i", "a", "s_v.c"] {
            assert!(is_valid_service_name(name), "expected valid: {name}");
        }
    }

    #[test]
    fn invalid_service_names() {
        let long = "a".repeat(65);
        for name in ["", "-sudo", "_sudo", ".sudo", "foo bar", "foo/bar", "x\n+su", "../etc", long.as_str()] {
            assert!(!is_valid_service_name(name), "expected invalid: {name:?}");
        }
    }
}
