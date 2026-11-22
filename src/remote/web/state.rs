//! The remote-control state frame: what the PWA renders. Pure
//! data + conversions — the push pipeline (remote/web/push.rs)
//! builds it on the UI thread from a `PlayerState` plus the
//! kiosk view flags, and the serialization here is the wire
//! contract. Everything is unit-tested without sockets or a UI.

use crate::player::state::PlayerState;
use serde::{Deserialize, Serialize};

/// the push frame: `{"event":"state","data":…}` — no id, clients
/// must never await a reply for it
#[derive(Debug, Serialize)]
pub struct PushFrame {
    pub event: &'static str,
    pub data: RemoteState,
}

impl PushFrame {
    pub fn state(data: RemoteState) -> Self {
        Self {
            event: "state",
            data,
        }
    }
}

/// the settings-changed push: `{"event":"settings"}` — no
/// payload, clients refetch the settings state. Published after
/// every successful set/reset (any source: web, slint, ctl)
pub const SETTINGS_EVENT: &str = r#"{"event":"settings"}"#;

/// what the phone shows (docs/remote.md documents the shape)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteState {
    /// kiosk ↔ Music Assistant
    pub connected: bool,
    /// the MA web UI base URL, "" = unknown (unconfigured)
    pub ma_url: String,
    pub player: RemotePlayer,
    pub view: RemoteView,
    pub display: RemoteDisplay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemotePlayer {
    /// the configured/auto-picked player name
    pub name: String,
    pub powered: bool,
    pub playing: bool,
    pub loading: bool,
    pub has_media: bool,
    /// the external source playing on the player ("AirPlay", ...),
    /// "" = Music Assistant's queue. While set, the queue (view,
    /// `queue` ctl) and next/prev/seek do not apply
    #[serde(default)]
    pub source: String,
    /// 0..100
    pub volume: i32,
    pub title: String,
    pub artist: String,
    /// "/cover?v=<n>" or "" (no cover)
    pub cover: String,
    /// "#rrggbb" from the cover palette, "" = default
    pub accent: String,
}

/// which kiosk surface is up (the PWA can mirror a view) plus
/// the VU skin position (vu-view-index/-count)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteView {
    pub now_playing: bool,
    pub queue: bool,
    pub vu: bool,
    pub standby: bool,
    pub vu_index: i32,
    pub vu_count: i32,
    /// the cycle's skin names in `vu_index` order
    pub vu_skins: Vec<String>,
}

/// the backlight — a remote primitive when the device has one
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDisplay {
    pub supported: bool,
    /// 1..100
    pub brightness: i32,
}

/// the kiosk view flags, the MA connection and the backlight,
/// sampled on the UI thread (not part of PlayerState)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteContext {
    pub connected: bool,
    pub ma_url: String,
    pub view: RemoteView,
    pub display: RemoteDisplay,
}

/// the cover's versioned URL + accent ("" = none/default)
pub struct RemoteArt {
    pub version: u64,
    pub accent: String,
}

/// build the frame from the display's state
pub fn from_player(
    state: &PlayerState,
    player: &str,
    ctx: &RemoteContext,
    art: &RemoteArt,
) -> RemoteState {
    RemoteState {
        connected: ctx.connected,
        ma_url: ctx.ma_url.clone(),
        player: RemotePlayer {
            name: player.to_string(),
            powered: state.powered,
            playing: state.playing,
            loading: state.loading,
            has_media: state.has_media,
            source: state.external.clone().unwrap_or_default(),
            volume: state.volume.clamp(0, 100),
            title: state.title.clone(),
            artist: state.artist.clone(),
            // the held artwork is not the shown track's: send none
            cover: if state.cover_missing {
                String::new()
            } else {
                cover_url(art.version)
            },
            accent: if state.cover_missing {
                String::new()
            } else {
                art.accent.clone()
            },
        },
        view: ctx.view.clone(),
        display: ctx.display,
    }
}

/// the MA web UI base URL from the configured ws endpoint:
/// ws→http, wss→https, a trailing `/ws` stripped — "" when the
/// endpoint is unconfigured
pub fn ma_web_url(ws_url: &str) -> String {
    let (secure, rest) = if let Some(rest) = ws_url.strip_prefix("wss://") {
        (true, rest)
    } else if let Some(rest) = ws_url.strip_prefix("ws://") {
        (false, rest)
    } else {
        return String::new();
    };
    let rest = rest.strip_suffix("/ws").unwrap_or(rest);
    format!("{}://{rest}", if secure { "https" } else { "http" })
}

fn cover_url(version: u64) -> String {
    if version == 0 {
        String::new()
    } else {
        // the counter restarts with the process; the boot tag keeps a
        // year-cached URL from a previous run from showing old art
        format!("/cover?v={}-{version}", boot_tag())
    }
}

/// a per-process tag: the start time in seconds, base 36
fn boot_tag() -> &'static str {
    static TAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TAG.get_or_init(|| {
        let mut secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut tag = Vec::new();
        loop {
            tag.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(secs % 36) as usize]);
            secs /= 36;
            if secs == 0 {
                break;
            }
        }
        tag.reverse();
        String::from_utf8(tag).unwrap_or_default()
    })
}

