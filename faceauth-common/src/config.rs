//! Administrator configuration: `/etc/faceauth/faceauth.conf`.
//!
//! Format: one `key = value` per line; `#` starts a comment. Missing keys
//! keep their built-in defaults. Unknown keys and invalid values are reported
//! as warnings and otherwise ignored, so a typo never locks anyone out.

use std::path::Path;

/// Path of the administrator configuration file.
pub const CONFIG_PATH: &str = "/etc/faceauth/faceauth.conf";

/// Upper bound for any authentication timeout, in seconds.
pub const MAX_AUTH_TIMEOUT_SECS: u32 = 30;
/// Per-capture timeout during enrollment, in seconds.
pub const ENROLL_CAPTURE_TIMEOUT_SECS: u32 = 30;
/// Number of captures stored per enrollment batch.
pub const ENROLL_CAPTURES: u32 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// PAM services allowed to use face authentication (admin allow-list).
    pub services: Vec<String>,
    /// Maximum Euclidean distance between a live and a stored encoding.
    pub threshold: f64,
    /// Default authentication timeout in seconds.
    pub timeout_secs: u32,
    /// Consecutive single-face matching frames required for success.
    pub required_matches: u32,
    /// Refuse cameras that are not greyscale-only (IR).
    pub require_ir: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            services: ["sudo", "sudo-i", "polkit-1", "kde-fingerprint"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            threshold: 0.45,
            timeout_secs: 5,
            required_matches: 3,
            require_ir: false,
        }
    }
}

impl Config {
    /// Load the system configuration file. A missing file yields defaults.
    pub fn load_system() -> (Config, Vec<String>) {
        Self::load(Path::new(CONFIG_PATH))
    }

    /// Load `path`; returns the configuration and any warnings.
    pub fn load(path: &Path) -> (Config, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), Vec::new()),
            Err(e) => (Config::default(), vec![format!("{}: {e}", path.display())]),
        }
    }

    /// Parse configuration text.
    pub fn parse(text: &str) -> (Config, Vec<String>) {
        let mut cfg = Config::default();
        let mut warnings = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {}: expected 'key = value'", n + 1));
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let result = match key {
                "services" => parse_services(value).map(|v| cfg.services = v),
                "threshold" => parse_range(value, 0.2, 0.6).map(|v| cfg.threshold = v),
                "timeout" => parse_range(value, 1, MAX_AUTH_TIMEOUT_SECS).map(|v| cfg.timeout_secs = v),
                "required_matches" => parse_range(value, 1, 10).map(|v| cfg.required_matches = v),
                "require_ir" => parse_bool(value).map(|v| cfg.require_ir = v),
                _ => Err("unknown key".to_string()),
            };
            if let Err(e) = result {
                warnings.push(format!("line {}: {key}: {e}", n + 1));
            }
        }
        (cfg, warnings)
    }

    /// True if the admin allow-list contains `service`.
    pub fn service_allowed(&self, service: &str) -> bool {
        self.services.iter().any(|s| s == service)
    }

    /// Clamp a requested timeout; 0 means "use the configured default".
    pub fn effective_timeout(&self, requested: u32) -> u32 {
        match requested {
            0 => self.timeout_secs,
            t => t.min(MAX_AUTH_TIMEOUT_SECS),
        }
    }
}

fn parse_services(value: &str) -> Result<Vec<String>, String> {
    let list: Vec<String> = value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    match list.iter().find(|s| !crate::is_valid_service_name(s)) {
        Some(bad) => Err(format!("invalid service name '{bad}'")),
        None => Ok(list),
    }
}

fn parse_range<T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy>(value: &str, min: T, max: T) -> Result<T, String> {
    let v: T = value.parse().map_err(|_| format!("invalid number '{value}'"))?;
    // `contains` also rejects NaN, which `v < min || v > max` lets through.
    if !(min..=max).contains(&v) {
        return Err(format!("{v} out of range {min}..={max}"));
    }
    Ok(v)
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" | "on" => Ok(true),
        "no" | "false" | "0" | "off" => Ok(false),
        _ => Err(format!("invalid boolean '{value}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_gives_defaults() {
        let (cfg, warnings) = Config::parse("");
        assert_eq!(cfg, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_all_keys() {
        let text = "# comment\nservices = sudo, login  sddm\nthreshold = 0.4\ntimeout=7\nrequired_matches = 2 # trailing\nrequire_ir = yes\n";
        let (cfg, warnings) = Config::parse(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.services, vec!["sudo", "login", "sddm"]);
        assert_eq!(cfg.threshold, 0.4);
        assert_eq!(cfg.timeout_secs, 7);
        assert_eq!(cfg.required_matches, 2);
        assert!(cfg.require_ir);
    }

    #[test]
    fn nan_and_infinite_thresholds_are_rejected() {
        for value in ["NaN", "nan", "inf", "-inf"] {
            let (cfg, warnings) = Config::parse(&format!("threshold = {value}\n"));
            assert_eq!(cfg.threshold, Config::default().threshold, "{value}");
            assert_eq!(warnings.len(), 1, "{value}");
        }
    }

    #[test]
    fn empty_services_disables_everything() {
        let (cfg, _) = Config::parse("services =\n");
        assert!(cfg.services.is_empty());
        assert!(!cfg.service_allowed("sudo"));
    }

    #[test]
    fn invalid_values_keep_defaults_and_warn() {
        let (cfg, warnings) = Config::parse("threshold = 0.9\ntimeout = abc\nrequire_ir = maybe\nbogus = 1\nnot a pair\nservices = sudo, x/y\n");
        assert_eq!(cfg, Config::default());
        assert_eq!(warnings.len(), 6, "{warnings:?}");
    }

    #[test]
    fn effective_timeout_clamps() {
        let cfg = Config::default();
        assert_eq!(cfg.effective_timeout(0), cfg.timeout_secs);
        assert_eq!(cfg.effective_timeout(3), 3);
        assert_eq!(cfg.effective_timeout(u32::MAX), MAX_AUTH_TIMEOUT_SECS);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let (cfg, warnings) = Config::load(Path::new("/nonexistent/faceauth.conf"));
        assert_eq!(cfg, Config::default());
        assert!(warnings.is_empty());
    }
}
