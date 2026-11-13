//! `faceauth doctor`: end-to-end setup diagnostics.

use serde::Serialize;

use faceauth_common::client::Client;
use faceauth_common::config::{Config, CONFIG_PATH};
use faceauth_common::dbus::Error;
use faceauth_common::v4l;

const AGENT: &str = concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-agent");
const SETUP_TOOL: &str = concat!(env!("FACEAUTH_LIBEXEC_DIR"), "/faceauth-setup");
/// Directories searched for PAM service files, in precedence order.
const PAM_DIRS: [&str; 3] = ["/etc/pam.d", "/usr/lib/pam.d", "/usr/etc/pam.d"];
/// Directories where the PAM module may be installed.
const PAM_MODULE_DIRS: [&str; 5] = [
    "/usr/lib64/security",
    "/usr/lib/security",
    "/lib/security",
    "/usr/lib/x86_64-linux-gnu/security",
    "/usr/lib/aarch64-linux-gnu/security",
];

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

fn check(name: &str, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name: name.to_string(),
        status,
        detail: detail.into(),
    }
}

/// Read a PAM service file from the standard locations.
fn read_pam_service(name: &str) -> Option<String> {
    if !faceauth_common::is_valid_service_name(name) {
        return None;
    }
    PAM_DIRS
        .iter()
        .find_map(|d| std::fs::read_to_string(format!("{d}/{name}")).ok())
}

/// True if the `auth` stack of `service` loads pam_faceauth.so, directly or
/// through `include`/`substack`/`@include`. `read` returns a service file.
pub fn auth_stack_has_faceauth(service: &str, read: &dyn Fn(&str) -> Option<String>) -> bool {
    fn walk(service: &str, read: &dyn Fn(&str) -> Option<String>, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        let Some(text) = read(service) else {
            return false;
        };
        text.lines().any(|raw| {
            let line = raw.split('#').next().unwrap_or("").trim();
            let mut tokens = line.split_whitespace();
            let Some(first) = tokens.next() else {
                return false;
            };
            if first == "@include" {
                return tokens.next().is_some_and(|t| walk(t, read, depth + 1));
            }
            if first.trim_start_matches('-') != "auth" {
                return false;
            }
            let Some(mut control) = tokens.next().map(str::to_string) else {
                return false;
            };
            // Bracketed controls may contain spaces: [success=1 default=ignore]
            while control.starts_with('[') && !control.ends_with(']') {
                match tokens.next() {
                    Some(t) => control = format!("{control} {t}"),
                    None => return false,
                }
            }
            let Some(target) = tokens.next() else {
                return false;
            };
            match control.as_str() {
                "include" | "substack" => walk(target, read, depth + 1),
                _ => target.rsplit('/').next() == Some("pam_faceauth.so"),
            }
        })
    }
    walk(service, read, 0)
}

