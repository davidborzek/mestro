//! The wifi state machine: STA (per the persisted conf), the setup
//! hotspot, a join in flight and a failed join — explicit
//! transitions only, all waits and radio work through the backend
//! (the fake runs the whole machine without a radio).
//!
//! Policy: the hotspot comes up when the boot finds no usable conf
//! (missing or the seeded template), or when STA mode has no route
//! for the grace window and no wired fallback. A fallback hotspot
//! (a usable conf exists, the route was lost) retries the saved
//! network periodically unless a phone is using the portal.

use super::backend::{RealBackend, WifiBackend, BUSYBOX, DNSMASQ, LEASES, WPA_SUPPLICANT};
use super::conf;
use super::wpa_ctrl::{self, ScanEntry};
use super::Plan;
use crate::locks;
use crate::remote::web::{Ingress, MdnsGate, WifiOp, WifiRequest, WifiShutdown};
use crate::HotspotReason;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// seconds without a route (and without the wired fallback) in STA
/// mode before the hotspot comes up — the boot screen's network
/// check, one boot-screen grace window longer
const NO_ROUTE_TO_AP: u32 = 60;
/// a fallback hotspot retries the saved network this often
const STA_RETRY: u32 = 120;
/// how long a join may wait for an address
const JOIN_WAIT: Duration = Duration::from_secs(30);
/// the scan budget — the web op allows 15 s
const SCAN_WAIT: Duration = Duration::from_secs(13);
/// the policy tick
const TICK: Duration = Duration::from_secs(5);
/// a freshly started supplicant may take this long to answer PING
const SUPPLICANT_START: Duration = Duration::from_secs(10);
/// how long a hotspot start waits for the mDNS announcer to confirm
/// its pause before it goes on without
const MDNS_PAUSE_WAIT: Duration = Duration::from_secs(3);
/// one poll step while waiting
const POLL: Duration = Duration::from_secs(1);
/// one scan result poll step (the fallback without an event stream)
const SCAN_POLL: Duration = Duration::from_secs(2);
/// how long one wait for the scan-finished event blocks before the
/// shutdown flag is looked at again
const SCAN_EVENT_SLICE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApReason {
    /// nothing to connect to — the first-run hotspot (never retried)
    Setup,
    /// a usable conf exists, the route was lost — the saved network
    /// gets retried while the portal stands by
    Fallback,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// STA per the persisted conf
    Sta,
    /// the setup hotspot
    Ap,
    /// credentials written, waiting for an address
    Joining { ssid: String },
    /// the last join failed — a working hotspot carries the error;
    /// without one it is only the error
    Failed(String),
}

pub struct Manager<B: WifiBackend> {
    backend: B,
    plan: Plan,
    ap_mode: Arc<AtomicBool>,
    /// the mDNS pause handshake: the announcer is confirmed down
    /// before a hotspot start touches the radio and stays paused
    /// until the station network is back, so it never overlaps the
    /// hotspot interface. Not the trust flag above
    mdns_gate: Arc<MdnsGate>,
    /// how long a hotspot start waits for the announcer's confirmation
    mdns_pause_wait: Duration,
    ui: slint::Weak<crate::Mestro>,
    phase: Phase,
    /// the trust flag, explicit: true ONLY once the hotspot is
    /// confirmed up (supplicant answering, address on, dnsmasq
    /// running) — a failed start leaves it down
    ap_up: bool,
    /// why the hotspot is up — the fallback retries, setup does not
    ap_reason: ApReason,
    /// seconds since the hotspot came up (the retry clock)
    ap_secs: u32,
    /// the setup hotspot's name (from the interface's MAC)
    ap_ssid: String,
    /// the configured web port — the portal URI the hotspot
    /// advertises (option 114) and the overlay's setup address
    /// name it
    web_port: u16,
    /// the current (or joining) STA network
    ssid: String,
    /// the last failure, reported by the status op
    last_error: String,
    /// the association power save was last applied to (the STATUS
    /// BSSID) — None = this association's state is unknown. brcmfmac
    /// re-enables power saving on reassociation, so the tick re-
    /// applies the configured state once per association, never per
    /// tick
    power_save_bssid: Option<String>,
    no_route_secs: u32,
    /// the current conf was seen online (and kept as the last good
    /// one) — reset whenever the conf changes
    good_kept: bool,
    /// the status payload, cheap to read without this manager's
    /// op lock (a long join must not block the status op)
    snapshot: Arc<Mutex<Value>>,
    /// the restart path's stop signal — the poll loops check it
    /// between steps so a shutdown never waits behind a long op
    shutdown: WifiShutdown,
}

