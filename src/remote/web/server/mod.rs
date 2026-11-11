//! The socket side of `src/web/`: one tokio thread, one listener,
//! one port. The HTTP head is read and routed by hand — `/ws`
//! upgrades to the JSON protocol (auth via the pairing PIN),
//! everything else gets the wizard SPA. tungstenite only ever sees
//! upgrade requests (its handshake rejects plain GETs before any
//! callback runs), so non-upgrade traffic is answered here.
//!
//! Domain split:
//! - [`settings`]: the UI-bound ops (settings reads/writes) —
//!   runs on the UI thread through the Bridge
//! - [`ma`]: the wizard's Music Assistant calls (probe/login/
//!   players) — plain HTTP, on the blocking pool of this thread
//! - `ota` (feature `ota`): the system-image upload, streamed into
//!   `mestro-upgrade`
//! - [`http`]/[`ws`]/[`auth`]: transport plumbing

mod auth;
mod ctl;
mod http;
mod ma;
#[cfg(feature = "ota")]
mod ota;
pub(crate) mod settings;
mod ws;

/// the shared command path onto the UI thread (owned by the
/// wiring in main.rs; used by the UI-bound op handlers)
pub(crate) type Dispatch = std::rc::Rc<dyn Fn(&crate::remote::control::Command)>;

use super::{Bridge, Envelope, Request, Response, WebInfo};
use serde_json::json;
use std::io::Cursor;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;

pub async fn serve(port: u16, info: WebInfo, bridge: Bridge, pin: String) {
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("web: cannot bind :{port}: {e}");
            return;
        }
    };
    tracing::info!("web: wizard + ws on http://0.0.0.0:{port}/");
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let info = info.clone();
                let bridge = bridge.clone();
                let pin = pin.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, info, bridge, pin).await {
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

async fn handle(
    mut stream: TcpStream,
    info: WebInfo,
    bridge: Bridge,
    pin: String,
) -> Result<(), String> {
    let head = http::read_head(&mut stream).await?;
    let (path, query) = http::target(&head);
    #[cfg(feature = "ota")]
    if path == "/ota" && http::method(&head) == "POST" {
        if !auth::authorized(&query, &pin) {
            return http::reply(&mut stream, "401 Unauthorized", "pairing required").await;
        }
        return ota::handle(&mut stream, &head, &ota::OtaCmds::from_env(), |phase| {
            bridge.notify(move |ui| show_update_status(ui, &phase));
        })
        .await;
    }
    match path.as_str() {
        "/ws" if auth::authorized(&query, &pin) && http::is_upgrade(&head) => {
            // the head is replayed into the handshake parser
            let ws = accept_async(ws::Prefixed {
                prefix: Cursor::new(head),
                inner: stream,
            })
            .await
            .map_err(|e| format!("handshake: {e}"))?;
            ws::ws_loop(ws, &info, &bridge).await
        }
        "/ws" => http::reply(&mut stream, "401 Unauthorized", "pairing required").await,
        "/" | "/index.html" => http::reply_html(&mut stream, "200 OK", http::SPA).await,
        _ => http::reply(&mut stream, "404 Not Found", "not found").await,
    }
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

/// one protocol round: the MA domain runs here, everything else
/// goes through the bridge onto the UI thread ([`settings`])
pub(crate) async fn dispatch(text: &str, info: &WebInfo, bridge: &Bridge) -> String {
    let Ok(env) = serde_json::from_str::<Envelope>(text) else {
        return http::json(&Response::err(0, "bad envelope"));
    };
    let id = env.id;
    match env.request {
        Request::Hello => http::json(&Response::ok(
            id,
            json!({
                "app": info.app_version,
                "version": info.app_version,
                "remote_url": info.remote_url(),
                "ota": cfg!(feature = "ota"),
            }),
        )),
        Request::Ma { action } => ma::handle(id, action, info, bridge).await,
        // settings + ctl are UI-bound — ride the bridge onto the
        // UI thread (dispatch there owns the command path)
        other => {
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PIN: &str = "ABCD2345";

    fn info() -> WebInfo {
        WebInfo {
            port: 8787,
            pin: String::new(),
            app_version: "0.1.0 (abc1234)".into(),
        }
    }

    // a bridge to no UI: these routes never reach the UI thread
    fn detached_bridge() -> Bridge {
        Bridge {
            ui: slint::Weak::default(),
        }
    }

    // one request through `handle` over a real socket → status line
    fn status_of(request: &str) -> String {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tokio::spawn(async move {
                    let (stream, _) = listener.accept().await.unwrap();
                    let _ = handle(stream, info(), detached_bridge(), PIN.into()).await;
                });
                let mut client = TcpStream::connect(addr).await.unwrap();
                client.write_all(request.as_bytes()).await.unwrap();
                let mut raw = Vec::new();
                client.read_to_end(&mut raw).await.unwrap();
                String::from_utf8_lossy(&raw)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
    }

    #[test]
    fn hello_reports_version_and_ota_capability() {
        let reply = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(dispatch(
                r#"{"id":1,"op":"hello"}"#,
                &info(),
                &detached_bridge(),
            ));
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["data"]["version"], "0.1.0 (abc1234)");
        assert_eq!(reply["data"]["ota"], cfg!(feature = "ota"));
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
        let head = format!("GET /ota?pair={PIN} HTTP/1.1\r\n\r\n");
        assert!(status_of(&head).contains("404"));
    }

    #[test]
    #[cfg(not(feature = "ota"))]
    fn ota_is_404_without_the_feature() {
        let head = format!("POST /ota?pair={PIN} HTTP/1.1\r\nContent-Length: 5242880\r\n\r\n");
        assert!(status_of(&head).contains("404"));
    }
}
