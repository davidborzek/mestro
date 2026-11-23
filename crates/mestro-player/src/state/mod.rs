//! The display's state machine — provider-agnostic: `reduce` takes
//! the state, one event and the time and returns the effects the
//! shell (`player::shell`) has to carry out. It keeps no state outside
//! `Live`, takes no locks, spawns nothing and reads no clock (time
//! comes in as `now`); logging is allowed. Guards against incoherent
//! event streams live here (stale replays, seek confirmation, loading
//! timeouts); the provider only supplies resolved events.

use crate::cover::CoverData;
use crate::source::{ExternalSource, QueueItem, RepeatMode, SourceCaps, TrackMeta};
use std::time::{Duration, Instant};

/// Full UI snapshot — the only struct the UI renders from.
/// powered defaults to true: a UI that starts in standby would
/// flash the clock before the first transport event arrives.
#[derive(Clone, PartialEq)]
pub struct PlayerState {
    pub playing: bool,
    pub volume: i32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: String,
    /// stream parts, preformatted (empty = not reported) —
    /// assembled into the detail line per [now-playing] quality
    pub rate: String,
    pub depth: String,
    pub bitrate: String,
    pub channels: String,
    pub lossless: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub loading: bool,
    pub powered: bool,
    /// the player currently has media (cleared queue / idle → false,
    /// the UI renders its empty state)
    pub has_media: bool,
    /// queue snapshot for the list view; empty until the first
    /// Queue event arrives
    pub queue: Vec<QueueItem>,
    pub queue_current: usize,
    /// queue-level playback options — rendered by the transport
    /// and the queue header
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// the player plays an external source (AirPlay, Spotify
    /// Connect, ...) — its display name. The queue is silent then:
    /// title/artist/album/position come from the source itself, the
    /// queue view is off and the transport follows `source_caps`.
    /// None = the MA queue plays.
    pub external: Option<String>,
    /// what the playing source can do — the transport greys out the
    /// rest. The queue (external None) can do everything.
    pub source_caps: SourceCaps,
    /// the cover on screen is not the shown track's (no artwork, a
    /// failed fetch, or a source switch with the fetch still
    /// running): the UI paints its placeholder art instead
    pub cover_missing: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            playing: false,
            volume: 0,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            codec: String::new(),
            rate: String::new(),
            depth: String::new(),
            bitrate: String::new(),
            channels: String::new(),
            lossless: false,
            position_ms: 0,
            duration_ms: 0,
            loading: false,
            powered: true,
            has_media: false,
            queue: Vec::new(),
            queue_current: usize::MAX,
            shuffle: false,
            repeat: RepeatMode::default(),
            external: None,
            source_caps: SourceCaps::default(),
            cover_missing: false,
        }
    }
}

pub struct PlayerUpdate {
    pub state: PlayerState,
    pub cover: Option<CoverData>,
    pub bg: Option<CoverData>,
    /// the cover's dominant color, for skins that tint by
    /// artwork colors (`[theme]` palette = "cover"), the M3
    /// tonalized accent/tertiary/danger trio
    pub palette: Option<crate::palette::ArtworkPalette>,
    /// transient notification for the OSD layer; Transport and
    /// Track toasts are suppressed while the now-playing overlay
    /// is open (its UI already shows the state), Queue always
    /// shows
    pub toast: Option<crate::state::Toast>,
    /// a REMOTE volume change (transport-reported, not the echo
    /// of our own command) — the update loop shows the volume OSD
    pub osd_volume: Option<i32>,
}

/// one transient OSD notification
#[derive(Clone, Debug)]
pub enum Toast {
    /// play/pause transition — "Playing" / "Paused"
    Transport(String),
    /// track change — "Title — Artist"
    Track(String),
    /// queue addition — "Title — Artist (+n)"
    Queue(String),
}

