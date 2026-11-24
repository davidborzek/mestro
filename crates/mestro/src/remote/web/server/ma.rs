//! MA setup operations over the WS channel (the wizard's server/
//! login/player steps). probe/login/players are plain HTTP calls
//! against Music Assistant — they run here on the web thread, on
//! the blocking pool, through the [`MaAuth`] port (the binary
//! implements it with its provider's REST client; the web side
//! never links ureq or the parser). The stored token only ever
//! goes to the stored server (the redaction model).

use super::http::json;
use crate::remote::web::{UiPort, WebInfo};
use mestro_ipc::web::{Envelope, MaOp, Request, Response, SettingsOp};
use serde_json::{json as sj, Value};
use std::sync::Arc;

/// how the server wants to be talked to (schema-derived)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// schema >= 28 with a login endpoint: username/password works
    Login,
    /// schema < 28: no auth at all
    None,
}

/// `GET /info` distilled — what the wizard's probe reply carries
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub version: String,
    pub schema: u32,
    pub auth: AuthMode,
}

/// one player of the wizard's player suggestions
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSummary {
    pub id: String,
    pub name: String,
}

/// the Music Assistant REST surface the wizard drives. Blocking
/// (plain HTTP in the binary's implementation) — call it from
/// the blocking pool, never on the async worker
pub trait MaAuth: Send + Sync + 'static {
    /// `GET /info` — unauthenticated server facts
    fn probe(&self, url: &str) -> Result<ServerInfo, String>;

    /// `POST /auth/login` — unauthenticated, the one endpoint
    /// that works without a token. Returns the (sliding) access
    /// token
    fn login(
        &self,
        url: &str,
        username: &str,
        password: &str,
        device_name: &str,
    ) -> Result<String, String>;

    /// `POST /api` `players/all` — the wizard's suggestions. The
    /// stored token only ever goes to the stored server (the
    /// caller checked with [`Self::same_endpoint`])
    fn players(&self, url: &str, token: &str) -> Result<Vec<PlayerSummary>, String>;

    /// the same MA server, modulo spelling (`http`/`ws`, trailing
    /// `/ws`) — gates the token handout: the stored token only
    /// ever goes to the stored server
    fn same_endpoint(&self, a: &str, b: &str) -> bool;
}

pub(super) async fn handle(
    id: u64,
    action: MaOp,
    info: &WebInfo,
    ui: &Arc<dyn UiPort>,
    auth: &Arc<dyn MaAuth>,
) -> String {
    match action {
        MaOp::Probe { url } => {
            let auth = Arc::clone(auth);
            match offload(move || auth.probe(&url)).await {
                Ok(Ok(s)) => json(&Response::ok(
                    id,
                    sj!({
                        "version": s.version,
                        "schema": s.schema,
                        "auth": match s.auth {
                            AuthMode::Login => "login",
                            AuthMode::None => "none",
                        },
                    }),
                )),
                Ok(Err(e)) => json(&Response::err(id, e)),
                Err(e) => json(&Response::err(id, e)),
            }
        }
        MaOp::Login {
            url,
            username,
            password,
        } => {
            let device = format!("mestro · {}", info.app_version);
            let auth = Arc::clone(auth);
            let login = offload({
                let (url, username, password, device) = (
                    url.clone(),
                    username.clone(),
                    password.clone(),
                    device.clone(),
                );
                move || auth.login(&url, &username, &password, &device)
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
                        match crate::remote::web::op_async(ui, json(&envelope)).await {
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
                Ok(Err(e)) => json(&Response::err(id, e)),
                Err(e) => json(&Response::err(id, e)),
            }
        }
        MaOp::Players { url } => {
            // the stored token only ever goes to the stored server
            // (redaction model) — a client-chosen url gets no
            // bearer. No token at all means an old (schema < 28)
            // server or a missing login — MA itself rejects if it
            // cares.
            let (token, stored_url) = fetch_conn(ui).await.unwrap_or_default();
            let token = if auth.same_endpoint(&url, &stored_url) {
                token
            } else {
                String::new()
            };
            let auth = Arc::clone(auth);
            let list = offload(move || auth.players(&url, &token)).await;
            match list {
                Ok(Ok(list)) => json(&Response::ok(
                    id,
                    sj!(list
                        .iter()
                        .map(|p| sj!({ "id": p.id, "name": p.name }))
                        .collect::<Vec<_>>()),
                )),
                Ok(Err(e)) => json(&Response::err(id, e)),
                Err(e) => json(&Response::err(id, e)),
            }
        }
    }
}

/// the stored MA connection (token + url), process-internally.
/// Never wire-visible: `dispatch` only forwards ops from the
/// public [`Request`] vocabulary, this shape is rejected there.
pub(super) async fn fetch_conn(ui: &Arc<dyn UiPort>) -> Result<(String, String), String> {
    let body =
        crate::remote::web::op_async(ui, json(&mestro_ipc::web::InternalOp::MaToken)).await?;
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

/// blocking provider/HTTP work on the blocking pool — never on
/// the async worker
async fn offload<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| "blocking pool gone".to_string())
}
