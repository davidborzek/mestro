//! The mDNS announcement: `_mestro._tcp.local.` on the web port,
//! so clients (the Home Assistant integration) find the player
//! without an address. The TXT record carries the player id, the
//! app version, the protocol revision and the label.
//!
//! The announcer is stopped while the setup hotspot is in play: the
//! responder must never sit on the hotspot interface (dnsmasq owns
//! that network). The wifi manager asks the [`MdnsGate`] to pause
//! before it builds the hotspot and waits until the announcer
//! confirms the daemon is shut down; it resumes the gate once the
//! hotspot is gone and the daemon is brought back. Address changes
//! (DHCP, link up/down) are followed by the daemon itself.

use super::WebInfo;
use crate::locks;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo, MDNS_PORT};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// the service type clients browse for
pub(crate) const SERVICE_TYPE: &str = "_mestro._tcp.local.";

/// the longest the announcer sleeps between looks at the gate (a
/// pause request wakes it at once)
const POLL: Duration = Duration::from_secs(1);
/// wait before the next start attempt after a failed one (port
/// held without sharing, no interface yet)
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30);
/// how long a goodbye or shutdown may take before the thread moves on
const STOP_WAIT: Duration = Duration::from_secs(2);

/// where the announcer stands in the pause handshake
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GatePhase {
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

/// the pause handshake between the wifi manager and the announcer:
/// the manager requests a pause and blocks until the announcer
/// confirms the daemon is shut down (or a timeout), so the
/// responder is provably off the hotspot interface before the
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
pub(crate) struct AttachedWorker(Arc<MdnsGate>);

impl Drop for AttachedWorker {
    fn drop(&mut self) {
        locks::lock(&self.0.state).worker_attached = false;
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
        let mut state = locks::lock(&self.state);
        if state.phase == GatePhase::Running {
            state.phase = GatePhase::PauseRequested;
            self.changed.notify_all();
        }
        let (_state, waited) = self
            .changed
            .wait_timeout_while(state, timeout, |s| {
                s.phase != GatePhase::Paused && s.worker_attached
            })
            .unwrap_or_else(PoisonError::into_inner);
        !waited.timed_out()
    }

    /// manager side: the hotspot is gone, the announcer may start again
    pub fn resume(&self) {
        let mut state = locks::lock(&self.state);
        if state.phase != GatePhase::Running {
            state.phase = GatePhase::Running;
            self.changed.notify_all();
        }
    }

    /// announcer side: register the thread; the guard unregisters it
    pub(crate) fn attach(self: &Arc<Self>) -> AttachedWorker {
        locks::lock(&self.state).worker_attached = true;
        AttachedWorker(self.clone())
    }

    /// announcer side: the daemon must not run (requested or confirmed)
    fn silenced(&self) -> bool {
        locks::lock(&self.state).phase != GatePhase::Running
    }

    /// announcer side: sleep up to `timeout`; a pause request, a
    /// resume or the timeout ends it. A request already pending
    /// returns at once
    fn wait_for_change(&self, timeout: Duration) {
        let state = locks::lock(&self.state);
        if state.phase == GatePhase::PauseRequested {
            return;
        }
        drop(
            self.changed
                .wait_timeout(state, timeout)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    /// announcer side: the daemon is shut down — answer a pending request
    fn confirm_paused(&self) {
        let mut state = locks::lock(&self.state);
        if state.phase == GatePhase::PauseRequested {
            state.phase = GatePhase::Paused;
            self.changed.notify_all();
        }
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> GatePhase {
        locks::lock(&self.state).phase
    }
}

/// what the supervisor does this tick
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Start,
    Stop,
    Keep,
}

/// the announce decision: announce exactly while not paused
fn step(running: bool, paused: bool) -> Step {
    match (running, paused) {
        (false, false) => Step::Start,
        (true, true) => Step::Stop,
        _ => Step::Keep,
    }
}

/// the service record for this player. The instance and host names
/// derive from the id (unique and stable); the addresses stay empty
/// and follow the host's interfaces
fn service_info(info: &WebInfo) -> Result<ServiceInfo, mdns_sd::Error> {
    let instance = format!("mestro-{}", info.id);
    let host = format!("{instance}.local.");
    let properties = [
        ("id", info.id.as_str()),
        ("version", info.app_version.as_str()),
        ("protocol", &super::server::PROTOCOL_VERSION.to_string()),
        ("name", info.name.as_str()),
    ];
    Ok(ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host,
        "",
        info.port,
        &properties[..],
    )?
    .enable_addr_auto())
}

/// start the supervisor thread; a failure to spawn warns and leaves
/// the player unannounced (it stays reachable by address)
pub(crate) fn spawn(info: WebInfo) {
    // attached before the thread runs: a pause request after this
    // point waits for the thread's confirmation
    let attached = info.mdns_gate.attach();
    let spawned = std::thread::Builder::new()
        .name("mestro-mdns".into())
        .spawn(move || supervise(&info, attached));
    if let Err(e) = spawned {
        tracing::warn!("mdns: thread: {e}");
    }
}

fn supervise(info: &WebInfo, _attached: AttachedWorker) -> ! {
    let gate = &info.mdns_gate;
    let mut daemon: Option<ServiceDaemon> = None;
    let mut retry_at = Instant::now();
    loop {
        let paused = gate.silenced();
        match step(daemon.is_some(), paused) {
            Step::Start if Instant::now() >= retry_at => match start(info, MDNS_PORT) {
                Ok(started) => daemon = Some(started),
                Err(e) => {
                    tracing::warn!("mdns: not announcing: {e}");
                    retry_at = Instant::now() + RETRY_AFTER_FAILURE;
                }
            },
            Step::Stop => {
                if let Some(running) = daemon.take() {
                    tracing::info!("mdns: hotspot up, announcement stopped");
                    stop(&running);
                }
            }
            Step::Start | Step::Keep => {}
        }
        // only now is the daemon known to be down
        if paused {
            gate.confirm_paused();
        }
        gate.wait_for_change(POLL);
    }
}

fn start(info: &WebInfo, mdns_port: u16) -> Result<ServiceDaemon, mdns_sd::Error> {
    let record = service_info(info)?;
    let fullname = record.get_fullname().to_string();
    let daemon = ServiceDaemon::new_with_port(mdns_port)?;
    // the web server binds 0.0.0.0 only: an IPv6 address in the
    // record would be unreachable for the client that picks it
    if let Err(e) = daemon
        .disable_interface(IfKind::IPv6)
        .and_then(|()| daemon.register(record))
    {
        stop(&daemon);
        return Err(e);
    }
    tracing::info!("mdns: announcing {fullname} on port {}", info.port);
    Ok(daemon)
}

/// shut the daemon down; its goodbye packets go out first
fn stop(daemon: &ServiceDaemon) {
    if let Ok(done) = daemon.shutdown() {
        let _ = done.recv_timeout(STOP_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::fake_announcer::confirming_worker;
    use super::*;
    use std::collections::HashMap;

    fn info() -> WebInfo {
        WebInfo {
            port: 8787,
            pin: String::new(),
            app_version: "0.1.0 (abc1234)".into(),
            id: "a8724d0c4171".into(),
            name: "Wohnzimmer".into(),
            display_supported: true,
            ap_mode: std::sync::Arc::default(),
            mdns_gate: std::sync::Arc::default(),
            ap_net: crate::config::Wifi::default().ap_net(),
            audio: crate::config::Audio::default(),
            system: crate::config::System::default(),
        }
    }

    fn txt(record: &ServiceInfo) -> HashMap<String, String> {
        record
            .get_properties()
            .iter()
            .map(|p| (p.key().to_string(), p.val_str().to_string()))
            .collect()
    }

    #[test]
    fn the_record_carries_type_port_and_the_txt_keys() {
        let record = service_info(&info()).unwrap();
        assert_eq!(record.get_type(), "_mestro._tcp.local.");
        assert_eq!(record.get_port(), 8787);
        assert_eq!(
            record.get_fullname(),
            "mestro-a8724d0c4171._mestro._tcp.local."
        );
        assert_eq!(
            txt(&record),
            HashMap::from([
                ("id".to_string(), "a8724d0c4171".to_string()),
                ("version".to_string(), "0.1.0 (abc1234)".to_string()),
                ("protocol".to_string(), "1".to_string()),
                ("name".to_string(), "Wohnzimmer".to_string()),
            ])
        );
        // no fixed addresses: the daemon follows the interfaces
        assert!(record.is_addr_auto());
        assert!(record.get_addresses().is_empty());
    }

    #[test]
    fn a_browser_resolves_the_announcement() {
        // a private mDNS port keeps the test off any system responder
        let port = 25_353;
        let mut web = info();
        web.port = 4242;
        let announcer = start(&web, port).unwrap();
        let browser = ServiceDaemon::new_with_port(port).unwrap();
        let events = browser.browse(SERVICE_TYPE).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut resolved = None;
        while resolved.is_none() && Instant::now() < deadline {
            if let Ok(mdns_sd::ServiceEvent::ServiceResolved(service)) =
                events.recv_timeout(Duration::from_millis(200))
            {
                resolved = Some(service);
            }
        }
        let service = resolved.expect("the service resolves");
        assert_eq!(service.get_port(), 4242);
        assert_eq!(service.get_property_val_str("id"), Some("a8724d0c4171"));
        assert_eq!(service.get_property_val_str("protocol"), Some("1"));
        assert_eq!(service.get_property_val_str("name"), Some("Wohnzimmer"));
        stop(&announcer);
        stop(&browser);
    }

    #[test]
    fn the_daemon_announces_no_ipv6_address() {
        // the web server binds 0.0.0.0 only: an IPv6 address in
        // the record would be unreachable
        let announcer = start(&info(), 25_354).unwrap();
        let status = announcer.status().unwrap().recv_timeout(STOP_WAIT).unwrap();
        stop(&announcer);
        let mdns_sd::DaemonStatus::Running(state) = status else {
            panic!("the daemon is not running");
        };
        assert!(
            state.enabled_intfs().iter().all(|i| i.ipv6().is_none()),
            "{:?}",
            state.enabled_intfs()
        );
    }

    #[test]
    fn the_port_follows_the_web_port() {
        let mut on_80 = info();
        on_80.port = 80;
        assert_eq!(service_info(&on_80).unwrap().get_port(), 80);
    }

    #[test]
    fn announcing_follows_the_pause_signal() {
        // not paused: start, then stay
        assert_eq!(step(false, false), Step::Start);
        assert_eq!(step(true, false), Step::Keep);
        // paused: stop what runs, never start
        assert_eq!(step(true, true), Step::Stop);
        assert_eq!(step(false, true), Step::Keep);
    }

    #[test]
    fn the_pause_gate_is_not_the_auth_flag() {
        let web = info();
        web.ap_mode
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(!web.mdns_gate.silenced());
        web.mdns_gate.pause_and_wait(Duration::ZERO);
        assert!(web.mdns_gate.silenced());
    }

    #[test]
    fn pause_and_wait_returns_true_once_the_worker_confirms() {
        let gate = Arc::new(MdnsGate::default());
        let _worker = confirming_worker(&gate);
        assert!(gate.pause_and_wait(Duration::from_secs(5)));
        assert_eq!(gate.phase(), GatePhase::Paused);
    }

    #[test]
    fn pause_and_wait_times_out_with_a_stuck_worker() {
        let gate = Arc::new(MdnsGate::default());
        // attached, but never confirms
        let _stuck = gate.attach();
        let started = Instant::now();
        assert!(!gate.pause_and_wait(Duration::from_millis(100)));
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(gate.phase(), GatePhase::PauseRequested);
    }

    #[test]
    fn pause_and_wait_returns_at_once_without_a_worker() {
        let gate = MdnsGate::default();
        let started = Instant::now();
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
        let started = Instant::now();
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
        let _worker = confirming_worker(&gate);
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
