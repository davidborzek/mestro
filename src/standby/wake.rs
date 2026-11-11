//! Leaving standby: the user's own wake (a tap on the clock)
//! reveals the page they left, a remote start (MA web UI, Home
//! Assistant, a phone) opens now playing when
//! `[ui] auto_open_now_playing` is on — the music started
//! elsewhere, the kiosk follows it.

use crate::ui::navigation::Navigation;
use crate::Mestro;
use std::cell::Cell;
use std::time::{Duration, Instant};

// a tap whose power-on never arrives must not mark a later remote start as local
const LOCAL_WAKE_WINDOW: Duration = Duration::from_secs(10);

pub struct StandbyWake {
    auto_open: bool,
    was_powered: Cell<Option<bool>>,
    local_since: Cell<Option<Instant>>,
}

impl StandbyWake {
    pub fn new(auto_open: bool) -> Self {
        Self {
            auto_open,
            was_powered: Cell::new(None),
            local_since: Cell::new(None),
        }
    }

    /// the standby view's own wake: the power-on that follows is
    /// not a remote start
    pub fn local_wake(&self) {
        self.local_since.set(Some(Instant::now()));
    }

    /// after every player update: acts on the off → on edge only
    pub fn observe(&self, ui: &Mestro, nav: &Navigation) {
        let powered = ui.get_powered();
        let woke = self.was_powered.replace(Some(powered)) == Some(false) && powered;
        if !woke {
            return;
        }
        let local = self
            .local_since
            .take()
            .is_some_and(|at| at.elapsed() < LOCAL_WAKE_WINDOW);
        if !local && self.auto_open {
            nav.open_np(ui);
        }
    }
}
