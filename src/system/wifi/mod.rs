//! The wifi setup lane: mestro owns the wifi from first boot on —
//! no usable credentials → the setup hotspot (`Manager`), the phone
//! joins it, the captive portal bounces it into `/setup/wifi`, and
//! the join flow writes the STA conf (atomic, with a `.bak` the
//! failure path restores). AP and STA are mutually exclusive:
//! brcmfmac allows both only on one channel and is flaky there —
//! the state machine switches modes instead.
//!
//! Layout:
//! - [`Plan`]: the `[wifi]` section resolved into paths and
//!   addresses — built once at spawn, so the binary carries no
//!   board paths
//! - [`manager`]: the mode state machine (Sta / Ap / Joining /
//!   Failed), the boot policy and the web ops
//! - [`backend`]: the hardware seam (`WifiBackend`) — wireless
//!   interface detection, the real device and a recording fake
//! - [`wpa_ctrl`]: the wpa_supplicant control-interface client and
//!   its tolerant reply parsers
//! - [`conf`]: conf writing (atomic, `.bak`, PSK hashing,
//!   template detection)
//! - [`pbkdf2`]: the WPA-PSK derivation (PBKDF2-HMAC-SHA1)

pub mod backend;
mod conf;
mod manager;
mod pbkdf2;
mod wpa_ctrl;

pub use manager::Phase;

use crate::remote::web::{WifiChannel, WifiRequest, WifiShutdown};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// the `[wifi]` section and the image paths resolved: interface,
/// socket, files, the hotspot's addresses and the radio policy
/// (power save, regdom)
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
    pub ap_net: crate::config::ApNet,
    /// the configured power-save state — brcmfmac's default (on)
    /// spikes the link latency, so mestro pins it after every
    /// association
    pub power_save: bool,
    /// the configured regulatory domain ("" = the kernel's stays)
    pub country: String,
}

/// resolve the config against the device: interface detection plus
/// the derived paths. Errors when the board has no wifi at all
pub fn plan(
    config: &crate::config::Wifi,
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

/// spawn the manager thread: its own tokio runtime, current-thread
/// like the web endpoint's — the ops and the policy tick run there,
/// the web thread only ever sends requests. Returns the channel the
/// web endpoint talks to; without a wireless interface the channel
/// stays dead (every op answers "manager gone")
pub fn spawn(
    ap_mode: Arc<std::sync::atomic::AtomicBool>,
    mdns_gate: Arc<crate::remote::web::MdnsGate>,
    ui: slint::Weak<crate::Mestro>,
    config: crate::config::Wifi,
    web_port: u16,
    shutdown: WifiShutdown,
) -> WifiChannel {
    // bounded: a full queue answers busy at once instead of piling
    // up behind a long join
    let (tx, rx) = tokio::sync::mpsc::channel::<WifiRequest>(crate::remote::web::WIFI_QUEUE);
    let spawned = std::thread::Builder::new()
        .name("mestro-wifi".into())
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
                rx, ap_mode, mdns_gate, ui, config, web_port, shutdown,
            ));
        });
    if let Err(e) = spawned {
        tracing::warn!("wifi: thread: {e}");
    }
    WifiChannel::new(Arc::new(tx))
}
