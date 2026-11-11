//! Music Assistant websocket protocol.
//!
//! The module splits along the wire directions:
//! - [`session`]: the loop — auth, request/response matching by
//!   message_id, push-event dispatch, player/queue matching
//! - [`auth`]: one-shot REST (probe/login/players) for the setup
//!   wizard — no session, just the HTTP surface
//! - [`parse`]: wire-format parsing — everything downstream
//!   consumes typed structs; no raw JSON escapes this module
//! - [`command`]: the outbound vocabulary (SourceCommand -> frames)
//!
//! MA 2.10.4 quirks encoded here (verified by live captures, see
//! tools/wscap.rs):
//! - auth must be the first frame and carries a message_id; without
//!   it the session silently never authenticates
//! - push events start flowing only after a state change (idle
//!   systems are silent)
//! - queue_updated replays the previous current_item for ~1.5s after
//!   a skip and replays the previous track's clock on queue_time

pub mod auth;
mod command;
mod parse;
mod session;
mod shuffle;

use super::PlaybackSource;

#[derive(Clone)]
pub struct Config {
    pub ws_url: String,
    pub token: String,
    pub player_name: String,
}

impl Config {
    /// the live connection settings: merged config value over the
    /// MESTRO_TOKEN env (the headless path, docs/configuration.md).
    /// Called per attempt — settings edits take effect without an
    /// app restart (see SourceCommand::Reconnect).
    pub fn from_music_assistant(ma: &crate::config::MusicAssistant) -> Self {
        Self {
            ws_url: ma.ws_endpoint(),
            token: if ma.token.is_empty() {
                std::env::var("MESTRO_TOKEN").unwrap_or_default()
            } else {
                ma.token.clone()
            },
            player_name: ma.player.clone(),
        }
    }
}

pub struct MaProvider {
    cfg: Config,
}

impl MaProvider {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }
}

impl PlaybackSource for MaProvider {
    fn name(&self) -> &'static str {
        "music-assistant"
    }

    fn configured(&self) -> bool {
        !self.cfg.ws_url.is_empty()
    }

    fn failure(&self, error: Box<dyn std::error::Error + Send + Sync>) -> super::Retry {
        use std::time::Duration;
        // MA keeps the socket OPEN on a rejected token — a retry
        // can never heal it in-process; poll slowly
        if let Some(auth) = error.downcast_ref::<AuthError>() {
            tracing::error!("ma auth failed: {}", auth.0);
            return super::Retry {
                message: format!("Music Assistant: {}", auth.0),
                backoff: Duration::from_secs(60),
                heals_on_retry: false,
            };
        }
        tracing::warn!("ma: {error}");
        super::Retry {
            message: format!("Music Assistant unreachable: {error}"),
            backoff: Duration::from_secs(5),
            heals_on_retry: true,
        }
    }

    async fn run(
        &mut self,
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<super::SourceCommand>,
        events: tokio::sync::mpsc::UnboundedSender<super::SourceEvent>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        session::run_session(&self.cfg, commands, &events).await
    }
}

/// auth failed — MA keeps the socket OPEN on a rejected token
/// (verified against 2.10.4: error_code 23 on the auth reply, 20
/// on everything after, no events ever), so the session must
/// fail itself instead of waiting for a close
#[derive(Debug)]
pub struct AuthError(pub String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AuthError {}