impl<B: WifiBackend> Manager<B> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend: B,
        plan: Plan,
        ap_mode: Arc<AtomicBool>,
        mdns_gate: Arc<MdnsGate>,
        ui: slint::Weak<crate::Mestro>,
        mac: &str,
        web_port: u16,
        shutdown: WifiShutdown,
    ) -> Self {
        // mestro-XXXX: the MAC's last two bytes as hex, so every
        // device names its hotspot differently without state
        let hex = mac.replace(':', "");
        let suffix = if hex.len() >= 4 {
            &hex[hex.len() - 4..]
        } else {
            hex.as_str()
        };
        let manager = Self {
            backend,
            plan,
            ap_mode,
            mdns_gate,
            mdns_pause_wait: MDNS_PAUSE_WAIT,
            ui,
            phase: Phase::Sta,
            ap_up: false,
            ap_reason: ApReason::Setup,
            ap_secs: 0,
            ap_ssid: format!("mestro-{}", suffix.to_uppercase()),
            web_port,
            ssid: String::new(),
            last_error: String::new(),
            power_save_bssid: None,
            no_route_secs: 0,
            good_kept: false,
            snapshot: Arc::new(Mutex::new(json!({}))),
            shutdown,
        };
        manager.update_snapshot();
        manager
    }

    /// the status payload the run loop serves without the op lock
    pub fn snapshot_arc(&self) -> Arc<Mutex<Value>> {
        self.snapshot.clone()
    }

    /// the AP-mode flag the ops re-check at execution time
    pub fn ap_mode_arc(&self) -> Arc<AtomicBool> {
        self.ap_mode.clone()
    }

    /// the current phase (tests read it; the run loop and the ops
    /// never need it)
    #[cfg(test)]
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// the trust flag as the web endpoint reads it (tests)
    #[cfg(test)]
    pub fn ap_up(&self) -> bool {
        self.ap_up
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        // fail closed: only a confirmed hotspot opens the gate
        self.ap_mode.store(self.ap_up, Ordering::Relaxed);
        // the station network is back: the hotspot is gone, mDNS may
        // announce again
        if self.phase == Phase::Sta {
            self.mdns_gate.resume();
        }
        self.update_snapshot();
        self.push_panel();
    }

    fn update_snapshot(&self) {
        // a Failed phase carries its own message; everything else
        // reports the last join error
        let error = match &self.phase {
            Phase::Failed(e) => e.clone(),
            _ => self.last_error.clone(),
        };
        *locks::lock(&self.snapshot) = json!({
            "state": self.state_name(),
            "ap_ssid": self.ap_ssid,
            "ssid": self.ssid,
            "error": error,
        });
    }

    /// the hotspot overlay's panel: a WIFI QR to join the hotspot,
    /// shown whenever it is up — the first-run setup and the
    /// lost-network fallback alike. The QR renders on the UI
    /// thread (slint::Image stays there)
    fn push_panel(&self) {
        let active = self.ap_up;
        let reason = if self.ap_reason == ApReason::Fallback {
            HotspotReason::Fallback
        } else {
            HotspotReason::Setup
        };
        let ssid = self.ap_ssid.clone();
        let saved = self.ssid.clone();
        // the setup address the overlay names — the configured
        // web port spelled out unless it is the URL default 80
        let portal = format!(
            "mestro.setup{}",
            crate::remote::web::url_port_suffix(self.web_port)
        );
        let weak = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_hotspot_active(active);
            ui.set_hotspot_reason(reason);
            ui.set_hotspot_ssid(ssid.clone().into());
            ui.set_hotspot_saved_ssid(saved.clone().into());
            ui.set_hotspot_portal(portal.clone().into());
            if active {
                ui.set_hotspot_qr(crate::remote::qr::image(
                    &format!("WIFI:T:nopass;S:{ssid};;"),
                    260,
                ));
            }
        });
    }

    /// the boot policy: mend a crash between the `.bak` and the
    /// conf, then — a usable conf boots into STA (the boot scripts
    /// already started the supplicant with it); anything else —
    /// missing, or still the seeded template — comes up as the
    /// hotspot
    pub fn boot(&mut self) {
        self.cleanup_strays();
        conf::recover_sta(&self.plan.sta, &self.plan.sta_bak);
        self.apply_country();
        self.apply_power_save();
        let usable = std::fs::read_to_string(&self.plan.sta)
            .map(|text| conf::usable_sta(&text))
            .unwrap_or(false);
        if usable {
            // the STA must exist after EVERY start — a restart left
            // it running (the pre-exec stop spares it), a device
            // boot had the boot scripts start it; anything else
            // (a crashed supplicant, the exec fallback's kill)
            // gets a fresh one here
            self.ensure_sta();
            // best effort: name the network the status op reports
            if let Ok(reply) = self.backend.ctrl("STATUS") {
                self.ssid = wpa_ctrl::parse_status(&reply).ssid;
            }
            self.set_phase(Phase::Sta);
            self.remember_if_online();
        } else {
            self.start_ap(ApReason::Setup);
        }
    }

    /// set the configured regulatory domain (`iw reg set`) — the
    /// kernel's default without it is the passive world set "00",
    /// which restricts 5 GHz. An empty `[wifi]` country leaves the
    /// kernel's alone; a failure is a warning, never fatal
    fn apply_country(&mut self) {
        let country = self.plan.country.clone();
        if country.is_empty() {
            return;
        }
        if let Err(e) = self.backend.run("iw", &["reg", "set", &country]) {
            tracing::warn!("wifi: reg set {country}: {e}");
        }
    }

    /// the configured power-save state on the managed interface
    /// (`iw dev <if> set power_save`). brcmfmac enables power saving
    /// by default and RE-ENABLES it on every (re)association — the
    /// ~300 ms latency spikes and stream reconnects — so this runs
    /// at boot, after every association mestro drives, and (via
    /// [`ensure_power_save`]) once per association the supplicant
    /// drove on its own. A failure is a warning, never fatal
    fn apply_power_save(&mut self) {
        let state = if self.plan.power_save { "on" } else { "off" };
        if let Err(e) = self
            .backend
            .run("iw", &["dev", &self.plan.iface, "set", "power_save", state])
        {
            tracing::warn!("wifi: power_save {state} on {}: {e}", self.plan.iface);
        }
        self.power_save_bssid = None;
    }

    /// the tick's power-save catch-up: an association the supplicant
    /// drove behind mestro's back (a roam, a driver-internal
    /// reassociation) re-enabled power saving — apply the configured
    /// state when the association (the STATUS BSSID) changed, never
    /// per tick
    fn ensure_power_save(&mut self) {
        let Ok(reply) = self.backend.ctrl("STATUS") else {
            return;
        };
        let info = wpa_ctrl::parse_status(&reply);
        if info.wpa_state != "COMPLETED" || info.bssid.is_empty() {
            return;
        }
        if self.power_save_bssid.as_deref() != Some(info.bssid.as_str()) {
            self.apply_power_save();
            self.power_save_bssid = Some(info.bssid);
        }
    }

    /// a conf that has the player online is the way back for a
    /// hotspot that cannot start
    fn remember_if_online(&mut self) {
        if self.backend.routable() {
            match conf::remember_good(&self.plan.sta, &self.plan.sta_good) {
                Ok(()) => self.good_kept = true,
                Err(e) => tracing::warn!("wifi: keep the last good conf: {e}"),
            }
        }
    }

    /// adopt-or-start the STA: a supplicant counts as running when
    /// the ctrl socket answers or one matches the exact identity
    /// (the boot scripts', ours from before an exec — orphans of
    /// it keep renewing the lease); a lease client the same way
    fn ensure_sta(&mut self) {
        let iface = self.plan.iface.clone();
        let ctrl_dir = self.plan.ctrl_dir.to_string_lossy().into_owned();
        let sta = self.plan.sta.clone();
        let iface_arg = ["-i", iface.as_str()];
        let supplicant_up = self
            .backend
            .ctrl("PING")
            .map(|reply| reply.trim() == "PONG")
            .unwrap_or(false)
            || self
                .backend
                .count_foreign(WPA_SUPPLICANT, "wpa_supplicant", &[&iface_arg])
                > 0;
        if !supplicant_up {
            tracing::info!("wifi: no supplicant for {iface} — starting one");
            // a corpse's socket file blocks the new supplicant
            self.backend.clear_stale_ctrl_socket();
            if let Err(e) = self.backend.spawn(
                "wpa_supplicant",
                WPA_SUPPLICANT,
                &["-i", &iface, "-C", &ctrl_dir, "-c", &sta.to_string_lossy()],
            ) {
                tracing::warn!("wifi: {e}");
            } else if let Err(e) = self.wait_for_ctrl(SUPPLICANT_START) {
                tracing::warn!("wifi: {e}");
            }
        }
        // a lease client renews the address — adopt a running one
        if self.backend.count_foreign(BUSYBOX, "udhcpc", &[&iface_arg]) == 0 {
            let _ = self
                .backend
                .spawn("udhcpc", "udhcpc", &["-i", &iface, "-f"]);
        }
    }

    /// the startup adopt-or-kill: a previous life's dnsmasq survived
    /// its exec with the handles lost — OURS is the one with our
    /// interface and our lease file in the argv (exact identity,
    /// never a foreign dnsmasq). The ownership proof is the lease
    /// file path: it lives under /run/mestro/ — mestro's private
    /// runtime dir (created 0700) — which a foreign dnsmasq would
    /// not use
    fn cleanup_strays(&mut self) {
        let iface_arg = format!("--interface={}", self.plan.iface);
        let lease_arg = format!("--dhcp-leasefile={LEASES}");
        self.backend.kill_foreign(
            DNSMASQ,
            "dnsmasq",
            &[&[iface_arg.as_str()], &[lease_arg.as_str()]],
        );
    }

    /// the policy tick: STA mode without a route and without the
    /// wired fallback for the grace window → the hotspot, so the
    /// portal can fix it. A FALLBACK hotspot (the saved network
    /// exists) retries it periodically — never while a phone is
    /// using the portal
    pub fn tick(&mut self) {
        match self.phase {
            Phase::Sta => {
                self.ensure_power_save();
                if self.backend.routable() {
                    self.no_route_secs = 0;
                    // the boot usually has no route yet (DHCP runs in
                    // parallel): the conf proves itself here
                    if !self.good_kept {
                        self.remember_if_online();
                    }
                    return;
                }
                if self.backend.eth_up() {
                    self.no_route_secs = 0;
                    return;
                }
                self.no_route_secs += TICK.as_secs() as u32;
                if self.no_route_secs >= NO_ROUTE_TO_AP {
                    tracing::warn!(
                        "wifi: no route for {}s — starting the hotspot",
                        self.no_route_secs
                    );
                    self.no_route_secs = 0;
                    self.start_ap(ApReason::Fallback);
                }
            }
            Phase::Ap | Phase::Failed(_) if self.ap_up && self.ap_reason == ApReason::Fallback => {
                if self.backend.ap_has_client() {
                    // a phone is on the portal: its setup wins over
                    // the retry clock
                    self.ap_secs = 0;
                    return;
                }
                self.ap_secs += TICK.as_secs() as u32;
                if self.ap_secs >= STA_RETRY {
                    self.ap_secs = 0;
                    self.retry_sta();
                }
            }
            _ => {}
        }
    }

    /// the conf in place (the fallback case) or the last good one
    /// restored (the setup case) — then STA again. False when
    /// there is no network to go back to
    fn back_to_saved_network(&mut self) -> bool {
        let usable = std::fs::read_to_string(&self.plan.sta)
            .map(|text| conf::usable_sta(&text))
            .unwrap_or(false);
        let restored = !usable
            && match conf::restore_good(&self.plan.sta, &self.plan.sta_good) {
                Ok(restored) => restored,
                Err(e) => {
                    tracing::warn!("wifi: restore the last good conf: {e}");
                    false
                }
            };
        if !usable && !restored {
            return false;
        }
        tracing::warn!("wifi: hotspot failed — back to the saved network");
        self.backend.kill_owned("dnsmasq");
        self.backend.kill_owned("wpa_supplicant");
        self.ensure_sta();
        true
    }

    fn start_ap(&mut self, reason: ApReason) {
        // the announcer must be confirmed down before the radio is
        // touched. A stuck announcer never blocks the hotspot: the
        // player would be unconfigurable without a network
        if !self.mdns_gate.pause_and_wait(self.mdns_pause_wait) {
            tracing::warn!("wifi: mdns did not confirm its pause — building the hotspot anyway");
        }
        self.ap_reason = reason;
        self.ap_secs = 0;
        if let Err(e) = self.enter_ap() {
            tracing::warn!("wifi: hotspot: {e}");
            self.ap_up = false;
            let message = format!("hotspot failed: {e}");
            // neither hotspot nor network would leave the player
            // unreachable: go back to a network that worked
            if self.back_to_saved_network() {
                self.last_error = message;
                self.set_phase(Phase::Sta);
            } else {
                self.set_phase(Phase::Failed(message));
            }
        } else {
            self.ap_up = true;
            self.set_phase(Phase::Ap);
        }
    }

    /// switch to the hotspot: the AP conf on a fresh supplicant
    /// (RECONFIGURE cannot switch conf files), the static address
    /// and dnsmasq (mestro owns it): DHCP, wildcard DNS — every
    /// name resolves to the player — and the RFC 8910 captive
    /// portal URI (option 114) so modern clients open the setup
    /// page without a probe redirect
    fn enter_ap(&mut self) -> Result<(), String> {
        let iface = self.plan.iface.clone();
        let ctrl_dir = self.plan.ctrl_dir.to_string_lossy().into_owned();
        let ap_conf = self.plan.ap.clone();
        let ap_net = self.plan.ap_net;
        // the safety guard: another interface already holds an
        // address inside the hotspot subnet — the route into it
        // would be ambiguous and the peer-subnet trust boundary
        // would leak onto that network. Fail before anything is
        // torn down (our own iface is skipped: it gets flushed
        // before the hotspot address goes on)
        let addrs = self
            .backend
            .run("ip", &["-o", "-4", "addr", "show"])
            .unwrap_or_default();
        if let Some(dev) = ap_subnet_clash(&addrs, ap_net, &iface) {
            return Err(format!(
                "{dev} already holds an address inside the hotspot subnet {}",
                ap_net.net
            ));
        }
        self.teardown_sta_processes(&iface);
        self.backend.kill_owned("dnsmasq");
        // the fallback can follow a join or an ensure_sta mestro ran
        // itself: the OWN supplicant dies too — a second one on the
        // same iface + ctrl socket never comes up
        self.backend.kill_owned("wpa_supplicant");
        let iface_arg = ["-i", iface.as_str()];
        self.backend
            .kill_foreign(WPA_SUPPLICANT, "wpa_supplicant", &[&iface_arg]);
        crate::atomic_file::write_atomic(
            &ap_conf,
            &conf::ap_conf(&self.ap_ssid, &ctrl_dir, &self.plan.country),
        )?;
        let ip = ap_net.ip.to_string();
        let portal = format!(
            "http://{ip}{}/setup/wifi",
            crate::remote::web::url_port_suffix(self.web_port)
        );
        self.backend.run("ip", &["link", "set", &iface, "up"])?;
        self.backend.run("ip", &["addr", "flush", "dev", &iface])?;
        // the killed supplicant's socket file blocks the new one
        self.backend.clear_stale_ctrl_socket();
        self.backend.spawn(
            "wpa_supplicant",
            WPA_SUPPLICANT,
            &[
                "-i",
                &iface,
                "-C",
                &ctrl_dir,
                "-c",
                &ap_conf.to_string_lossy(),
            ],
        )?;
        self.wait_for_ctrl(SUPPLICANT_START)?;
        self.backend.run(
            "ip",
            &[
                "addr",
                "add",
                &format!("{}/{}", ip, ap_net.prefix),
                "dev",
                &iface,
            ],
        )?;
        self.backend.spawn(
            "dnsmasq",
            DNSMASQ,
            &[
                "--no-daemon",
                &format!("--address=/#/{ip}"),
                &format!("--dhcp-option=114,{portal}"),
                &format!("--dhcp-range={},{},1h", ap_net.dhcp_low, ap_net.dhcp_high),
                &format!("--interface={iface}"),
                "--bind-interfaces",
                &format!("--dhcp-leasefile={LEASES}"),
            ],
        )?;
        tracing::info!("wifi: hotspot {} up on {ip}", self.ap_ssid);
        Ok(())
    }

    /// join a network: write the conf (with a `.bak` of the previous
    /// one), associate and wait for an address. A failure restores
    /// the previous conf and returns to where it came from — the
    /// hotspot (the portal shows the error and a retry) or the old
    /// network (a paired phone is still reachable there)
    pub fn join(&mut self, ssid: &str, psk: Option<&str>) -> Result<(), String> {
        let ctrl_dir = self.plan.ctrl_dir.to_string_lossy().into_owned();
        let text = conf::sta_conf(ssid, psk, &ctrl_dir, &self.plan.country)?;
        let from_ap = self.ap_up;
        if let Err(e) = conf::write_sta(&self.plan.sta, &self.plan.sta_bak, &text) {
            return Err(format!("could not write the wifi conf: {e}"));
        }
        // the hotspot stops serving the moment the join starts —
        // the trust flag must not outlive it
        self.ap_up = false;
        self.set_phase(Phase::Joining {
            ssid: ssid.to_string(),
        });
        match self.associate(from_ap) {
            Ok(()) => {
                self.last_error.clear();
                self.ssid = ssid.to_string();
                conf::confirm_sta(&self.plan.sta_bak);
                self.set_phase(Phase::Sta);
                self.good_kept = false;
                self.remember_if_online();
                tracing::info!("wifi: joined {ssid}");
                Ok(())
            }
            Err(e) => {
                let message = format!("could not join {ssid}: {e}");
                tracing::warn!("wifi: {message}");
                self.last_error = message.clone();
                if let Err(e) = conf::restore_sta(&self.plan.sta, &self.plan.sta_bak) {
                    tracing::warn!("wifi: restore: {e}");
                }
                if from_ap {
                    self.start_ap(self.ap_reason);
                    self.set_phase(Phase::Failed(message.clone()));
                } else {
                    // back to the previous network
                    let _ = self.backend.ctrl("RECONFIGURE");
                    self.set_phase(Phase::Sta);
                }
                Err(message)
            }
        }
    }

    /// apply the freshly written conf and wait for an address
    fn associate(&mut self, from_ap: bool) -> Result<(), String> {
        let iface = self.plan.iface.clone();
        let ctrl_dir = self.plan.ctrl_dir.to_string_lossy().into_owned();
        let sta = self.plan.sta.clone();
        self.backend.kill_owned("dnsmasq");
        self.teardown_sta_processes(&iface);
        if from_ap {
            // the supplicant runs the AP conf — restart it with the
            // STA conf (RECONFIGURE only re-reads its start file)
            self.backend.kill_owned("wpa_supplicant");
            let iface_arg = ["-i", iface.as_str()];
            self.backend
                .kill_foreign(WPA_SUPPLICANT, "wpa_supplicant", &[&iface_arg]);
            self.backend.clear_stale_ctrl_socket();
            self.backend.spawn(
                "wpa_supplicant",
                WPA_SUPPLICANT,
                &["-i", &iface, "-C", &ctrl_dir, "-c", &sta.to_string_lossy()],
            )?;
            self.wait_for_ctrl(SUPPLICANT_START)?;
        } else {
            // same conf file — RECONFIGURE is the robust route
            wpa_ctrl::expect_ok(&self.backend.ctrl("RECONFIGURE")?)?;
        }
        self.backend.run("ip", &["link", "set", &iface, "up"])?;
        self.backend.run("ip", &["addr", "flush", "dev", &iface])?;
        // a lease needs a client: the boot's udhcpc is long done,
        // mestro owns this one
        self.backend
            .spawn("udhcpc", "udhcpc", &["-i", &iface, "-f"])?;
        self.wait_for_address()?;
        // the association re-enabled brcmfmac's power saving — the
        // configured state goes back on with the link
        self.apply_power_save();
        Ok(())
    }

    /// the fallback retry: leave the hotspot and give the saved
    /// network one join window — a route means back to STA, anything
    /// else rebuilds the hotspot (a phone would have to rejoin it)
    fn retry_sta(&mut self) {
        tracing::info!("wifi: retrying the saved network from the hotspot");
        // the hotspot tears down for the attempt — the trust flag
        // must not outlive it
        self.ap_up = false;
        self.set_phase(Phase::Ap);
        if self.associate(true).is_ok() && self.backend.routable() {
            self.no_route_secs = 0;
            self.set_phase(Phase::Sta);
            tracing::info!("wifi: the saved network is back");
        } else {
            tracing::warn!("wifi: the saved network is still unreachable");
            self.start_ap(ApReason::Fallback);
        }
    }

    /// stop the dhcp client(s) — mestro's own tracked child plus the
    /// boot scripts' one (exact match: busybox exe + the udhcpc
    /// applet in argv[0], see the backend)
    fn teardown_sta_processes(&mut self, iface: &str) {
        self.backend.kill_owned("udhcpc");
        self.backend
            .kill_foreign(BUSYBOX, "udhcpc", &[&["-i", iface]]);
    }

    fn wait_for_address(&mut self) -> Result<(), String> {
        let mut waited = Duration::ZERO;
        loop {
            // the shutdown token: checked between steps, so the
            // pre-exec stop never waits behind a long op
            if self.shutdown.is_requested() {
                return Err("shutting down".into());
            }
            if self.has_address() {
                return Ok(());
            }
            if waited >= JOIN_WAIT {
                return Err("no address".into());
            }
            self.backend.sleep(POLL);
            waited += POLL;
        }
    }

    /// does the interface have an IPv4 address?
    fn has_address(&mut self) -> bool {
        self.backend
            .run("ip", &["-o", "-4", "addr", "show", "dev", &self.plan.iface])
            .map(|out| out.contains("inet "))
            .unwrap_or(false)
    }

    /// scan and return the COMPLETE result list, within the budget.
    /// Right after boot SCAN_RESULTS answers at once with the
    /// supplicant's cache of an earlier, partial scan — so the results
    /// are read only after `CTRL-EVENT-SCAN-RESULTS` (this client is
    /// attached to the event stream BEFORE the scan starts: an event
    /// sent before the ATTACH would be lost). If the stream cannot be
    /// had, the fallback polls until the count has been the same for
    /// two polls. Either way the wait ends at [`SCAN_WAIT`] with what
    /// the supplicant has
    pub fn scan(&mut self) -> Result<Vec<ScanEntry>, String> {
        if self.shutdown.is_requested() {
            return Err("shutting down".into());
        }
        let attached = self.backend.events_attach().is_ok();
        let _ = self.backend.ctrl("SCAN");
        let result = if attached {
            self.scan_by_event()
        } else {
            self.scan_by_polling()
        };
        if attached {
            self.backend.events_detach();
        }
        result
    }

    /// wait for the scan-finished event, then read the results once
    fn scan_by_event(&mut self) -> Result<Vec<ScanEntry>, String> {
        let mut waited = Duration::ZERO;
        while waited < SCAN_WAIT {
            if self.shutdown.is_requested() {
                return Err("shutting down".into());
            }
            // short slices: a shutdown must not wait out the budget
            let slice = SCAN_EVENT_SLICE.min(SCAN_WAIT - waited);
            if self.backend.wait_scan_finished(slice)? {
                break;
            }
            waited += slice;
        }
        Ok(wpa_ctrl::parse_scan_results(
            &self.backend.ctrl("SCAN_RESULTS")?,
        ))
    }

    /// no event stream: poll until the number of networks has not
    /// changed over two consecutive polls (three equal readings, ~4 s
    /// — a cache that repeats once is not yet proof) and is not zero,
    /// or the budget ends
    fn scan_by_polling(&mut self) -> Result<Vec<ScanEntry>, String> {
        let mut waited = Duration::ZERO;
        let mut previous: Option<usize> = None;
        let mut unchanged = 0;
        loop {
            if self.shutdown.is_requested() {
                return Err("shutting down".into());
            }
            let entries = wpa_ctrl::parse_scan_results(&self.backend.ctrl("SCAN_RESULTS")?);
            unchanged = if previous == Some(entries.len()) {
                unchanged + 1
            } else {
                0
            };
            if (unchanged >= 2 && !entries.is_empty()) || waited >= SCAN_WAIT {
                return Ok(entries);
            }
            previous = Some(entries.len());
            self.backend.sleep(SCAN_POLL);
            waited += SCAN_POLL;
        }
    }

    /// the visible networks: one entry per SSID (hidden networks
    /// join by hand), the strongest BSS wins, sorted by signal
    fn networks(&mut self) -> Result<Value, String> {
        let mut best: Vec<(String, i32, bool)> = Vec::new();
        for entry in self.scan()? {
            if entry.ssid.is_empty() {
                continue;
            }
            match best.iter_mut().find(|(ssid, ..)| *ssid == entry.ssid) {
                Some(slot) => slot.1 = slot.1.max(entry.signal),
                None => {
                    let secured = entry.secured();
                    let signal = entry.signal;
                    best.push((entry.ssid, signal, secured))
                }
            }
        }
        best.sort_by_key(|(_, signal, _)| std::cmp::Reverse(*signal));
        Ok(json!({
            "networks": best
                .iter()
                .map(|(ssid, signal, secured)| json!({
                    "ssid": ssid,
                    "signal": signal,
                    "secured": secured,
                }))
                .collect::<Vec<_>>()
        }))
    }

    fn state_name(&self) -> &'static str {
        match self.phase {
            Phase::Sta => "sta",
            Phase::Ap => "ap",
            Phase::Joining { .. } => "joining",
            Phase::Failed(_) => "failed",
        }
    }

    /// one web op (serialized by the ops lock)
    pub fn op(&mut self, op: WifiOp) -> Result<Value, String> {
        match op {
            WifiOp::Scan => self.networks(),
            WifiOp::Join { ssid, psk } => {
                self.join(&ssid, psk.as_deref())?;
                Ok(json!({ "ssid": ssid, "state": "sta" }))
            }
            // the run loop serves this from the snapshot without
            // the lock — this path stays for the tests
            WifiOp::Status => Ok(locks::lock(&self.snapshot).clone()),
        }
    }

    /// PING until the freshly started supplicant answers
    fn wait_for_ctrl(&mut self, budget: Duration) -> Result<(), String> {
        let mut waited = Duration::ZERO;
        loop {
            if self.shutdown.is_requested() {
                return Err("shutting down".into());
            }
            if let Ok(reply) = self.backend.ctrl("PING") {
                if reply.trim() == "PONG" {
                    return Ok(());
                }
            }
            if waited >= budget {
                // a supplicant that died says so — "did not come up"
                // alone never explained the fallback failures
                let detail = self
                    .backend
                    .child_exit_status("wpa_supplicant")
                    .map(|status| format!(" ({status})"))
                    .unwrap_or_default();
                return Err(format!("wpa_supplicant did not come up{detail}"));
            }
            self.backend.sleep(POLL);
            waited += POLL;
        }
    }
}

