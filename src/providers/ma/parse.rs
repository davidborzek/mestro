//! Wire-format parsing: MA JSON frames -> typed structs. No raw
//! JSON escapes this module's callers.

use crate::providers::{MediaEntry, QueueItem, RepeatMode, TrackMeta};
use serde_json::Value;

/// Per-player identity and transport for discovery/auto-pick.
/// Player-side metadata (current_media) lags behind the queue during
/// a skip and must never reach the UI: it is read ONLY while the
/// player plays an external source (`source`), where the queue is
/// silent and current_media is the one place that knows the track.
pub(super) struct PlayerSnapshot {
    pub(super) player_id: String,
    pub(super) name: String,
    pub(super) playing: bool,
    pub(super) volume: i32,
    pub(super) powered: bool,
    pub(super) available: bool,
    pub(super) has_media: bool,
    pub(super) source: PlayerSource,
}

/// what the player plays out of, from its `active_source`
// ExternalMedia carries a whole TrackMeta; snapshots are consumed
// right away, boxing would only add an allocation per event
#[allow(clippy::large_enum_variant)]
pub(super) enum PlayerSource {
    /// MA's own queue (active_source is the player's own id, empty
    /// or not a listed source): queue events own the metadata,
    /// current_media is never read
    Queue,
    /// an external source (AirPlay, Spotify Connect, ...)
    External(ExternalMedia),
}

/// current_media of a player running an external source
pub(super) struct ExternalMedia {
    /// display name of the source, MA's listed name without the
    /// player suffix: "AirPlay Receiver", "Spotify Connect", ...
    pub(super) source: String,
    /// None = no metadata yet (absent, or MA's placeholder title)
    pub(super) track: Option<TrackMeta>,
    /// current_media.elapsed_time, as of `elapsed_updated`
    pub(super) elapsed_sec: f64,
    /// unix time of that reading, 0 = not reported
    elapsed_updated: f64,
}

impl ExternalMedia {
    /// identifies one reading of MA's external clock: it changes
    /// only when MA writes a new elapsed_time, so an unchanged key
    /// after a skip means the clock still belongs to the old track
    pub(super) fn reading_key(&self) -> (u64, u64) {
        (self.elapsed_sec.to_bits(), self.elapsed_updated.to_bits())
    }

    /// the position now: MA stamps elapsed_time with the time it was
    /// true and does not tick it, so a playing source is carried
    /// forward by the reading's age — when that age is plausible
    /// (stamped, not from the future, within the track's length;
    /// the two machines' clocks are not trusted further)
    pub(super) fn position_at(&self, now_unix: f64, playing: bool) -> f64 {
        let duration = self
            .track
            .as_ref()
            .map_or(0.0, |track| track.duration_ms as f64 / 1000.0);
        let age = now_unix - self.elapsed_updated;
        if playing && self.elapsed_updated > 0.0 && age > 0.0 && age <= duration {
            (self.elapsed_sec + age).min(duration)
        } else {
            self.elapsed_sec
        }
    }
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

pub(super) fn parse_player(v: &Value, img_base: &str) -> Option<PlayerSnapshot> {
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
        source: parse_source(v, img_base),
    })
}

/// MA lists a player's sources with the player's name appended
/// ("AirPlay Receiver (HiFi)"); the display drops that suffix
fn display_source_name(listed: &str, player_name: &str) -> String {
    listed
        .strip_suffix(&format!(" ({player_name})"))
        .filter(|stripped| !player_name.is_empty() && !stripped.trim().is_empty())
        .unwrap_or(listed)
        .to_string()
}