pub struct Live {
    pub state: PlayerState,
    /// when the shared position was last touched (interpolation base)
    pub at: Instant,
    /// set once at bootstrap, never updated — the window in which
    /// position corrections are accepted unconditionally (the state
    /// can be seeded from arbitrarily stale snapshots)
    pub created_at: Instant,
    /// when the current loading phase started (timeout base)
    pub loading_since: Option<Instant>,
    /// a position correction that disagreed with the interpolation —
    /// held back until the next correction confirms which clock is real
    /// (a stale replay is a one-off outlier, a seek keeps reporting
    /// the new position)
    pub pending_pos: Option<(Instant, u64)>,
    /// requested position; ignore pre-seek echoes until confirmation or timeout
    pub pending_seek: Option<(Instant, u64)>,
    /// (when, previous title) — sources can replay the previous item
    /// after a skip; reverts inside this window are stale
    pub recent_switch: Option<(Instant, String)>,
    /// the artwork url the shown track expects ("" = none): a cover
    /// result for any other url belongs to a track no longer shown
    pub cover_wanted: String,
    /// the artwork url last FETCHED ("" = none, or forgotten so the
    /// same art fetches again) — a new fetch starts only when the
    /// shown track's url differs from it
    pub cover_requested: String,
    /// the title the queue showed when an external source took over:
    /// `recent_switch` stays the queue's own meanwhile, the re-seed
    /// after the source ends re-arms the replay guard from both
    pub queue_title_before_external: Option<String>,
    /// (when, value) of a locally initiated volume change — the
    /// transport echo that matches it is OUR command confirming,
    /// anything else is a remote change (OSD material)
    pub volume_local: Option<(Instant, i32)>,
}

impl Live {
    /// from a snapshot — the web server's tests construct one
    pub fn new(state: PlayerState, now: Instant) -> Self {
        Self {
            state,
            at: now,
            created_at: now,
            loading_since: None,
            pending_pos: None,
            pending_seek: None,
            recent_switch: None,
            cover_wanted: String::new(),
            cover_requested: String::new(),
            queue_title_before_external: None,
            volume_local: None,
        }
    }
}

impl Live {
    /// The state a first event creates when there is none yet. Only the
    /// events that describe the player itself carry enough to start from
    /// (the transport, the options, the queue): they publish what they
    /// made, with no toast and no OSD. Anything else has nothing to
    /// change yet and is dropped.
    pub(crate) fn bootstrap(event: Event, now: Instant) -> Option<(Self, Effects)> {
        let state = match event {
            Event::Transport {
                playing,
                volume,
                powered,
            } => PlayerState {
                playing,
                volume,
                powered,
                ..Default::default()
            },
            // transport has not arrived yet — park it so the first
            // transport event ships the queue too
            Event::Queue {
                items,
                current_index,
            } => PlayerState {
                queue: items,
                queue_current: current_index,
                ..Default::default()
            },
            Event::Options { shuffle, repeat } => PlayerState {
                shuffle,
                repeat,
                ..Default::default()
            },
            _ => return None,
        };
        let effects = Effects::snapshot(&state, None);
        Some((Self::new(state, now), effects))
    }
}

/// mm:ss for durations and positions
pub fn mmss(ms: u64) -> String {
    format!("{}:{:02}", ms / 60000, (ms / 1000) % 60)
}

