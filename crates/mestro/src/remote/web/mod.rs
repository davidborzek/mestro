//! The embedded web endpoint: one port, two faces — `GET /ws`
//! upgrades to the JSON protocol for browser clients, everything
//! else serves the PWA (the Vite build `web/dist`, embedded at
//! compile time): static assets, SPA fallback, pairing, cover.
//! The remote-control state reaches the sockets through the push
//! pipeline ([`push`]).
//!
//! Threading: the socket side runs on its own tokio thread;
//! settings and slint handles are `!Sync`, so every state op
//! round-trips onto the UI thread through the `web-op` callback
//! behind the [`UiPort`] (the binary implements it with slint).
//! MA calls (`ma`) are plain HTTP and run on the socket thread.
//!
//! Auth: the kiosk URL carries a pairing PIN in the hash
//! (`#pair=…` — never logged by HTTP middlemen); the PWA trades
//! it once for the `mestro_pair` cookie (see the `pair` module),
//! and either opens `/ws`, `/ota` and `/cover`.

mod identity;
pub use identity::device_suffix;
mod mdns;
pub mod push;
pub(crate) mod server;
mod state;

// wifi's own types (the ops channel, the shutdown/children
// registry, the mDNS pause gate) live in `mestro-os`;
// re-exported here under the paths the web modules historically
// use
pub use mestro_os::wifi::{Children, MdnsGate, WifiChannel, WifiRequest, WifiShutdown, WIFI_QUEUE};
pub use push::{Events, Push, UiView};

/// MA's player id of this device's own squeezelite (see
/// [`identity::own_player_id`])
pub fn own_player_id() -> Option<String> {
    identity::own_player_id(std::path::Path::new("/sys/class/net"))
}

use mestro_player::source::SourceCommand;

/// what the `player` op touches on the UI thread: the player
/// shell (volume bookkeeping) and the provider command channel.
/// Built in main.rs before the web endpoint wires up
#[derive(Clone)]
pub struct PlayerCtx {
    pub shell: mestro_player::shell::Shell,
    pub cmd_tx: tokio::sync::mpsc::UnboundedSender<SourceCommand>,
}

use std::sync::Arc;

pub use server::ma::{AuthMode, MaAuth, PlayerSummary, ServerInfo};

/// the endpoint's own knobs — the `[remote]` section distilled
/// into plain data, so the web side never sees the config crate
/// (the binary converts)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSettings {
    /// serve the wizard + WS channel at all
    pub enabled: bool,
    /// TCP port on all interfaces
    pub port: u16,
}

/// what the web thread needs from the UI side: op round-trips
/// and progress notifications. The settings and slint handles
/// live on the UI thread (`!Sync`) — the port is the only way
/// across. The binary implements it against the `web-op`
/// callback (slint's event loop); tests use a fake
pub trait UiPort: Send + Sync + 'static {
    /// one op round-trip. Blocking from the caller's view —
    /// implementations with a thread hop must bound it (today:
    /// 10 s), so callers wrap it in [`op_async`]
    fn op(&self, envelope: &str) -> Result<String, String>;

    /// fire-and-forget UI update (progress hints must never
    /// stall the web runtime)
    fn notify(&self, update: UiUpdate);
}

/// the updates [`UiPort::notify`] carries: today only the OTA
/// overlay status (the text and whether it is a failure — a
/// failure clears itself after a while)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiUpdate {
    UpdateStatus { text: String, failure: bool },
}

/// [`UiPort::op`] as an async boundary: the blocking UI
/// round-trip runs on the blocking pool — one slow or hung UI
/// op must not freeze the web runtime for every client
pub async fn op_async(port: &Arc<dyn UiPort>, envelope: String) -> Result<String, String> {
    let port = Arc::clone(port);
    tokio::task::spawn_blocking(move || port.op(&envelope))
        .await
        .map_err(|_| "blocking pool gone".to_string())?
}

#[cfg(test)]
pub(crate) mod test_port {
    //! a [`UiPort`] to no UI for the server tests: every route
    //! they exercise must never reach the UI thread, and one
    //! that does fails fast instead of hanging for the timeout
    use super::{UiPort, UiUpdate};
    use std::sync::Arc;

