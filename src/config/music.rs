#![doc = "[music-assistant]"]

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Default, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MusicAssistant {
    /// Music Assistant server — either the human `http(s)://host:port`
    /// form (the setup wizard stores this) or a full
    /// `ws(s)://host:port/ws` endpoint (see [`Self::ws_endpoint`])
    pub url: String,
    /// exact name of the player to show; empty = auto-pick (the
    /// playing one, else the most available — deterministic by id)
    pub player: String,
    /// long-lived API token. Priority: this value (config file ⊕
    /// settings overlay) over the MESTRO_TOKEN env — the
    /// headless path keeps working
    pub token: String,
}

impl MusicAssistant {
    /// the websocket endpoint, derived from `url` — accepts both
    /// the human `http(s)://host:port` form and a `ws(s)://…/ws`
    /// endpoint and normalizes to the latter
    pub fn ws_endpoint(&self) -> String {
        let trimmed = self.url.trim().trim_end_matches('/');
        // unconfigured: no endpoint, the provider must not dial
        if trimmed.is_empty() {
            return String::new();
        }
        let (secure, rest) = if let Some(r) = trimmed.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = trimmed.strip_prefix("wss://") {
            (true, r)
        } else if let Some(r) = trimmed.strip_prefix("http://") {
            (false, r)
        } else if let Some(r) = trimmed.strip_prefix("ws://") {
            (false, r)
        } else {
            (false, trimmed)
        };
        let host = rest.split('/').next().unwrap_or(rest);
        format!("{}://{host}/ws", if secure { "wss" } else { "ws" })
    }
}
