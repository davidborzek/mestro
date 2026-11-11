//! The embedded web endpoint (`src/web/`): one port, two faces —
//! `GET /ws` upgrades to the JSON protocol for browser clients,
//! everything else serves the setup-wizard SPA (the Vite build
//! `web/dist/index.html`, embedded at compile time).
//!
//! Threading: the socket side runs on its own tokio thread;
//! settings and slint handles are `!Sync`, so every state op
//! round-trips onto the UI thread through the `web-op` callback
//! (`Bridge`). MA calls (`ma-*`) are plain HTTP and run on the
//! socket thread.
//!
//! Auth: the wizard URL carries a pairing PIN in the hash
//! (`#pair=…` — never logged by HTTP middlemen); the SPA passes it
//! as the WS query.

mod protocol;
mod server;

pub use protocol::{Envelope, MaOp, Request, Response, SettingsOp};

use crate::remote::control::Command;
use crate::settings::Handle;
use crate::Mestro;
use slint::ComponentHandle as _;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

type Dispatch = Rc<dyn Fn(&Command)>;

/// device-side facts the protocol reports (wizard chrome)
#[derive(Debug, Clone)]
pub struct WebInfo {
    /// port + pairing PIN; empty PIN = the endpoint is off
    pub port: u16,
    pub pin: String,
    pub app_version: String,
}

impl WebInfo {
    /// the wizard URL including the auth fragment, for the SPA
    /// footer and the kiosk's connection page. Built per call: the
    /// kiosk usually starts before DHCP has handed out an address
    pub fn remote_url(&self) -> String {
        if self.pin.is_empty() {
            return String::new();
        }
        format!("http://{}:{}/#pair={}", lan_ip(), self.port, self.pin)
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
pub fn info(remote: &crate::config::Remote, app_version: &str) -> (WebInfo, String) {
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
    let info = WebInfo {
        port: remote.port,
        pin: pin.clone(),
        app_version: app_version.to_string(),
    };
    // the wizard URL carries the pairing PIN — the kiosk connection
    // page shows it too (restart-stable secret for a LAN device,
    // persisted 0600)
    tracing::info!("web: wizard url: {}", info.remote_url());
    (info, pin)
}

/// spawn the endpoint thread. Call AFTER [`wire`] — a request in
/// the start/wire window would hit an unset `web-op` handler.
/// Errors (port busy, no permission) warn and continue — the
/// kiosk must boot regardless.
pub fn serve(remote: &crate::config::Remote, info: WebInfo, bridge: Bridge, pin: String) {
    if !remote.enabled {
        return;
    }
    let port = remote.port;
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
            runtime.block_on(server::serve(port, info, bridge, pin));
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
/// command path (the same one touch UI and text protocol use).
pub fn wire(ui: &Mestro, settings: &Rc<Handle>, dispatch: &Dispatch, info: &WebInfo) {
    let ui_weak = ui.as_weak();
    let settings = settings.clone();
    let dispatch = dispatch.clone();
    let info = info.clone();
    ui.on_web_op(move |envelope| -> slint::SharedString {
        let Some(ui) = ui_weak.upgrade() else {
            return r#"{"id":0,"ok":false,"error":"ui unavailable"}"#.into();
        };
        server::settings::handle_ui(envelope.as_str(), &ui, &settings, &dispatch, &info).into()
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
