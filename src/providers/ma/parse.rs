//! Wire-format parsing: MA JSON frames -> typed structs. No raw
//! JSON escapes this module's callers.

use crate::providers::{MediaEntry, QueueItem, RepeatMode, TrackMeta};
use serde_json::Value;

/// Per-player identity and transport for discovery/auto-pick.
/// Player-side metadata (current_media) is deliberately NOT parsed:
/// it lags behind the queue during a skip and must never reach the UI.
pub(super) struct PlayerSnapshot {
    pub(super) player_id: String,
    pub(super) name: String,
    pub(super) playing: bool,
    pub(super) volume: i32,
    pub(super) powered: bool,
    pub(super) available: bool,
    pub(super) has_media: bool,
}

/// Typed push events from the MA socket (provider-internal; the
/// queue/player matching to the active player happens in session).
// the Queue variant carries a whole TrackMeta; events are consumed
// right away, boxing would only add an allocation per event
#[allow(clippy::large_enum_variant)]
pub(super) enum MaEvent {
    Player(PlayerSnapshot),
    /// active-item metadata change on a queue (elapsed = the queue's
    /// own position for the new item — the event can arrive seconds
    /// after a natural advance). track is None on a cleared queue.
    Queue {
        queue_id: String,
        track: Option<TrackMeta>,
        elapsed_sec: f64,
        current_index: usize,
        shuffle: bool,
        repeat: RepeatMode,
    },
    /// elapsed-time correction for a queue
    QueueTime {
        queue_id: String,
        elapsed_sec: f64,
    },
}

/// Reply payload of player_queues/get_active_queue.
pub(super) struct ActiveQueue {
    pub(super) queue_id: String,
    pub(super) elapsed_sec: f64,
    pub(super) track: TrackMeta,
    pub(super) current_index: usize,
    pub(super) shuffle: bool,
    pub(super) repeat: RepeatMode,
}

/// Derive the http base (for imageproxy URLs) from the WS URL.
pub(super) fn http_base_from_ws(ws_url: &str) -> String {
    let (scheme, rest) = if let Some(r) = ws_url.strip_prefix("wss://") {
        ("https", r)
    } else if let Some(r) = ws_url.strip_prefix("ws://") {
        ("http", r)
    } else if let Some(r) = ws_url.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = ws_url.strip_prefix("http://") {
        ("http", r)
    } else {
        ("http", ws_url)
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    format!("{scheme}://{authority}")
}

pub(super) fn str_at(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for p in path {
        cur = cur.get(p).unwrap_or(&Value::Null);
    }
    cur.as_str().unwrap_or_default().to_string()
}

/// Parse one entry of a browse/drill reply. MA's parent
/// reference is a folder named ".." with a level-specific
/// item_id ("back", "root") — dropped by name: the capsule owns
/// the back affordance. Covers can sit at the top level or under
/// metadata.images — albums deliver them only in metadata.
pub(super) fn parse_media_entry(v: &Value, img_base: &str) -> Option<MediaEntry> {
    let name = v.get("name")?.as_str()?.to_string();
    if name == ".." {
        return None;
    }
    let kind = v.get("media_type")?.as_str()?.to_string();
    let proxy_id = v
        .pointer("/image/proxy_id")
        .and_then(|p| p.as_str())
        .or_else(|| {
            v.pointer("/metadata/images/0/proxy_id")
                .and_then(|p| p.as_str())
        });
    Some(MediaEntry {
        name,
        kind,
        path: str_at(v, &["path"]),
        item_id: str_at(v, &["item_id"]),
        provider: str_at(v, &["provider"]),
        uri: str_at(v, &["uri"]),
        image_url: proxy_id
            .map(|pid| format!("{img_base}/imageproxy/{pid}?size=256&fmt=jpg"))
            .unwrap_or_default(),
        artist: v
            .pointer("/artists/0/name")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string(),
        year: v
            .get("year")
            .and_then(|y| y.as_i64())
            .filter(|y| *y > 0)
            .map(|y| y.to_string())
            .unwrap_or_default(),
        artist_id: v
            .pointer("/artists/0/item_id")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string(),
        artist_uri: v
            .pointer("/artists/0/uri")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string(),
        album_id: v
            .pointer("/album/item_id")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string(),
        album_name: v
            .pointer("/album/name")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string(),
        album_uri: v
            .pointer("/album/uri")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string(),
        duration_sec: v.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0),
    })
}

