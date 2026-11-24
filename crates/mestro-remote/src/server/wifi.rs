//! The wifi setup ops (see `protocol::WifiOp`): scan/join/status
//! ride the [`WifiChannel`] to the manager thread (feature
//! `wifi-setup`) — nothing here touches a socket or a process
//! directly, and the reply never echoes the join passphrase. The
//! auth context rides along and the manager re-checks it at
//! execution time (a queued request must not run after the hotspot
//! went down).

use super::http::json;
use crate::{Ingress, WebInfo, WifiChannel};
use mestro_ipc::web::{Response, WifiOp};
use std::time::Duration;

/// the op budgets: scan polls the radio, join also waits for an
/// address — a hung ctrl socket must not hold a client forever
const SCAN: Duration = Duration::from_secs(15);
const JOIN: Duration = Duration::from_secs(45);
const STATUS: Duration = Duration::from_secs(5);

pub(super) async fn handle(
    id: u64,
    action: WifiOp,
    info: &WebInfo,
    wifi: &WifiChannel,
    ingress: Ingress,
    paired: bool,
) -> String {
    // the route already gates this (unpaired only for an
    // AP-trusted peer) — repeat the verdict here so the op itself
    // stays safe, the manager re-checks once more at execution
    if !(paired || (ingress == Ingress::Ap && info.ap())) {
        return json(&Response::err(id, "pairing required"));
    }
    let (op, timeout) = match action {
        WifiOp::Scan => (WifiOp::Scan, SCAN),
        WifiOp::Join { ssid, psk } => (WifiOp::Join { ssid, psk }, JOIN),
        WifiOp::Status => (WifiOp::Status, STATUS),
    };
    match wifi
        .request(op, ingress == Ingress::Ap, paired, timeout)
        .await
    {
        Ok(data) => json(&Response::ok(id, data)),
        Err(e) => json(&Response::err(id, e)),
    }
}
