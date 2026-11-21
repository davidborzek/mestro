//! The mDNS announcement: `_mestro._tcp.local.` on the web port,
//! so clients (the Home Assistant integration) find the player
//! without an address. The TXT record carries the player id, the
//! app version, the protocol revision and the label.
//!
//! The announcer is stopped while the setup hotspot is in play: the
//! responder must never sit on the hotspot interface (dnsmasq owns
//! that network). The wifi manager raises a pause signal before it
//! builds the hotspot and drops it once the hotspot is gone; the
//! daemon is torn down on it and brought back after. Address
//! changes (DHCP, link up/down) are followed by the daemon itself.

use super::WebInfo;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo, MDNS_PORT};
use std::time::{Duration, Instant};

/// the service type clients browse for
pub(crate) const SERVICE_TYPE: &str = "_mestro._tcp.local.";

/// how often the pause signal is looked at; a poll is enough because
/// enter_ap takes longer than one before the hotspot address exists
const POLL: Duration = Duration::from_secs(1);
/// wait before the next start attempt after a failed one (port
/// held without sharing, no interface yet)
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30);
/// how long a goodbye or shutdown may take before the thread moves on
const STOP_WAIT: Duration = Duration::from_secs(2);

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
    let spawned = std::thread::Builder::new()
        .name("mestro-mdns".into())
        .spawn(move || supervise(&info));
    if let Err(e) = spawned {
        tracing::warn!("mdns: thread: {e}");
    }
}

fn supervise(info: &WebInfo) -> ! {
    let mut daemon: Option<ServiceDaemon> = None;
    let mut retry_at = Instant::now();
    loop {
        match step(daemon.is_some(), info.mdns_paused()) {
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
        std::thread::sleep(POLL);
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
            mdns_pause: std::sync::Arc::default(),
            ap_net: crate::config::Wifi::default().ap_net(),
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
    fn the_pause_signal_is_not_the_auth_flag() {
        let web = info();
        web.ap_mode
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(!web.mdns_paused());
        web.mdns_pause
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(web.mdns_paused());
    }
}
