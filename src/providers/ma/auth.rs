//! One-shot Music Assistant HTTP calls for the setup wizard
//! (`src/web/`): server probe, username/password login and the
//! player list ride the plain REST surface — no session loop here,
//! `session.rs` owns the live socket.
//!
//! Token model (verified against music-assistant/server auth.py):
//! `POST /auth/login` mints a short-lived sliding token — the
//! server renews it on use (30d idle expiry, hard cap 90d from
//! creation). We store it as-is; a dead token surfaces as a
//! re-login in the wizard, there is no client-side renewal and no
//! long-lived minting.

use serde_json::{json, Value};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

/// how the server wants to be talked to (schema-derived)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// schema >= 28 with a login endpoint: username/password works
    Login,
    /// schema < 28: no auth at all
    None,
}

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub version: String,
    pub schema: u32,
    pub auth: AuthMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// server unreachable / not an MA server
    Unreachable,
    /// username/password rejected
    BadCredentials,
    /// stored token rejected or expired — re-login needed
    TokenRejected,
    /// the server is too old for this operation
    Unsupported,
    /// unexpected response shape
    Protocol(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable => write!(f, "server unreachable"),
            Self::BadCredentials => write!(f, "invalid username or password"),
            Self::TokenRejected => write!(f, "token rejected or expired"),
            Self::Unsupported => write!(f, "server too old for this operation"),
            Self::Protocol(m) => write!(f, "unexpected response: {m}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSummary {
    pub id: String,
    pub name: String,
}

/// `GET /info` — unauthenticated server facts
pub fn probe(url: &str) -> Result<ServerInfo, AuthError> {
    let body = get_json(&join(url, "info")).map_err(|_| AuthError::Unreachable)?;
    server_info(&body)
}

/// `POST /auth/login` — unauthenticated, the one endpoint that
/// works without a token. Returns the (sliding) access token.
pub fn login(
    url: &str,
    username: &str,
    password: &str,
    device_name: &str,
) -> Result<String, AuthError> {
    // recent servers read a nested "credentials" object, older ones
    // the flat fields — send both shapes like the official client
    let payload = json!({
        "username": username,
        "password": password,
        "device_name": device_name,
        "credentials": { "username": username, "password": password },
    });
    let body = post_json(&join(url, "auth/login"), None, &payload).map_err(|e| match e {
        HttpErr::Status(401) | HttpErr::Status(403) => AuthError::BadCredentials,
        HttpErr::Status(404) => AuthError::Unsupported,
        _ => AuthError::Unreachable,
    })?;
    login_token(&body)
}

/// `POST /api` `players/all` — the wizard's player suggestions
pub fn players(url: &str, token: &str) -> Result<Vec<PlayerSummary>, AuthError> {
    let body = api_call(url, token, "players/all", json!({}))?;
    player_list(&body)
}

/// one JSON-RPC-ish frame over `POST /api` (same envelope as the
/// WS protocol: message_id/command/args → result | error)
pub fn api_call(url: &str, token: &str, command: &str, args: Value) -> Result<Value, AuthError> {
    let payload = json!({ "message_id": "1", "command": command, "args": args });
    let body = post_json(&join(url, "api"), Some(token), &payload).map_err(|e| match e {
        HttpErr::Status(401) | HttpErr::Status(403) => AuthError::TokenRejected,
        HttpErr::Status(404) => AuthError::Unsupported,
        _ => AuthError::Unreachable,
    })?;
    api_result(&body)
}

// ── response parsing (pure — unit-tested below) ──

fn server_info(body: &Value) -> Result<ServerInfo, AuthError> {
    let schema = body
        .get("schema_version")
        .or_else(|| body.get("schema"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let version = body
        .get("version")
        .or_else(|| body.get("server_version"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    // auth became mandatory with schema 28; the login endpoint may
    // still be missing on odd builds — login() maps its 404
    let auth = if schema == 0 || schema >= 28 {
        AuthMode::Login
    } else {
        AuthMode::None
    };
    Ok(ServerInfo {
        version,
        schema,
        auth,
    })
}

fn login_token(body: &Value) -> Result<String, AuthError> {
    if body.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(AuthError::BadCredentials);
    }
    body.get("token")
        .or_else(|| body.get("access_token"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AuthError::Protocol("login response has no access token".into()))
}

fn api_result(body: &Value) -> Result<Value, AuthError> {
    if let Some(error) = body.get("error") {
        let code = error.get("code").and_then(Value::as_u64).unwrap_or(0);
        // the WS error vocabulary: 20 = auth required, 23 = token
        // invalid/expired
        return Err(match code {
            20 | 23 => AuthError::TokenRejected,
            _ => AuthError::Protocol(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("api error")
                    .to_string(),
            ),
        });
    }
    Ok(match body.get("result") {
        Some(result) => result.clone(),
        // some builds answer with the bare result, no envelope
        None if body.is_array() => body.clone(),
        None => Value::Null,
    })
}

fn player_list(result: &Value) -> Result<Vec<PlayerSummary>, AuthError> {
    // WS-era shape: [{player_id, name}, …]; newer servers return
    // a dict keyed by player_id
    let list = match result {
        Value::Array(items) => items.iter().filter_map(player_from).collect::<Vec<_>>(),
        Value::Object(map) => map
            .iter()
            .map(|(id, player)| {
                let mut summary = player_from(player).unwrap_or_else(|| PlayerSummary {
                    id: id.clone(),
                    name: id.clone(),
                });
                if summary.id.is_empty() {
                    summary.id = id.clone();
                }
                summary
            })
            .collect(),
        other => {
            return Err(AuthError::Protocol(format!(
                "players/all returned {}",
                match other {
                    Value::Null => "nothing".to_string(),
                    value => value.to_string(),
                }
            )))
        }
    };
    Ok(list)
}

fn player_from(player: &Value) -> Option<PlayerSummary> {
    let id = player
        .get("player_id")
        .or_else(|| player.get("playerId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let name = player
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    (!name.is_empty()).then_some(PlayerSummary { id, name })
}

// ── HTTP plumbing ──

enum HttpErr {
    Down,
    Status(u16),
    BadBody,
}

pub(crate) fn normalize(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    // accept the ws endpoint form too — [music-assistant].url may
    // hold either spelling
    let http = trimmed
        .strip_prefix("ws://")
        .map(|rest| format!("http://{rest}"))
        .or_else(|| {
            trimmed
                .strip_prefix("wss://")
                .map(|rest| format!("https://{rest}"))
        })
        .unwrap_or_else(|| trimmed.to_string());
    let base = http.strip_suffix("/ws").unwrap_or(&http);
    if base.contains("://") {
        base.to_string()
    } else {
        format!("http://{base}")
    }
}

fn join(url: &str, path: &str) -> String {
    format!("{}/{path}", normalize(url))
}

/// the same MA server, modulo spelling (`http`/`ws`, trailing
/// `/ws`) — gates the token handout: the stored token only ever
/// goes to the stored server
pub(crate) fn same_endpoint(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && !b.trim().is_empty() && normalize(a) == normalize(b)
}

fn parse_body(resp: ureq::Response) -> Result<Value, HttpErr> {
    resp.into_json::<Value>().map_err(|_| HttpErr::BadBody)
}

fn get_json(url: &str) -> Result<Value, HttpErr> {
    let resp = ureq::get(url).timeout(TIMEOUT).call().map_err(http_err)?;
    parse_body(resp)
}

fn post_json(url: &str, bearer: Option<&str>, body: &Value) -> Result<Value, HttpErr> {
    let mut req = ureq::post(url)
        .set("Content-Type", "application/json")
        .timeout(TIMEOUT);
    // an empty token sends no header at all — a bare "Bearer"
    // confuses strict servers and lies about auth
    if let Some(token) = bearer.filter(|t| !t.is_empty()) {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let resp = req.send_string(&body.to_string()).map_err(http_err)?;
    parse_body(resp)
}

fn http_err(err: ureq::Error) -> HttpErr {
    match err {
        ureq::Error::Status(code, _) => HttpErr::Status(code),
        _ => HttpErr::Down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_maps_schema_to_auth_mode() {
        let v = json!({"version": "2.12.0", "schema_version": 28});
        let info = server_info(&v).unwrap();
        assert_eq!(info.schema, 28);
        assert_eq!(info.version, "2.12.0");
        assert_eq!(info.auth, AuthMode::Login);

        let old = json!({"version": "2.4.0", "schema_version": 22});
        assert_eq!(server_info(&old).unwrap().auth, AuthMode::None);

        // no schema field: assume a modern server
        let bare = json!({"version": "2.12.0"});
        assert_eq!(server_info(&bare).unwrap().auth, AuthMode::Login);
    }

    #[test]
    fn login_accepts_both_token_keys() {
        assert_eq!(
            login_token(&json!({"user": {}, "token": "abc"})).unwrap(),
            "abc"
        );
        assert_eq!(
            login_token(&json!({"user": {}, "access_token": "def"})).unwrap(),
            "def"
        );
        assert_eq!(
            login_token(&json!({"success": false})),
            Err(AuthError::BadCredentials)
        );
        assert!(matches!(
            login_token(&json!({"user": {}})),
            Err(AuthError::Protocol(_))
        ));
    }

    #[test]
    fn api_errors_map_to_auth_errors() {
        assert_eq!(
            api_result(&json!({"error": {"code": 23, "message": "expired"}})),
            Err(AuthError::TokenRejected)
        );
        assert_eq!(
            api_result(&json!({"error": {"code": 20, "message": "auth required"}})),
            Err(AuthError::TokenRejected)
        );
        assert!(matches!(
            api_result(&json!({"error": {"code": 9, "message": "boom"}})),
            Err(AuthError::Protocol(_))
        ));
        assert_eq!(
            api_result(&json!({"message_id": "1", "result": ["x"]})).unwrap(),
            json!(["x"])
        );
    }

    #[test]
    fn player_list_accepts_array_and_dict_shapes() {
        let v = json!([
            {"player_id": "a1", "name": "Wohnzimmer", "powered": true},
            {"player_id": "b2", "name": "Küche"},
            {"name": "broken"}
        ]);
        let list = player_list(&v).unwrap();
        assert_eq!(
            list,
            vec![
                PlayerSummary {
                    id: "a1".into(),
                    name: "Wohnzimmer".into()
                },
                PlayerSummary {
                    id: "b2".into(),
                    name: "Küche".into()
                },
                PlayerSummary {
                    id: "".into(),
                    name: "broken".into()
                },
            ]
        );

        let dict = json!({
            "a1": {"player_id": "a1", "name": "Wohnzimmer"},
            "b2": {"name": "Küche"}
        });
        let list = player_list(&dict).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].id, "b2");
        assert_eq!(list[1].name, "Küche");
    }

    #[test]
    fn url_normalization() {
        assert_eq!(
            join("http://ma.local:8095/", "info"),
            "http://ma.local:8095/info"
        );
        assert_eq!(join("ma.local:8095", "info"), "http://ma.local:8095/info");
        // the config endpoint form works for REST calls too
        assert_eq!(
            join("ws://ma.local:8095/ws", "info"),
            "http://ma.local:8095/info"
        );
        assert_eq!(
            join("wss://ma.local/ws", "auth/login"),
            "https://ma.local/auth/login"
        );
    }

    #[test]
    fn ws_endpoint_derivation() {
        let ma = |url: &str| crate::config::MusicAssistant {
            url: url.into(),
            player: String::new(),
            token: String::new(),
        };
        assert_eq!(
            ma("http://ma.example.com:8095").ws_endpoint(),
            "ws://ma.example.com:8095/ws"
        );
        assert_eq!(
            ma("ws://ma.example.com:8095/ws").ws_endpoint(),
            "ws://ma.example.com:8095/ws"
        );
        assert_eq!(
            ma("https://ma.local:443").ws_endpoint(),
            "wss://ma.local:443/ws"
        );
    }
}
