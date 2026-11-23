//! The socket side of the web endpoint: one tokio thread, one
//! listener, one port. The HTTP head is read and routed by hand —
//! `/ws` upgrades to the JSON protocol, everything else serves
//! the embedded PWA (static assets, SPA fallback, pairing,
//! cover). tungstenite only ever sees upgrade requests (its
//! handshake rejects plain GETs before any callback runs), so
//! non-upgrade traffic is answered here.
//!
//! Domain split:
//! - [`settings`]: the UI-bound ops (settings reads/writes) —
//!   runs on the UI thread through the Bridge
//! - [`ma`]: the wizard's Music Assistant calls (probe/login/
//!   players) — plain HTTP, on the blocking pool of this thread
//! - `ota` (feature `ota`): the system-image upload, streamed into
//!   `mestro-upgrade`
//! - [`assets`]/[`pair`]/[`http`]/[`ws`]/[`auth`]: transport plumbing

mod assets;
mod audio;
mod auth;
mod captive;
mod ctl;
pub(crate) mod diagnostics;
mod http;
mod ma;
#[cfg(feature = "ota")]
mod ota;
mod pair;
mod player;
pub(crate) mod settings;
mod timezone;
mod wifi;
mod ws;

/// the shared command path onto the UI thread (owned by the
/// wiring in main.rs; used by the UI-bound op handlers)
pub(crate) type Dispatch = std::rc::Rc<dyn Fn(&crate::remote::control::Command)>;

use super::{Bridge, Envelope, Ingress, Request, Response, WebInfo, WifiChannel};
use pair::Pairing;
use serde_json::json;
use std::io::Cursor;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;

/// the wire protocol revision `hello` reports — bumped only on
/// a breaking change (additive fields and ops keep it)
pub(crate) const PROTOCOL_VERSION: u32 = 1;