/// Everything that changes the player state: the provider's events, what
/// the user does on the player, a fetched cover and the ticker.
#[derive(Debug)]
pub enum Event {
    /// Transport events → playing/volume only. Track identity is owned
    /// by Track events; a transport event must never touch it.
    Transport {
        playing: bool,
        volume: i32,
        powered: bool,
    },
    /// full metadata + the position seed for a track change, behind the
    /// stale-replay guard
    Track {
        track: TrackMeta,
        elapsed_sec: f64,
    },
    /// the queue's own answer to "what plays": authoritative, the replay
    /// guard never rejects it
    QueueSeed {
        track: TrackMeta,
        elapsed_sec: f64,
    },
    /// a position correction, coherence-guarded
    Position(f64),
    Queue {
        items: Vec<QueueItem>,
        current_index: usize,
    },
    Options {
        shuffle: bool,
        repeat: RepeatMode,
    },
    /// the source's own metadata, shown in place of the queue's
    External {
        source: ExternalSource,
        track: Option<TrackMeta>,
        elapsed_sec: Option<f64>,
    },
    /// the player is back on the queue
    ExternalEnded,
    /// a cover fetch finished; `external` is the mode that asked for it,
    /// `found` false = the download or decode failed
    CoverLanded {
        url: String,
        external: bool,
        found: bool,
    },
    /// a user command was accepted: show loading until the source
    /// reports "playing" again (or the 6s timeout in `Tick` fires)
    MarkLoading,
    /// a locally initiated volume change (slider, control channel): the
    /// transport echo matching it is OUR command confirming — anything
    /// else is remote and OSD material
    MarkVolumeLocal(i32),
    /// a seek was sent to the queue: ignore pre-seek echoes until it is
    /// confirmed or times out
    SeekRequested {
        seconds: u64,
    },
    /// the ticker: the loading timeout backstop and the position
    /// interpolation
    Tick,
}

/// What the shell has to do after an event.
#[derive(Default)]
pub struct Effects {
    pub publish: Option<Publish>,
    pub fetch_cover: Option<CoverRequest>,
}

/// a snapshot for the UI, with the notifications that go with it
pub struct Publish {
    pub state: PlayerState,
    pub toast: Option<Toast>,
    pub osd_volume: Option<i32>,
}

/// download this cover; its result comes back as `Event::CoverLanded`
pub struct CoverRequest {
    pub url: String,
    pub external: bool,
}

impl Effects {
    fn snapshot(state: &PlayerState, toast: Option<Toast>) -> Self {
        Self {
            publish: Some(Publish {
                state: state.clone(),
                toast,
                osd_volume: None,
            }),
            fetch_cover: None,
        }
    }

    fn fetching(mut self, request: Option<CoverRequest>) -> Self {
        self.fetch_cover = request;
        self
    }
}

/// One event at `now`: the new state in `live` and what to do about it.
pub fn reduce(live: &mut Live, event: Event, now: Instant) -> Effects {
    match event {
        Event::Transport {
            playing,
            volume,
            powered,
        } => transport(live, playing, volume, powered, now),
        Event::Track { track, elapsed_sec } => {
            queue_track(live, &track, elapsed_sec, Authority::Event, now)
        }
        Event::QueueSeed { track, elapsed_sec } => {
            queue_track(live, &track, elapsed_sec, Authority::Seed, now)
        }
        Event::Position(secs) => position(live, secs, now),
        Event::Queue {
            items,
            current_index,
        } => queue(live, items, current_index, now),
        Event::Options { shuffle, repeat } => options(live, shuffle, repeat, now),
        Event::External {
            source,
            track,
            elapsed_sec,
        } => external(live, &source, track.as_ref(), elapsed_sec, now),
        Event::ExternalEnded => external_ended(live, now),
        Event::CoverLanded {
            url,
            external,
            found,
        } => cover_landed(live, &url, external, found),
        Event::MarkLoading => mark_loading(live, now),
        Event::MarkVolumeLocal(volume) => {
            live.volume_local = Some((now, volume));
            Effects::default()
        }
        Event::SeekRequested { seconds } => {
            // an external source seeks on the player: no echo to guard
            if live.state.external.is_none() {
                live.pending_seek = Some((now, seconds * 1000));
                live.pending_pos = None;
            }
            Effects::default()
        }
        Event::Tick => tick(live, now),
    }
}

