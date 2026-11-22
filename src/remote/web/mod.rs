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
//! (`Bridge`). MA calls (`ma`) are plain HTTP and run on the
//! socket thread.
//!
//! Auth: the kiosk URL carries a pairing PIN in the hash
//! (`#pair=…` — never logged by HTTP middlemen); the PWA trades
//! it once for the `mestro_pair` cookie (see the `pair` module),
//! and either opens `/ws`, `/ota` and `/cover`.

mod identity;
#[cfg(all(test, feature = "wifi-setup"))]
pub(crate) use identity::device_id;
pub use identity::device_suffix;
mod mdns;
mod protocol;
pub mod push;
mod server;
mod state;

pub use mdns::MdnsGate;
#[cfg(all(test, feature = "wifi-setup"))]
pub(crate) use mdns::{fake_announcer::confirming_worker, GatePhase};
pub use protocol::{
    AudioOp, Envelope, MaOp, PlayerAction, Request, Response, SettingsOp, TimezoneOp, WifiOp,
};
pub use push::{Events, Push};

use crate::locks;
use crate::providers::SourceCommand;

/// what the `player` op touches on the UI thread: the shared
/// live state (seek and volume bookkeeping) and the provider
/// command channel. Built in main.rs before the web endpoint
/// wires up
#[derive(Clone)]
pub struct PlayerCtx {
    pub shared: crate::player::state::Shared,
    pub cmd_tx: tokio::sync::mpsc::UnboundedSender<SourceCommand>,
}

use crate::remote::control::Command;
use crate::settings::Handle;
use crate::Mestro;
use slint::ComponentHandle as _;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

type Dispatch = Rc<dyn Fn(&Command)>;

/// renders one settings-preview thumbnail (kind "vu" or
/// "clock", name = skin/style) as PNG bytes on the UI thread;
/// None when the name is unknown. Built in main.rs from the
/// compiled skin registries and threaded into [`wire`]
pub type PreviewRenderer = Rc<dyn Fn(&str, &str) -> Option<Vec<u8>>>;

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
    pub ap_net: crate::config::ApNet,
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
            lan_ip(),
            self.port_suffix(),
            self.pin
        )
    }
}

/// cross-thread bridge: web thread → UI thread. The slint handles
/// and the settings live on the UI thread (`!Sync`) — one request
/// goes through `invoke_from_event_loop`, the reply comes back on
/// a channel.
#[derive(Clone)]
pub struct Bridge {
    ui: slint::Weak<Mestro>,
}

impl Bridge {
    pub fn new(ui: &Mestro) -> Self {
        Self { ui: ui.as_weak() }
    }

    /// [`Self::op`] as an async boundary: the blocking UI
    /// roundtrip runs on the blocking pool — one slow or hung UI
    /// op must not freeze the web runtime for every client
    pub async fn op_async(&self, envelope: &str) -> Result<String, String> {
        let this = self.clone();
        let envelope = envelope.to_string();
        tokio::task::spawn_blocking(move || this.op(&envelope))
            .await
            .map_err(|_| "blocking pool gone".to_string())?
    }

    /// fire-and-forget: run `update` on the UI thread, no reply
    /// (progress hints must never stall the web runtime)
    pub fn notify(&self, update: impl FnOnce(&Mestro) + Send + 'static) {
        let ui = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                update(&ui);
            }
        });
    }

    /// run one JSON envelope on the UI thread, wait for the JSON
    /// response (the wizard is interactive — latency is invisible)
    pub fn op(&self, envelope: &str) -> Result<String, String> {
        let (tx, rx) = mpsc::channel();
        let ui = self.ui.clone();
        let envelope = envelope.to_string();
        let _ = slint::invoke_from_event_loop(move || {
            let out = ui
                .upgrade()
                .map(|u| u.invoke_web_op(envelope.as_str().into()).to_string());
            let _ = tx.send(out);
        });
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "ui thread timeout".to_string())?
            .ok_or_else(|| "ui unavailable".to_string())
    }
}