/// does any OTHER interface hold an address inside the hotspot
/// subnet? (`ip -o -4 addr show` output: "2: eth0    inet
/// 10.42.0.5/24 brd … scope global eth0" — the token before
/// `inet` is the device, the one after it the address). Our own
/// interface is skipped: it is flushed before the hotspot
/// address goes on
fn ap_subnet_clash(out: &str, ap_net: crate::config::ApNet, own_iface: &str) -> Option<String> {
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(inet) = parts.iter().position(|t| *t == "inet") else {
            continue;
        };
        let (Some(dev), Some(addr)) = (parts.get(inet.checked_sub(1)?), parts.get(inet + 1)) else {
            continue;
        };
        if *dev == own_iface {
            continue;
        }
        let Ok(ip) = addr
            .split('/')
            .next()
            .unwrap_or("")
            .parse::<std::net::Ipv4Addr>()
        else {
            continue;
        };
        if ap_net.contains(ip) {
            return Some((*dev).to_string());
        }
    }
    None
}

/// the ops runtime: the web thread's entry into the manager. The
/// status op reads the snapshot (never the op lock — a long join
/// must not block it); scans and joins run under the lock with the
/// deadline the client paid for, and one join runs at a time
/// how many radio ops may be in flight at once (queued for or
/// holding the manager lock) — bounded work, no unbounded pile-up
/// behind spawn_blocking
const IN_FLIGHT: usize = 2;