fn transport(live: &mut Live, playing: bool, volume: i32, powered: bool, now: Instant) -> Effects {
    // play/pause transition toast — whoever pressed it
    let toast = if live.state.has_media && live.state.playing != playing {
        Some(Toast::Transport(
            if playing { "Playing" } else { "Paused" }.to_string(),
        ))
    } else {
        None
    };
    // volume: a transport echo matching a recent local
    // set is OUR command confirming (marker consumed);
    // any other change is remote → OSD
    let mut osd_volume = None;
    if volume != live.state.volume {
        let echo = live.volume_local.is_some_and(|(at, v)| {
            now.saturating_duration_since(at) < Duration::from_secs(4) && v == volume
        });
        if echo {
            live.volume_local = None;
        } else {
            osd_volume = Some(volume);
        }
    }
    live.state.playing = playing;
    live.state.volume = volume;
    live.state.powered = powered;
    // the player reporting "playing" ends a command in
    // progress (mirrors MA's play_action_in_progress)
    if playing {
        live.state.loading = false;
        live.loading_since = None;
    }
    live.at = now;
    Effects {
        publish: Some(Publish {
            state: live.state.clone(),
            toast,
            osd_volume,
        }),
        fetch_cover: None,
    }
}

/// shuffle/repeat state off a queue event → the transport + queue
/// header buttons
fn options(live: &mut Live, shuffle: bool, repeat: RepeatMode, now: Instant) -> Effects {
    // an external source reports its own state
    if live.state.external.is_some() {
        return Effects::default();
    }
    live.state.shuffle = shuffle;
    live.state.repeat = repeat;
    live.at = now;
    Effects::snapshot(&live.state, None)
}

/// Queue snapshot → list view data; current_index marks the playing
/// row. No guards needed: the snapshot is always fresher than what
/// the UI holds. An EMPTY queue means the player went idle (queue
/// cleared) — the media state resets so the blurred backdrop and
/// the metadata don't linger on the last track.
fn queue(live: &mut Live, items: Vec<QueueItem>, current_index: usize, now: Instant) -> Effects {
    let toast = queue_addition_toast(&live.state.queue, &items);
    live.state.queue = items;
    live.state.queue_current = current_index;
    // an empty queue means the player went idle (queue
    // cleared): the media state must not linger on the
    // last track — unless an external source plays, whose
    // track the queue knows nothing about
    if live.state.queue.is_empty() && live.state.external.is_none() {
        go_idle(live);
    }
    live.at = now;
    Effects::snapshot(&live.state, toast)
}

/// toast on additions: entries whose queue_item_id is new
/// since the last queue snapshot (reorders stay silent); an
/// empty previous queue is the first snapshot (startup) — no
/// toast for everything
fn queue_addition_toast(known: &[QueueItem], items: &[QueueItem]) -> Option<Toast> {
    if known.is_empty() {
        return None;
    }
    let known: std::collections::HashSet<&str> =
        known.iter().map(|q| q.queue_item_id.as_str()).collect();
    let added: Vec<&QueueItem> = items
        .iter()
        .filter(|q| !known.contains(q.queue_item_id.as_str()))
        .collect();
    tracing::debug!("queue snapshot: {} items, {} new", items.len(), added.len());
    let first = added.first()?;
    Some(Toast::Queue(if added.len() == 1 {
        format!("{} — {}", first.title, first.artist)
    } else {
        format!("{} — {} (+{})", first.title, first.artist, added.len() - 1)
    }))
}

/// the queue has nothing to show: no media, no cover on its way
fn go_idle(live: &mut Live) {
    live.cover_wanted = String::new();
    live.cover_requested = String::new();
    reset_media(&mut live.state);
    live.loading_since = None;
}

/// wipe the current media: an idle player shows nothing, not the
/// leftovers of the last track
fn reset_media(state: &mut PlayerState) {
    state.title.clear();
    state.artist.clear();
    state.album.clear();
    state.codec.clear();
    state.rate.clear();
    state.depth.clear();
    state.bitrate.clear();
    state.channels.clear();
    state.lossless = false;
    state.duration_ms = 0;
    state.position_ms = 0;
    state.has_media = false;
    state.cover_missing = false;
    state.loading = false;
}

/// who is asking in `queue_track`: an ordinary queue event, or the
/// queue's own answer to the active-queue request (at startup and right
/// after an external source ended), which is authoritative: the replay
/// guard never rejects it, and it re-arms the guard against the item the
/// queue showed before it — a late `queue_updated` may still replay that one.
#[derive(Clone, Copy, PartialEq)]
enum Authority {
    Event,
    Seed,
}

