//! `Config::parse` must never panic, and every accepted value must respect
//! the documented ranges.
#![no_main]

use faceauth_common::config::{Config, MAX_AUTH_TIMEOUT_SECS};
use faceauth_common::is_valid_service_name;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let (cfg, _warnings) = Config::parse(text);
    assert!((0.2..=0.6).contains(&cfg.threshold));
    assert!((1..=10).contains(&cfg.required_matches));
    assert!((1..=MAX_AUTH_TIMEOUT_SECS).contains(&cfg.timeout_secs));
    assert!(cfg.services.iter().all(|s| is_valid_service_name(s)));
    assert!(cfg.effective_timeout(u32::MAX) <= MAX_AUTH_TIMEOUT_SECS);
});