pub struct Ops<B: WifiBackend> {
    manager: Arc<Mutex<Manager<B>>>,
    snapshot: Arc<Mutex<Value>>,
    joining: Arc<AtomicBool>,
    /// the AP-mode flag — the auth re-check under the op lock
    ap_mode: Arc<AtomicBool>,
    /// bounds the TOTAL in-flight work, queue wait included
    permits: Arc<tokio::sync::Semaphore>,
}

impl<B: WifiBackend> Ops<B> {
    pub fn new(manager: Arc<Mutex<Manager<B>>>) -> Self {
        let snapshot = locks::lock(&manager).snapshot_arc();
        let ap_mode = locks::lock(&manager).ap_mode_arc();
        Self {
            manager,
            snapshot,
            joining: Arc::new(AtomicBool::new(false)),
            ap_mode,
            permits: Arc::new(tokio::sync::Semaphore::new(IN_FLIGHT)),
        }
    }

    /// the manager behind the ops (the run loop's tick)
    pub fn manager_arc(&self) -> Arc<Mutex<Manager<B>>> {
        self.manager.clone()
    }

    /// one op: served immediately, queued behind the current one, or
    /// rejected — never silently kept for later
    pub fn request(
        &self,
        op: WifiOp,
        deadline: Instant,
        ingress: Ingress,
        paired: bool,
    ) -> Result<Value, String> {
        match op {
            // never behind the op lock, no radio, no credentials —
            // the auth verdict of the HTTP route stands for it
            WifiOp::Status => Ok(locks::lock(&self.snapshot).clone()),
            WifiOp::Scan | WifiOp::Join { .. } => {
                // bound the total in-flight work BEFORE queueing for
                // the lock — no permit, no pile-up
                let _permit = Arc::clone(&self.permits)
                    .try_acquire_owned()
                    .map_err(|_| "wifi manager busy — try again".to_string())?;
                let is_join = matches!(op, WifiOp::Join { .. });
                if is_join && self.joining.swap(true, Ordering::Relaxed) {
                    return Err("another join is in progress".into());
                }
                let out = self.long_op(op, deadline, ingress, paired);
                if is_join {
                    self.joining.store(false, Ordering::Relaxed);
                }
                out
            }
        }
    }

    /// an op that needs the radio: the deadline covers the queue
    /// wait — an op whose time is up before it starts is rejected,
    /// it never runs (a late join must not change credentials under
    /// a client that gave up). The auth is re-checked UNDER the
    /// lock, right before the radio: a request that queued while
    /// the hotspot was up must not run after it ended
    fn long_op(
        &self,
        op: WifiOp,
        deadline: Instant,
        ingress: Ingress,
        paired: bool,
    ) -> Result<Value, String> {
        let mut manager = locks::lock(&self.manager);
        if Instant::now() > deadline {
            return Err("wifi op timed out".into());
        }
        if !(paired || (ingress == Ingress::Ap && self.ap_mode.load(Ordering::Relaxed))) {
            return Err("pairing required".into());
        }
        manager.op(op)
    }
}

/// the manager thread: ops from the web endpoint and the policy tick.
/// Ops never wait for the manager mutex on the async path — they run
/// on the blocking pool; the tick shares the lock with them
pub async fn run(
    mut rx: tokio::sync::mpsc::Receiver<WifiRequest>,
    ap_mode: Arc<AtomicBool>,
    mdns_gate: Arc<MdnsGate>,
    ui: slint::Weak<crate::Mestro>,
    config: crate::config::Wifi,
    web_port: u16,
    shutdown: WifiShutdown,
) {
    // one plan per device: interface detection plus the image paths
    let roots = super::backend::Roots::default();
    let Ok(plan) = super::plan(
        &config,
        &roots,
        std::path::Path::new(crate::system::paths::WPA_CTRL_DIR),
        std::path::Path::new(crate::system::paths::WPA_CONF),
    ) else {
        tracing::warn!("wifi: no wireless interface — the setup lane stays off");
        return;
    };
    // /run/mestro is mestro's private runtime dir (0700): the AP
    // conf and the dnsmasq lease file live in it, the lease path
    // proves ownership in the stray match
    if let Some(dir) = plan.ap.parent() {
        secure_runtime_dir(dir);
    }
    let children = shutdown.children_arc();
    let stop_children = children.clone();
    let mut backend = RealBackend::new(
        &plan.iface,
        &plan.ctrl_dir.to_string_lossy(),
        &roots.net_class,
        &roots.proc,
        children,
    );
    let mac = backend.mac().unwrap_or_default();
    let manager = Arc::new(Mutex::new(Manager::new(
        backend,
        plan,
        ap_mode.clone(),
        mdns_gate,
        ui,
        &mac,
        web_port,
        shutdown.clone(),
    )));
    {
        let mut mgr = locks::lock(&manager);
        mgr.boot();
        let status = mgr.op(WifiOp::Status).ok();
        tracing::info!("wifi: boot: {:?}", status);
    }
    let ops = Arc::new(Ops::new(manager));
    // the tick shares the manager lock with the ops (it may wait
    // behind a long join — the runtime stays free)
    let tick_manager = ops.manager_arc();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // the restart path's stop signal: the children must be gone
    // before the process is exec'd away. The stop takes the small
    // children lock directly — never the op lock — and spares a
    // working STA (the next start adopts it); the running ops see
    // the token between steps
    let mut shutdown_watch = tokio::time::interval(Duration::from_millis(200));
    shutdown_watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown_watch.tick() => {
                if shutdown.is_requested() {
                    let ap = ap_mode.load(Ordering::Relaxed);
                    super::backend::stop_ap_processes(&stop_children, ap);
                    shutdown.mark_stopped();
                    tracing::info!("wifi: manager stopped — a working STA survives");
                    return;
                }
            }
            _ = ticker.tick() => {
                let tick_manager = tick_manager.clone();
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut mgr) = tick_manager.lock() {
                        mgr.tick();
                    }
                });
            }
            req = rx.recv() => {
                let Some((op, deadline, ingress, paired, reply)) = req else { return; };
                let ops = ops.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = reply.send(ops.request(op, deadline, ingress, paired));
                });
            }
        }
    }
}