    struct Detached;

    impl UiPort for Detached {
        fn op(&self, _envelope: &str) -> Result<String, String> {
            Err("ui unavailable".into())
        }

        fn notify(&self, _update: UiUpdate) {}
    }

    pub(crate) fn detached() -> Arc<dyn UiPort> {
        Arc::new(Detached)
    }
}

/// device-side facts the protocol reports (wizard chrome)
#[derive(Debug, Clone)]
pub struct WebInfo {
    /// port + pairing PIN; empty PIN = the endpoint is off
    pub port: u16,
    pub pin: String,
    pub app_version: String,
    /// the stable player id (a hardware MAC, 12 lowercase hex
    /// chars) — `hello` and the mDNS announcement carry it
    pub id: String,
    /// the player's label: the configured MA player name, else
    /// the hostname
    pub name: String,
    /// a backlight exists (hello capabilities + the remote's
    /// display row)
    pub display_supported: bool,
    /// the wifi manager's AP-mode flag: true while the kiosk runs
    /// its setup hotspot — the web thread routes the captive
    /// portal and relaxes the pairing gate on it (feature
    /// `wifi-setup`; a build without it never sets it)
    pub ap_mode: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// the wifi manager's pause handshake with the mDNS announcer:
    /// the announcer is confirmed down before the hotspot is built
    /// and resumes when the hotspot is gone. Separate from
    /// `ap_mode`, the auth trust, which only opens once the
    /// hotspot is confirmed up
    pub mdns_gate: std::sync::Arc<MdnsGate>,
    /// the hotspot's subnet (`[wifi] ap-address`, ApNet) — the
    /// peer-side half of the AP trust boundary: a connection is
    /// hotspot-trusted when its peer address lies inside and the
    /// hotspot is confirmed up
    pub ap_net: mestro_os::wifi::ApNet,
    /// what the diagnostics report collects
    pub diagnostics: server::diagnostics::Sources,
}

impl WebInfo {
    /// the AP-mode shortcut the server routes on
    pub fn ap(&self) -> bool {
        self.ap_mode.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// the port in URL form: nothing on 80 (the default port, the
    /// kiosk image), ":8787" on every other port
    fn port_suffix(&self) -> String {
        url_port_suffix(self.port)
    }

    /// the wizard URL including the auth fragment, for the SPA
    /// footer and the kiosk's connection page. Built per call: the
    /// kiosk usually starts before DHCP has handed out an address
    pub fn remote_url(&self) -> String {
        if self.pin.is_empty() {
            return String::new();
        }
        format!(
            "http://{}{}/#pair={}",
            mestro_os::lan_ip(),
            self.port_suffix(),
            self.pin
        )
    }
}

/// the device facts + the wizard URL (for the kiosk connection
/// page and the SPA chrome). Computed BEFORE `wire` — see
/// [`serve`]. With the endpoint disabled the URL is empty and
/// no PIN file is written (the first-run overlay then points at
/// config.toml instead of a dead end). `player` is the configured
/// MA player name — the label falls back to the hostname, then
/// "mestro" (the binary owns the config, this takes the
/// distilled values)
pub fn info(
    settings: &WebSettings,
    player: &str,
    app_version: &str,
    display_supported: bool,
    ap_net: mestro_os::wifi::ApNet,
) -> (WebInfo, String) {
    tracing::info!(
        "web: start: enabled={} port={}",
        settings.enabled,
        settings.port
    );
    let pin = if settings.enabled {
        pairing_pin()
    } else {
        String::new()
    };
    let hostname = identity::hostname();
    let info = WebInfo {
        port: settings.port,
        pin: pin.clone(),
        app_version: app_version.to_string(),
        id: identity::device_id(
            std::path::Path::new("/sys/class/net"),
            &mestro_os::paths::state_dir(),
            &hostname,
        ),
        name: identity::label(player, &hostname),
        display_supported,
        ap_mode: Arc::default(),
        mdns_gate: Arc::default(),
        ap_net,
        diagnostics: server::diagnostics::Sources::image(),
    };
    // the wizard URL carries the pairing PIN — the kiosk connection
    // page shows it too (restart-stable secret for a LAN device,
    // persisted 0600)
    tracing::info!("web: wizard url: {}", info.remote_url());
    (info, pin)
}

/// the port in URL form: nothing on 80 (the URL default, the
/// kiosk image's port), ":<port>" on every other port — every
/// URL mestro shows or advertises (the wizard URL, the captive
/// portal URI, the hotspot overlay's address) uses it
pub fn url_port_suffix(port: u16) -> String {
    if port == 80 {
        String::new()
    } else {
        format!(":{port}")
    }
}

/// where a connection came from: a peer inside the hotspot's
/// subnet (and not the gateway itself) while the hotspot is
/// confirmed up may run the setup vocabulary unpaired — the setup
/// hotspot is the trust boundary, not the listener that accepted.
/// The wifi ops re-check the distilled verdict (`ap_trusted`) at
/// execution; the request type itself lives in `mestro-os`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingress {
    /// an AP-trusted peer (`Ingress::of_peer`)
    Ap,
    /// everything else: paired-only
    Lan,
}

