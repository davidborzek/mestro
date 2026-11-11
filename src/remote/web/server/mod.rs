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
//! - [`http`]/[`ws`]/[`auth`]: transport plumbing

mod auth;
mod ctl;
mod http;
mod ma;
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
            json!({ "app": info.app_version, "remote_url": info.remote_url }),
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
