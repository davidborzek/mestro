//! The mDNS pause handshake between the wifi manager and the web
//! endpoint's announcer: the manager requests a pause and blocks
//! until the announcer confirms its daemon is shut down (or a
//! timeout), so the responder is provably off the hotspot interface
//! before the hotspot is built. The announcer side (the supervisor
//! loop in `remote/web/mdns`) attaches to the gate and confirms
//! once its daemon is down; the manager resumes the gate when the
//! hotspot is gone and the station network is back. Always
//! compiled — the announcer runs in every build, the manager
//! (feature `wifi-setup`) is the other side of the handshake.

use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::time::Duration;

/// where the announcer stands in the pause handshake
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePhase {
    Running,
    /// the wifi manager asked; the daemon may still be up
    PauseRequested,
    /// the announcer confirmed: the daemon is shut down
    Paused,
}

#[derive(Debug)]
struct GateState {
    phase: GatePhase,
    /// an announcer thread is attached — without one nothing needs
    /// to be waited for
    worker_attached: bool,
}

/// the pause handshake between the wifi manager and the mDNS
/// announcer: the manager requests a pause and blocks until the
/// announcer confirms the daemon is shut down (or a timeout), so
/// the responder is provably off the hotspot interface before the
/// hotspot is built
#[derive(Debug)]
pub struct MdnsGate {
    state: Mutex<GateState>,
    changed: Condvar,
}

impl Default for MdnsGate {
    fn default() -> Self {
        Self {
            state: Mutex::new(GateState {
                phase: GatePhase::Running,
                worker_attached: false,
            }),
            changed: Condvar::new(),
        }
    }
}

/// held by the announcer thread while it lives; dropping it (thread
/// gone) releases a waiting manager
pub struct AttachedWorker(Arc<MdnsGate>);

impl Drop for AttachedWorker {
    fn drop(&mut self) {
        self.0.state.lock().worker_attached = false;
        self.0.changed.notify_all();
    }
}

impl MdnsGate {
    /// manager side: request the pause and block until the announcer
    /// confirms the shutdown. True when confirmed or when no
    /// announcer runs (nothing to wait for); false on timeout — the
    /// caller goes on anyway, a stuck announcer must never keep the
    /// player from its setup hotspot. A request made before the
    /// announcer attaches is honored once it does
    pub fn pause_and_wait(&self, timeout: Duration) -> bool {
        let mut state = self.state.lock();
        if state.phase == GatePhase::Running {
            state.phase = GatePhase::PauseRequested;
            self.changed.notify_all();
        }
        self.changed.wait_while_for(
            &mut state,
            |s| s.phase != GatePhase::Paused && s.worker_attached,
            timeout,
        );
        // the wait condition negated: false only when it timed out still waiting
        state.phase == GatePhase::Paused || !state.worker_attached
    }

    /// manager side: the hotspot is gone, the announcer may start again
    pub fn resume(&self) {
        let mut state = self.state.lock();
        if state.phase != GatePhase::Running {
            state.phase = GatePhase::Running;
            self.changed.notify_all();
        }
    }

    /// announcer side: register the thread; the guard unregisters it
    pub fn attach(self: &Arc<Self>) -> AttachedWorker {
        self.state.lock().worker_attached = true;
        AttachedWorker(self.clone())
    }

    /// announcer side: the daemon must not run (requested or confirmed)
    pub fn silenced(&self) -> bool {
        self.state.lock().phase != GatePhase::Running
    }

    /// announcer side: sleep up to `timeout`; a pause request, a
    /// resume or the timeout ends it. A request already pending
    /// returns at once
    pub fn wait_for_change(&self, timeout: Duration) {
        let mut state = self.state.lock();
        if state.phase == GatePhase::PauseRequested {
            return;
        }
        self.changed.wait_for(&mut state, timeout);
    }

    /// announcer side: the daemon is shut down — answer a pending request
    pub fn confirm_paused(&self) {
        let mut state = self.state.lock();
        if state.phase == GatePhase::PauseRequested {
            state.phase = GatePhase::Paused;
            self.changed.notify_all();
        }
    }

    /// the current phase (tests on both sides of the handshake)
    pub fn phase(&self) -> GatePhase {
        self.state.lock().phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_and_wait_returns_true_once_the_worker_confirms() {
        let gate = Arc::new(MdnsGate::default());
        let _worker = fake_announcer::confirming_worker(&gate);
        assert!(gate.pause_and_wait(Duration::from_secs(5)));
        assert_eq!(gate.phase(), GatePhase::Paused);
    }

    #[test]
    fn pause_and_wait_times_out_with_a_stuck_worker() {
        let gate = Arc::new(MdnsGate::default());
        // attached, but never confirms
        let _stuck = gate.attach();
        let started = std::time::Instant::now();
        assert!(!gate.pause_and_wait(Duration::from_millis(100)));
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(gate.phase(), GatePhase::PauseRequested);
    }

    #[test]
    fn pause_and_wait_returns_at_once_without_a_worker() {
        let gate = MdnsGate::default();
        let started = std::time::Instant::now();
        assert!(gate.pause_and_wait(Duration::from_secs(60)));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(gate.silenced());
    }

    #[test]
    fn a_worker_that_dies_releases_the_waiting_manager() {
        let gate = Arc::new(MdnsGate::default());
        let attached = gate.attach();
        let dying = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(attached);
        });
        let started = std::time::Instant::now();
        assert!(gate.pause_and_wait(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(4));
        dying.join().unwrap();
    }

    #[test]
    fn a_request_before_the_worker_attaches_is_honored() {
        let gate = Arc::new(MdnsGate::default());
        assert!(gate.pause_and_wait(Duration::from_secs(5)));
        let _late = gate.attach();
        assert!(gate.silenced(), "the late announcer must not start");
    }

    #[test]
    fn resume_lets_the_announcer_run_again() {
        let gate = Arc::new(MdnsGate::default());
        let _worker = fake_announcer::confirming_worker(&gate);
        assert!(gate.pause_and_wait(Duration::from_secs(5)));
        assert!(gate.silenced());
        gate.resume();
        assert!(!gate.silenced());
        assert_eq!(gate.phase(), GatePhase::Running);
        // and a second round works the same
        assert!(gate.pause_and_wait(Duration::from_secs(5)));
        assert_eq!(gate.phase(), GatePhase::Paused);
    }
}

/// a stand-in announcer for tests: answers pause requests like
/// `supervise` does once its daemon is down
#[cfg(test)]
pub(crate) mod fake_announcer {
    use super::*;

    /// stops (and joins) when dropped
    pub(crate) struct ConfirmingWorker {
        stop: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for ConfirmingWorker {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub(crate) fn confirming_worker(gate: &Arc<MdnsGate>) -> ConfirmingWorker {
        let attached = gate.attach();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (gate, flag) = (gate.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let _attached = attached;
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                if gate.silenced() {
                    gate.confirm_paused();
                }
                gate.wait_for_change(Duration::from_millis(10));
            }
        });
        ConfirmingWorker {
            stop,
            thread: Some(thread),
        }
    }
}
