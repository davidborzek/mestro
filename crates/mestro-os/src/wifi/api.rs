//! The web endpoint's contract with the wifi manager: the ops
//! vocabulary and the request channel ([`WifiOp`],
//! [`WifiChannel`]), the tracked-children registry ([`Children`])
//! and the restart stop signal ([`WifiShutdown`]). Always
//! compiled — the protocol, the web endpoint and the restart path
//! name these types even in a build without the setup lane
//! (feature `wifi-setup`): a build without the lane wires no
//! manager, and every request answers "unavailable" instead.

use serde::{Deserialize, Serialize};

/// the wifi setup verbs (the web protocol's `Request::Wifi`).
/// `join` takes no `psk` for open networks — the device hashes the
/// passphrase itself (PBKDF2-SHA1), the cleartext never leaves this
/// request
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum WifiOp {
    /// scan and return the visible networks, strongest first,
    /// one entry per SSID (hidden networks join by hand)
    Scan,
    /// join a network: write the credentials, leave AP mode when
    /// in it, wait for an address — a failure restores the previous
    /// configuration and returns to AP mode
    Join { ssid: String, psk: Option<String> },
    /// the current mode, SSIDs and the last error
    Status,
}

/// the request path to the wifi manager thread: one op and the slot
/// for its reply. A build without the setup lane wires no manager —
/// every request answers "unavailable". The ops run on the manager's
/// own thread; the async request here only ever awaits
#[derive(Clone)]
pub struct WifiChannel {
    tx: Option<std::sync::Arc<tokio::sync::mpsc::Sender<WifiRequest>>>,
}

/// how many ops may wait for the manager — a full queue answers
/// busy at once instead of piling up behind a long join
pub const WIFI_QUEUE: usize = 4;

/// one op + its deadline + its auth context + its reply slot. The
/// deadline is the client's full budget (queue wait included):
/// the manager rejects an op that would start too late — a join
/// must never change credentials under a client that gave up.
/// The auth context (the web endpoint's per-connection verdict:
/// whether the peer is a hotspot-trusted one, and whether it is
/// paired) is re-checked at EXECUTION: a request that queued
/// while the hotspot was up must not run after it ended
pub type WifiRequest = (
    WifiOp,
    std::time::Instant,
    bool,
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
    /// manager never blocks the web runtime. `ap_trusted` is the
    /// sender-side verdict that the peer came in over the hotspot
    /// (the web endpoint's Ingress decision); the manager re-checks
    /// it against the live AP state at execution
    pub async fn request(
        &self,
        op: WifiOp,
        ap_trusted: bool,
        paired: bool,
        timeout: std::time::Duration,
    ) -> Result<serde_json::Value, String> {
        let tx = self.tx.as_ref().ok_or("wifi setup not available")?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = (
            op,
            std::time::Instant::now() + timeout,
            ap_trusted,
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
pub type Children = parking_lot::Mutex<std::collections::HashMap<String, std::process::Child>>;

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
        for (name, mut child) in self.children.lock().drain() {
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
