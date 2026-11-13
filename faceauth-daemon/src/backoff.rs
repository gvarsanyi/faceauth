//! Back-off for automatic retry loops.
//!
//! Some callers retry face authentication as soon as it fails; the KDE lock
//! screen restarts its `kde-fingerprint` check immediately, which would keep
//! the camera on for as long as the screen is locked and nobody is in front
//! of it. A new attempt that starts within `RAPID_RETRY` of a failure for the
//! same user and service counts as such a retry. After `FREE_RETRIES` of
//! them, each attempt first waits (camera off) for a growing delay.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A retry this soon after a failure is automatic, not a person.
const RAPID_RETRY: Duration = Duration::from_secs(3);
/// Rapid retries that run without delay.
const FREE_RETRIES: u32 = 2;
const FIRST_DELAY: Duration = Duration::from_secs(5);
/// Upper bound; stays below the PAM module's 30 s call margin.
const MAX_DELAY: Duration = Duration::from_secs(20);

struct Failure {
    streak: u32,
    at: Instant,
}

/// Per (UID, service) failure history.
#[derive(Default)]
pub struct Backoff {
    failures: HashMap<(u32, String), Failure>,
}

impl Backoff {
    /// Start an attempt: returns its rapid-retry streak and the delay to
    /// wait before opening the camera.
    pub fn begin(&self, uid: u32, service: &str, now: Instant) -> (u32, Duration) {
        let streak = match self.failures.get(&(uid, service.to_string())) {
            Some(f) if now.saturating_duration_since(f.at) < RAPID_RETRY => f.streak + 1,
            _ => 0,
        };
        (streak, delay_for(streak))
    }

    /// Record the outcome of an attempt started with `streak`.
    pub fn finish(&mut self, uid: u32, service: &str, streak: u32, success: bool, now: Instant) {
        let key = (uid, service.to_string());
        // Only failures within RAPID_RETRY matter; drop the rest.
        self.failures.retain(|_, f| now.saturating_duration_since(f.at) < RAPID_RETRY);
        if success {
            self.failures.remove(&key);
        } else {
            self.failures.insert(key, Failure { streak, at: now });
        }
    }
}

fn delay_for(streak: u32) -> Duration {
    if streak <= FREE_RETRIES {
        return Duration::ZERO;
    }
    let doublings = (streak - FREE_RETRIES - 1).min(8);
    (FIRST_DELAY * 2u32.pow(doublings)).min(MAX_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVC: &str = "kde-fingerprint";

    /// Run `n` failing attempts `gap` apart, starting at `now`; return each
    /// attempt's delay and the time after the last one.
    fn run_loop(b: &mut Backoff, n: usize, gap: Duration, mut now: Instant) -> (Vec<u64>, Instant) {
        let delays = (0..n)
            .map(|_| {
                let (streak, delay) = b.begin(1000, SVC, now);
                now += delay + Duration::from_secs(5);
                b.finish(1000, SVC, streak, false, now);
                now += gap;
                delay.as_secs()
            })
            .collect();
        (delays, now)
    }

    fn loop_delays(b: &mut Backoff, n: usize, gap: Duration) -> Vec<u64> {
        run_loop(b, n, gap, Instant::now()).0
    }

    #[test]
    fn automatic_retry_loop_backs_off() {
        let delays = loop_delays(&mut Backoff::default(), 7, Duration::ZERO);
        assert_eq!(delays, vec![0, 0, 0, 5, 10, 20, 20]);
    }

    #[test]
    fn slow_human_retries_are_never_delayed() {
        let delays = loop_delays(&mut Backoff::default(), 5, Duration::from_secs(10));
        assert_eq!(delays, vec![0; 5]);
    }

    #[test]
    fn success_resets() {
        let mut b = Backoff::default();
        let (_, now) = run_loop(&mut b, 5, Duration::ZERO, Instant::now());
        let (streak, delay) = b.begin(1000, SVC, now);
        assert!(!delay.is_zero());
        b.finish(1000, SVC, streak, true, now);
        assert_eq!(b.begin(1000, SVC, now), (0, Duration::ZERO));
    }

    #[test]
    fn users_and_services_are_independent() {
        let mut b = Backoff::default();
        loop_delays(&mut b, 5, Duration::ZERO);
        let now = Instant::now();
        assert_eq!(b.begin(1001, SVC, now).1, Duration::ZERO);
        assert_eq!(b.begin(1000, "sudo", now).1, Duration::ZERO);
    }
}
