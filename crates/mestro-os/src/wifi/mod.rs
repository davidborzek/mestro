//! mestro's wifi: the web endpoint's contract with the manager and
//! — behind the `wifi-setup` feature, forwarded by the binary's
//! feature of the same name — the setup lane itself. mestro owns the wifi from
//! first boot on: no usable credentials → the setup hotspot
//! (`manager`), the phone joins it, the captive portal bounces it
//! into `/setup/wifi`, and the join flow writes the STA conf
//! (atomic, with a `.bak` the failure path restores). AP and STA
//! are mutually exclusive: brcmfmac allows both only on one
//! channel and is flaky there — the state machine switches modes
//! instead.
//!
//! Layout:
//! - [`api`]: the web endpoint's contract — the ops vocabulary
//!   ([`WifiOp`]), the request channel ([`WifiChannel`]), the
//!   tracked children ([`Children`]) and the restart stop signal
//!   ([`WifiShutdown`]). Always compiled: the protocol, the web
//!   endpoint and the restart path name these types in every
//!   build; a build without the lane wires no manager and every
//!   request answers "unavailable"
//! - [`gate`]: the mDNS pause handshake with the web endpoint's
//!   announcer (always compiled — the announcer side runs in
//!   every build)
//! - [`paths`]: the image's wpa_supplicant paths (always compiled
//!   — the web endpoint names the STA conf in every build)
//! - `reset`: the factory reset's wifi half — credentials to
//!   the template, sidecars gone (behind `wifi-setup`: only the
//!   binary's `wifi-setup` build calls it)
//! - [`settings`]: the `[wifi]` section ([`WifiSettings`]) and the
//!   hotspot address plan ([`ApNet`]) — the binary's config embeds
//!   them
//! - `Plan`: the `[wifi]` section resolved into paths and
//!   addresses — built once at spawn, so the binary carries no
//!   board paths
//! - `manager`: the mode state machine (Sta / Ap / Joining /
//!   Failed), the boot policy and the web ops
//! - `backend`: the hardware seam (`WifiBackend`) — wireless
//!   interface detection, the real device and a recording fake
//! - `wpa_ctrl`: the wpa_supplicant control-interface client and
//!   its tolerant reply parsers
//! - `conf`: conf writing (atomic, `.bak`, PSK hashing,
//!   template detection)
//! - `pbkdf2`: the WPA-PSK derivation (PBKDF2-HMAC-SHA1)
//!
//! The manager runs on its own thread or the UI thread, never on
//! the web runtime; the binary compiles this module always (the
//! contract) and the lane only with `wifi-setup`.

pub mod api;
pub mod gate;
pub mod paths;
pub mod settings;

#[cfg(feature = "wifi-setup")]
pub mod backend;
#[cfg(feature = "wifi-setup")]
mod conf;
#[cfg(feature = "wifi-setup")]
mod manager;
#[cfg(feature = "wifi-setup")]
mod pbkdf2;
#[cfg(feature = "wifi-setup")]
pub mod reset;
#[cfg(feature = "wifi-setup")]
mod wpa_ctrl;

pub use api::{Children, WifiChannel, WifiOp, WifiRequest, WifiShutdown, WIFI_QUEUE};
pub use gate::{AttachedWorker, GatePhase, MdnsGate};
#[cfg(feature = "wifi-setup")]
pub use manager::{ApReason, HotspotSink, HotspotView, Phase};
#[cfg(feature = "wifi-setup")]
pub use reset::{reset_credentials, WifiReset, WifiResetPaths};
pub use settings::{parse_ap_address, ApNet, WifiSettings, DEFAULT_AP_ADDRESS};

#[cfg(feature = "wifi-setup")]
use std::path::{Path, PathBuf};
#[cfg(feature = "wifi-setup")]
use std::sync::Arc;

