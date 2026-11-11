//! When a failed reconnect becomes the visible banner: a healthy
//! connection that drops (a server restart, a WiFi roam) usually
//! comes back within seconds, so transient failures surface only
//! once they outlast a grace window. Failures a retry never heals
//! (a rejected token) show at once. The boot is startup.rs's job.

use std::time::{Duration, Instant};

/// how long transient failures stay quiet after a healthy
/// connection dropped
pub const GRACE: Duration = Duration::from_secs(30);

pub struct BannerGate {
    quiet_until: Instant,
}

impl BannerGate {
    pub fn new(now: Instant) -> Self {
        Self {
            quiet_until: now + GRACE,
        }
    }

    /// a failed attempt: whether the banner shows now
    pub fn on_failure(&self, now: Instant, heals_on_retry: bool) -> bool {
        !heals_on_retry || now >= self.quiet_until
    }

    /// real events flow: a later drop gets a fresh grace window
    /// (a server restart, a WiFi roam)
    pub fn on_connected(&mut self, now: Instant) {
        self.quiet_until = now + GRACE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_failures_stay_quiet_within_the_grace_window() {
        let start = Instant::now();
        let gate = BannerGate::new(start);
        assert!(!gate.on_failure(start, true));
        assert!(!gate.on_failure(start + Duration::from_secs(5), true));
    }

    #[test]
    fn failures_past_the_grace_window_show() {
        let start = Instant::now();
        let gate = BannerGate::new(start);
        assert!(gate.on_failure(start + GRACE, true));
    }

    #[test]
    fn a_rejected_token_shows_at_once() {
        let start = Instant::now();
        let gate = BannerGate::new(start);
        assert!(gate.on_failure(start, false));
    }

    #[test]
    fn a_drop_after_a_connection_gets_a_fresh_window() {
        let start = Instant::now();
        let mut gate = BannerGate::new(start);
        let later = start + Duration::from_secs(600);
        gate.on_connected(later);
        assert!(!gate.on_failure(later + Duration::from_secs(5), true));
        assert!(gate.on_failure(later + GRACE, true));
    }
}