/// MA's own rule (its frontend's `resolveExternalSource`): the
/// player plays an external source when `active_source` is set, is
/// not the player's own id (the queue) and is listed in
/// `source_list`. Anything else — also when unsure — is the queue;
/// then current_media is not read.
fn parse_source(v: &Value, img_base: &str) -> PlayerSource {
    let active_source = str_at(v, &["active_source"]);
    if active_source.is_empty() || active_source == str_at(v, &["player_id"]) {
        return PlayerSource::Queue;
    }
    let listed_name = v
        .get("source_list")
        .and_then(|list| list.as_array())
        .and_then(|list| {
            list.iter()
                .find(|entry| entry.get("id").and_then(|id| id.as_str()) == Some(&active_source))
        })
        .map(|entry| str_at(entry, &["name"]))
        .filter(|name| !name.is_empty());
    let Some(listed_name) = listed_name else {
        return PlayerSource::Queue;
    };
    let player_name = match str_at(v, &["display_name"]) {
        name if name.is_empty() => str_at(v, &["name"]),
        name => name,
    };
    let source = display_source_name(&listed_name, &player_name);
    let is_source_name = |text: &str| text == listed_name || text == source;
    let media = v.get("current_media").unwrap_or(&Value::Null);
    let title = str_at(media, &["title"]);
    let album = str_at(media, &["album"]);
    // MA fills current_media with the source's own name until the
    // first real track arrives (its frontend's rule); the capture
    // also shows "<source> | <player> | Music Assistant", which that
    // rule does not catch
    let placeholder = is_source_name(&title) || title.ends_with("| Music Assistant");
    let track = (!title.is_empty() && !placeholder).then(|| TrackMeta {
        title,
        artist: str_at(media, &["artist"]),
        // a source without an album lands its own name there
        album: if is_source_name(&album) {
            String::new()
        } else {
            album
        },
        image_url: rebase_image_url(&str_at(media, &["image_url"]), img_base),
        duration_ms: (media
            .get("duration")
            .and_then(|d| d.as_f64())
            .unwrap_or(0.0)
            * 1000.0) as u64,
        ..Default::default()
    });
    PlayerSource::External(ExternalMedia {
        source,
        track,
        elapsed_sec: media
            .get("elapsed_time")
            .and_then(|t| t.as_f64())
            .unwrap_or(0.0),
        elapsed_updated: media
            .get("elapsed_time_last_updated")
            .and_then(|t| t.as_f64())
            .unwrap_or(0.0),
    })
}