/// create the private runtime dir (the AP conf and the dnsmasq
/// lease file live here; the lease path proves ownership in the
/// stray match — a foreign dnsmasq would not use it)
fn secure_runtime_dir(dir: &std::path::Path) {
    if std::fs::create_dir_all(dir).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::backend::fake::FakeBackend;
    use super::*;
    use crate::remote::web::{confirming_worker, GatePhase};

    const MAC: &str = "d8:3a:dd:77:34:56";

    /// the default [wifi] resolved for tests: wlan0, the default
    /// hotspot addresses, conf files in a temp dir
    fn plan(dir: &std::path::Path) -> Plan {
        Plan {
            iface: "wlan0".into(),
            ctrl_dir: "/run/wpa_supplicant".into(),
            sta: dir.join("wpa_supplicant.conf"),
            sta_bak: dir.join("wpa_supplicant.conf.bak"),
            sta_good: dir.join("wpa_supplicant.conf.good"),
            ap: dir.join("run").join("ap.conf"),
            ap_net: crate::config::Wifi::default().ap_net(),
            power_save: false,
            country: String::new(),
        }
    }

    fn manager(backend: FakeBackend) -> (Manager<FakeBackend>, Arc<AtomicBool>) {
        let dir = tempfile::tempdir().unwrap();
        let ap_mode = Arc::new(AtomicBool::new(false));
        // the URL-default web port: the portal URI carries no port
        let mgr = Manager::new(
            backend,
            plan(dir.path()),
            ap_mode.clone(),
            Arc::default(),
            slint::Weak::default(),
            MAC,
            80,
            WifiShutdown::default(),
        );
        std::mem::forget(dir); // the paths outlive the helper; the OS cleans up
        (mgr, ap_mode)
    }

    /// the canned `ip` reply for "wlan0 has an address"
    const WLAN_ADDR: &str = "3: wlan0    inet 192.168.178.20/24 brd ... scope global wlan0";

    fn sta_conf_file(ssid: &str) -> String {
        conf::sta_conf(ssid, Some("password12"), "/run/wpa_supplicant", "").unwrap()
    }

    #[test]
    fn the_hotspot_name_comes_from_the_mac() {
        let (mgr, _) = manager(FakeBackend::new(MAC));
        // always four hex chars, whatever the MAC looks like
        assert_eq!(mgr.ap_ssid, "mestro-3456");
        let suffix = mgr.ap_ssid.strip_prefix("mestro-").unwrap();
        assert_eq!(suffix.len(), 4);
        assert!(suffix.bytes().all(|b| b.is_ascii_hexdigit()));
        // a short MAC still names a 4-hex-char hotspot
        let dir = tempfile::tempdir().unwrap();
        let short = Manager::new(
            FakeBackend::new("aa:bb"),
            plan(dir.path()),
            Arc::new(AtomicBool::new(false)),
            Arc::default(),
            slint::Weak::default(),
            "aa:bb",
            80,
            WifiShutdown::default(),
        );
        assert_eq!(short.ap_ssid, "mestro-AABB");
    }

    #[test]
    fn boot_without_a_conf_starts_the_hotspot() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
        assert!(ap_mode.load(Ordering::Relaxed));
        // the AP conf names the hotspot, the supplicant runs it,
        // dnsmasq serves DNS + DHCP with the RFC 8910 URI
        let ap_conf = std::fs::read_to_string(&mgr.plan.ap).unwrap();
        assert!(ap_conf.contains("mode=2"));
        assert!(ap_conf.contains("mestro-3456"));
        let log = mgr.backend.log();
        assert!(log.iter().any(|l| l
            == &format!(
            "spawn wpa_supplicant /usr/sbin/wpa_supplicant -i wlan0 -C /run/wpa_supplicant -c {}",
            mgr.plan.ap.display()
        )));
        assert!(log.iter().any(|l| l.contains(
            "spawn dnsmasq /usr/sbin/dnsmasq --no-daemon --address=/#/10.42.0.1 --dhcp-option=114,http://10.42.0.1/setup/wifi --dhcp-range=10.42.0.10,10.42.0.50,1h --interface=wlan0 --bind-interfaces --dhcp-leasefile=/run/mestro/dnsmasq.leases"
        )));
        assert!(log
            .iter()
            .any(|l| l == "run ip addr add 10.42.0.1/24 dev wlan0"));
    }

    #[test]
    fn a_foreign_address_in_the_hotspot_subnet_refuses_the_hotspot() {
        // the safety guard: another interface already holds an
        // address inside the hotspot subnet — the route into it
        // would be ambiguous, the peer-subnet trust would leak onto
        // that network. The hotspot fails closed and the existing
        // fallback paths run (no saved network here: Failed)
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            all_addrs: "2: eth0    inet 10.42.0.5/24 brd 10.42.0.255 scope global eth0".into(),
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert!(matches!(mgr.phase(), Phase::Failed(_)), "{:?}", mgr.phase());
        assert!(!ap_mode.load(Ordering::Relaxed));
        let status = mgr.op(WifiOp::Status).unwrap();
        assert!(
            status["error"].as_str().unwrap().contains("eth0"),
            "{status}"
        );
        // the guard fires before any radio work
        assert!(!mgr
            .backend
            .log()
            .iter()
            .any(|l| l.starts_with("spawn dnsmasq")));
    }

    #[test]
    fn an_own_iface_address_in_the_subnet_does_not_refuse_the_hotspot() {
        // a stale hotspot address on our own interface is flushed
        // before the new one goes on — not a clash
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            all_addrs: "3: wlan0    inet 10.42.0.1/24 brd 10.42.0.255 scope global wlan0".into(),
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
        assert!(ap_mode.load(Ordering::Relaxed));
    }

    #[test]
    fn the_subnet_clash_parse_reads_ip_o_output() {
        let ap_net = crate::config::Wifi::default().ap_net();
        let out = "1: lo    inet 127.0.0.1/8 brd 127.0.0.0 scope host lo\\       valid_lft forever preferred_lft forever\n\
                   2: eth0    inet 192.168.178.20/24 brd 192.168.178.255 scope global eth0\n\
                   3: wlan0    inet 10.42.0.1/24 brd 10.42.0.255 scope global wlan0\\       valid_lft forever preferred_lft forever\n";
        // loopback + LAN + our own iface (skipped): no clash
        assert_eq!(ap_subnet_clash(out, ap_net, "wlan0"), None);
        // a foreign interface inside the hotspot subnet is named
        let clash = out.replace(
            "192.168.178.20/24 brd 192.168.178.255 scope global eth0",
            "10.42.0.5/24 brd 10.42.0.255 scope global eth0",
        );
        assert_eq!(
            ap_subnet_clash(&clash, ap_net, "wlan0").as_deref(),
            Some("eth0")
        );
        // a foreign interface in ANOTHER private subnet is no clash
        let far = out.replace(
            "192.168.178.20/24 brd 192.168.178.255 scope global eth0",
            "10.99.0.5/24 brd 10.99.0.255 scope global eth0",
        );
        assert_eq!(ap_subnet_clash(&far, ap_net, "wlan0"), None);
    }

    #[test]
    fn the_portal_uri_names_the_configured_web_port() {
        // 80 (the URL default) carries no port — covered by
        // boot_without_a_conf_starts_the_hotspot; any other port
        // is spelled out in the RFC 8910 URI
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = Manager::new(
            FakeBackend {
                ping_ok: true,
                ..FakeBackend::new(MAC)
            },
            plan(dir.path()),
            Arc::new(AtomicBool::new(false)),
            Arc::default(),
            slint::Weak::default(),
            MAC,
            8787,
            WifiShutdown::default(),
        );
        std::mem::forget(dir);
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
        assert!(mgr
            .backend
            .log()
            .iter()
            .any(|l| l.contains("--dhcp-option=114,http://10.42.0.1:8787/setup/wifi")));
    }

    #[test]
    fn a_failed_hotspot_start_keeps_the_gate_closed() {
        // the radio path is broken (spawn fails): the trust flag must
        // NOT flip up — fail closed, not open
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            spawn_fails: true,
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert!(matches!(mgr.phase(), Phase::Failed(_)), "{:?}", mgr.phase());
        assert!(!mgr.ap_up());
        assert!(!ap_mode.load(Ordering::Relaxed));
        // and the status says so without naming a hotspot
        let status = mgr.op(WifiOp::Status).unwrap();
        assert_eq!(status["state"], "failed");
        assert!(status["error"].as_str().unwrap().contains("hotspot failed"));
    }

    #[test]
    fn a_failed_setup_hotspot_falls_back_to_the_last_good_network() {
        // no conf, but a network that worked before: the hotspot
        // cannot start — bring the player back online instead of
        // leaving it with neither
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            spawn_fails: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta_good, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert!(!ap_mode.load(Ordering::Relaxed));
        assert!(std::fs::read_to_string(&mgr.plan.sta)
            .unwrap()
            .contains("Home"));
        let status = mgr.op(WifiOp::Status).unwrap();
        assert!(
            status["error"].as_str().unwrap().contains("hotspot failed"),
            "the failure stays visible: {status}"
        );
    }

    #[test]
    fn a_failed_fallback_hotspot_returns_to_the_saved_network() {
        // the route was lost, the hotspot cannot start: the saved
        // conf is still there — stay on (retry) STA, never offline
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        mgr.backend.spawn_fails = true;
        mgr.start_ap(ApReason::Fallback);
        assert!(!ap_mode.load(Ordering::Relaxed));
        assert_eq!(mgr.phase(), &Phase::Sta, "back on the saved network");
        assert!(std::fs::read_to_string(&mgr.plan.sta)
            .unwrap()
            .contains("Home"));
    }

    #[test]
    fn the_last_good_copy_waits_for_the_route() {
        // the boot runs before DHCP: no route yet, no copy — the tick
        // that sees the route makes it
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert!(!mgr.plan.sta_good.exists());
        mgr.backend.routable = true;
        mgr.tick();
        assert!(std::fs::read_to_string(&mgr.plan.sta_good)
            .unwrap()
            .contains("Home"));
    }

    #[test]
    fn a_confirmed_join_keeps_a_last_good_copy() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            routable: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert!(
            std::fs::read_to_string(&mgr.plan.sta_good)
                .unwrap()
                .contains("Home"),
            "a conf that booted into STA with a route is the last good one"
        );
    }

    #[test]
    fn boot_with_the_template_starts_the_hotspot() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(
            &mgr.plan.sta,
            "network={\n\tssid=\"YourNetwork\"\n\tpsk=\"YourPassword\"\n}\n",
        )
        .unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
    }

    #[test]
    fn boot_mends_a_conf_lost_to_a_crash() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        // a join crashed after the bak write, before the conf rename
        std::fs::write(&mgr.plan.sta_bak, sta_conf_file("Home")).unwrap();
        mgr.boot();
        // the bak is back as the conf — and STA booted from it
        assert_eq!(mgr.phase(), &Phase::Sta);
        assert!(std::fs::read_to_string(&mgr.plan.sta)
            .unwrap()
            .contains("Home"));
        assert!(!mgr.plan.sta_bak.exists());
    }

    #[test]
    fn boot_with_a_usable_conf_stays_sta() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        assert!(!ap_mode.load(Ordering::Relaxed));
        assert!(!mgr
            .backend
            .log()
            .iter()
            .any(|l| l.starts_with("spawn dnsmasq")));
    }

    #[test]
    fn a_join_from_the_hotspot_reaches_sta_and_leaves_ap_behind() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            wlan_addrs: WLAN_ADDR.into(),
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
        let old_log_len = mgr.backend.log().len();

        mgr.join("FritzBox", Some("password12")).unwrap();
        assert_eq!(mgr.phase(), &Phase::Sta);
        assert!(!ap_mode.load(Ordering::Relaxed));
        // the conf is written (with the hashed psk) and the hotspot
        // teardown happened — own children by handle, boot strays by
        // the exact match
        let sta = std::fs::read_to_string(&mgr.plan.sta).unwrap();
        assert!(sta.contains("FritzBox"));
        assert!(!sta.contains("password12"));
        let log = mgr.backend.log()[old_log_len..].to_vec();
        assert!(log.iter().any(|l| l == "kill udhcpc"));
        assert!(log
            .iter()
            .any(|l| l == "kill foreign /bin/busybox udhcpc -i wlan0"));
        assert!(log.iter().any(|l| l == "kill dnsmasq"));
        assert!(log.iter().any(|l| l == "kill wpa_supplicant"));
        assert!(log
            .iter()
            .any(|l| l == "kill foreign /usr/sbin/wpa_supplicant wpa_supplicant -i wlan0"));
        assert!(log.iter().any(|l| l
            == &format!(
            "spawn wpa_supplicant /usr/sbin/wpa_supplicant -i wlan0 -C /run/wpa_supplicant -c {}",
            mgr.plan.sta.display()
        )));
        assert!(log.iter().any(|l| l == "spawn udhcpc udhcpc -i wlan0 -f"));
        // the confirmed join dropped the predecessor backup
        assert!(!mgr.plan.sta_bak.exists());
        // the status op reports the network
        let status = mgr.op(WifiOp::Status).unwrap();
        assert_eq!(status["ssid"], "FritzBox");
        assert_eq!(status["state"], "sta");
    }

    #[test]
    fn the_hotspot_is_built_only_after_the_mdns_gate_reports_paused() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(MdnsGate::default());
        let _announcer = confirming_worker(&gate);
        let ap_mode = Arc::new(AtomicBool::new(false));
        let mut mgr = Manager::new(
            FakeBackend {
                ping_ok: true,
                watch: Some(gate.clone()),
                ..FakeBackend::new(MAC)
            },
            plan(dir.path()),
            ap_mode.clone(),
            gate.clone(),
            slint::Weak::default(),
            MAC,
            80,
            WifiShutdown::default(),
        );
        std::mem::forget(dir);
        mgr.start_ap(ApReason::Setup);
        // every command of the transition saw the confirmed pause
        assert!(!mgr.backend.watched.is_empty());
        assert!(
            mgr.backend.watched.iter().all(|p| *p == GatePhase::Paused),
            "{:?}",
            mgr.backend.watched
        );
        // it is its own signal: the trust flag follows the confirmed
        // hotspot, the pause covers the whole span
        assert_eq!(gate.phase(), GatePhase::Paused);
        assert!(ap_mode.load(Ordering::Relaxed));
    }

    #[test]
    fn a_stuck_mdns_announcer_does_not_block_the_hotspot() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(MdnsGate::default());
        // attached, never confirms
        let _stuck = gate.attach();
        let ap_mode = Arc::new(AtomicBool::new(false));
        let mut mgr = Manager::new(
            FakeBackend {
                ping_ok: true,
                ..FakeBackend::new(MAC)
            },
            plan(dir.path()),
            ap_mode.clone(),
            gate.clone(),
            slint::Weak::default(),
            MAC,
            80,
            WifiShutdown::default(),
        );
        std::mem::forget(dir);
        mgr.mdns_pause_wait = Duration::from_millis(50);
        mgr.start_ap(ApReason::Setup);
        assert_eq!(mgr.phase(), &Phase::Ap);
        assert!(ap_mode.load(Ordering::Relaxed));
        assert_eq!(gate.phase(), GatePhase::PauseRequested);
    }

    #[test]
    fn the_mdns_pause_also_covers_a_failed_hotspot_and_ends_on_sta() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(MdnsGate::default());
        let mut mgr = Manager::new(
            FakeBackend {
                ping_ok: true,
                wlan_addrs: WLAN_ADDR.into(),
                ..FakeBackend::new(MAC)
            },
            plan(dir.path()),
            Arc::new(AtomicBool::new(false)),
            gate.clone(),
            slint::Weak::default(),
            MAC,
            80,
            WifiShutdown::default(),
        );
        std::mem::forget(dir);
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
        assert_ne!(gate.phase(), GatePhase::Running);
        // the join leaves the hotspot: announcing may resume
        mgr.join("FritzBox", Some("password12")).unwrap();
        assert_eq!(mgr.phase(), &Phase::Sta);
        assert_eq!(gate.phase(), GatePhase::Running);
    }

    #[test]
    fn a_failed_join_from_the_hotspot_restores_and_returns_to_ap() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            wlan_addrs: String::new(), // no address ever
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);

        let error = mgr.join("FritzBox", Some("password12")).unwrap_err();
        assert!(error.contains("FritzBox"), "{error}");
        assert!(matches!(mgr.phase(), Phase::Failed(_)));
        // the hotspot is back up — the flag went down during the
        // join and up again with the rebuilt AP
        assert!(mgr.ap_up());
        assert!(ap_mode.load(Ordering::Relaxed));
        // no predecessor: the failed conf is gone again
        assert!(!mgr.plan.sta.exists());
        let status = mgr.op(WifiOp::Status).unwrap();
        assert_eq!(status["state"], "failed");
        assert_eq!(status["ap_ssid"], "mestro-3456");
        assert!(status["error"].as_str().unwrap().contains("could not join"));
    }

    #[test]
    fn a_failed_join_from_the_hotspot_with_a_predecessor_restores_it() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            wlan_addrs: String::new(),
            ..FakeBackend::new(MAC)
        });
        // the boot conf exists but is the template (not usable)
        std::fs::write(&mgr.plan.sta, "network={\n\tssid=\"YourNetwork\"\n}\n").unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
        mgr.join("FritzBox", Some("password12")).unwrap_err();
        // the template is back — the user's PC edits survive a
        // failed portal join
        assert!(std::fs::read_to_string(&mgr.plan.sta)
            .unwrap()
            .contains("YourNetwork"));
        assert!(
            !mgr.plan.sta_bak.exists(),
            "the bak is consumed by the restore"
        );
    }

    #[test]
    fn a_failed_join_from_sta_returns_to_the_old_network() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            wlan_addrs: String::new(),
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        // the join cannot get an address but the old network has one
        let log_len = mgr.backend.log().len();
        let error = mgr.join("Other", Some("password12")).unwrap_err();
        assert!(error.contains("Other"), "{error}");
        // back in STA on the restored conf — no hotspot, no phone
        // left behind
        assert_eq!(mgr.phase(), &Phase::Sta);
        assert!(!ap_mode.load(Ordering::Relaxed));
        assert!(std::fs::read_to_string(&mgr.plan.sta)
            .unwrap()
            .contains("Home"));
        // the old conf was re-applied over the ctrl socket
        assert!(mgr.backend.log()[log_len..]
            .iter()
            .any(|l| l == "ctrl RECONFIGURE"));
    }

    #[test]
    fn the_policy_tick_raises_the_hotspot_after_the_grace_window() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            routable: false,
            eth_up: false,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        // 55 s of no route: still waiting
        for _ in 0..11 {
            mgr.tick();
        }
        assert_eq!(mgr.phase(), &Phase::Sta, "the grace window is 60s");
        mgr.tick();
        assert_eq!(mgr.phase(), &Phase::Ap);
        assert!(ap_mode.load(Ordering::Relaxed));
    }

    #[test]
    fn a_route_or_the_wired_fallback_keeps_sta_mode() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            routable: true,
            eth_up: false,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        for _ in 0..20 {
            mgr.tick();
        }
        assert_eq!(mgr.phase(), &Phase::Sta);

        // the wired interface alone (no wifi route) is a working
        // setup too
        mgr.backend.routable = false;
        mgr.backend.eth_up = true;
        for _ in 0..20 {
            mgr.tick();
        }
        assert_eq!(mgr.phase(), &Phase::Sta);
    }

    /// a fallback hotspot (usable conf, route lost) driven by ticks
    fn fallback_ap(mgr: &mut Manager<FakeBackend>) {
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        mgr.backend.routable = false;
        mgr.backend.eth_up = false;
        for _ in 0..12 {
            mgr.tick();
        }
        assert_eq!(
            mgr.phase(),
            &Phase::Ap,
            "the no-route grace window ends in AP"
        );
    }

    #[test]
    fn a_fallback_hotspot_retries_the_saved_network() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            routable: false,
            eth_up: false,
            wlan_addrs: WLAN_ADDR.into(),
            ..FakeBackend::new(MAC)
        });
        fallback_ap(&mut mgr);
        // the router is back before the retry window: the retry
        // reaches STA and the flag goes down
        mgr.backend.routable = true;
        for _ in 0..23 {
            mgr.tick(); // 115 s: still waiting
        }
        assert_eq!(mgr.phase(), &Phase::Ap);
        mgr.tick(); // 120 s: the retry runs
        assert_eq!(mgr.phase(), &Phase::Sta);
        assert!(!ap_mode.load(Ordering::Relaxed));
    }

    #[test]
    fn a_fallback_hotspot_without_a_route_rebuilds_the_ap() {
        let (mut mgr, ap_mode) = manager(FakeBackend {
            ping_ok: true,
            routable: false,
            eth_up: false,
            wlan_addrs: WLAN_ADDR.into(), // an address but no route
            ..FakeBackend::new(MAC)
        });
        fallback_ap(&mut mgr);
        for _ in 0..24 {
            mgr.tick(); // 120 s: the retry runs, the route is absent
        }
        assert_eq!(
            mgr.phase(),
            &Phase::Ap,
            "the retry fails back into the hotspot"
        );
        assert!(ap_mode.load(Ordering::Relaxed));
        // the retry clock restarted: the next retry is 120 s out
        for _ in 0..23 {
            mgr.tick();
        }
        assert_eq!(mgr.phase(), &Phase::Ap, "no second retry inside the window");
    }

    #[test]
    fn a_phone_on_the_portal_wins_over_the_retry_clock() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            routable: false,
            eth_up: false,
            ap_client: true, // a phone holds a lease
            ..FakeBackend::new(MAC)
        });
        fallback_ap(&mut mgr);
        for _ in 0..48 {
            mgr.tick(); // 240 s — the whole retry window passes
        }
        assert_eq!(
            mgr.phase(),
            &Phase::Ap,
            "a client on the portal blocks the retry"
        );
        // the client left: the next window retries
        mgr.backend.ap_client = false;
        mgr.backend.routable = true;
        mgr.backend.wlan_addrs = WLAN_ADDR.into();
        for _ in 0..24 {
            mgr.tick();
        }
        assert_eq!(mgr.phase(), &Phase::Sta);
    }

    #[test]
    fn a_first_run_hotspot_never_retries() {
        // no usable conf: nothing to retry — the hotspot stays until
        // a join comes through the portal
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            routable: false,
            eth_up: false,
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Ap);
        for _ in 0..48 {
            mgr.tick();
        }
        assert_eq!(mgr.phase(), &Phase::Ap);
        assert!(!mgr.backend.log().iter().any(|l| l.contains("RECONFIGURE")));
    }

    #[test]
    fn the_scan_lists_one_entry_per_ssid_strongest_first() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            scan_results: "3\n\
                aa:aa:aa:aa:aa:01\t2412\t-52\t[WPA2-PSK-CCMP][ESS]\tFritzBox\n\
                aa:aa:aa:aa:aa:02\t2437\t-44\t[WPA2-PSK-CCMP][ESS]\tFritzBox\n\
                bb:bb:bb:bb:bb:01\t2462\t-70\t[ESS]\tOpenCafe\n\
                cc:cc:cc:cc:cc:01\t2412\t-60\t[ESS]\t\n"
                .into(),
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        let data = mgr.op(WifiOp::Scan).unwrap();
        assert_eq!(
            data,
            serde_json::json!({
                "networks": [
                    { "ssid": "FritzBox", "signal": -44, "secured": true },
                    { "ssid": "OpenCafe", "signal": -70, "secured": false },
                ]
            })
        );
    }

    // ── the first scan after boot: the supplicant's cache holds a partial
    // list; the scan returns the full one once the scan-finished event
    // arrived ──

    const PARTIAL: &str = "1\n\
        aa:aa:aa:aa:aa:01\t2412\t-52\t[WPA2-PSK-CCMP][ESS]\tFritzBox\n";
    const FULL: &str = "3\n\
        aa:aa:aa:aa:aa:01\t2412\t-52\t[WPA2-PSK-CCMP][ESS]\tFritzBox\n\
        bb:bb:bb:bb:bb:01\t2462\t-70\t[ESS]\tOpenCafe\n\
        cc:cc:cc:cc:cc:01\t2437\t-61\t[WPA2-PSK-CCMP][ESS]\tNachbar\n";

    fn ssids(entries: &[wpa_ctrl::ScanEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.ssid.as_str()).collect()
    }

    #[test]
    fn a_scan_waits_for_the_finished_event_instead_of_returning_the_cached_partial_list() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            stale_results: Some(PARTIAL.into()),
            scan_results: FULL.into(),
            ..FakeBackend::new(MAC)
        });
        let entries = mgr.scan().unwrap();
        assert_eq!(ssids(&entries), ["FritzBox", "OpenCafe", "Nachbar"]);
        // subscribed BEFORE the scan started (an event after SCAN would
        // otherwise be missed), results read only after the event, and
        // the subscription dropped again
        let log = mgr.backend.log();
        let at = |needle: &str| log.iter().position(|l| l == needle).unwrap();
        assert!(at("attach") < at("ctrl SCAN"), "{log:?}");
        assert!(at("ctrl SCAN") < at("wait scan finished"), "{log:?}");
        assert!(
            at("wait scan finished") < at("ctrl SCAN_RESULTS"),
            "{log:?}"
        );
        assert_eq!(log.last().map(String::as_str), Some("detach"));
    }

    #[test]
    fn a_supplicant_that_never_reports_ends_at_the_budget_with_what_it_has() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            stale_results: Some(PARTIAL.into()),
            scan_results: FULL.into(),
            scan_event: false,
            ..FakeBackend::new(MAC)
        });
        let entries = mgr.scan().unwrap();
        assert_eq!(
            ssids(&entries),
            ["FritzBox"],
            "the cache beats an empty answer"
        );
        assert!(mgr.backend.log().contains(&"detach".to_string()));
    }

    #[test]
    fn without_an_event_stream_the_scan_polls_until_the_list_stops_growing() {
        // ATTACH refused: fall back to the count being stable for two
        // polls — growing results return the full list, not the first
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            attach_fails: true,
            scan_sequence: vec![
                PARTIAL.into(),
                PARTIAL.into(),
                FULL.into(),
                FULL.into(),
                FULL.into(),
            ],
            ..FakeBackend::new(MAC)
        });
        let entries = mgr.scan().unwrap();
        assert_eq!(ssids(&entries), ["FritzBox", "OpenCafe", "Nachbar"]);
        assert!(!mgr.backend.log().iter().any(|l| l == "wait scan finished"));
    }

    #[test]
    fn the_polling_fallback_keeps_an_empty_result_waiting_until_the_budget() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            attach_fails: true,
            scan_results: "0\n".into(),
            ..FakeBackend::new(MAC)
        });
        let entries = mgr.scan().unwrap();
        assert!(entries.is_empty());
        let polls = mgr
            .backend
            .log()
            .iter()
            .filter(|l| *l == "ctrl SCAN_RESULTS")
            .count();
        assert!(polls >= 6, "waited out the budget: {polls} polls");
    }

    #[test]
    fn a_shutdown_ends_the_scan_at_once() {
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            scan_results: FULL.into(),
            ..FakeBackend::new(MAC)
        });
        mgr.shutdown.request();
        assert!(mgr.scan().is_err());
    }

    // ── the radio policy: power save (brcmfmac re-enables it on
    // every association) and the regulatory domain ──

    /// a manager with the [wifi] radio policy resolved into its
    /// plan (power save, regdom)
    fn policy_manager(
        backend: FakeBackend,
        dir: &std::path::Path,
        power_save: bool,
        country: &str,
    ) -> Manager<FakeBackend> {
        let mut resolved = plan(dir);
        resolved.power_save = power_save;
        resolved.country = country.into();
        Manager::new(
            backend,
            resolved,
            Arc::new(AtomicBool::new(false)),
            Arc::default(),
            slint::Weak::default(),
            MAC,
            80,
            WifiShutdown::default(),
        )
    }

    /// how many power-save applies landed in the log
    fn power_saves(log: &[String]) -> usize {
        log.iter().filter(|l| l.contains("set power_save")).count()
    }

    #[test]
    fn boot_pins_the_regdom_and_the_power_save_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = policy_manager(
            FakeBackend {
                ping_ok: true,
                ..FakeBackend::new(MAC)
            },
            dir.path(),
            false,
            "DE",
        );
        std::mem::forget(dir);
        mgr.boot();
        let log = mgr.backend.log();
        assert!(log.iter().any(|l| l == "run iw reg set DE"), "{log:?}");
        assert!(
            log.iter()
                .any(|l| l == "run iw dev wlan0 set power_save off"),
            "{log:?}"
        );
        // the hotspot conf carries the regdom too
        assert!(std::fs::read_to_string(&mgr.plan.ap)
            .unwrap()
            .contains("country=DE\n"));
    }

    #[test]
    fn a_configured_power_save_on_is_applied_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = policy_manager(
            FakeBackend {
                ping_ok: true,
                ..FakeBackend::new(MAC)
            },
            dir.path(),
            true,
            "",
        );
        std::mem::forget(dir);
        mgr.boot();
        // and without a country the regdom stays untouched
        let log = mgr.backend.log();
        assert!(
            log.iter()
                .any(|l| l == "run iw dev wlan0 set power_save on"),
            "{log:?}"
        );
        assert!(!log.iter().any(|l| l.starts_with("run iw reg")));
    }

    #[test]
    fn a_join_reapplies_power_save_with_the_new_link() {
        // brcmfmac re-enables power saving on association — every
        // link mestro builds gets the configured state back
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            wlan_addrs: WLAN_ADDR.into(),
            ..FakeBackend::new(MAC)
        });
        mgr.boot(); // the boot apply
        assert_eq!(power_saves(&mgr.backend.log()), 1);
        mgr.join("FritzBox", Some("password12")).unwrap();
        assert_eq!(
            power_saves(&mgr.backend.log()),
            2,
            "the association re-applies it"
        );
    }

    #[test]
    fn the_tick_reapplies_power_save_once_per_association() {
        // a roam re-associates behind mestro's back and re-enables
        // power saving: the tick applies the configured state once
        // per association (the BSSID names it), never per tick
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            routable: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(power_saves(&mgr.backend.log()), 1, "the boot apply");
        mgr.tick(); // the boot association's state is unknown
        assert_eq!(power_saves(&mgr.backend.log()), 2, "one catch-up");
        for _ in 0..5 {
            mgr.tick(); // the same association: nothing new
        }
        assert_eq!(power_saves(&mgr.backend.log()), 2, "no iw per tick");
        // the supplicant roamed to another BSSID
        mgr.backend.status = "wpa_state=COMPLETED\nbssid=aa:bb:cc:dd:ee:ff\n".into();
        mgr.tick();
        assert_eq!(power_saves(&mgr.backend.log()), 3, "a new association");
        // and a link that is not up leaves it alone
        mgr.backend.status = "wpa_state=SCANNING\n".into();
        mgr.tick();
        assert_eq!(power_saves(&mgr.backend.log()), 3);
    }

    // ── the ops runtime: status never blocks, one join at a time,
    // expired ops never start ──

    fn ops(
        backend: FakeBackend,
    ) -> (
        Ops<FakeBackend>,
        Arc<Mutex<Manager<FakeBackend>>>,
        Arc<AtomicBool>,
    ) {
        let (mgr, ap_mode) = manager(backend);
        let manager = Arc::new(Mutex::new(mgr));
        let ops = Ops::new(manager.clone());
        (ops, manager, ap_mode)
    }

    #[test]
    fn status_answers_while_the_manager_is_locked() {
        let (ops, manager, _) = ops(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        // another thread holds the op lock for as long as it likes
        let (started, held) = std::sync::mpsc::channel::<()>();
        let blocker = std::thread::spawn(move || {
            let guard = locks::lock(&manager);
            started.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            drop(guard);
        });
        held.recv().unwrap();
        // the status op answers from the snapshot regardless
        let status = ops
            .request(
                WifiOp::Status,
                Instant::now() + Duration::from_secs(1),
                Ingress::Ap,
                true,
            )
            .unwrap();
        assert_eq!(status["state"], "sta");
        blocker.join().unwrap();
    }

    #[test]
    fn an_expired_op_never_runs() {
        let (ops, manager, _) = ops(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        let before = locks::lock(&manager).backend.log().len();
        let out = ops.request(
            WifiOp::Scan,
            Instant::now() - Duration::from_secs(1), // already expired
            Ingress::Ap,
            true,
        );
        assert!(out.unwrap_err().contains("timed out"));
        // nothing touched the radio
        assert_eq!(locks::lock(&manager).backend.log().len(), before);
    }

    #[test]
    fn a_second_join_is_busy_not_queued() {
        let (ops, _, _) = ops(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        // mark a join as running (the flag the runtime flips)
        ops.joining.store(true, Ordering::Relaxed);
        let out = ops.request(
            WifiOp::Join {
                ssid: "Home".into(),
                psk: Some("password12".into()),
            },
            Instant::now() + Duration::from_secs(45),
            Ingress::Ap,
            true,
        );
        assert!(out.unwrap_err().contains("in progress"));
        // scans still go through — they do not change credentials
        assert!(ops
            .request(
                WifiOp::Scan,
                Instant::now() + Duration::from_secs(15),
                Ingress::Ap,
                true
            )
            .is_ok());
    }

    #[test]
    fn unpaired_requests_re_check_the_trust_basis_at_execution() {
        // an unpaired request that queued while the hotspot was up
        // must not run once it is down — and never from a LAN
        // listener at all
        let (ops, _manager, ap_mode) = ops(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        let deadline = Instant::now() + Duration::from_secs(45);
        // LAN ingress: rejected even with the flag up
        ap_mode.store(true, Ordering::Relaxed);
        let out = ops.request(WifiOp::Scan, deadline, Ingress::Lan, false);
        assert!(out.unwrap_err().contains("pairing required"));
        // hotspot ingress with the flag down (AP mode ended since
        // the request was made): rejected too
        ap_mode.store(false, Ordering::Relaxed);
        let out = ops.request(WifiOp::Scan, deadline, Ingress::Ap, false);
        assert!(out.unwrap_err().contains("pairing required"));
        // hotspot ingress + flag up: runs
        ap_mode.store(true, Ordering::Relaxed);
        assert!(ops
            .request(WifiOp::Scan, deadline, Ingress::Ap, false)
            .is_ok());
        // paired requests never depend on the hotspot
        ap_mode.store(false, Ordering::Relaxed);
        assert!(ops
            .request(WifiOp::Scan, deadline, Ingress::Lan, true)
            .is_ok());
    }

    #[test]
    fn the_startup_cleanup_targets_our_dnsmasq_by_identity() {
        // the previous life's dnsmasq survived the exec — the boot
        // asks for it by exact identity (interface + lease file)
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        mgr.boot();
        assert!(mgr.backend.log().iter().any(|l| l ==
            "kill foreign /usr/sbin/dnsmasq dnsmasq --interface=wlan0 + --dhcp-leasefile=/run/mestro/dnsmasq.leases"));
    }

    // ── restart keeps a working STA (the first-run "apply &
    // restart" must not take the player offline) ──

    #[test]
    fn boot_with_a_conf_and_no_supplicant_starts_one() {
        // a restart's exec fallback killed the supplicant, or it
        // crashed: the boot with an existing conf ENSURES the STA —
        // supplicant with the conf + a lease client
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: false,            // no ctrl socket answers
            supplicant_running: false, // and none runs
            udhcpc_running: false,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        let log = mgr.backend.log();
        assert!(log.iter().any(|l| l
            == &format!(
            "spawn wpa_supplicant /usr/sbin/wpa_supplicant -i wlan0 -C /run/wpa_supplicant -c {}",
            mgr.plan.sta.display()
        )));
        assert!(log.iter().any(|l| l == "spawn udhcpc udhcpc -i wlan0 -f"));
        // the stale ctrl socket is gone BEFORE the supplicant spawns
        let clear = log
            .iter()
            .position(|l| l == "clear stale ctrl socket")
            .unwrap();
        let spawn = log
            .iter()
            .position(|l| l.starts_with("spawn wpa_supplicant"))
            .unwrap();
        assert!(clear < spawn, "the socket is cleared before the spawn");
    }

    #[test]
    fn the_fallback_hotspot_kills_every_supplicant_before_spawning_its_own() {
        // the fallback path ran while the boot-started STA
        // supplicant was still alive: the new AP supplicant on the
        // same iface + ctrl socket never came up. Own AND foreign
        // supplicants must be gone before the spawn
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        mgr.start_ap(ApReason::Fallback);
        assert_eq!(mgr.phase(), &Phase::Ap);
        let log = mgr.backend.log();
        let kill_own = log.iter().position(|l| l == "kill wpa_supplicant").unwrap();
        let kill_foreign = log
            .iter()
            .position(|l| l == "kill foreign /usr/sbin/wpa_supplicant wpa_supplicant -i wlan0")
            .unwrap();
        let clear = log
            .iter()
            .position(|l| l == "clear stale ctrl socket")
            .unwrap();
        let spawn = log
            .iter()
            .position(|l| l.starts_with("spawn wpa_supplicant"))
            .unwrap();
        assert!(kill_own < kill_foreign, "the owned supplicant dies too");
        assert!(
            kill_foreign < clear,
            "the foreign kill waits before the socket goes"
        );
        assert!(clear < spawn, "the stale socket is gone before the spawn");
    }

    #[test]
    fn a_supplicant_that_died_on_startup_names_its_exit_status() {
        // "did not come up" alone never explained the device's
        // fallback failures — a dead supplicant says how it ended
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: false,
            wpa_exit: Some("exit status signal: 15 (SIGTERM)".into()),
            ..FakeBackend::new(MAC)
        });
        let error = mgr.wait_for_ctrl(Duration::ZERO).unwrap_err();
        assert!(
            error.contains("wpa_supplicant did not come up (exit status signal: 15"),
            "{error}"
        );
    }

    #[test]
    fn boot_with_a_running_supplicant_adopts_it() {
        // the boot scripts' one, or ours from before the exec — the
        // boot neither replaces nor doubles the lease client
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: true,
            supplicant_running: true,
            udhcpc_running: true,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        assert!(!mgr
            .backend
            .log()
            .iter()
            .any(|l| l.starts_with("spawn wpa_supplicant")));
        assert!(!mgr
            .backend
            .log()
            .iter()
            .any(|l| l.starts_with("spawn udhcpc")));

        // a supplicant that matches by identity but does not answer
        // the ctrl socket (an orphan from before the exec) is
        // adopted too — only its missing lease client gets one
        let (mut mgr, _) = manager(FakeBackend {
            ping_ok: false,
            supplicant_running: true,
            udhcpc_running: false,
            ..FakeBackend::new(MAC)
        });
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert!(
            !mgr.backend
                .log()
                .iter()
                .any(|l| l.starts_with("spawn wpa_supplicant")),
            "the running supplicant is adopted, not replaced"
        );
        assert!(mgr
            .backend
            .log()
            .iter()
            .any(|l| l.starts_with("spawn udhcpc")));
    }

    #[test]
    fn the_shutdown_token_cancels_waiting_ops() {
        // the token is checked between poll steps: a join waiting
        // for an address returns at once instead of running past
        // the pre-exec stop
        let shutdown = WifiShutdown::default();
        let dir = tempfile::tempdir().unwrap();
        let ap_mode = Arc::new(AtomicBool::new(false));
        let mut mgr = Manager::new(
            FakeBackend {
                ping_ok: true,
                wlan_addrs: String::new(), // the address never comes
                ..FakeBackend::new(MAC)
            },
            plan(dir.path()),
            ap_mode,
            Arc::default(),
            slint::Weak::default(),
            MAC,
            80,
            shutdown.clone(),
        );
        std::fs::write(&mgr.plan.sta, sta_conf_file("Home")).unwrap();
        mgr.boot();
        assert_eq!(mgr.phase(), &Phase::Sta);
        shutdown.request();
        let error = mgr.join("Other", Some("password12")).unwrap_err();
        assert!(error.contains("shutting down"), "{error}");
    }

    #[test]
    fn an_unpaired_op_waiting_for_the_lock_is_refused_when_the_ap_ends() {
        // the auth check happens UNDER the manager lock, right
        // before the radio: the AP flag may drop while the op waits
        let (ops, manager, ap_mode) = ops(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        ap_mode.store(true, Ordering::Relaxed);
        // another thread holds the op lock (a long join)
        let (started, held) = std::sync::mpsc::channel::<()>();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let blocker = std::thread::spawn(move || {
            let guard = locks::lock(&manager);
            started.send(()).unwrap();
            released.recv().unwrap();
            drop(guard);
        });
        held.recv().unwrap();
        // the unpaired request queues for the lock while the AP is
        // up …
        let requester = std::thread::spawn(move || {
            ops.request(
                WifiOp::Scan,
                Instant::now() + Duration::from_secs(30),
                Ingress::Ap,
                false,
            )
        });
        std::thread::sleep(Duration::from_millis(100));
        // … the hotspot goes down while it waits …
        ap_mode.store(false, Ordering::Relaxed);
        release.send(()).unwrap();
        blocker.join().unwrap();
        // … and the op is refused instead of touching the radio
        assert!(requester
            .join()
            .unwrap()
            .unwrap_err()
            .contains("pairing required"));
    }

    #[test]
    fn the_in_flight_permits_bound_the_work() {
        // the total in-flight work is bounded: with both permits
        // taken, the next radio op is busy at once
        let (ops, _manager, _) = ops(FakeBackend {
            ping_ok: true,
            ..FakeBackend::new(MAC)
        });
        let first = ops.permits.clone().try_acquire_owned().unwrap();
        let second = ops.permits.clone().try_acquire_owned().unwrap();
        let out = ops.request(
            WifiOp::Scan,
            Instant::now() + Duration::from_secs(15),
            Ingress::Ap,
            true,
        );
        assert!(out.unwrap_err().contains("busy"));
        // releasing a permit opens the door again
        drop(second);
        assert!(ops
            .request(
                WifiOp::Scan,
                Instant::now() + Duration::from_secs(15),
                Ingress::Ap,
                true
            )
            .is_ok());
        let _ = first;
    }
}