/// Track events → full metadata (title/artist/album/quality/duration)
/// plus the position seed for a track change. The stale-replay guard
/// drops reverts to the title we just switched away from. An empty
/// track (cleared queue / idle player) resets to the empty state.
fn queue_track(
    live: &mut Live,
    track: &TrackMeta,
    elapsed_sec: f64,
    authority: Authority,
    now: Instant,
) -> Effects {
    // an external source owns the metadata: queue events that
    // still arrive (a client editing the queue) say nothing
    // about what plays
    if live.state.external.is_some() {
        return Effects::default();
    }
    // empty metadata = no media: reset to the idle state; the
    // normal path below only ever sees non-empty tracks
    if track.title.is_empty() && track.artist.is_empty() {
        go_idle(live);
        live.pending_seek = None;
        live.pending_pos = None;
        live.at = now;
        return Effects::snapshot(&live.state, None).fetching(cover_request(
            live,
            &track.image_url,
            false,
        ));
    }
    let changed = !track.title.is_empty() && live.state.title != track.title;
    if changed {
        if authority == Authority::Seed {
            rearm_replay_guard(live, track, now);
        } else {
            // stale-replay guard: ignore a revert to the title we
            // switched away from within the window
            if let Some((since, prev)) = &live.recent_switch {
                if now.saturating_duration_since(*since) < Duration::from_secs(3)
                    && track.title == *prev
                {
                    return Effects::default();
                }
            }
            live.recent_switch = Some((now, live.state.title.clone()));
        }
        // seed the bar from the event's own elapsed value — natural
        // advances can arrive seconds after the track started
        live.state.position_ms = (elapsed_sec * 1000.0) as u64;
        live.state.duration_ms = track.duration_ms;
        live.pending_seek = None;
        live.pending_pos = None;
    }
    adopt_queue_metadata(&mut live.state, track);
    if changed || !track.image_url.is_empty() {
        live.cover_wanted = track.image_url.clone();
    }
    live.state.has_media = true;
    live.at = now;
    if changed {
        tracing::info!("track -> {:?} — {:?}", track.title, track.artist);
    }
    // track-change toast: whoever skipped (mestro, remote, the
    // web UI, natural advance) — the display says what's on
    let toast = changed.then(|| Toast::Track(format!("{} — {}", track.title, track.artist)));
    Effects::snapshot(&live.state, toast).fetching(cover_request(live, &track.image_url, false))
}

/// what a late queue event could replay after the seed: the item before
/// this one (back from an external source, the queue's own switch info
/// survived it)
fn rearm_replay_guard(live: &mut Live, track: &TrackMeta, now: Instant) {
    let replayable = match live.queue_title_before_external.take() {
        Some(before) if before == track.title => live.recent_switch.take().map(|(_, prev)| prev),
        Some(before) => Some(before),
        None => Some(live.state.title.clone()),
    };
    live.recent_switch = replayable
        .filter(|prev| !prev.is_empty())
        .map(|prev| (now, prev));
}

/// the fields a queue event carries; what it leaves empty stays
fn adopt_queue_metadata(state: &mut PlayerState, track: &TrackMeta) {
    if !track.title.is_empty() {
        state.title = track.title.clone();
    }
    if !track.artist.is_empty() {
        state.artist = track.artist.clone();
    }
    if !track.album.is_empty() {
        state.album = track.album.clone();
    }
    if !track.codec.is_empty() {
        state.codec = track.codec.clone();
    }
    if !track.rate.is_empty() {
        state.rate = track.rate.clone();
    }
    if !track.depth.is_empty() {
        state.depth = track.depth.clone();
    }
    if !track.bitrate.is_empty() {
        state.bitrate = track.bitrate.clone();
    }
    if !track.channels.is_empty() {
        state.channels = track.channels.clone();
    }
    if !track.codec.is_empty() {
        state.lossless = track.lossless;
    }
    if track.duration_ms > 0 {
        state.duration_ms = track.duration_ms;
    }
}

