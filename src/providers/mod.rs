//! Playback providers — the abstraction boundary between mestro's core
//! (state machine + UI) and a remote control protocol (Music Assistant,
//! later LMS/MPD).

mod banner;
pub mod ma;
pub mod session;
pub mod startup;

use std::future::Future;

/// Queue-owned track metadata. The source of truth for what is playing;
/// a provider must never emit stale values here — during a skip the
/// player-side metadata lags behind the queue-side.
#[derive(Clone, Default, Debug)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub image_url: String,
    /// codec name for the badge, e.g. "FLAC"
    pub codec: String,
    /// stream parts, preformatted (empty = not reported):
    /// rate "44.1 kHz", depth "16 bit", bitrate "320 kbps",
    /// channels "2 ch" — [now-playing] quality picks + orders
    /// them for display
    pub rate: String,
    pub depth: String,
    pub bitrate: String,
    pub channels: String,
    /// codec is a lossless format — badge renders accent-filled
    pub lossless: bool,
    pub duration_ms: u64,
}

/// One entry of the player's queue, provider-neutral.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    /// stable id of the queue entry (survives reordering —
    /// what a tap on the row sends back)
    pub queue_item_id: String,
    /// position in the queue SNAPSHOT (the array index of the
    /// items reply — the same space the queue's current_index
    /// counts in; not MA's insertion-order sort_index)
    pub sort_index: usize,
    pub title: String,
    pub artist: String,
    pub duration_sec: f64,
    /// go-to navigation references (provider-scoped ids)
    pub provider: String,
    pub artist_id: String,
    pub artist_uri: String,
    pub album_id: String,
    pub album_name: String,
    pub album_uri: String,
    /// the item's artwork — feeds the go-to-album subject header
    pub image_url: String,
}

/// One entry of a library browse listing, provider-neutral.
/// Folders carry the browse path; playable items carry item_id,
/// provider and uri. Tracks and albums carry references to their
/// artist/album for go-to navigation.
#[derive(Clone, Debug)]
pub struct MediaEntry {
    pub name: String,
    /// "folder" | "artist" | "album" | "track" | "playlist" | ...
    pub kind: String,
    /// browse path for folders/categories, empty for items
    pub path: String,
    pub item_id: String,
    pub provider: String,
    /// provider-scoped play URI
    pub uri: String,
    pub image_url: String,
    /// primary artist (albums), empty otherwise
    pub artist: String,
    /// release year as string (albums), empty otherwise
    pub year: String,
    /// seconds; 0 when unknown
    pub duration_sec: f64,
    /// artist reference (name + id + uri) for go-to-artist
    pub artist_id: String,
    pub artist_uri: String,
    /// album reference for go-to-album (tracks)
    pub album_id: String,
    pub album_name: String,
    pub album_uri: String,
}

/// How play_media enqueues the media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enqueue {
    /// the server's configured default behavior
    Default,
    /// insert right after the playing item
    Next,
    /// append at the end of the queue
    Add,
}

/// queue-level repeat state as MA reports it
/// (player_queues/repeat: off | all | one)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    /// parse MA's wire string; anything unknown is off
    pub fn parse(s: &str) -> Self {
        match s {
            "all" => Self::All,
            "one" => Self::One,
            _ => Self::Off,
        }
    }
    /// the MA wire string
    pub fn wire(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
            Self::One => "one",
        }
    }
}

/// Transport-level command, already resolved (TogglePlay becomes
/// Play or Pause at the call site — the provider does not know the
/// current state).
#[derive(Debug, PartialEq)]
pub enum SourceCommand {
    Play,
    Pause,
    Next,
    Prev,
    Volume(i32),
    /// absolute position in seconds in the active queue item
    Seek(u64),
    /// power the active player on/off
    Power(bool),
    /// play the queue entry with this queue_item_id
    PlayQueueItem(String),
    /// remove the queue entry with this queue_item_id
    RemoveQueueItem(String),
    /// empty the queue (stops playback)
    ClearQueue,
    /// drop the session and reconnect NOW — the connection
    /// settings changed (url/token/player), the retry loop's
    /// factory re-reads them per attempt
    Reconnect,
    /// browse a library path ("root" = provider list)
    Browse(String),
    /// search the library (music/search)
    Search(String),
    /// albums of an artist
    ArtistAlbums {
        item_id: String,
        provider: String,
    },
    /// tracks of an album
    AlbumTracks {
        item_id: String,
        provider: String,
    },
    /// tracks of a playlist
    PlaylistTracks {
        item_id: String,
        provider: String,
    },
    /// play a media item (uri) on the player's queue
    PlayMedia {
        uri: String,
        enqueue: Enqueue,
    },
    /// replace the queue with shuffled media, including its first track
    PlayMediaShuffled {
        uri: String,
    },
    /// play a container (album/playlist uri) starting at a given
    /// item — the tapped track keeps its context
    PlayMediaFrom {
        media: String,
        start_item: String,
    },
    /// queue-level shuffle toggle
    Shuffle(bool),
    /// queue-level repeat
    Repeat(RepeatMode),
}

/// Events for the track being displayed — already resolved to the
/// active player; the core never sees player or queue ids.
#[derive(Debug)]
pub enum SourceEvent {
    /// transport snapshot of the active player
    Transport {
        playing: bool,
        volume: i32,
        powered: bool,
    },
    /// full metadata for the current track + the source's own position
    /// for it (natural advances can surface long after the track
    /// actually started, so always trust the carried position)
    Track { track: TrackMeta, elapsed_sec: f64 },
    /// position correction for the current track
    Position(f64),
    /// full queue snapshot; current_index points at the playing
    /// entry (usize::MAX = none of them / unknown)
    Queue {
        items: Vec<QueueItem>,
        current_index: usize,
    },
    /// playback options (shuffle/repeat) resolved to the active
    /// player's queue — rides queue events and the initial snapshot
    Options { shuffle: bool, repeat: RepeatMode },
    /// library listing reply, keyed by the request (browse path or
    /// drill key) so stale answers can be dropped
    Browse {
        key: String,
        entries: Vec<MediaEntry>,
    },
    /// search results, keyed by the query (stale answers drop)
    Search {
        key: String,
        entries: Vec<MediaEntry>,
    },
    /// late-arriving image for a detail subject (the artist image
    /// comes from a chained artists/get alongside the albums)
    SubjectImage { key: String, image_url: String },
}

/// How the retry loop reacts to one failed attempt: the banner
/// message and the backoff until the next attempt. The provider
/// owns the whole policy — MA knows its auth failures never heal
/// in-process (60s poll) while transport errors are transient
/// (5s); a provider without credentials just returns a short
/// backoff for everything.
pub struct Retry {
    pub message: String,
    pub backoff: std::time::Duration,
    /// a later attempt can succeed (network, server restart) — the
    /// banner waits out a grace window; false shows it at once
    pub heals_on_retry: bool,
}

/// A playback source drives one display session. `run` consumes
/// commands and emits events until the connection dies; the caller
/// owns reconnecting (call `run` again with the same channels).
pub trait PlaybackSource {
    fn name(&self) -> &'static str;

    fn run(
        &mut self,
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<SourceCommand>,
        events: tokio::sync::mpsc::UnboundedSender<SourceEvent>,
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send;

    /// judge one failed attempt: banner message + backoff (log the
    /// error here too — severity is provider knowledge)
    fn failure(&self, error: Box<dyn std::error::Error + Send + Sync>) -> Retry;

    /// enough configuration to dial at all — an unconfigured
    /// source idles instead of hammering a bogus endpoint (the
    /// first-run state)
    fn configured(&self) -> bool {
        true
    }
}