/// Parse one entry of a player_queues/items reply: list rows only
/// — no cover fetch here. media_item carries title and artists
/// separately; the item's own `name` is the combined
/// "Artist - Title" string as a fallback.
pub(super) fn parse_queue_item(v: &Value, img_base: &str) -> Option<QueueItem> {
    let queue_item_id = v.get("queue_item_id")?.as_str()?.to_string();
    let (title, artist) = match v.get("media_item") {
        Some(media) => {
            let title = media
                .get("name")
                .and_then(|t| t.as_str())
                .or_else(|| v.get("name").and_then(|n| n.as_str()))
                .unwrap_or("")
                .to_string();
            let artist = media
                .get("artists")
                .and_then(|a| a.as_array())
                .map(|artists| {
                    artists
                        .iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            (title, artist)
        }
        None => {
            // combined string: split once from the left
            let combined = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
            match combined.split_once(" - ") {
                Some((artist, title)) => (title.to_string(), artist.to_string()),
                None => (combined.to_string(), String::new()),
            }
        }
    };
    Some(QueueItem {
        queue_item_id,
        sort_index: 0, // set by the caller: the array position
        title,
        artist,
        duration_sec: v.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0),
        // go-to references from the media item's mappings
        provider: v
            .pointer("/media_item/provider")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        artist_id: v
            .pointer("/media_item/artists/0/item_id")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        artist_uri: v
            .pointer("/media_item/artists/0/uri")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        album_id: v
            .pointer("/media_item/album/item_id")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        album_name: v
            .pointer("/media_item/album/name")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        album_uri: v
            .pointer("/media_item/album/uri")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        // the item's artwork (track thumb = album cover) — feeds
        // the go-to-album subject header
        image_url: v
            .pointer("/image/proxy_id")
            .or_else(|| v.pointer("/media_item/image/proxy_id"))
            .and_then(|p| p.as_str())
            .map(|pid| format!("{img_base}/imageproxy/{pid}?size=256&fmt=jpg"))
            .unwrap_or_default(),
    })
}

pub(super) fn parse_player(v: &Value) -> Option<PlayerSnapshot> {
    Some(PlayerSnapshot {
        player_id: str_at(v, &["player_id"]),
        name: str_at(v, &["name"]),
        playing: str_at(v, &["playback_state"]) == "playing",
        volume: v
            .get("volume_level")
            .and_then(|x| x.as_f64())
            .map(|f| f.round() as i32)
            .unwrap_or(0),
        powered: v.get("powered").and_then(|p| p.as_bool()).unwrap_or(true),
        available: v
            .get("available")
            .and_then(|a| a.as_bool())
            .unwrap_or(false),
        has_media: v
            .get("current_media")
            .map(|m| !m.is_null())
            .unwrap_or(false),
    })
}

/// Extract the queue-owned metadata from a queue item (current_item).
/// NOTE: the queue's media_item uses rich objects (name, artists[],
/// album.name, image.proxy_id) — unlike the player's flat current_media.
pub(super) fn parse_track(item: &Value, img_base: &str) -> TrackMeta {
    let media = item.pointer("/media_item").unwrap_or(item);
    let title = media
        .get("name")
        .and_then(|t| t.as_str())
        .or_else(|| media.get("title").and_then(|t| t.as_str()))
        .unwrap_or("")
        .to_string();
    let artist = {
        let list = media.get("artists").and_then(|a| a.as_array());
        match list {
            // queue shape: array of artist objects
            Some(arr) => arr
                .iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
                .join(", "),
            // fallback: flat string (player current_media shape)
            None => media
                .get("artist")
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string(),
        }
    };
    let album = media
        .pointer("/album/name")
        .and_then(|t| t.as_str())
        .or_else(|| media.get("album").and_then(|t| t.as_str()))
        .unwrap_or("")
        .to_string();
    // the queue carries the image as a proxy ref; the fetchable URL is
    // constructed from the proxy_id (same shape the player's
    // current_media delivers ready-made)
    let image_url = item
        .pointer("/image/proxy_id")
        .and_then(|p| p.as_str())
        .map(|pid| format!("{img_base}/imageproxy/{pid}?size=512&fmt=jpg"))
        .unwrap_or_default();
    let duration_sec = item
        .get("duration")
        .and_then(|d| d.as_f64())
        .or_else(|| media.get("duration").and_then(|d| d.as_f64()))
        .unwrap_or(0.0);
    let t = quality_from(item);
    TrackMeta {
        title,
        artist,
        album,
        image_url,
        codec: t.0,
        rate: t.1,
        depth: t.2,
        bitrate: t.3,
        channels: t.4,
        lossless: t.5,
        duration_ms: (duration_sec * 1000.0) as u64,
    }
}

/// (codec, rate, depth, bitrate, channels, lossless) for the
/// streamdetails of a queue item; the parts arrive preformatted
/// (empty when unreported) — the codec renders as a badge, the
/// rest assemble into the detail line per config
fn quality_from(item: &Value) -> (String, String, String, String, String, bool) {
    let Some(fmt) = item.pointer("/streamdetails/audio_format") else {
        return (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            false,
        );
    };
    let codec = fmt
        .get("content_type")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_uppercase();
    if codec.is_empty() {
        return (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            false,
        );
    }
    let lossless = matches!(
        codec.as_str(),
        "FLAC"
            | "ALAC"
            | "WAV"
            | "WAVE"
            | "AIFF"
            | "AIF"
            | "PCM"
            | "LPCM"
            | "DSD"
            | "DSF"
            | "DFF"
            | "APE"
            | "WV"
    );
    let rate = fmt.get("sample_rate").and_then(|x| x.as_i64()).unwrap_or(0);
    let depth = fmt.get("bit_depth").and_then(|x| x.as_i64()).unwrap_or(0);
    // bit_rate in bit/s; 0 for most lossless streams
    let bitrate = fmt.get("bit_rate").and_then(|x| x.as_i64()).unwrap_or(0);
    let channels = fmt.get("channels").and_then(|x| x.as_i64()).unwrap_or(0);

    let rate_s = if rate > 0 {
        if rate % 1000 == 0 {
            format!("{} kHz", rate / 1000)
        } else {
            format!("{:.1} kHz", rate as f64 / 1000.0)
        }
    } else {
        String::new()
    };
    let depth_s = if depth > 0 {
        format!("{depth} bit")
    } else {
        String::new()
    };
    let bitrate_s = if bitrate > 0 {
        format!("{} kbps", (bitrate + 500) / 1000)
    } else {
        String::new()
    };
    let channels_s = if channels > 0 {
        format!("{channels} ch")
    } else {
        String::new()
    };

    (codec, rate_s, depth_s, bitrate_s, channels_s, lossless)
}

/// Parse a push event frame into a typed event. Returns None for
/// non-event frames (replies) and events without a usable payload.
pub(super) fn parse_event(v: &Value, img_base: &str) -> Option<MaEvent> {
    let event = v.get("event")?.as_str()?;
    let object_id = v.get("object_id").and_then(|o| o.as_str()).unwrap_or("");
    match event {
        "player_updated" => {
            let data = v.get("data").cloned().unwrap_or(Value::Null);
            let player = data.get("player").unwrap_or(&data);
            Some(MaEvent::Player(parse_player(player)?))
        }
        "queue_time_updated" => {
            let elapsed_sec = v.get("data").and_then(|d| d.as_f64())?;
            Some(MaEvent::QueueTime {
                queue_id: object_id.to_string(),
                elapsed_sec,
            })
        }
        "queue_updated" => {
            // current_item is absent on a cleared queue — the event
            // must still flow so the item list refetches
            let item = v.pointer("/data/current_item");
            let track = item.map(|item| parse_track(item, img_base));
            let elapsed_sec = v
                .pointer("/data/elapsed_time")
                .and_then(|d| d.as_f64())
                .unwrap_or(0.0);
            let current_index = v
                .pointer("/data/current_index")
                .and_then(|d| d.as_u64())
                .map(|i| i as usize)
                .unwrap_or(usize::MAX);
            Some(MaEvent::Queue {
                queue_id: object_id.to_string(),
                track,
                elapsed_sec,
                current_index,
                // playback options ride every queue event
                shuffle: v
                    .pointer("/data/shuffle_enabled")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
                repeat: v
                    .pointer("/data/repeat_mode")
                    .and_then(|m| m.as_str())
                    .map(RepeatMode::parse)
                    .unwrap_or_default(),
            })
        }
        _ => None,
    }
}

/// Parse the reply of player_queues/get_active_queue.
pub(super) fn parse_active_queue(v: &Value, img_base: &str) -> Option<ActiveQueue> {
    let queue_id = v.pointer("/result/queue_id")?.as_str()?.to_string();
    let elapsed_sec = v
        .pointer("/result/elapsed_time")
        .and_then(|d| d.as_f64())
        .unwrap_or(0.0);
    let track = v
        .pointer("/result/current_item")
        .map(|item| parse_track(item, img_base))
        .unwrap_or_default();
    let current_index = v
        .pointer("/result/current_index")
        .and_then(|d| d.as_u64())
        .map(|i| i as usize)
        .unwrap_or(usize::MAX);
    let shuffle = v
        .pointer("/result/shuffle_enabled")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let repeat = v
        .pointer("/result/repeat_mode")
        .and_then(|m| m.as_str())
        .map(RepeatMode::parse)
        .unwrap_or_default();
    Some(ActiveQueue {
        queue_id,
        elapsed_sec,
        track,
        current_index,
        shuffle,
        repeat,
    })
}