/// Run all checks for `user`.
pub fn run(client: Result<&Client, &Error>, user: &str) -> Vec<Check> {
    let mut checks = Vec::new();

    let (config, warnings) = Config::load_system();
    checks.push(if warnings.is_empty() {
        check(
            "config",
            Status::Ok,
            format!("{CONFIG_PATH}: services = {}", config.services.join(", ")),
        )
    } else {
        check("config", Status::Warn, warnings.join("; "))
    });

    match PAM_MODULE_DIRS
        .iter()
        .map(|d| format!("{d}/pam_faceauth.so"))
        .find(|p| std::path::Path::new(p).exists())
    {
        Some(p) => checks.push(check("pam-module", Status::Ok, p)),
        None => checks.push(check(
            "pam-module",
            Status::Fail,
            "pam_faceauth.so not found in the PAM module directories",
        )),
    }

    for service in &config.services {
        let (status, detail) = if read_pam_service(service).is_none() {
            (Status::Warn, format!("no PAM service file for '{service}'"))
        } else if auth_stack_has_faceauth(service, &read_pam_service) {
            (
                Status::Ok,
                format!("'{service}' auth stack includes pam_faceauth.so"),
            )
        } else {
            (Status::Warn, format!("'{service}' auth stack does not include pam_faceauth.so (run: sudo {SETUP_TOOL} configure)"))
        };
        checks.push(check("pam-service", status, detail));
    }

    let cameras = v4l::list_cameras();
    checks.push(
        match (cameras.is_empty(), cameras.iter().any(|c| c.greyscale_only)) {
            (true, _) => check("cameras", Status::Fail, "no usable camera found"),
            (false, true) => check(
                "cameras",
                Status::Ok,
                format!("{} camera(s), IR available", cameras.len()),
            ),
            (false, false) => check(
                "cameras",
                Status::Warn,
                format!(
                    "{} camera(s), none IR: a photo can fool a colour camera",
                    cameras.len()
                ),
            ),
        },
    );

    // No session bus (e.g. over SSH): nothing to check.
    if let Ok(running) = faceauth_common::client::agent_running() {
        checks.push(if running {
            check("agent", Status::Ok, "notification agent running in this session")
        } else {
            check(
                "agent",
                Status::Warn,
                format!("faceauth-agent is not running in this session; no notifications are shown (log in again or start {AGENT})"),
            )
        });
    }

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            checks.push(check("daemon", Status::Fail, e.message()));
            return checks;
        }
    };
    match client.version() {
        Ok(v) => checks.push(check(
            "daemon",
            Status::Ok,
            format!("faceauth-daemon {v} reachable"),
        )),
        Err(e) => {
            checks.push(check("daemon", Status::Fail, e.message()));
            return checks;
        }
    }

    match client.get_model(user) {
        Ok(m) => checks.push(match m.camera_index {
            Some(i) => check(
                "model",
                Status::Ok,
                format!("{} batch(es), camera /dev/video{i} present", m.batches),
            ),
            None => check(
                "model",
                Status::Fail,
                format!("enrolled camera '{}' is not connected", m.camera_id),
            ),
        }),
        Err(Error::NoModel(_)) => checks.push(check(
            "model",
            Status::Warn,
            "no face enrolled; run 'faceauth add'",
        )),
        Err(e) => checks.push(check("model", Status::Fail, e.message())),
    }

    if let Ok(services) = client.get_services(user) {
        let enabled: Vec<&str> = services
            .iter()
            .filter(|(_, on)| *on)
            .map(|(s, _)| s.as_str())
            .collect();
        checks.push(if enabled.is_empty() {
            check(
                "services",
                Status::Warn,
                "face authentication is disabled for every service",
            )
        } else {
            check(
                "services",
                Status::Ok,
                format!("enabled for: {}", enabled.join(", ")),
            )
        });
        if enabled.contains(&"sudo") && last_sudo_attempt_was_for_root() {
            checks.push(check(
                "sudo",
                Status::Warn,
                "sudo authenticates root (targetpw/rootpw), so face authentication cannot approve it; \
                 make sudo authenticate the invoking user (openSUSE: sudo-policy-wheel-auth-self)",
            ));
        }
    }
    checks
}

/// True if the most recent pam_faceauth entry for sudo in this boot's
/// journal was skipped because sudo authenticated root. Needs read access
/// to the system journal; false if unavailable.
fn last_sudo_attempt_was_for_root() -> bool {
    let out = std::process::Command::new("journalctl")
        .args(["-b", "-q", "--no-pager", "-o", "cat", "-n", "1", "-g"])
        .arg(r"pam_faceauth: \[sudo(-i)?\] (starting|skipped) for ")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    out.is_ok_and(|o| is_root_skip(&String::from_utf8_lossy(&o.stdout)))
}

/// True if a pam_faceauth journal line reports a sudo attempt for root.
fn is_root_skip(line: &str) -> bool {
    line.contains("] skipped for root:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn reader(files: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = files
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn root_skip_lines() {
        assert!(is_root_skip("pam_faceauth: [sudo] skipped for root: no face model enrolled\n"));
        assert!(!is_root_skip("pam_faceauth: [sudo] starting for greg using IR camera\n"));
        assert!(!is_root_skip("pam_faceauth: [sudo] skipped for greg: remote session\n"));
        assert!(!is_root_skip(""));
    }

    #[test]
    fn direct_module_line() {
        let read = reader(&[(
            "sudo",
            "auth sufficient pam_faceauth.so\nauth required pam_unix.so\n",
        )]);
        assert!(auth_stack_has_faceauth("sudo", &read));
    }

    #[test]
    fn opensuse_include_chain() {
        let read = reader(&[
            (
                "sudo",
                "#%PAM-1.0\nauth include common-auth\naccount include common-account\n",
            ),
            ("common-auth", "auth include common-auth-pc\n"),
            (
                "common-auth-pc",
                "auth required pam_env.so\nauth sufficient pam_faceauth.so\n",
            ),
        ]);
        assert!(auth_stack_has_faceauth("sudo", &read));
    }

    #[test]
    fn debian_at_include_and_bracket_control() {
        let read = reader(&[
            ("sudo", "@include common-auth\n"),
            ("common-auth", "auth [success=end default=ignore] /usr/lib/x86_64-linux-gnu/security/pam_faceauth.so\n"),
        ]);
        assert!(auth_stack_has_faceauth("sudo", &read));
    }

    #[test]
    fn non_auth_and_commented_lines_do_not_count() {
        let read = reader(&[(
            "svc",
            "# auth sufficient pam_faceauth.so\naccount required pam_faceauth.so\n",
        )]);
        assert!(!auth_stack_has_faceauth("svc", &read));
    }

    #[test]
    fn include_cycles_terminate() {
        let read = reader(&[("a", "auth include b\n"), ("b", "auth include a\n")]);
        assert!(!auth_stack_has_faceauth("a", &read));
    }
}
