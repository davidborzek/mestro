//! The boot screen's state: the kiosk starts before the network is
//! up, so the first seconds show "waiting for network" and then
//! "connecting" instead of an error banner over the standby clock.
//! The screen gives way once the source delivers events, or after
//! a grace window — then the regular banner says what is wrong and
//! the settings are reachable. A failure that a retry cannot heal
//! (a rejected token) ends it at once.

use std::time::{Duration, Instant};

/// how long the boot screen may wait for the first connection
pub const GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    WaitingForNetwork,
    Connecting,
    /// the boot screen is gone for good
    Done,
}

pub struct StartupGate {
    started: Instant,
    done: bool,
}

impl StartupGate {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            done: false,
        }
    }

    /// the screen for this moment; `network` = the kiosk has a
    /// routable address
    pub fn phase(&mut self, now: Instant, network: bool) -> Startup {
        if self.done || now >= self.started + GRACE {
            self.done = true;
            return Startup::Done;
        }
        if network {
            Startup::Connecting
        } else {
            Startup::WaitingForNetwork
        }
    }

    /// the first real events
    pub fn connected(&mut self) {
        self.done = true;
    }

    /// a failed attempt: whether its banner shows now. Transient
    /// failures stay behind the boot screen until it ends
    pub fn show_failure(&mut self, now: Instant, heals_on_retry: bool) -> bool {
        if !heals_on_retry {
            self.done = true;
        }
        self.done || now >= self.started + GRACE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_network_then_connects() {
        let start = Instant::now();
        let mut gate = StartupGate::new(start);
        assert_eq!(gate.phase(start, false), Startup::WaitingForNetwork);
        assert_eq!(
            gate.phase(start + Duration::from_secs(3), true),
            Startup::Connecting
        );
    }

    #[test]
    fn a_connection_ends_the_boot_screen_for_good() {
        let start = Instant::now();
        let mut gate = StartupGate::new(start);
        gate.connected();
        assert_eq!(
            gate.phase(start, false),
            Startup::Done,
            "a later network loss is no boot"
        );
    }

    #[test]
    fn transient_failures_stay_quiet_during_the_boot_screen() {
        let start = Instant::now();
        let mut gate = StartupGate::new(start);
        assert!(!gate.show_failure(start + Duration::from_secs(2), true));
        assert_eq!(
            gate.phase(start + Duration::from_secs(2), false),
            Startup::WaitingForNetwork
        );
    }

    #[test]
    fn the_grace_window_ends_it_and_failures_show() {
        let start = Instant::now();
        let mut gate = StartupGate::new(start);
        assert!(gate.show_failure(start + GRACE, true));
        assert_eq!(gate.phase(start + GRACE, true), Startup::Done);
    }

    #[test]
    fn a_rejected_token_ends_it_at_once() {
        let start = Instant::now();
        let mut gate = StartupGate::new(start);
        assert!(gate.show_failure(start, false));
        assert_eq!(gate.phase(start, true), Startup::Done);
    }
}