pub async fn serve(
    port: u16,
    info: WebInfo,
    bridge: Bridge,
    pin: String,
    events: super::Events,
    wifi: WifiChannel,
) {
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("web: cannot bind :{port}: {e}");
            return;
        }
    };
    tracing::info!("web: app + ws on http://0.0.0.0:{port}/");
    let pairing = Arc::new(Pairing::new(&pin));
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let info = info.clone();
                let bridge = bridge.clone();
                let pairing = pairing.clone();
                let events = events.clone();
                let wifi = wifi.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(
                        stream,
                        peer,
                        info,
                        bridge,
                        pairing,
                        events,
                        assets::TABLE,
                        wifi,
                    )
                    .await
                    {
                        tracing::debug!("web: {peer}: {e}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("web: accept: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

// the plumbing is wide by nature (stream + facts + the shared
// state every route needs) — same allowance as web::wire
#[allow(clippy::too_many_arguments)]
async fn handle(
    mut stream: TcpStream,
    // the trust boundary lives on the peer address (see
    // [`Ingress::of_peer`]): the connection's source decides the
    // unpaired gate — never the listener that accepted it
    peer: std::net::SocketAddr,
    info: WebInfo,
    bridge: Bridge,
    pairing: Arc<Pairing>,
    events: super::Events,
    assets: &[assets::Asset],
    wifi: WifiChannel,
) -> Result<(), String> {
    // small frames go out at once: Nagle on our side plus the
    // delayed ACKs of iOS stall each tap and push 100–200 ms
    let _ = stream.set_nodelay(true);
    let head = http::read_head(&mut stream).await?;
    // judged after the head arrived: a held-back request must not
    // ride a hotspot that has ended meanwhile
    let ingress = Ingress::of_peer(peer, &info);
    let (path, query) = http::target(&head);
    let method = http::method(&head);
    let authorized = auth::authorized(
        &query,
        http::header(&head, "cookie"),
        pairing.pin(),
        pairing.token(),
    );
    // AP-trusted connection: the OS connectivity probes and any
    // request that names another host bounce into the setup page —
    // this is the captive portal. Any other connection keeps the
    // regular behavior, hotspot or not
    if ingress == Ingress::Ap
        && method == "GET"
        && captive::redirects(
            &path,
            http::header(&head, "host"),
            &info.ap_net.ip.to_string(),
        )
    {
        return captive::redirect(
            &mut stream,
            &format!("http://{}{}/setup/wifi", info.ap_net.ip, info.port_suffix()),
        )
        .await;
    }
    #[cfg(feature = "ota")]
    if path == "/ota" && method == "POST" {
        if !authorized {
            return http::reply(&mut stream, "401 Unauthorized", "pairing required").await;
        }
        return ota::handle(&mut stream, &head, &ota::OtaCmds::from_env(), |phase| {
            bridge.notify(move |ui| show_update_status(ui, &phase));
        })
        .await;
    }
    match path.as_str() {
        "/pair" if method == "POST" => pair::handle(&mut stream, &head, &pairing).await,
        "/pair/status" => pair::status(&mut stream, &head, &pairing).await,
        // AP-trusted peers open the channel unpaired — the peer
        // is inside the hotspot subnet and the hotspot is
        // confirmed up (Ingress::of_peer checked both). dispatch
        // keeps unpaired sockets on the wifi vocabulary
        "/ws" if (authorized || ingress == Ingress::Ap) && http::is_upgrade(&head) => {
            // the head is replayed into the handshake parser
            let ws = accept_async(ws::Prefixed {
                prefix: Cursor::new(head),
                inner: stream,
            })
            .await
            .map_err(|e| format!("handshake: {e}"))?;
            ws::ws_loop(ws, &info, &bridge, &events, &wifi, ingress, authorized).await
        }
        "/ws" => http::reply(&mut stream, "401 Unauthorized", "pairing required").await,
        "/cover" if authorized => {
            // ?blur=1: the pre-blurred, pre-dimmed backdrop — the
            // phone paints it as a plain image, no CSS filter
            let jpeg = if http::query_param(&query, "blur").as_deref() == Some("1") {
                backdrop_jpeg(&events).await
            } else {
                cover_jpeg(&events).await
            };
            match jpeg {
                Some(jpeg) => {
                    // the URL carries the version — cache per artwork
                    http::reply_with_headers(
                        &mut stream,
                        "200 OK",
                        &[],
                        "image/jpeg",
                        "Cache-Control: private, max-age=31536000",
                        &jpeg,
                    )
                    .await
                }
                None => http::reply(&mut stream, "404 Not Found", "no cover").await,
            }
        }
        "/cover" => http::reply(&mut stream, "401 Unauthorized", "pairing required").await,
        // the bug-report download: paired only, never on the hotspot
        // path (the unpaired AP vocabulary is hello + wifi)
        "/diagnostics" if method == "GET" && authorized => {
            diagnostics::serve(&mut stream, &info, &bridge, &pairing, &wifi).await
        }
        "/diagnostics" if method == "GET" => {
            http::reply(&mut stream, "401 Unauthorized", "pairing required").await
        }
        // the settings thumbnails (the schema's preview URLs):
        // auth like /cover, served from the Events cache, rendered
        // once per (name, version) on the UI thread
        "/vu-preview" if authorized => {
            serve_preview(&mut stream, "vu", &query, &bridge, &events).await
        }
        "/vu-preview" => http::reply(&mut stream, "401 Unauthorized", "pairing required").await,
        "/clock-preview" if authorized => {
            serve_preview(&mut stream, "clock", &query, &bridge, &events).await
        }
        "/clock-preview" => http::reply(&mut stream, "401 Unauthorized", "pairing required").await,
        // the PWA: static assets, `/` as the shell, SPA routes
        // fall back to the shell, missing file-like paths 404
        _ => {
            if method != "GET" {
                return http::reply(&mut stream, "404 Not Found", "not found").await;
            }
            // the portal landing: an unpaired AP-trusted client
            // browsing an app route goes to the wifi setup, never
            // the pair screen. Assets keep serving (the setup page
            // needs them), /ws and /pair* are their own routes, and
            // every other connection never grants the ingress —
            // unpaired LAN clients keep the pair screen exactly as
            // before
            if !authorized
                && ingress == Ingress::Ap
                && path != "/setup/wifi"
                && assets::is_app_route(assets, &path)
            {
                return captive::redirect(&mut stream, "/setup/wifi").await;
            }
            match assets::resolve(assets, &path) {
                Some(asset) => serve_asset(&mut stream, asset).await,
                None if !assets::looks_like_file(&path) => {
                    match assets::resolve(assets, "/index.html") {
                        Some(index) => serve_asset(&mut stream, index).await,
                        None => http::reply(&mut stream, "404 Not Found", "not found").await,
                    }
                }
                None => http::reply(&mut stream, "404 Not Found", "not found").await,
            }
        }
    }
}

async fn serve_asset(stream: &mut TcpStream, asset: &assets::Asset) -> Result<(), String> {
    http::reply_with_headers(
        stream,
        "200 OK",
        &[],
        assets::content_type(asset.path),
        &format!("Cache-Control: {}", assets::cache_control(asset.path)),
        asset.bytes,
    )
    .await
}

/// the current cover as JPEG: a fresh one is encoded off the single
/// web thread (milliseconds of CPU)
async fn cover_jpeg(events: &super::Events) -> Option<Vec<u8>> {
    let events = events.clone();
    tokio::task::spawn_blocking(move || events.cover_jpeg())
        .await
        .ok()
        .flatten()
}

/// the pre-blurred backdrop, same treatment
async fn backdrop_jpeg(events: &super::Events) -> Option<Vec<u8>> {
    let events = events.clone();
    tokio::task::spawn_blocking(move || events.backdrop_jpeg())
        .await
        .ok()
        .flatten()
}

/// one settings thumbnail: the slot cache first, then the
/// UI-thread render through the bridge op (PNG bytes as hex)
async fn serve_preview(
    stream: &mut TcpStream,
    kind: &str,
    query: &str,
    bridge: &Bridge,
    events: &super::Events,
) -> Result<(), String> {
    let name_param = if kind == "vu" { "skin" } else { "style" };
    let (Some(name), Some(version)) = (
        http::query_param(query, name_param),
        http::query_param(query, "v"),
    ) else {
        return http::reply(stream, "404 Not Found", "skin and v required").await;
    };
    let png = preview_png(kind, &name, &version, bridge, events).await;
    match png {
        Some(bytes) => {
            // the version changes with every preview-relevant
            // setting; a day bounds the staleness of edits that
            // never touch the schema (custom skin art files)
            http::reply_with_headers(
                stream,
                "200 OK",
                &[],
                "image/png",
                "Cache-Control: private, max-age=86400",
                &bytes,
            )
            .await
        }
        None => http::reply(stream, "404 Not Found", "no preview").await,
    }
}

async fn preview_png(
    kind: &str,
    name: &str,
    version: &str,
    bridge: &Bridge,
    events: &super::Events,
) -> Option<Vec<u8>> {
    let events = events.clone();
    let bridge = bridge.clone();
    let (kind, name, version) = (kind.to_string(), name.to_string(), version.to_string());
    tokio::task::spawn_blocking(move || {
        events.preview_png(&kind, &name, &version, || {
            // process-internal op (the public protocol rejects
            // it): the UI thread renders, PNG bytes come back hex
            let op = format!(r#"{{"op":"preview","kind":"{kind}","name":"{name}"}}"#);
            let reply = bridge.op(&op).ok()?;
            let value: serde_json::Value = serde_json::from_str(&reply).ok()?;
            http::hex_decode(value["data"]["png"].as_str()?).ok()
        })
    })
    .await
    .ok()
    .flatten()
}

/// the device overlay: a failure text clears itself, unless a
/// newer status replaced it meanwhile
#[cfg(feature = "ota")]
fn show_update_status(ui: &crate::Mestro, phase: &ota::OtaPhase) {
    use slint::ComponentHandle as _;
    const FAILURE_VISIBLE: std::time::Duration = std::time::Duration::from_secs(8);
    let text = slint::SharedString::from(phase.status_text());
    ui.set_update_status(text.clone());
    if phase.is_failure() {
        let ui = ui.as_weak();
        slint::Timer::single_shot(FAILURE_VISIBLE, move || {
            if let Some(ui) = ui.upgrade().filter(|ui| ui.get_update_status() == text) {
                ui.set_update_status("".into());
            }
        });
    }
}

/// the `hello` payload; the wizard URL carries the pairing PIN and
/// is only filled in for a paired caller
pub(crate) fn hello_data(info: &WebInfo, paired: bool) -> serde_json::Value {
    let remote_url = if paired {
        info.remote_url()
    } else {
        String::new()
    };
    json!({
        "app": info.app_version,
        "version": info.app_version,
        "protocol": PROTOCOL_VERSION,
        "id": info.id,
        "name": info.name,
        "remote_url": remote_url,
        "ota": cfg!(feature = "ota"),
        "capabilities": {
            "ota": cfg!(feature = "ota"),
            "display": info.display_supported,
            "wifi": cfg!(feature = "wifi-setup"),
            "factory_reset": cfg!(feature = "wifi-setup"),
            "audio_output": audio::capable(),
            "timezone": timezone::capable(),
        },
    })
}

/// one protocol round: the MA and wifi domains run here, everything
/// else goes through the bridge onto the UI thread ([`settings`]).
/// `paired` carries the HTTP route's verdict: unpaired callers only
/// come through on the hotspot, and there only hello + wifi answer —
/// the setup hotspot is the trust boundary, not the device data
pub(crate) async fn dispatch(
    text: &str,
    info: &WebInfo,
    bridge: &Bridge,
    wifi: &WifiChannel,
    ingress: Ingress,
    paired: bool,
) -> String {
    let Ok(env) = serde_json::from_str::<Envelope>(text) else {
        return http::json(&Response::err(0, "bad envelope"));
    };
    let id = env.id;
    match env.request {
        // hello_data withholds the wizard URL (it carries the pairing
        // PIN) from an unpaired socket
        Request::Hello => http::json(&Response::ok(id, hello_data(info, paired))),
        Request::Wifi { action } => wifi::handle(id, action, info, wifi, ingress, paired).await,
        // paired-only, like every device op; the work is blocking
        Request::Audio { action } => {
            if !paired {
                return http::json(&Response::err(id, "pairing required"));
            }
            audio::handle(id, action, bridge).await
        }
        Request::Ma { action } => {
            if !paired {
                return http::json(&Response::err(id, "pairing required"));
            }
            ma::handle(id, action, info, bridge).await
        }
        // settings + ctl + player are UI-bound — ride the bridge
        // onto the UI thread (dispatch there owns the command
        // path)
        other => {
            if !paired {
                return http::json(&Response::err(id, "pairing required"));
            }
            let envelope = Envelope { id, request: other };
            bridge
                .op_async(&http::json(&envelope))
                .await
                .unwrap_or_else(|e| http::json(&Response::err(id, e)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::web::Events;
    use crate::remote::web::WifiOp;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PIN: &str = "ABCD2345";

    /// the default hotspot subnet (`[wifi] ap-address`)
    fn ap_net() -> crate::config::ApNet {
        crate::config::Wifi::default().ap_net()
    }

    /// a hotspot subnet the test client can be trusted from: the
    /// whole loopback range, with `gateway` as the hotspot's own
    /// address — the roundtrip peer is 127.0.0.1
    fn loop_net(gateway: std::net::Ipv4Addr) -> crate::config::ApNet {
        crate::config::ApNet {
            ip: gateway,
            prefix: 8,
            net: std::net::Ipv4Addr::new(127, 0, 0, 0),
            dhcp_low: std::net::Ipv4Addr::new(127, 0, 0, 10),
            dhcp_high: std::net::Ipv4Addr::new(127, 0, 0, 50),
        }
    }

    fn info() -> WebInfo {
        WebInfo {
            port: 8787,
            pin: String::new(),
            app_version: "0.1.0 (abc1234)".into(),
            id: "a8724d0c4171".into(),
            name: "wohnzimmer".into(),
            display_supported: true,
            ap_mode: std::sync::Arc::default(),
            mdns_gate: std::sync::Arc::default(),
            ap_net: ap_net(),
            diagnostics: diagnostics::Sources::image(),
        }
    }

    // an AP-mode info handle: the wifi manager's flag flipped up
    // (the hotspot subnet stays the default — a LAN peer is still
    // paired-only even with the hotspot up)
    fn ap_info() -> WebInfo {
        let info = info();
        info.ap_mode
            .store(true, std::sync::atomic::Ordering::Relaxed);
        info
    }

    // a channel to no manager: wifi ops answer "unavailable"
    fn no_wifi() -> WifiChannel {
        WifiChannel::unavailable()
    }

    // a channel to a fake manager: a canned echo manager — enough
    // to pin the wire shape and the auth matrix without a radio
    fn fake_wifi() -> (
        WifiChannel,
        tokio::sync::mpsc::Receiver<super::super::WifiRequest>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(crate::remote::web::WIFI_QUEUE);
        (WifiChannel::new(std::sync::Arc::new(tx)), rx)
    }

    // a bridge to no UI: these routes never reach the UI thread
    fn detached_bridge() -> Bridge {
        Bridge {
            ui: slint::Weak::default(),
        }
    }

    // one request through `handle` over a real socket → the full
    // response (status line + headers + body). The peer is
    // 127.0.0.1 — the info's subnet decides whether it is trusted
    fn roundtrip(request: &str) -> String {
        roundtrip_with(
            request,
            info(),
            Arc::new(Pairing::new(PIN)),
            Events::new(),
            assets::TABLE,
            no_wifi(),
        )
    }

    fn roundtrip_with(
        request: &str,
        info: WebInfo,
        pairing: Arc<Pairing>,
        events: Events,
        table: &'static [assets::Asset],
        wifi: WifiChannel,
    ) -> String {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tokio::spawn(async move {
                    let (stream, peer) = listener.accept().await.unwrap();
                    let _ = handle(
                        stream,
                        peer,
                        info,
                        detached_bridge(),
                        pairing,
                        events,
                        table,
                        wifi,
                    )
                    .await;
                });
                let mut client = TcpStream::connect(addr).await.unwrap();
                client.write_all(request.as_bytes()).await.unwrap();
                let mut raw = Vec::new();
                client.read_to_end(&mut raw).await.unwrap();
                String::from_utf8_lossy(&raw).to_string()
            })
    }

    fn status_of(request: &str) -> String {
        roundtrip(request)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn headers_of(response: &str) -> String {
        response
            .split("\r\n\r\n")
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn body_of(response: &str) -> &str {
        response
            .split_once("\r\n\r\n")
            .map(|(_, b)| b)
            .unwrap_or_default()
    }

    #[test]
    fn hello_reports_version_and_capabilities() {
        let reply = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(dispatch(
                r#"{"id":1,"op":"hello"}"#,
                &info(),
                &detached_bridge(),
                &no_wifi(),
                Ingress::Lan,
                true,
            ));
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["data"]["version"], "0.1.0 (abc1234)");
        assert_eq!(reply["data"]["protocol"], 1);
        let id = reply["data"]["id"].as_str().unwrap();
        assert!(
            id.len() == 12 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{id}"
        );
        assert!(!reply["data"]["name"].as_str().unwrap().is_empty());
        assert_eq!(reply["data"]["ota"], cfg!(feature = "ota"));
        assert_eq!(reply["data"]["capabilities"]["ota"], cfg!(feature = "ota"));
        assert_eq!(reply["data"]["capabilities"]["display"], true);
        assert_eq!(
            reply["data"]["capabilities"]["wifi"],
            cfg!(feature = "wifi-setup")
        );
        assert_eq!(
            reply["data"]["capabilities"]["factory_reset"],
            cfg!(feature = "wifi-setup")
        );
    }

    #[test]
    fn the_root_serves_the_shell_as_html() {
        let response = roundtrip("GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            headers_of(&response).contains("Content-Type: text/html"),
            "{response}"
        );
        assert!(
            headers_of(&response).contains("Cache-Control: no-cache"),
            "{response}"
        );
        assert!(!body_of(&response).is_empty());
    }

    #[test]
    fn spa_routes_fall_back_to_the_shell() {
        for path in ["/system", "/setup/player"] {
            let response = roundtrip(&format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"));
            assert!(response.starts_with("HTTP/1.1 200"), "{path}: {response}");
            assert!(headers_of(&response).contains("text/html"), "{path}");
        }
    }

    #[test]
    fn the_blurred_backdrop_cover_jpeg_needs_auth_and_a_cover() {
        let token = auth::pair_token(PIN);
        // no cover yet: 404, with and without blur
        for query in ["", "?blur=1"] {
            let request = format!(
                "GET /cover{query} HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={token}\r\n\r\n"
            );
            assert!(roundtrip(&request).starts_with("HTTP/1.1 404"), "{query}");
        }
        // unauthorized
        let response = roundtrip("GET /cover?blur=1 HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");

        // a cover + its blur in the slot: both variants serve JPEG
        let events = Events::new();
        events.set_cover(
            1,
            crate::player::cover::CoverData {
                rgba: vec![255, 0, 0, 255],
                width: 1,
                height: 1,
            },
            Some(crate::player::cover::CoverData {
                rgba: vec![0, 0, 255, 255],
                width: 1,
                height: 1,
            }),
        );
        for (query, check) in [("", "cover"), ("?blur=1", "backdrop")] {
            let request = format!(
                "GET /cover{query} HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={token}\r\n\r\n"
            );
            let response = roundtrip_with(
                &request,
                info(),
                Arc::new(Pairing::new(PIN)),
                events.clone(),
                assets::TABLE,
                no_wifi(),
            );
            assert!(response.starts_with("HTTP/1.1 200"), "{check}: {response}");
            assert!(headers_of(&response).contains("Cache-Control: private, max-age=31536000"));
            // the roundtrip is a lossy UTF-8 string, so the JPEG
            // magic is asserted in push.rs — here: a body arrived
            // as image/jpeg
            assert!(headers_of(&response).contains("Content-Type: image/jpeg"));
            assert!(!body_of(&response).is_empty(), "{check}: no body");
        }
    }

    #[test]
    fn missing_files_404_but_routes_do_not() {
        let response = roundtrip("GET /assets/app-missing.js HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
        let response = roundtrip("GET /icons/icon-999.png HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    }

    #[test]
    fn non_get_static_requests_404() {
        let response = roundtrip("POST /system HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    }

    fn pair_post(pin: &str) -> String {
        let body = format!("{{\"pin\":\"{pin}\"}}");
        format!(
            "POST /pair HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn a_correct_pin_pairs_and_sets_the_cookie() {
        let response = roundtrip(&pair_post(PIN));
        assert!(response.starts_with("HTTP/1.1 204"), "{response}");
        assert!(
            headers_of(&response).contains("Set-Cookie: mestro_pair="),
            "{response}"
        );
        assert!(headers_of(&response).contains("HttpOnly"), "{response}");
        assert!(
            headers_of(&response).contains("SameSite=Strict"),
            "{response}"
        );
        assert!(
            headers_of(&response).contains("Max-Age=31536000"),
            "{response}"
        );
    }

    #[test]
    fn a_wrong_pin_is_401() {
        let response = roundtrip(&pair_post("WRONG1234"));
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    }

    #[test]
    fn a_malformed_pairing_body_is_400() {
        let response = roundtrip("POST /pair HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\n\r\nxxx");
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    }

    #[test]
    fn pair_status_reports_the_cookie() {
        // without a cookie: unpaired
        let response = roundtrip("GET /pair/status HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert_eq!(body_of(&response), "{\"paired\":false}");
        // with the right cookie: paired
        let token = auth::pair_token(PIN);
        let request =
            format!("GET /pair/status HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={token}\r\n\r\n");
        let response = roundtrip(&request);
        assert_eq!(body_of(&response), "{\"paired\":true}");
        // a cookie minted from another pin must not pair
        let other = auth::pair_token("MMMMMMMM");
        let request =
            format!("GET /pair/status HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={other}\r\n\r\n");
        let response = roundtrip(&request);
        assert_eq!(body_of(&response), "{\"paired\":false}");
    }

    #[test]
    fn five_wrong_pins_close_the_gate_for_30s() {
        let pairing = Arc::new(Pairing::new(PIN));
        let bad = pair_post("WRONG1234");
        for i in 0..4 {
            assert!(
                roundtrip_with(
                    &bad,
                    info(),
                    pairing.clone(),
                    Events::new(),
                    assets::TABLE,
                    no_wifi()
                )
                .starts_with("HTTP/1.1 401"),
                "attempt {i}"
            );
        }
        let fifth = roundtrip_with(
            &bad,
            info(),
            pairing.clone(),
            Events::new(),
            assets::TABLE,
            no_wifi(),
        );
        assert!(fifth.starts_with("HTTP/1.1 429"), "{fifth}");
        // and the right pin is locked out too
        let response = roundtrip_with(
            &pair_post(PIN),
            info(),
            pairing,
            Events::new(),
            assets::TABLE,
            no_wifi(),
        );
        assert!(response.starts_with("HTTP/1.1 429"), "{response}");
    }

    #[test]
    fn a_cookie_opens_the_ws_and_the_first_frame_is_the_snapshot() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let events = Events::new();
                events.publish(r#"{"event":"state","data":{"connected":false}}"#.into());
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                let pairing = Arc::new(Pairing::new(PIN));
                tokio::spawn(async move {
                    let (stream, peer) = listener.accept().await.unwrap();
                    let _ = handle(
                        stream,
                        peer,
                        info(),
                        detached_bridge(),
                        pairing,
                        events,
                        assets::TABLE,
                        no_wifi(),
                    )
                    .await;
                });
                let token = auth::pair_token(PIN);
                use tokio_tungstenite::tungstenite::client::IntoClientRequest;
                let mut request = format!("ws://{addr}/ws").into_client_request().unwrap();
                request
                    .headers_mut()
                    .insert("cookie", format!("mestro_pair={token}").parse().unwrap());
                let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
                let first = ws.next().await.unwrap().unwrap();
                assert_eq!(
                    first.into_text().unwrap(),
                    "{\"event\":\"state\",\"data\":{\"connected\":false}}"
                );
                // and the request/response path still answers over
                // the same socket
                ws.send("{\"id\":1,\"op\":\"hello\"}".into()).await.unwrap();
                let second = ws.next().await.unwrap().unwrap().into_text().unwrap();
                assert!(
                    second.contains("\"app\":\"0.1.0"),
                    "unexpected frame: {second}"
                );
            });
    }

    #[test]
    fn a_socket_opened_right_after_startup_gets_a_state_frame_first() {
        // startup order: the initial snapshot is published before the
        // listener accepts — no player update has happened yet
        let events = Events::new();
        let ma_cfg = std::sync::Arc::new(std::sync::RwLock::new(crate::providers::ma::Config {
            ws_url: String::new(),
            token: String::new(),
            player_name: "wohnzimmer".into(),
            own_player_id: None,
        }));
        crate::remote::web::Push::new(events.clone(), ma_cfg, "wohnzimmer").publish_snapshot();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                let pairing = Arc::new(Pairing::new(PIN));
                tokio::spawn(async move {
                    let (stream, peer) = listener.accept().await.unwrap();
                    let _ = handle(
                        stream,
                        peer,
                        info(),
                        detached_bridge(),
                        pairing,
                        events,
                        assets::TABLE,
                        no_wifi(),
                    )
                    .await;
                });
                use tokio_tungstenite::tungstenite::client::IntoClientRequest;
                let mut request = format!("ws://{addr}/ws").into_client_request().unwrap();
                request.headers_mut().insert(
                    "cookie",
                    format!("mestro_pair={}", auth::pair_token(PIN))
                        .parse()
                        .unwrap(),
                );
                let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
                // a hello sent at once must not overtake the snapshot
                ws.send("{\"id\":1,\"op\":\"hello\"}".into()).await.unwrap();
                let first: serde_json::Value =
                    serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap())
                        .unwrap();
                assert_eq!(first["event"], "state", "{first}");
                assert_eq!(first["data"]["player"]["name"], "wohnzimmer");
            });
    }

    #[test]
    #[cfg(feature = "ota")]
    fn ota_without_the_pairing_pin_is_401() {
        let head = "POST /ota HTTP/1.1\r\nContent-Length: 5242880\r\n\r\n";
        assert!(status_of(head).contains("401"));
        let head = "POST /ota?pair=WRONG234 HTTP/1.1\r\nContent-Length: 5242880\r\n\r\n";
        assert!(status_of(head).contains("401"));
    }

    #[test]
    #[cfg(feature = "ota")]
    fn ota_only_answers_post() {
        // a GET never reaches the upload handler — it falls through
        // to the SPA route fallback (the shell, never the upload)
        let head = format!("GET /ota?pair={PIN} HTTP/1.1\r\n\r\n");
        let response = roundtrip(&head);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(headers_of(&response).contains("text/html"), "{response}");
    }

    #[cfg(not(feature = "ota"))]
    #[test]
    fn ota_is_404_without_the_feature() {
        let head = format!("POST /ota?pair={PIN} HTTP/1.1\r\nContent-Length: 5242880\r\n\r\n");
        assert!(status_of(&head).contains("404"));
    }

    // ── the preview thumbnails ──

    /// a request through `handle` with a pre-populated preview
    /// slot (the cache path — the bridge is never touched, the
    /// detached bridge would wait for a UI thread that is absent)
    fn preview_roundtrip(request: &str, events: &Events) -> String {
        roundtrip_with(
            request,
            info(),
            Arc::new(Pairing::new(PIN)),
            events.clone(),
            assets::TABLE,
            no_wifi(),
        )
    }

    #[test]
    fn previews_need_the_pairing_cookie() {
        let events = Events::new();
        events
            .preview_png("vu", "round", "v1", || Some(vec![1]))
            .unwrap();
        let ok = preview_roundtrip(
            "GET /vu-preview?skin=round&v=v1 HTTP/1.1\r\nHost: x\r\n\r\n",
            &events,
        );
        assert!(ok.starts_with("HTTP/1.1 401"), "{ok}");
        let ok = preview_roundtrip(
            "GET /clock-preview?style=digital&v=v1 HTTP/1.1\r\nHost: x\r\n\r\n",
            &events,
        );
        assert!(ok.starts_with("HTTP/1.1 401"), "{ok}");
    }

    #[test]
    fn a_cached_preview_serves_png_bytes() {
        let events = Events::new();
        // ASCII-safe bytes: the test helper reads the response as
        // lossy UTF-8 (the real PNG magic would be mangled there)
        let png = b"PNGDATA-1".to_vec();
        events
            .preview_png("vu", "round", "v1", || Some(png.clone()))
            .unwrap();
        let token = auth::pair_token(PIN);
        let request = format!(
            "GET /vu-preview?skin=round&v=v1 HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={token}\r\n\r\n"
        );
        let response = preview_roundtrip(&request, &events);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            headers_of(&response).contains("Content-Type: image/png"),
            "{response}"
        );
        assert!(
            headers_of(&response).contains("max-age=86400"),
            "{response}"
        );
        assert_eq!(body_of(&response).as_bytes(), png);
        // a cached clock preview rides the same path
        events
            .preview_png("clock", "digital", "v2", || Some(png.clone()))
            .unwrap();
        let request = format!(
            "GET /clock-preview?style=digital&v=v2 HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={token}\r\n\r\n"
        );
        assert!(preview_roundtrip(&request, &events).starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn previews_without_params_404() {
        let events = Events::new();
        let token = auth::pair_token(PIN);
        for path in ["/vu-preview", "/clock-preview", "/vu-preview?skin=round"] {
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={token}\r\n\r\n");
            let response = preview_roundtrip(&request, &events);
            assert!(response.starts_with("HTTP/1.1 404"), "{path}: {response}");
        }
    }

    // ── the captive portal (AP-trusted connections only) ──

    #[test]
    fn the_captive_probe_paths_redirect_only_ap_trusted_peers() {
        for path in [
            "/generate_204",
            "/gen_204",
            "/hotspot-detect.html",
            "/library/test/success.html",
            "/connecttest.txt",
            "/ncsi.txt",
        ] {
            // STA mode: the regular behavior (file-like paths 404,
            // routes keep falling back to the shell)
            let response = roundtrip(&format!("GET {path} HTTP/1.1\r\nHost: 10.42.0.1\r\n\r\n"));
            assert!(!response.starts_with("HTTP/1.1 302"), "{path}: {response}");
            // AP mode, but a LAN peer: still the regular behavior
            // — the portal follows the peer, never the flag alone
            let response = roundtrip_ap(&format!("GET {path} HTTP/1.1\r\nHost: 10.42.0.1\r\n\r\n"));
            assert!(!response.starts_with("HTTP/1.1 302"), "{path}: {response}");
            // an AP-trusted peer: the portal takes them (the
            // redirect names the configured web port)
            let response =
                roundtrip_hotspot(&format!("GET {path} HTTP/1.1\r\nHost: 10.42.0.1\r\n\r\n"));
            assert!(response.starts_with("HTTP/1.1 302"), "{path}: {response}");
            assert!(
                headers_of(&response).contains("Location: http://127.0.0.2:8787/setup/wifi"),
                "{path}: {response}"
            );
        }
    }

    #[test]
    fn a_foreign_host_redirects_only_ap_trusted_peers() {
        let probe = "GET / HTTP/1.1\r\nHost: captive.apple.com\r\n\r\n";
        // STA mode: a foreign Host header changes nothing
        let response = roundtrip(probe);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        // AP up but a LAN peer: still nothing
        let response = roundtrip_ap(probe);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        // an AP-trusted peer: anything that did not dial the kiosk
        // by its own address is a captive-portal probe
        let response = roundtrip_hotspot(probe);
        assert!(response.starts_with("HTTP/1.1 302"), "{response}");
        // own names — with or without a port — still serve (the
        // trusted subnet's gateway is 127.0.0.2 here)
        for host in [
            "127.0.0.2",
            "127.0.0.2:8787",
            "mestro.setup",
            "localhost:8787",
            "[::1]:8787",
        ] {
            let request = format!("GET /setup/wifi HTTP/1.1\r\nHost: {host}\r\n\r\n");
            let response = roundtrip_hotspot(&request);
            assert!(response.starts_with("HTTP/1.1 200"), "{host}: {response}");
        }
    }

    // AP mode up, but the peer is not inside the hotspot subnet —
    // every unpaired request stays behind the pairing gate
    fn roundtrip_ap(request: &str) -> String {
        roundtrip_with(
            request,
            ap_info(),
            Arc::new(Pairing::new(PIN)),
            Events::new(),
            assets::TABLE,
            no_wifi(),
        )
    }

    // an AP-trusted roundtrip: the peer (127.0.0.1) lies inside
    // the info's hotspot subnet and is not the gateway
    fn roundtrip_hotspot(request: &str) -> String {
        roundtrip_with(
            request,
            hotspot_info(),
            Arc::new(Pairing::new(PIN)),
            Events::new(),
            assets::TABLE,
            no_wifi(),
        )
    }

    #[test]
    fn unpaired_hotspot_app_routes_land_on_the_wifi_setup() {
        // the portal landing: any app route an unpaired hotspot
        // client opens 302s to /setup/wifi — never the pair screen
        for path in ["/", "/settings", "/settings/info", "/setup/player"] {
            let request = format!("GET {path} HTTP/1.1\r\nHost: mestro.setup\r\n\r\n");
            let response = roundtrip_hotspot(&request);
            assert!(response.starts_with("HTTP/1.1 302"), "{path}: {response}");
            assert!(
                headers_of(&response).contains("Location: /setup/wifi"),
                "{path}: {response}"
            );
        }
        // the setup route itself serves the shell
        let request = "GET /setup/wifi HTTP/1.1\r\nHost: mestro.setup\r\n\r\n";
        let response = roundtrip_hotspot(request);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(headers_of(&response).contains("text/html"), "{response}");
        // assets keep serving — the setup page needs them
        let index = assets::TABLE
            .iter()
            .find(|a| a.path == "/index.html")
            .unwrap();
        let request = format!("GET {} HTTP/1.1\r\nHost: mestro.setup\r\n\r\n", index.path);
        let response = roundtrip_hotspot(&request);
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "an asset must serve: {response}"
        );
        // a paired hotspot client keeps the regular routes
        let token = auth::pair_token(PIN);
        let request =
            format!("GET / HTTP/1.1\r\nHost: mestro.setup\r\nCookie: mestro_pair={token}\r\n\r\n");
        let response = roundtrip_hotspot(&request);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        // the LAN listener never redirects — an unpaired LAN
        // client keeps the pair screen, even with AP mode up
        let response = roundtrip_ap("GET / HTTP/1.1\r\nHost: 10.42.0.1\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        // a non-GET request to an app route stays a 404
        let response = roundtrip_hotspot(
            "POST /settings HTTP/1.1\r\nHost: mestro.setup\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    }

    // ── the AP-mode trust boundary ──

    /// an AP-mode info whose hotspot subnet holds the test peer
    /// (127.0.0.1) without being the gateway itself — the peer
    /// shape a hotspot phone has. Carries a REAL pin: it must
    /// never reach an unpaired socket
    fn hotspot_info() -> WebInfo {
        let mut info = ap_info();
        info.ap_net = loop_net(std::net::Ipv4Addr::new(127, 0, 0, 2));
        info.pin = PIN.into();
        info
    }

    #[test]
    fn hello_hides_the_pairing_pin_from_unpaired_sockets() {
        // a REAL pin (an empty one could never leak): an unpaired
        // AP-mode client must not learn it from hello — it could
        // pair with it and get everything, factory reset included
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let mut info = info();
                info.pin = PIN.into();
                let unpaired = dispatch(
                    r#"{"id":1,"op":"hello"}"#,
                    &info,
                    &detached_bridge(),
                    &no_wifi(),
                    Ingress::Ap,
                    false,
                )
                .await;
                assert!(!unpaired.contains(PIN), "the pin leaked: {unpaired}");
                assert!(
                    !unpaired.contains("#pair="),
                    "the pin fragment leaked: {unpaired}"
                );
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&unpaired).unwrap()["data"]
                        ["remote_url"],
                    serde_json::json!(""),
                    "the wizard URL carries the pin: {unpaired}"
                );
                // the paired wizard gets its URL — that is the point
                // of pairing
                let paired = dispatch(
                    r#"{"id":2,"op":"hello"}"#,
                    &info,
                    &detached_bridge(),
                    &no_wifi(),
                    Ingress::Ap,
                    true,
                )
                .await;
                assert!(paired.contains(&format!("#pair={PIN}")), "{paired}");
            });
    }

    #[test]
    fn unpaired_websockets_open_only_for_ap_trusted_peers() {
        // STA mode: no pairing, no channel
        let response = roundtrip("GET /ws HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        // AP mode up, but the peer is a LAN address (outside the
        // hotspot subnet): still no channel
        let response = roundtrip_ap("GET /ws HTTP/1.1\r\nHost: 10.42.0.1\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        // the peer lies inside the subnet — but IS the gateway
        // itself: still no channel (a local connection must never
        // ride the gate)
        let mut gateway_info = hotspot_info();
        gateway_info.ap_net = loop_net(std::net::Ipv4Addr::new(127, 0, 0, 1));
        let response = roundtrip_with(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            gateway_info,
            Arc::new(Pairing::new(PIN)),
            Events::new(),
            assets::TABLE,
            no_wifi(),
        );
        assert!(
            response.starts_with("HTTP/1.1 401"),
            "the gateway itself must not ride the AP gate: {response}"
        );
        // the peer would be trusted, but the hotspot is down (the
        // confirmed-up flag): still no channel
        let ap_over_info = hotspot_info();
        ap_over_info
            .ap_mode
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let response = roundtrip_with(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.2\r\n\r\n",
            ap_over_info,
            Arc::new(Pairing::new(PIN)),
            Events::new(),
            assets::TABLE,
            no_wifi(),
        );
        assert!(
            response.starts_with("HTTP/1.1 401"),
            "the flag down must close the gate for a subnet peer: {response}"
        );
    }

    #[test]
    fn unpaired_hotspot_sockets_get_no_pushes_and_close_with_the_ap() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (wifi, mut requests) = fake_wifi();
                tokio::spawn(async move {
                    while let Some((op, _deadline, _ingress, _paired, reply)) = requests.recv().await {
                        // echo the action only — the psk never travels back
                        let action = serde_json::to_value(&op).unwrap()["action"].clone();
                        let _ = reply.send(Ok(json!({ "action": action })));
                    }
                });
                // player state exists — an unpaired socket must not
                // receive a single frame of it
                let events = Events::new();
                events.publish(
                    r#"{"event":"state","data":{"connected":true,"player":{"name":"Secret Speaker","volume":42}}}"#.into(),
                );
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                let pairing = Arc::new(Pairing::new(PIN));
                let info = hotspot_info();
                let flag = info.ap_mode.clone();
                tokio::spawn(async move {
                    let (stream, peer) = listener.accept().await.unwrap();
                    // a hotspot phone's connection: the peer lies in
                    // the subnet and the hotspot is up — the trust
                    // that grants the unpaired setup vocabulary
                    let _ = handle(
                        stream, peer, info, detached_bridge(), pairing, events, assets::TABLE, wifi,
                    )
                    .await;
                });
                use tokio_tungstenite::tungstenite::client::IntoClientRequest;
                let request = format!("ws://{addr}/ws").into_client_request().unwrap();
                let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
                // the FIRST frame is the hello reply — no snapshot
                // ever arrives, no push either
                ws.send("{\"id\":1,\"op\":\"hello\"}".into()).await.unwrap();
                let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
                assert!(reply.contains("\"id\":1,\"ok\":true"), "{reply}");
                assert!(!reply.contains("Secret Speaker"), "no player data on an unpaired socket");
                assert!(!reply.contains(PIN), "the pairing pin never travels unpaired: {reply}");
                assert_eq!(serde_json::from_str::<serde_json::Value>(&reply).unwrap()["data"]["remote_url"], "", "no wizard URL (it carries the PIN) unpaired");
                // the wifi vocabulary answers
                ws.send("{\"id\":2,\"op\":\"wifi\",\"action\":\"status\"}"
                    .into())
                    .await
                    .unwrap();
                let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
                assert!(reply.contains("\"action\":\"status\""), "{reply}");
                assert!(!reply.contains("Secret Speaker"), "{reply}");
                // everything else stays behind the pairing gate
                ws.send("{\"id\":3,\"op\":\"settings\",\"action\":\"state\"}"
                    .into())
                    .await
                    .unwrap();
                let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
                assert!(reply.contains("pairing required"), "{reply}");
                ws.send("{\"id\":4,\"op\":\"ma\",\"action\":\"probe\",\"url\":\"http://ma\"}"
                    .into())
                    .await
                    .unwrap();
                let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
                assert!(reply.contains("pairing required"), "{reply}");
                // AP mode ends: the trust basis is gone, the socket
                // closes within the watch window
                flag.store(false, std::sync::atomic::Ordering::Relaxed);
                let closed = tokio::time::timeout(std::time::Duration::from_secs(4), ws.next()).await;
                assert!(
                    matches!(closed, Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))))),
                    "the unpaired socket must close when AP mode ends"
                );
            });
    }

    #[test]
    fn wifi_ops_refuse_unpaired_callers_in_sta_mode() {
        // defense in depth: the /ws route already gates unpaired
        // STA clients out — the op handler repeats the verdict
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (wifi, requests) = fake_wifi();
                drop(requests); // no manager serves: the refusal below must not depend on it
                let reply = dispatch(
                    r#"{"id":9,"op":"wifi","action":"scan"}"#,
                    &info(),
                    &detached_bridge(),
                    &wifi,
                    Ingress::Ap,
                    false,
                )
                .await;
                let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(value["ok"], serde_json::json!(false));
                assert!(value["error"]
                    .as_str()
                    .unwrap()
                    .contains("pairing required"));
            });
    }

    #[test]
    fn wifi_ops_reach_the_manager_and_report_an_absent_one() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                // a live manager echoes the op back as data
                let (wifi, mut requests) = fake_wifi();
                tokio::spawn(async move {
                    while let Some((op, _deadline, _ingress, _paired, reply)) =
                        requests.recv().await
                    {
                        // echo the action only — the psk never travels back
                        let action = serde_json::to_value(&op).unwrap()["action"].clone();
                        let _ = reply.send(Ok(json!({ "action": action })));
                    }
                });
                for (op, action) in [
                    (r#"{"id":11,"op":"wifi","action":"scan"}"#, "scan"),
                    (r#"{"id":12,"op":"wifi","action":"status"}"#, "status"),
                ] {
                    let reply =
                        dispatch(op, &info(), &detached_bridge(), &wifi, Ingress::Lan, true).await;
                    let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
                    assert_eq!(value["ok"], serde_json::json!(true), "{op}");
                    assert_eq!(value["data"]["action"], action);
                }
                // a join carries the psk only to the manager — the
                // reply never echoes it
                let reply = dispatch(
                    r#"{"id":13,"op":"wifi","action":"join","ssid":"Home","psk":"secret"}"#,
                    &info(),
                    &detached_bridge(),
                    &wifi,
                    Ingress::Lan,
                    true,
                )
                .await;
                let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(value["data"]["action"], "join");
                assert!(
                    !reply.contains("secret"),
                    "the psk never leaves the device: {reply}"
                );
                // a build without the manager reports it instead of
                // hanging
                let reply = dispatch(
                    r#"{"id":14,"op":"wifi","action":"scan"}"#,
                    &info(),
                    &detached_bridge(),
                    &no_wifi(),
                    Ingress::Lan,
                    true,
                )
                .await;
                let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(value["ok"], serde_json::json!(false));
                assert!(value["error"].as_str().unwrap().contains("not available"));
            });
    }

    #[test]
    fn a_full_op_queue_answers_busy_at_once() {
        // the queue is bounded: while the manager is stuck behind a
        // long join, the request beyond the capacity fails fast
        // instead of piling up (each queued request here times out
        // unanswered — no manager drains the channel)
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (tx, _undrained) = tokio::sync::mpsc::channel(crate::remote::web::WIFI_QUEUE);
                let wifi = WifiChannel::new(std::sync::Arc::new(tx));
                for _ in 0..crate::remote::web::WIFI_QUEUE {
                    let out = wifi
                        .request(
                            WifiOp::Scan,
                            true,
                            false,
                            std::time::Duration::from_millis(30),
                        )
                        .await;
                    assert!(out.unwrap_err().contains("timed out"));
                }
                // the queue is full: busy at once, never queued
                let out = wifi
                    .request(
                        WifiOp::Join {
                            ssid: "Home".into(),
                            psk: Some("password12".into()),
                        },
                        true,
                        false,
                        std::time::Duration::from_secs(45),
                    )
                    .await;
                assert!(out.unwrap_err().contains("busy"));
            });
    }

    // ── the audio ops: paired-only, capability-gated ──

    fn dispatch_blocking(text: &str, info: &WebInfo, ingress: Ingress, paired: bool) -> String {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(dispatch(
                text,
                info,
                &detached_bridge(),
                &no_wifi(),
                ingress,
                paired,
            ))
    }

    #[test]
    fn audio_ops_refuse_unpaired_callers_even_on_the_hotspot() {
        let info = hotspot_info();
        for op in [
            r#"{"id":1,"op":"audio","action":"outputs"}"#,
            r#"{"id":2,"op":"audio","action":"set","output":"hw:CARD=S3"}"#,
        ] {
            for ingress in [Ingress::Ap, Ingress::Lan] {
                let reply = dispatch_blocking(op, &info, ingress, false);
                let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(value["ok"], false, "{op}: {reply}");
                assert!(
                    value["error"]
                        .as_str()
                        .unwrap()
                        .contains("pairing required"),
                    "{op}: {reply}"
                );
            }
        }
    }

    #[cfg(not(feature = "audio-output"))]
    #[test]
    fn audio_ops_answer_not_available_without_the_feature() {
        let info = info();
        for op in [
            r#"{"id":1,"op":"audio","action":"outputs"}"#,
            r#"{"id":2,"op":"audio","action":"set","output":"hw:CARD=S3"}"#,
        ] {
            let reply = dispatch_blocking(op, &info, Ingress::Lan, true);
            assert!(reply.contains("not available"), "{reply}");
        }
    }

    // ── the timezone ops: paired-only (they ride the UI bridge, the
    // pairing check comes first), capability-gated ──

    #[test]
    fn timezone_ops_refuse_unpaired_callers_even_on_the_hotspot() {
        let info = hotspot_info();
        for op in [
            r#"{"id":1,"op":"timezone","action":"zones"}"#,
            r#"{"id":2,"op":"timezone","action":"set","zone":"Europe/Berlin"}"#,
            r#"{"id":3,"op":"timezone","action":"adopt","zone":"Europe/Berlin"}"#,
        ] {
            for ingress in [Ingress::Ap, Ingress::Lan] {
                let reply = dispatch_blocking(op, &info, ingress, false);
                let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(value["ok"], false, "{op}: {reply}");
                assert!(
                    value["error"]
                        .as_str()
                        .unwrap()
                        .contains("pairing required"),
                    "{op}: {reply}"
                );
            }
        }
    }

    // ── GET /diagnostics: paired only ──

    #[test]
    fn diagnostics_need_the_pairing_cookie_everywhere() {
        // no cookie: 401 — STA, hotspot-up-LAN peer and the AP-trusted
        // hotspot peer alike (the unpaired AP vocabulary is hello +
        // wifi, never the report)
        let request = "GET /diagnostics HTTP/1.1\r\nHost: x\r\n\r\n";
        // the hotspot peer dials the gateway by its own address (any
        // other host would be bounced into the portal page first)
        let gateway = "GET /diagnostics HTTP/1.1\r\nHost: 127.0.0.2\r\n\r\n";
        for response in [
            roundtrip(request),
            roundtrip_ap(request),
            roundtrip_hotspot(gateway),
        ] {
            assert!(response.starts_with("HTTP/1.1 401"), "{response}");
            assert!(!response.contains("mestro-diagnostics"), "{response}");
        }
        // a wrong cookie too
        let response =
            roundtrip("GET /diagnostics HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair=nope\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        // only GET is a route; anything else falls to the app (404 for POST)
        let response = roundtrip("POST /diagnostics HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    }

    #[test]
    fn a_paired_download_is_a_json_attachment_without_any_known_secret() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("mestro.log");
        // the log carries the pairing PIN and a bearer token the way a
        // real log would
        std::fs::write(
            &log,
            format!(
                "INFO web: wizard url: http://192.168.1.50/#pair={PIN}\nINFO ma: connected\nWARN auth Bearer {PIN}-leftover\n"
            ),
        )
        .unwrap();
        let mut info = info();
        info.pin = PIN.into();
        info.diagnostics = diagnostics::Sources {
            logs: vec![log.display().to_string()],
            log_lines: 200,
            files: vec![],
            status_command: String::new(),
        };
        let pairing = Arc::new(Pairing::new(PIN));
        let token = auth::pair_token(PIN);
        let request =
            format!("GET /diagnostics HTTP/1.1\r\nHost: x\r\nCookie: mestro_pair={token}\r\n\r\n");
        let response = roundtrip_with(
            &request,
            info,
            pairing,
            Events::new(),
            assets::TABLE,
            no_wifi(),
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let headers = headers_of(&response);
        assert!(
            headers.contains("Content-Type: application/json"),
            "{headers}"
        );
        assert!(
            headers.contains("Content-Disposition: attachment; filename=\"mestro-diagnostics-a8724d0c4171.json\""),
            "{headers}"
        );
        assert!(headers.contains("Cache-Control: no-store"), "{headers}");
        let body = body_of(&response);
        for secret in [PIN, token.as_str()] {
            assert!(!body.contains(secret), "{secret} leaked:\n{body}");
        }
        let report: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(report["report"], "mestro-diagnostics");
        assert_eq!(report["version"], "0.1.0 (abc1234)");
        assert_eq!(
            report["hello"]["remote_url"], "",
            "no pairing URL in the report"
        );
        // the detached UI and the missing wifi manager are reported,
        // the download still works
        assert!(report["state"]["error"].is_string(), "{report}");
        assert!(report["wifi"]["error"].is_string(), "{report}");
        assert!(report["logs"]
            .as_object()
            .unwrap()
            .values()
            .any(|lines| lines
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap().contains("connected"))));
    }
}