/// the device facts + the wizard URL (for the kiosk connection
/// page and the SPA chrome). Computed BEFORE `wire` — see
/// [`serve`]. With `[remote].enabled = false` the URL is empty and
/// no PIN file is written (the first-run overlay then points at
/// config.toml instead of a dead end).
pub fn info(
    config: &crate::config::Config,
    app_version: &str,
    display_supported: bool,
) -> (WebInfo, String) {
    let remote = &config.remote;
    tracing::info!(
        "web: start: enabled={} port={}",
        remote.enabled,
        remote.port
    );
    let pin = if remote.enabled {
        pairing_pin()
    } else {
        String::new()
    };
    let hostname = identity::hostname();
    let info = WebInfo {
        port: remote.port,
        pin: pin.clone(),
        app_version: app_version.to_string(),
        id: identity::device_id(
            std::path::Path::new("/sys/class/net"),
            &crate::settings::state_dir(),
            &hostname,
        ),
        name: identity::label(&config.music_assistant.player, &hostname),
        display_supported,
        ap_mode: std::sync::Arc::default(),
        mdns_gate: std::sync::Arc::default(),
        ap_net: config.wifi.ap_net(),
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
pub(crate) fn url_port_suffix(port: u16) -> String {
    if port == 80 {
        String::new()
    } else {
        format!(":{port}")
    }
}

/// the request path to the wifi manager thread (feature
/// `wifi-setup`, `system::wifi`): one op and the slot for its
/// reply. A build without the feature wires no manager — every
/// request answers "unavailable". The ops run on the manager's
/// own thread; the async request here only ever awaits
#[derive(Clone)]
pub struct WifiChannel {
    tx: Option<std::sync::Arc<tokio::sync::mpsc::Sender<WifiRequest>>>,
}

/// how many ops may wait for the manager — a full queue answers
/// busy at once instead of piling up behind a long join
pub const WIFI_QUEUE: usize = 4;

/// where a connection came from: a peer inside the hotspot's
/// subnet (and not the gateway itself) while the hotspot is
/// confirmed up may run the setup vocabulary unpaired — the setup
/// hotspot is the trust boundary, not the listener that accepted
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

/// one op + its deadline + its auth context + its reply slot. The
/// deadline is the client's full budget (queue wait included):
/// the manager rejects an op that would start too late — a join
/// must never change credentials under a client that gave up.
/// The auth context is re-checked at EXECUTION: a request that
/// queued while the hotspot was up must not run after it ended
pub type WifiRequest = (
    WifiOp,
    std::time::Instant,
    Ingress,
    bool,
    tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
);

impl WifiChannel {
    pub fn new(tx: std::sync::Arc<tokio::sync::mpsc::Sender<WifiRequest>>) -> Self {
        Self { tx: Some(tx) }
    }
    /// no manager behind it (feature off): every op fails fast
    pub fn unavailable() -> Self {
        Self { tx: None }
    }
    /// one op with a budget — the reply rides a oneshot, a hung
    /// manager never blocks the web runtime
    pub async fn request(
        &self,
        op: WifiOp,
        ingress: Ingress,
        paired: bool,
        timeout: std::time::Duration,
    ) -> Result<serde_json::Value, String> {
        let tx = self.tx.as_ref().ok_or("wifi setup not available")?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = (
            op,
            std::time::Instant::now() + timeout,
            ingress,
            paired,
            reply_tx,
        );
        use tokio::sync::mpsc::error::TrySendError;
        match tx.try_send(request) {
            Ok(()) => {}
            // the bounded queue: busy at once, no piling up
            Err(TrySendError::Full(_)) => return Err("wifi manager busy — try again".into()),
            Err(TrySendError::Closed(_)) => return Err("wifi manager gone".into()),
        }
        match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err("wifi manager gone".to_string()),
            Err(_) => Err("wifi op timed out".to_string()),
        }
    }
}

/// mestro's tracked wifi child processes (dnsmasq, udhcpc, the
/// supplicant it started), name → handle. A separate small lock:
/// the restart path reads it without the manager's op lock
pub type Children = std::sync::Mutex<std::collections::HashMap<String, std::process::Child>>;

/// the app-restart stop signal for the wifi manager: exec keeps
/// the children (dnsmasq, udhcpc, the supplicant) alive with their
/// handles lost — the manager must stop what the hotspot owns
/// before the process is replaced (a working STA survives), and
/// the next start cleans up strays by exact identity
#[derive(Clone, Default)]
pub struct WifiShutdown {
    state: std::sync::Arc<ShutdownState>,
    children: std::sync::Arc<Children>,
}

#[derive(Default)]
struct ShutdownState {
    stop: std::sync::atomic::AtomicBool,
    stopped: std::sync::atomic::AtomicBool,
}

impl WifiShutdown {
    /// the tracked-children registry — the backend registers its
    /// spawns here, the restart paths read it without any op lock
    pub fn children_arc(&self) -> std::sync::Arc<Children> {
        self.children.clone()
    }

    /// the exec fallback: no confirmation in time — kill every
    /// tracked child directly (we know them; the next start's
    /// ensure_sta rebuilds the STA side)
    pub fn stop_tracked(&self) {
        for (name, mut child) in locks::lock(&self.children).drain() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::debug!("wifi: stopped {name} (restart fallback)");
        }
    }

    pub fn request(&self) {
        self.state
            .stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_requested(&self) -> bool {
        self.state.stop.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// the manager's confirmation (children stopped)
    pub fn mark_stopped(&self) {
        self.state
            .stopped
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// request + bounded wait for the confirmation — called on the
    /// restart path, right before the exec
    pub fn wait_stopped(&self, timeout: std::time::Duration) -> bool {
        self.request();
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self
                .state
                .stopped
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.state
            .stopped
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// spawn the endpoint thread. Call AFTER [`wire`] — a request in
/// the start/wire window would hit an unset `web-op` handler.
/// Errors (port busy, no permission) warn and continue — the
/// kiosk must boot regardless.
pub fn serve(
    remote: &crate::config::Remote,
    info: WebInfo,
    bridge: Bridge,
    pin: String,
    events: Events,
    wifi: WifiChannel,
) {
    if !remote.enabled {
        return;
    }
    let port = remote.port;
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
            runtime.block_on(server::serve(port, info, bridge, pin, events, wifi));
        });
    if let Err(e) = spawned {
        tracing::warn!("web: thread: {e}");
    }
}

/// the LAN address a phone can reach (UDP connect trick — no
/// packets are sent, it only picks the outbound interface).
/// 127.0.0.1 while no interface has a route yet
pub fn lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("8.8.8.8:80")?;
            Ok(socket.local_addr()?.ip().to_string())
        })
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// install the UI-bound handler on the `web-op` callback. It runs
/// on the UI thread (via [`Bridge`]); `dispatch` is the shared
/// command path (the same one touch UI and text protocol use),
/// `player` carries the live state the `player` op's volume
/// bookkeeping needs.
pub fn wire(
    ui: &Mestro,
    settings: &Rc<Handle>,
    dispatch: &Dispatch,
    info: &WebInfo,
    player: &PlayerCtx,
    previews: &PreviewRenderer,
) {
    let ui_weak = ui.as_weak();
    let settings = settings.clone();
    let dispatch = dispatch.clone();
    let info = info.clone();
    let player = player.clone();
    let previews = previews.clone();
    ui.on_web_op(move |envelope| -> slint::SharedString {
        let Some(ui) = ui_weak.upgrade() else {
            return r#"{"id":0,"ok":false,"error":"ui unavailable"}"#.into();
        };
        server::settings::handle_ui(
            envelope.as_str(),
            &ui,
            &settings,
            &dispatch,
            &info,
            &player,
            &previews,
        )
        .into()
    });
}

/// the pairing PIN: stable across restarts (the URL a phone
/// typed must survive app updates) — device-local like the
/// settings overlay, persisted 0600.
pub(crate) fn pairing_pin() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let path = crate::settings::state_dir().join("web-pin");
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

fn save_pin(path: &std::path::Path, pin: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, pin)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    const PIN: &str = "ABCD2345";

    fn ap_net(ip: [u8; 4]) -> crate::config::ApNet {
        crate::config::ApNet {
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
