//! MA setup operations over the WS channel (the wizard's server/
//! login/player steps). probe/login/players are plain HTTP calls
//! against Music Assistant and run here on the web thread — on the
//! blocking pool, never on the async worker. The stored token only
//! ever goes to the stored server (the redaction model).

use super::http::json;
use crate::providers::ma::auth as ma_auth;
use crate::remote::web::{Bridge, Envelope, MaOp, Request, Response, SettingsOp, WebInfo};
use serde_json::{json as sj, Value};

pub(super) async fn handle(id: u64, action: MaOp, info: &WebInfo, bridge: &Bridge) -> String {
    match action {
        MaOp::Probe { url } => match offload(move || ma_auth::probe(&url)).await {
            Ok(Ok(s)) => json(&Response::ok(
                id,
                sj!({
                    "version": s.version,
                    "schema": s.schema,
                    "auth": match s.auth {
                        ma_auth::AuthMode::Login => "login",
                        ma_auth::AuthMode::None => "none",
                    },
                }),
            )),
            Ok(Err(e)) => json(&Response::err(id, e.to_string())),
            Err(e) => json(&Response::err(id, e)),
        },
        MaOp::Login {
            url,
            username,
            password,
        } => {
            let device = format!("mestro · {}", info.app_version);
            let login = offload({
                let (url, username, password, device) = (
                    url.clone(),
                    username.clone(),
                    password.clone(),
                    device.clone(),
                );
                move || ma_auth::login(&url, &username, &password, &device)
            })
            .await;
            match login {
                Ok(Ok(token)) => {
                    // url first, and a failed write STOPS the flow:
                    // no token without its server (partial state)
                    let mut last = Response::err(id, "could not store the login");
                    for (key, value) in [
                        ("music-assistant.url", url),
                        ("music-assistant.token", token),
                    ] {
                        let envelope = Envelope {
                            id,
                            request: Request::Settings {
                                action: SettingsOp::Set {
                                    key: key.into(),
                                    value: toml::Value::String(value),
                                },
                            },
                        };
                        match bridge.op_async(&json(&envelope)).await {
                            Ok(body) => match serde_json::from_str::<Response>(&body) {
                                Ok(r) if r.ok => last = r,
                                Ok(r) => return json(&r),
                                Err(_) => return json(&Response::err(id, "bad ui response")),
                            },
                            Err(e) => return json(&Response::err(id, e)),
                        }
                    }
                    json(&last)
                }
                Ok(Err(e)) => json(&Response::err(id, e.to_string())),
                Err(e) => json(&Response::err(id, e)),
            }
        }
        MaOp::Players { url } => {
            // the stored token only ever goes to the stored server
            // (redaction model) — a client-chosen url gets no
            // bearer. No token at all means an old (schema < 28)
            // server or a missing login — MA itself rejects if it
            // cares.
            let (token, stored_url) = fetch_conn(bridge).await.unwrap_or_default();
            let token = if ma_auth::same_endpoint(&url, &stored_url) {
                token
            } else {
                String::new()
            };
            let list = offload(move || ma_auth::players(&url, &token)).await;
            match list {
                Ok(Ok(list)) => json(&Response::ok(
                    id,
                    sj!(list
                        .iter()
                        .map(|p| sj!({ "id": p.id, "name": p.name }))
                        .collect::<Vec<_>>()),
                )),
                Ok(Err(e)) => json(&Response::err(id, e.to_string())),
                Err(e) => json(&Response::err(id, e)),
            }
        }
    }
}

/// the stored MA connection (token + url), process-internally.
/// Never wire-visible: `dispatch` only forwards ops from the
/// public [`Request`] vocabulary, this shape is rejected there.
async fn fetch_conn(bridge: &Bridge) -> Result<(String, String), String> {
    let body = bridge
        .op_async(&json(&crate::remote::web::protocol::InternalOp::MaToken))
        .await?;
    let value: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if value["ok"] != sj!(true) {
        return Err(value["error"]
            .as_str()
            .unwrap_or("connection unavailable")
            .to_string());
    }
    Ok((
        value["data"]["token"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        value["data"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    ))
}

/// blocking provider/HTTP work (ureq is synchronous) on the
/// blocking pool — never on the async worker
async fn offload<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| "blocking pool gone".to_string())
}