/// External source events → the source's own metadata, shown in
/// place of the queue's. The first event of a source (or another
/// source) wipes the queue's media first, so nothing of it lingers
/// under the new title. `track` None = MA has no metadata yet (its
/// placeholder titles never get here): the source is named, the
/// display stays empty. `elapsed_sec` seeds the bar on a title
/// change and corrects it otherwise — None while MA still reports
/// the previous track's clock (it lags a skip, like the queue).
fn external(
    live: &mut Live,
    source: &ExternalSource,
    track: Option<&TrackMeta>,
    elapsed_sec: Option<f64>,
    now: Instant,
) -> Effects {
    if live.state.external.as_deref() != Some(source.name.as_str()) {
        tracing::info!("source -> {}", source.name);
        if live.state.external.is_none() {
            live.queue_title_before_external = Some(live.state.title.clone());
        }
        reset_media(&mut live.state);
        live.state.external = Some(source.name.clone());
        live.loading_since = None;
        live.pending_seek = None;
        live.pending_pos = None;
        live.cover_requested = String::new();
        // the queue's cover on screen is not this source's
        live.state.cover_missing = true;
    }
    // the source's own abilities and ordering state, as MA lists them
    live.state.source_caps = source.caps;
    live.state.shuffle = source.shuffle;
    live.state.repeat = source.repeat;
    let mut changed = false;
    match track {
        None => {
            reset_media(&mut live.state);
            live.state.cover_missing = true;
            live.cover_wanted = String::new();
            live.cover_requested = String::new();
        }
        Some(track) => {
            changed = live.state.title != track.title;
            live.cover_wanted = track.image_url.clone();
            live.state.title = track.title.clone();
            live.state.artist = track.artist.clone();
            live.state.album = track.album.clone();
            live.state.duration_ms = track.duration_ms;
            live.state.has_media = true;
            if track.image_url.is_empty() {
                live.state.cover_missing = true;
                // the same art coming back later must fetch again
                live.cover_requested = String::new();
            }
            // a reading is authoritative: nothing replays an
            // external clock, and a seek on the phone arrives
            // as ONE new reading (no stream of confirmations
            // like the queue's clock)
            if changed {
                live.state.position_ms = elapsed_sec.map_or(0, |s| (s * 1000.0) as u64);
            } else if let Some(secs) = elapsed_sec {
                live.state.position_ms = (secs * 1000.0) as u64;
            }
            if changed || elapsed_sec.is_some() {
                live.pending_seek = None;
                live.pending_pos = None;
            }
        }
    }
    live.at = now;
    if changed {
        tracing::info!("track ({}) -> {:?}", source.name, live.state.title);
    }
    let toast =
        changed.then(|| Toast::Track(format!("{} — {}", live.state.title, live.state.artist)));
    let request = track.and_then(|track| cover_request(live, &track.image_url, true));
    Effects::snapshot(&live.state, toast).fetching(request)
}

/// The player is back on the queue: the external track is wiped, the
/// queue's own events re-seed the display right after. A no-op when
/// no external source was shown.
fn external_ended(live: &mut Live, now: Instant) -> Effects {
    if live.state.external.is_none() {
        return Effects::default();
    }
    tracing::info!("source -> queue");
    reset_media(&mut live.state);
    live.state.external = None;
    // the queue's own options arrive with the re-seed
    live.state.source_caps = SourceCaps::default();
    live.state.shuffle = false;
    live.state.repeat = RepeatMode::Off;
    live.cover_wanted = String::new();
    // the external cover on screen is not the queue's
    live.state.cover_missing = true;
    live.loading_since = None;
    live.pending_seek = None;
    live.pending_pos = None;
    live.at = now;
    // the queue's cover is fetched again, whatever came before
    live.cover_requested = String::new();
    Effects::snapshot(&live.state, None)
}