/// current_media carries a ready-made imageproxy URL on MA's public
/// hostname with size=0 (the original). Fetch it the way queue
/// covers are fetched: through the configured MA host, at cover size.
fn rebase_image_url(url: &str, img_base: &str) -> String {
    let Some((_, rest)) = url.split_once("/imageproxy/") else {
        return url.to_string();
    };
    let proxy_id = rest.split('?').next().unwrap_or(rest);
    if proxy_id.is_empty() {
        return url.to_string();
    }
    format!("{img_base}/imageproxy/{proxy_id}?size=512&fmt=jpg")
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
            Some(MaEvent::Player(parse_player(player, img_base)?))
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

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "http://ma.test:8095";

    fn fixture(json: &str) -> Value {
        let frame: Value = serde_json::from_str(json).unwrap();
        frame["data"].clone()
    }

    fn placeholder() -> Value {
        fixture(include_str!(
            "../../../tests/fixtures/ma/airplay_placeholder.json"
        ))
    }

    fn airplay_track() -> Value {
        fixture(include_str!(
            "../../../tests/fixtures/ma/airplay_track.json"
        ))
    }

    fn queue_back() -> Value {
        fixture(include_str!("../../../tests/fixtures/ma/queue_back.json"))
    }

    fn external(v: &Value) -> ExternalMedia {
        match parse_player(v, BASE).unwrap().source {
            PlayerSource::External(media) => media,
            PlayerSource::Queue => panic!("expected an external source"),
        }
    }

    /// a source entry as MA lists it, with the active source switched to it
    fn switch_to(v: &mut Value, id: &str, name: &str) {
        v["active_source"] = id.into();
        v["source_list"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": id, "name": name, "passive": false,
                "can_play_pause": true, "can_seek": false, "can_next_previous": false,
                "can_shuffle": false, "can_repeat": false,
            }));
    }

    fn is_queue(v: &Value) -> bool {
        matches!(parse_player(v, BASE).unwrap().source, PlayerSource::Queue)
    }

    #[test]
    fn an_airplay_track_is_read_from_current_media() {
        let media = external(&airplay_track());
        assert_eq!(media.source, "AirPlay Receiver");
        let track = media.track.unwrap();
        assert_eq!(track.title, "Stand by Me");
        assert_eq!(track.artist, "Ben E. King");
        assert_eq!(track.album, "Stand by Me");
        assert_eq!(track.duration_ms, 179_000);
        assert_eq!(media.elapsed_sec, 0.0);
        assert!((media.elapsed_updated - 1795350805.2879329).abs() < 1e-6);
    }

    #[test]
    fn the_cover_loads_through_the_configured_host_at_cover_size() {
        // current_media ships MA's public hostname and size=0
        let track = external(&airplay_track()).track.unwrap();
        assert_eq!(
            track.image_url,
            "http://ma.test:8095/imageproxy/7c47ad991a1d0d057e5952472824556d2fd55efcd016123ca8e99ec723be0704?size=512&fmt=jpg"
        );
    }

    #[test]
    fn a_listed_source_other_than_the_players_own_id_is_external() {
        assert!(!is_queue(&placeholder()));
        assert!(!is_queue(&airplay_track()));
        assert!(is_queue(&queue_back()));
    }

    #[test]
    fn the_players_own_id_is_the_queue_even_when_listed() {
        // the queue is always listed under the player's id
        let mut v = airplay_track();
        v["active_source"] = v["player_id"].clone();
        assert!(is_queue(&v));
    }

    #[test]
    fn an_active_source_missing_from_the_source_list_is_the_queue() {
        // a URI-shaped id MA does not list: not sure, so the queue
        let mut v = airplay_track();
        v["active_source"] = "spotify_connect://audio_source/x".into();
        assert!(is_queue(&v));
        let mut v = airplay_track();
        v["source_list"] = Value::Null;
        assert!(is_queue(&v));
    }

    #[test]
    fn an_empty_or_missing_active_source_is_the_queue() {
        let mut v = airplay_track();
        v["active_source"] = "".into();
        assert!(is_queue(&v));
        v["active_source"] = Value::Null;
        assert!(is_queue(&v));
        v.as_object_mut().unwrap().remove("active_source");
        assert!(is_queue(&v));
    }

    #[test]
    fn a_source_without_a_uri_shape_is_external_when_listed() {
        // no URI-scheme heuristics: MA's source list decides
        let mut v = airplay_track();
        switch_to(&mut v, "optical", "Optical (HiFi)");
        assert_eq!(external(&v).source, "Optical");
    }

    #[test]
    fn the_player_name_suffix_is_stripped_from_the_source_name() {
        assert_eq!(
            display_source_name("AirPlay Receiver (HiFi)", "HiFi"),
            "AirPlay Receiver"
        );
        // another player's name, or none at all, stays
        assert_eq!(
            display_source_name("AirPlay Receiver (Kitchen)", "HiFi"),
            "AirPlay Receiver (Kitchen)"
        );
        assert_eq!(display_source_name("Line-In", "HiFi"), "Line-In");
        assert_eq!(display_source_name("Line-In (HiFi)", ""), "Line-In (HiFi)");
        // nothing would be left
        assert_eq!(display_source_name(" (HiFi)", "HiFi"), " (HiFi)");
    }

    #[test]
    fn the_airplay_placeholder_is_no_metadata_yet() {
        let media = external(&placeholder());
        assert_eq!(media.source, "AirPlay Receiver");
        assert!(media.track.is_none());
    }

    #[test]
    fn a_title_equal_to_the_source_name_is_a_placeholder_listed_or_stripped() {
        for title in ["AirPlay Receiver (HiFi)", "AirPlay Receiver"] {
            let mut v = airplay_track();
            v["current_media"]["title"] = title.into();
            assert!(external(&v).track.is_none(), "{title}");
        }
    }

    #[test]
    fn a_placeholder_is_one_even_when_it_carries_artwork() {
        // MA's rule looks at the title only
        let mut v = airplay_track();
        v["current_media"]["title"] = "AirPlay Receiver (HiFi)".into();
        assert!(v["current_media"]["image_url"].is_string());
        assert!(external(&v).track.is_none());
    }

    #[test]
    fn the_music_assistant_suffix_title_is_a_placeholder() {
        // the second placeholder of the capture: not the source's name,
        // so MA's own rule would show it
        let mut v = placeholder();
        v["current_media"]["title"] = "AirPlay | HiFi | Music Assistant".into();
        assert!(external(&v).track.is_none());
    }

    #[test]
    fn a_title_that_merely_starts_like_the_source_is_a_track() {
        let mut v = airplay_track();
        v["current_media"]["title"] = "AirPlay Receiver Blues".into();
        assert_eq!(external(&v).track.unwrap().title, "AirPlay Receiver Blues");
    }

    #[test]
    fn an_album_equal_to_the_source_name_is_no_album() {
        for album in ["AirPlay Receiver (HiFi)", "AirPlay Receiver"] {
            let mut v = airplay_track();
            v["current_media"]["album"] = album.into();
            assert_eq!(external(&v).track.unwrap().album, "", "{album}");
        }
        assert_eq!(
            external(&airplay_track()).track.unwrap().album,
            "Stand by Me"
        );
    }

    #[test]
    fn a_missing_current_media_is_no_metadata_yet() {
        let mut v = placeholder();
        v["current_media"] = Value::Null;
        let media = external(&v);
        assert!(media.track.is_none());
        assert_eq!(media.elapsed_updated, 0.0);
    }

    #[test]
    fn a_track_without_artwork_has_an_empty_image_url() {
        let mut v = airplay_track();
        v["current_media"]["image_url"] = Value::Null;
        assert_eq!(external(&v).track.unwrap().image_url, "");
    }

    #[test]
    fn the_queue_as_active_source_never_reads_current_media() {
        // the stale current_media (Teardrop) must not surface: the
        // queue owns the metadata
        let snapshot = parse_player(&queue_back(), BASE).unwrap();
        assert!(matches!(snapshot.source, PlayerSource::Queue));
        assert!(snapshot.has_media);
    }

    #[test]
    fn a_playing_reading_is_carried_forward_by_its_age() {
        // 0 s of a 179 s track, stamped at 1795350805.29
        let media = external(&airplay_track());
        let stamp = 1795350805.2879329;
        assert!((media.position_at(stamp + 30.0, true) - 30.0).abs() < 1e-6);
        // paused: frozen where MA left it
        assert_eq!(media.position_at(stamp + 30.0, false), 0.0);
        // a stamp from the future (clock skew) is taken as is
        assert_eq!(media.position_at(stamp - 5.0, true), 0.0);
        // older than the track is long: a frozen clock, not a position
        assert_eq!(media.position_at(stamp + 500.0, true), 0.0);
        // never past the end
        assert_eq!(media.position_at(stamp + 179.0, true), 179.0);
    }

    #[test]
    fn unstamped_readings_and_unknown_durations_are_taken_as_is() {
        let mut v = airplay_track();
        v["current_media"]["elapsed_time"] = 7.into();
        v["current_media"]["elapsed_time_last_updated"] = Value::Null;
        assert_eq!(external(&v).position_at(1795350900.0, true), 7.0);
        // the placeholder has no track, hence no duration
        let media = external(&placeholder());
        assert_eq!(media.position_at(1795350321.0 + 10.0, true), 62.0);
    }

    #[test]
    fn the_reading_key_changes_only_with_a_new_reading() {
        let a = external(&airplay_track());
        let same = external(&airplay_track());
        assert_eq!(a.reading_key(), same.reading_key());
        let mut v = airplay_track();
        v["current_media"]["elapsed_time_last_updated"] = 1795350900.5.into();
        assert_ne!(a.reading_key(), external(&v).reading_key());
    }
}