impl Ingress {
    /// the per-connection verdict from the peer's address: AP
    /// while the hotspot is confirmed up AND the peer lies inside
    /// the hotspot subnet AND it is not the gateway itself.
    /// The address rule cannot be spoofed from outside: a TCP
    /// handshake needs our SYN-ACK to reach the peer, and a route
    /// into the hotspot subnet exists only over the hotspot
    /// interface — a LAN host cannot complete a connection with a
    /// forged 10.42.0.x source (a probe dies at the handshake)
    pub fn of_peer(peer: std::net::SocketAddr, info: &WebInfo) -> Self {
        if !info.ap() {
            return Self::Lan;
        }
        // IPv4-mapped IPv6 peers normalize to the plain v4 form
        let std::net::IpAddr::V4(v4) = peer.ip().to_canonical() else {
            return Self::Lan;
        };
        if v4 != info.ap_net.ip && info.ap_net.contains(v4) {
            Self::Ap
        } else {
            Self::Lan
        }
    }
}

/// spawn the endpoint thread. Call AFTER [`wire`] — a request in
/// the start/wire window would hit an unset `web-op` handler.
/// Errors (port busy, no permission) warn and continue — the
/// kiosk must boot regardless.
// the wiring is wide by nature (facts + the ports + the push
// pipeline) — same allowance as the socket plumbing
#[allow(clippy::too_many_arguments)]
pub fn serve(
    settings: &WebSettings,
    info: WebInfo,
    ui: std::sync::Arc<dyn UiPort>,
    ma_auth: std::sync::Arc<dyn MaAuth>,
    pin: String,
    events: Events,
    push: Push,
    wifi: WifiChannel,
) {
    if !settings.enabled {
        return;
    }
    let port = settings.port;
    mdns::spawn(info.clone());
    let spawned = std::thread::Builder::new()
        .name("mestro-web".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!("web: tokio runtime: {e}");
                    return;
                }
            };
            runtime.block_on(async move {
                // the push pipeline's own background tasks: the view
                // watcher (change-driven, the binary publishes the
                // snapshots) and the coalescing flush
                tokio::spawn(push.clone().observe_views());
                tokio::spawn(push.clone().run_flush());
                server::serve(port, info, ui, ma_auth, pin, events, wifi).await
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("web: thread: {e}");
    }
}

/// the pairing PIN: stable across restarts (the URL a phone
/// typed must survive app updates) — device-local like the
/// settings overlay, persisted 0600.
pub(crate) fn pairing_pin() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let path = mestro_os::paths::state_dir().join("web-pin");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let pin = existing.trim().to_string();
        if pin.len() == 8 && pin.bytes().all(|b| ALPHABET.contains(&b)) {
            return pin;
        }
    }
    let mut bytes = [0u8; 8];
    // NB: read_exact, never fs::read — /dev/urandom is an endless
    // device, a to-EOF read spins forever
    let random_ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .is_ok();
    if !random_ok {
        tracing::warn!("web: pairing pin falls back to time entropy");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or_default();
        for (i, slot) in bytes.iter_mut().enumerate() {
            *slot = (nanos >> (i * 7)) as u8;
        }
    }
    let pin: String = bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect();
    if let Err(e) = save_pin(&path, &pin) {
        tracing::warn!("web: cannot persist the pairing pin: {e}");
    }
    pin
}