/// push rate limiting: changes coalesce, at most one frame per
/// interval (100 ms in production)
#[derive(Debug, Default)]
pub struct Throttle {
    interval: std::time::Duration,
    last: Option<std::time::Instant>,
}

impl Throttle {
    pub fn new(interval: std::time::Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// true when a push may go out now (and stamps the clock)
    pub fn permit(&mut self, now: std::time::Instant) -> bool {
        let ok = self
            .last
            .is_none_or(|at| now.duration_since(at) >= self.interval);
        if ok {
            self.last = Some(now);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> PlayerState {
        PlayerState {
            playing: true,
            volume: 42,
            title: "Track".into(),
            artist: "Artist".into(),
            duration_ms: 200_000,
            powered: true,
            has_media: true,
            ..Default::default()
        }
    }

    fn ctx() -> RemoteContext {
        RemoteContext {
            connected: true,
            ma_url: "http://ma.local:8095".into(),
            view: RemoteView {
                now_playing: true,
                queue: false,
                vu: false,
                standby: false,
                vu_index: 2,
                vu_count: 7,
                vu_skins: vec!["night".into(), "cream".into(), "round".into()],
            },
            display: RemoteDisplay {
                supported: true,
                brightness: 80,
            },
        }
    }

    fn art() -> RemoteArt {
        RemoteArt {
            version: 3,
            accent: "#f3bd6e".into(),
        }
    }

    #[test]
    fn golden_serialization_shape() {
        let state = from_player(&sample(), "wohnzimmer", &ctx(), &art());
        let frame = serde_json::to_value(PushFrame::state(state)).unwrap();
        assert_eq!(
            frame,
            json!({
                "event": "state",
                "data": {
                    "connected": true,
                    "ma_url": "http://ma.local:8095",
                    "player": {
                        "name": "wohnzimmer",
                        "powered": true,
                        "playing": true,
                        "loading": false,
                        "has_media": true,
                        "source": "",
                        "volume": 42,
                        "title": "Track",
                        "artist": "Artist",
                        "cover": format!("/cover?v={}-3", boot_tag()),
                        "accent": "#f3bd6e"
                    },
                    "view": {
                        "now_playing": true,
                        "queue": false,
                        "vu": false,
                        "standby": false,
                        "vu_index": 2,
                        "vu_count": 7,
                        "vu_skins": ["night", "cream", "round"]
                    },
                    "display": { "supported": true, "brightness": 80 }
                }
            })
        );
    }

    #[test]
    fn an_external_source_is_named_in_the_player() {
        let mut state = sample();
        state.external = Some("AirPlay".into());
        let out = from_player(&state, "p", &ctx(), &art());
        let value = serde_json::to_value(&out).unwrap();
        assert_eq!(value["player"]["source"], json!("AirPlay"));
        // an older frame without the field still reads
        let mut old = value.clone();
        old["player"].as_object_mut().unwrap().remove("source");
        let parsed: RemoteState = serde_json::from_value(old).unwrap();
        assert_eq!(parsed.player.source, "");
    }

    #[test]
    fn no_cover_serializes_empty() {
        let empty = from_player(
            &sample(),
            "",
            &ctx(),
            &RemoteArt {
                version: 0,
                accent: String::new(),
            },
        );
        let value = serde_json::to_value(&empty).unwrap();
        assert_eq!(value["player"]["cover"], json!(""));
        assert_eq!(value["player"]["accent"], json!(""));
        assert_eq!(value["player"]["name"], json!(""));
    }

    #[test]
    fn a_missing_cover_hides_the_previous_artwork_and_accent() {
        let mut state = sample();
        state.cover_missing = true;
        let out = from_player(&state, "p", &ctx(), &art());
        let value = serde_json::to_value(&out).unwrap();
        assert_eq!(value["player"]["cover"], json!(""));
        assert_eq!(value["player"]["accent"], json!(""));
    }

    #[test]
    fn ma_url_derives_from_the_ws_endpoint() {
        assert_eq!(ma_web_url("ws://ma.local:8095/ws"), "http://ma.local:8095");
        assert_eq!(ma_web_url("wss://ma.local/ws"), "https://ma.local");
        // endpoints without the /ws suffix still resolve
        assert_eq!(ma_web_url("wss://ma.local:8090"), "https://ma.local:8090");
        // unconfigured or foreign schemes stay unknown
        assert_eq!(ma_web_url(""), "");
        assert_eq!(ma_web_url("http://ma.local"), "");
    }

    #[test]
    fn volume_clamps_into_range() {
        let mut loud = sample();
        loud.volume = 150;
        let out = from_player(&loud, "p", &ctx(), &art());
        assert_eq!(out.player.volume, 100);
    }

    #[test]
    fn the_throttle_permits_at_most_one_per_interval() {
        let mut throttle = Throttle::new(std::time::Duration::from_millis(100));
        let t0 = std::time::Instant::now();
        assert!(throttle.permit(t0));
        assert!(
            !throttle.permit(t0 + std::time::Duration::from_millis(50)),
            "inside the interval"
        );
        assert!(throttle.permit(t0 + std::time::Duration::from_millis(100)));
    }

    #[test]
    fn the_settings_event_is_a_bare_event_frame() {
        let frame: serde_json::Value = serde_json::from_str(SETTINGS_EVENT).unwrap();
        assert_eq!(frame, json!({"event": "settings"}));
    }
}