/// cover: download whenever the url differs from the last one
/// FETCHED — never gate on the shown state, it seeds no download.
/// `external` says which mode asked: a result that lands after the
/// mode changed belongs to the other one and is dropped.
fn cover_request(live: &mut Live, url: &str, external: bool) -> Option<CoverRequest> {
    if url.is_empty() || url == live.cover_requested {
        return None;
    }
    live.cover_requested = url.to_string();
    Some(CoverRequest {
        url: url.to_string(),
        external,
    })
}

/// A fetched cover reaches the state: published, or dropped when the
/// result is stale: the source mode flipped, or the shown track wants
/// another url (no `cover_missing` flip, no publish).
fn cover_landed(live: &mut Live, url: &str, external: bool, found: bool) -> Effects {
    if live.state.external.is_some() != external || live.cover_wanted != url {
        return Effects::default();
    }
    // a landed cover ends the placeholder art; a failed
    // fetch keeps it rather than a stale cover
    if external || live.state.cover_missing {
        live.state.cover_missing = !found;
    }
    Effects::snapshot(&live.state, None)
}

/// Position events → corrections. Coherence-guarded: a correction
/// disagreeing with the interpolation by >2.5s is held back until the
/// next correction confirms the new position — one-off outliers are
/// stale replays, a consistent new position is a seek.
fn position(live: &mut Live, secs: f64, now: Instant) -> Effects {
    // the queue's clock says nothing about an external source
    if live.state.external.is_some() {
        return Effects::default();
    }
    if !secs.is_finite() || secs < 0.0 {
        return Effects::default();
    }
    let new_ms = (secs * 1000.0) as u64;
    let seek_confirmed = if let Some((since, target)) = live.pending_seek {
        if now.saturating_duration_since(since) < Duration::from_secs(5) {
            if new_ms.abs_diff(target) > 2_500 {
                return Effects::default();
            }
            live.pending_seek = None;
            true
        } else {
            live.pending_seek = None;
            false
        }
    } else {
        false
    };
    let off = new_ms.abs_diff(live.state.position_ms);
    if seek_confirmed
        || now.saturating_duration_since(live.created_at) < Duration::from_secs(2)
        || off <= 2_500
    {
        live.pending_pos = None;
        live.state.position_ms = new_ms;
    } else {
        let confirms = live.pending_pos.as_ref().is_some_and(|(since, pending)| {
            now.saturating_duration_since(*since) < Duration::from_secs(5)
                && new_ms.abs_diff(*pending) <= 2_500
        });
        if confirms {
            tracing::debug!("seek adopted -> {new_ms}");
            live.pending_pos = None;
            live.state.position_ms = new_ms;
        } else {
            tracing::debug!(
                "hold correction {new_ms} (interp {})",
                live.state.position_ms
            );
            live.pending_pos = Some((now, new_ms));
            return Effects::default();
        }
    }
    live.at = now;
    Effects::snapshot(&live.state, None)
}

/// User command accepted → show loading until the source reports
/// "playing" again (or the 6s timeout in `tick` fires).
fn mark_loading(live: &mut Live, now: Instant) -> Effects {
    live.state.loading = true;
    live.loading_since = Some(now);
    Effects::snapshot(&live.state, None)
}

/// Ticker mutation: the loading timeout backstop and the position
/// interpolation. Advance only happens with a known duration —
/// min(0) would pin radio streams to zero between corrections.
fn tick(live: &mut Live, now: Instant) -> Effects {
    // loading timeout BEFORE everything else: during loading the
    // duration is often 0, which would skip this check
    if live.state.loading {
        if let Some(ls) = live.loading_since {
            if now.saturating_duration_since(ls) > Duration::from_secs(6) {
                live.state.loading = false;
                live.loading_since = None;
            }
        }
    }
    if live.state.playing && live.state.duration_ms > 0 {
        live.state.position_ms = (live.state.position_ms
            + now.saturating_duration_since(live.at).as_millis() as u64)
            .min(live.state.duration_ms);
        live.at = now;
    }
    Effects::default()
}

#[cfg(test)]
mod tests;