/// the hotspot's URLs, built by the caller — the manager never
/// formats a URL: the web endpoint's address is its concern
#[cfg(feature = "wifi-setup")]
pub struct Portal {
    /// the RFC 8910 captive portal URI DHCP option 114 advertises
    /// (`http://<hotspot ip>[:<port>]/setup/wifi`)
    pub uri: String,
    /// the setup address the overlay's panel names (`mestro.setup`,
    /// the web port spelled out unless it is the URL default 80)
    pub address: String,
}

/// the `[wifi]` section and the image paths resolved: interface,
/// socket, files, the hotspot's addresses and the radio policy
/// (power save, regdom)
#[cfg(feature = "wifi-setup")]
pub struct Plan {
    pub iface: String,
    pub ctrl_dir: PathBuf,
    /// the persistent STA conf (the boot scripts start
    /// wpa_supplicant with it) and its predecessor
    pub sta: PathBuf,
    pub sta_bak: PathBuf,
    /// the last conf that brought the player online — the way back
    /// when the hotspot cannot start
    pub sta_good: PathBuf,
    /// the ephemeral AP conf (tmpfs, regenerated on every boot)
    pub ap: PathBuf,
    /// the hotspot's gateway, subnet and DHCP pool (from
    /// `[wifi] ap-address`)
    pub ap_net: ApNet,
    /// the configured power-save state — brcmfmac's default (on)
    /// spikes the link latency, so mestro pins it after every
    /// association
    pub power_save: bool,
    /// the configured regulatory domain ("" = the kernel's stays)
    pub country: String,
}

/// resolve the config against the device: interface detection plus
/// the derived paths. Errors when the board has no wifi at all
#[cfg(feature = "wifi-setup")]
pub fn plan(
    config: &WifiSettings,
    roots: &backend::Roots,
    ctrl_dir: &Path,
    sta: &Path,
) -> Result<Plan, String> {
    let iface = backend::detect_wireless(roots, &config.interface)?;
    let sta = sta.to_path_buf();
    Ok(Plan {
        iface,
        ctrl_dir: ctrl_dir.to_path_buf(),
        sta_bak: sta.with_extension("conf.bak"),
        sta_good: sta.with_extension("conf.good"),
        sta,
        ap: PathBuf::from("/run/mestro/ap.conf"),
        ap_net: config.ap_net(),
        power_save: config.power_save,
        country: config.country.clone(),
    })
}

/// the `NAME`/hotspot suffix of this device: from the wifi interface's
/// MAC (`requested` as in `[wifi] interface`), else the player id
#[cfg(feature = "wifi-setup")]
pub fn name_suffix(roots: &backend::Roots, requested: &str, device_id: &str) -> String {
    let mac = backend::detect_wireless(roots, requested)
        .ok()
        .and_then(|iface| std::fs::read_to_string(roots.net_class.join(iface).join("address")).ok())
        .unwrap_or_default();
    crate::identity::device_suffix(&mac, device_id)
}

/// spawn the manager thread: its own tokio runtime, current-thread
/// like the web endpoint's — the ops and the policy tick run there,
/// the web thread only ever sends requests. Returns the channel the
/// web endpoint talks to; without a wireless interface the channel
/// stays dead (every op answers "manager gone")
#[cfg(feature = "wifi-setup")]
pub fn spawn(
    ap_mode: Arc<std::sync::atomic::AtomicBool>,
    mdns_gate: Arc<MdnsGate>,
    view: HotspotSink,
    config: WifiSettings,
    device_id: String,
    portal: Portal,
    shutdown: WifiShutdown,
) -> WifiChannel {
    // bounded: a full queue answers busy at once instead of piling
    // up behind a long join
    let (tx, rx) = tokio::sync::mpsc::channel::<WifiRequest>(api::WIFI_QUEUE);
    let spawned = std::thread::Builder::new()
        .name("wifi".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!("wifi: tokio runtime: {e}");
                    return;
                }
            };
            runtime.block_on(manager::run(
                rx, ap_mode, mdns_gate, view, config, device_id, portal, shutdown,
            ));
        });
    if let Err(e) = spawned {
        tracing::warn!("wifi: thread: {e}");
    }
    WifiChannel::new(Arc::new(tx))
}