/// the PIN is 0600 from the moment it exists under its name: the
/// mode is set on the tmp file before the rename
fn save_pin(path: &std::path::Path, pin: &str) -> Result<(), String> {
    mestro_os::atomic_file::write_atomic_mode(path, pin, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    const PIN: &str = "ABCD2345";

    #[test]
    fn save_pin_is_0600_replaces_a_wider_file_and_leaves_no_tmp() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("web-pin");
        save_pin(&path, "OLDPIN23").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_pin(&path, PIN).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PIN);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    fn ap_net(ip: [u8; 4]) -> mestro_os::wifi::ApNet {
        mestro_os::wifi::ApNet {
            ip: Ipv4Addr::from(ip),
            prefix: 24,
            net: Ipv4Addr::from([ip[0], ip[1], ip[2], 0]),
            dhcp_low: Ipv4Addr::from([ip[0], ip[1], ip[2], 10]),
            dhcp_high: Ipv4Addr::from([ip[0], ip[1], ip[2], 50]),
        }
    }

    fn info(ap_up: bool) -> WebInfo {
        let info = WebInfo {
            port: 8787,
            pin: String::new(),
            app_version: "0.1.0".into(),
            id: "a8724d0c4171".into(),
            name: "wohnzimmer".into(),
            display_supported: true,
            ap_mode: std::sync::Arc::default(),
            mdns_gate: std::sync::Arc::default(),
            ap_net: ap_net([10, 42, 0, 1]),
            diagnostics: server::diagnostics::Sources::image(),
        };
        if ap_up {
            info.ap_mode
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        info
    }

    fn peer(addr: &str) -> SocketAddr {
        addr.parse().unwrap()
    }

    #[test]
    fn the_peer_rule_decides_the_trust() {
        // AP up + peer inside the subnet: trusted
        assert_eq!(
            Ingress::of_peer(peer("10.42.0.20:40000"), &info(true)),
            Ingress::Ap
        );
        // AP up but peer outside (a LAN host): paired-only
        assert_eq!(
            Ingress::of_peer(peer("192.168.178.20:40000"), &info(true)),
            Ingress::Lan
        );
        // peer inside but the hotspot is down (the flag is the
        // confirmed-up bit): paired-only
        assert_eq!(
            Ingress::of_peer(peer("10.42.0.20:40000"), &info(false)),
            Ingress::Lan
        );
        // the gateway itself is never a trusted peer (its own
        // loopback/local connections must not ride the gate)
        assert_eq!(
            Ingress::of_peer(peer("10.42.0.1:40000"), &info(true)),
            Ingress::Lan
        );
        // an IPv4-mapped IPv6 peer normalizes before the subnet check
        assert_eq!(
            Ingress::of_peer(peer("[::ffff:10.42.0.20]:40000"), &info(true)),
            Ingress::Ap
        );
        // plain IPv6 has no place in the v4 hotspot subnet
        assert_eq!(
            Ingress::of_peer(peer("[::1]:40000"), &info(true)),
            Ingress::Lan
        );
    }

    #[test]
    fn the_wizard_url_omits_the_port_only_on_80() {
        // 80 is the URL default: no port in the URL
        let mut web = info(true);
        web.pin = PIN.into();
        web.port = 80;
        let url = web.remote_url();
        let authority = url.split("//").nth(1).unwrap().split('/').next().unwrap();
        assert!(!authority.contains(':'), "{url}");
        assert!(url.ends_with(&format!("/#pair={PIN}")), "{url}");
        // any other port is spelled out
        web.port = 8787;
        assert!(
            web.remote_url().contains(":8787/#pair="),
            "{}",
            web.remote_url()
        );
        // an empty pin means the endpoint is off: no URL at all
        web.pin = String::new();
        assert_eq!(web.remote_url(), "");
    }
}
