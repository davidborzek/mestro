//! The display's state machine — provider-agnostic. Guards against
//! incoherent event streams live here (stale replays, seek
//! confirmation, loading timeouts); the provider only supplies
//! resolved Transport/Track/Position events.

use crate::locks;
use crate::player::cover::{load_cover, make_blur, CoverData};
use crate::providers::{ExternalSource, QueueItem, RepeatMode, SourceCaps, TrackMeta};
use std::sync::{Arc, Mutex};
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
    /// artwork colors ([theme] palette = "cover"), the M3
    /// tonalized accent/tertiary/danger trio
    pub palette: Option<crate::ui::palette::ArtworkPalette>,
    /// transient notification for the OSD layer; Transport and
    /// Track toasts are suppressed while the now-playing overlay
    /// is open (its UI already shows the state), Queue always
    /// shows
    pub toast: Option<crate::player::state::Toast>,
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
    pub(crate) fn new(state: PlayerState, now: Instant) -> Self {
        Self {
            state,
            at: now,
            created_at: now,
            loading_since: None,
            pending_pos: None,
            pending_seek: None,
            recent_switch: None,
            cover_wanted: String::new(),
            queue_title_before_external: None,
            volume_local: None,
        }
    }
}

pub type Shared = Arc<Mutex<Option<Live>>>;

pub type UpdateTx = tokio::sync::mpsc::UnboundedSender<PlayerUpdate>;

/// Transport events → playing/volume only. Track identity is owned
/// by Track events; a transport event must never touch it.
/// mm:ss for durations and positions
pub fn mmss(ms: u64) -> String {
    format!("{}:{:02}", ms / 60000, (ms / 1000) % 60)
}

pub fn apply_transport(
    shared: &Shared,
    update_tx: &UpdateTx,
    playing: bool,
    volume: i32,
    powered: bool,
    now: Instant,
) {
    let (st, toast, osd_volume) = {
        let mut g = locks::lock(shared);
        match g.as_mut() {
            Some(live) => {
                // play/pause transition toast — whoever pressed it
                let toast = if live.state.has_media && live.state.playing != playing {
                    Some(crate::player::state::Toast::Transport(
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
                (live.state.clone(), toast, osd_volume)
            }
            None => {
                let state = PlayerState {
                    playing,
                    volume,
                    powered,
                    ..Default::default()
                };
                *g = Some(Live::new(state.clone(), now));
                (state, None, None)
            }
        }
    };
    let _ = update_tx.send(PlayerUpdate {
        state: st,
        cover: None,
        bg: None,
        palette: None,
        toast,
        osd_volume,
    });
}

/// shuffle/repeat state off a queue event → the transport + queue
/// header buttons. Same channel discipline: the UI applies the
/// state on its own thread.
pub fn apply_options(
    shared: &Shared,
    update_tx: &UpdateTx,
    shuffle: bool,
    repeat: RepeatMode,
    now: Instant,
) {
    let st = {
        let mut g = locks::lock(shared);
        match g.as_mut() {
            Some(live) => {
                // an external source reports its own state
                if live.state.external.is_some() {
                    return;
                }
                live.state.shuffle = shuffle;
                live.state.repeat = repeat;
                live.at = now;
                live.state.clone()
            }
            None => {
                let state = PlayerState {
                    shuffle,
                    repeat,
                    ..Default::default()
                };
                *g = Some(Live::new(state.clone(), now));
                state
            }
        }
    };
    let _ = update_tx.send(PlayerUpdate {
        state: st,
        cover: None,
        bg: None,
        palette: None,
        toast: None,
        osd_volume: None,
    });
}

/// Queue snapshot → list view data; current_index marks the playing
/// row. No guards needed: the snapshot is always fresher than what
/// the UI holds. An EMPTY queue means the player went idle (queue
/// cleared) — the media state resets so the blurred backdrop and
/// the metadata don't linger on the last track.
pub fn apply_queue(
    shared: &Shared,
    update_tx: &UpdateTx,
    last_cover: &mut String,
    items: Vec<QueueItem>,
    current_index: usize,
    now: Instant,
) {
    // toast on additions: entries whose queue_item_id is new
    // since the last queue snapshot (reorders stay silent); an
    // empty previous queue is the first snapshot (startup) — no
    // toast for everything
    let mut toast = None;
    {
        let g = locks::lock(shared);
        if let Some(live) = g.as_ref() {
            if !live.state.queue.is_empty() {
                let known: std::collections::HashSet<&str> = live
                    .state
                    .queue
                    .iter()
                    .map(|q| q.queue_item_id.as_str())
                    .collect();
                let added: Vec<&QueueItem> = items
                    .iter()
                    .filter(|q| !known.contains(q.queue_item_id.as_str()))
                    .collect();
                tracing::debug!("queue snapshot: {} items, {} new", items.len(), added.len());
                if let Some(first) = added.first() {
                    toast = Some(crate::player::state::Toast::Queue(if added.len() == 1 {
                        format!("{} — {}", first.title, first.artist)
                    } else {
                        format!("{} — {} (+{})", first.title, first.artist, added.len() - 1)
                    }));
                }
            }
        }
    }
    let st = {
        let mut g = locks::lock(shared);
        match g.as_mut() {
            Some(live) => {
                live.state.queue = items;
                live.state.queue_current = current_index;
                // an empty queue means the player went idle (queue
                // cleared): the media state must not linger on the
                // last track — unless an external source plays, whose
                // track the queue knows nothing about
                if live.state.queue.is_empty() && live.state.external.is_none() {
                    live.cover_wanted = String::new();
                    *last_cover = String::new();
                    reset_media(&mut live.state);
                    live.loading_since = None;
                }
                live.at = now;
                live.state.clone()
            }
            None => {
                // transport has not arrived yet — park it so the first
                // transport event ships the queue too
                let state = PlayerState {
                    queue: items,
                    queue_current: current_index,
                    ..Default::default()
                };
                *g = Some(Live::new(state.clone(), now));
                state
            }
        }
    };
    let _ = update_tx.send(PlayerUpdate {
        state: st,
        cover: None,
        bg: None,
        palette: None,
        toast,
        osd_volume: None,
    });
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

/// Track events → full metadata (title/artist/album/quality/duration)
/// plus the position seed for a track change. The stale-replay guard
/// drops reverts to the title we just switched away from. An empty
/// track (cleared queue / idle player) resets to the empty state.
pub fn apply_track(
    shared: &Shared,
    update_tx: &UpdateTx,
    last_cover: &mut String,
    track: &TrackMeta,
    elapsed_sec: f64,
    variant: crate::ui::palette::Variant,
    now: Instant,
) {
    apply_queue_track(
        shared,
        update_tx,
        last_cover,
        track,
        elapsed_sec,
        variant,
        false,
        now,
    );
}

/// The queue's own answer to "what plays" (the reply to the active-queue
/// request, at startup and right after an external source ended) is
/// authoritative: the replay guard never rejects it, and it re-arms
/// the guard against the item the queue showed before it — a late
/// `queue_updated` may still replay that one.
pub fn apply_queue_seed(
    shared: &Shared,
    update_tx: &UpdateTx,
    last_cover: &mut String,
    track: &TrackMeta,
    elapsed_sec: f64,
    variant: crate::ui::palette::Variant,
    now: Instant,
) {
    apply_queue_track(
        shared,
        update_tx,
        last_cover,
        track,
        elapsed_sec,
        variant,
        true,
        now,
    );
}

#[allow(clippy::too_many_arguments)]
fn apply_queue_track(
    shared: &Shared,
    update_tx: &UpdateTx,
    last_cover: &mut String,
    track: &TrackMeta,
    elapsed_sec: f64,
    variant: crate::ui::palette::Variant,
    authoritative: bool,
    now: Instant,
) {
    let (update, changed) = {
        let mut g = locks::lock(shared);
        let Some(live) = g.as_mut() else { return };
        // an external source owns the metadata: queue events that
        // still arrive (a client editing the queue) say nothing
        // about what plays
        if live.state.external.is_some() {
            return;
        }
        // empty metadata = no media: reset to the idle state; the
        // normal path below only ever sees non-empty tracks
        if track.title.is_empty() && track.artist.is_empty() {
            live.cover_wanted = String::new();
            *last_cover = String::new();
            reset_media(&mut live.state);
            live.loading_since = None;
            live.pending_seek = None;
            live.pending_pos = None;
            live.at = now;
            (Some(live.state.clone()), false)
        } else {
            let changed = !track.title.is_empty() && live.state.title != track.title;
            if changed {
                if authoritative {
                    // what a late queue event could replay: the item before
                    // this one (back from an external source, the queue's
                    // own switch info survived it)
                    let replayable = match live.queue_title_before_external.take() {
                        Some(before) if before == track.title => {
                            live.recent_switch.take().map(|(_, prev)| prev)
                        }
                        Some(before) => Some(before),
                        None => Some(live.state.title.clone()),
                    };
                    live.recent_switch = replayable
                        .filter(|prev| !prev.is_empty())
                        .map(|prev| (now, prev));
                } else {
                    // stale-replay guard: ignore a revert to the title we
                    // switched away from within the window
                    if let Some((since, prev)) = &live.recent_switch {
                        if now.saturating_duration_since(*since) < Duration::from_secs(3)
                            && track.title == *prev
                        {
                            return;
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
            if !track.title.is_empty() {
                live.state.title = track.title.clone();
            }
            if !track.artist.is_empty() {
                live.state.artist = track.artist.clone();
            }
            if !track.album.is_empty() {
                live.state.album = track.album.clone();
            }
            if !track.codec.is_empty() {
                live.state.codec = track.codec.clone();
            }
            if !track.rate.is_empty() {
                live.state.rate = track.rate.clone();
            }
            if !track.depth.is_empty() {
                live.state.depth = track.depth.clone();
            }
            if !track.bitrate.is_empty() {
                live.state.bitrate = track.bitrate.clone();
            }
            if !track.channels.is_empty() {
                live.state.channels = track.channels.clone();
            }
            if !track.codec.is_empty() {
                live.state.lossless = track.lossless;
            }
            if track.duration_ms > 0 {
                live.state.duration_ms = track.duration_ms;
            }
            if changed || !track.image_url.is_empty() {
                live.cover_wanted = track.image_url.clone();
            }
            live.state.has_media = true;
            live.at = now;
            (Some(live.state.clone()), changed)
        }
    };
    if changed {
        tracing::info!("track -> {:?} — {:?}", track.title, track.artist);
    }
    // track-change toast: whoever skipped (mestro, remote, the
    // web UI, natural advance) — the display says what's on
    let toast = changed
        .then(|| crate::player::state::Toast::Track(format!("{} — {}", track.title, track.artist)));
    if let Some(st) = update {
        let _ = update_tx.send(PlayerUpdate {
            state: st,
            cover: None,
            bg: None,
            palette: None,
            toast,
            osd_volume: None,
        });
    }
    fetch_cover(
        shared,
        update_tx,
        last_cover,
        &track.image_url,
        false,
        variant,
    );
}

/// External source events → the source's own metadata, shown in
/// place of the queue's. The first event of a source (or another
/// source) wipes the queue's media first, so nothing of it lingers
/// under the new title. `track` None = MA has no metadata yet (its
/// placeholder titles never get here): the source is named, the
/// display stays empty. `elapsed_sec` seeds the bar on a title
/// change and corrects it otherwise — None while MA still reports
/// the previous track's clock (it lags a skip, like the queue).
#[allow(clippy::too_many_arguments)]
pub fn apply_external(
    shared: &Shared,
    update_tx: &UpdateTx,
    last_cover: &mut String,
    source: &ExternalSource,
    track: Option<&TrackMeta>,
    elapsed_sec: Option<f64>,
    variant: crate::ui::palette::Variant,
    now: Instant,
) {
    let (st, changed) = {
        let mut g = locks::lock(shared);
        let Some(live) = g.as_mut() else { return };
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
            *last_cover = String::new();
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
                *last_cover = String::new();
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
                    *last_cover = String::new();
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
        (live.state.clone(), changed)
    };
    if changed {
        tracing::info!("track ({}) -> {:?}", source.name, st.title);
    }
    let toast = changed.then(|| Toast::Track(format!("{} — {}", st.title, st.artist)));
    let _ = update_tx.send(PlayerUpdate {
        state: st,
        cover: None,
        bg: None,
        palette: None,
        toast,
        osd_volume: None,
    });
    if let Some(track) = track {
        fetch_cover(
            shared,
            update_tx,
            last_cover,
            &track.image_url,
            true,
            variant,
        );
    }
}

/// The player is back on the queue: the external track is wiped, the
/// queue's own events re-seed the display right after. A no-op when
/// no external source was shown.
pub fn apply_external_ended(
    shared: &Shared,
    update_tx: &UpdateTx,
    last_cover: &mut String,
    now: Instant,
) {
    let st = {
        let mut g = locks::lock(shared);
        let Some(live) = g.as_mut() else { return };
        if live.state.external.is_none() {
            return;
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
        *last_cover = String::new();
        live.state.clone()
    };
    let _ = update_tx.send(PlayerUpdate {
        state: st,
        cover: None,
        bg: None,
        palette: None,
        toast: None,
        osd_volume: None,
    });
}

/// cover: download whenever the url differs from the last one
/// FETCHED — never gate on the shared state, it seeds no download.
/// `external` says which mode asked: a result that lands after the
/// mode changed belongs to the other one and is dropped.
fn fetch_cover(
    shared: &Shared,
    update_tx: &UpdateTx,
    last_cover: &mut String,
    image_url: &str,
    external: bool,
    variant: crate::ui::palette::Variant,
) {
    if !image_url.is_empty() && image_url != *last_cover {
        *last_cover = image_url.to_string();
        let cover_tx = update_tx.clone();
        let url = image_url.to_string();
        let shared = shared.clone();
        tokio::task::spawn_blocking(move || {
            let mut cover = load_cover(&url);
            if let Some(c) = &mut cover {
                crate::player::cover::cap_size(c, 900);
            }
            tracing::debug!(
                "cover fetch {:?} -> {}",
                url,
                cover
                    .as_ref()
                    .map(|c| format!("{}x{}", c.width, c.height))
                    .unwrap_or_else(|| "FAIL".into())
            );
            let bg = make_blur(&cover);
            // read the state at SEND time, not spawn time: a slow cover
            // fetch must not resurrect a stale loading flag in the UI
            // (and a result that lands after the source mode flipped
            // belongs to the other mode: dropped)
            if let Some(st) = land_cover(&shared, &url, external, cover.is_some()) {
                let palette = cover
                    .as_ref()
                    .and_then(|c| crate::ui::palette::from_cover(c, variant));
                if let Some(p) = palette {
                    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
                    tracing::info!(
                        "palette: accent {} + tertiary {} + danger {}",
                        hex(p.accent),
                        hex(p.tertiary),
                        hex(p.danger)
                    );
                }
                let _ = cover_tx.send(PlayerUpdate {
                    state: st,
                    cover,
                    bg,
                    palette,
                    toast: None,
                    osd_volume: None,
                });
            }
        });
    }
}

/// A fetched cover reaches the state: the snapshot to publish, or
/// None when the result is stale: the source mode flipped, or the
/// shown track wants another url (no `cover_missing` flip, no publish).
fn land_cover(shared: &Shared, url: &str, external: bool, landed: bool) -> Option<PlayerState> {
    let mut g = locks::lock(shared);
    g.as_mut()
        .filter(|live| live.state.external.is_some() == external && live.cover_wanted == url)
        .map(|live| {
            // a landed cover ends the placeholder art; a failed
            // fetch keeps it rather than a stale cover
            if external || live.state.cover_missing {
                live.state.cover_missing = !landed;
            }
            live.state.clone()
        })
}

/// Position events → corrections. Coherence-guarded: a correction
/// disagreeing with the interpolation by >2.5s is held back until the
/// next correction confirms the new position — one-off outliers are
/// stale replays, a consistent new position is a seek.
pub fn apply_position(shared: &Shared, update_tx: &UpdateTx, secs: f64, now: Instant) {
    let st = {
        let mut g = locks::lock(shared);
        let Some(live) = g.as_mut() else { return };
        // the queue's clock says nothing about an external source
        if live.state.external.is_some() {
            return;
        }
        if !secs.is_finite() || secs < 0.0 {
            return;
        }
        let new_ms = (secs * 1000.0) as u64;
        let seek_confirmed = if let Some((since, target)) = live.pending_seek {
            if now.saturating_duration_since(since) < Duration::from_secs(5) {
                if new_ms.abs_diff(target) > 2_500 {
                    return;
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
                return;
            }
        }
        live.at = now;
        live.state.clone()
    };
    let _ = update_tx.send(PlayerUpdate {
        state: st,
        cover: None,
        bg: None,
        palette: None,
        toast: None,
        osd_volume: None,
    });
}

/// User command accepted → show loading until the source reports
/// "playing" again (or the 6s timeout in `advance` fires).
pub fn mark_loading(shared: &Shared, update_tx: &UpdateTx, now: Instant) {
    let mut g = locks::lock(shared);
    if let Some(live) = g.as_mut() {
        live.state.loading = true;
        live.loading_since = Some(now);
        let _ = update_tx.send(PlayerUpdate {
            state: live.state.clone(),
            cover: None,
            bg: None,
            palette: None,
            toast: None,
            osd_volume: None,
        });
    }
}

/// mark a locally initiated volume change (slider, control
/// channel): the transport echo matching it is OUR command
/// confirming — anything else is remote and OSD material
pub fn mark_volume_local(shared: &Shared, v: i32, now: Instant) {
    let mut g = locks::lock(shared);
    if let Some(live) = g.as_mut() {
        live.volume_local = Some((now, v));
    }
}

/// Ticker mutation: the loading timeout backstop and the position
/// interpolation. Returns the render tuple (pos, dur, loading).
/// Advance only happens with a known duration — min(0) would pin
/// radio streams to zero between corrections.
pub fn advance(shared: &Shared, now: Instant) -> Option<(u64, u64, bool)> {
    let mut g = locks::lock(shared);
    let live = g.as_mut()?;
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
    Some((
        live.state.position_ms,
        live.state.duration_ms,
        live.state.loading,
    ))
}

#[cfg(test)]
mod seek_tests {
    use super::*;
    fn live() -> Live {
        let mut live = Live::new(
            PlayerState {
                title: "Track".into(),
                has_media: true,
                duration_ms: 120_000,
                position_ms: 10_000,
                ..Default::default()
            },
            Instant::now(),
        );
        live.created_at = Instant::now() - Duration::from_secs(10);
        live.pending_seek = Some((Instant::now(), 60_000));
        live
    }
    #[test]
    fn requested_seek_ignores_old_echo_and_accepts_first_matching_confirmation() {
        let shared = Arc::new(Mutex::new(Some(live())));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        apply_position(&shared, &tx, 11.0, Instant::now());
        assert!(rx.try_recv().is_err());
        assert_eq!(
            locks::lock(&shared).as_ref().unwrap().state.position_ms,
            10_000
        );
        apply_position(&shared, &tx, 60.0, Instant::now());
        assert_eq!(rx.try_recv().unwrap().state.position_ms, 60_000);
        assert!(shared
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .pending_seek
            .is_none());
    }
    #[test]
    fn failed_seek_times_out_without_rewriting_playback_position() {
        let mut live = live();
        live.pending_seek = Some((Instant::now() - Duration::from_secs(6), 60_000));
        let shared = Arc::new(Mutex::new(Some(live)));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        apply_position(&shared, &tx, 12.0, Instant::now());
        assert_eq!(rx.try_recv().unwrap().state.position_ms, 12_000);
    }
    #[test]
    fn new_stream_does_not_inherit_previous_track_duration_or_seek_request() {
        let shared = Arc::new(Mutex::new(Some(live())));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        apply_track(
            &shared,
            &tx,
            &mut String::new(),
            &TrackMeta {
                title: "Radio".into(),
                ..Default::default()
            },
            0.0,
            crate::ui::palette::Variant::TonalSpot,
            Instant::now(),
        );
        let guard = locks::lock(&shared);
        let live = guard.as_ref().unwrap();
        assert_eq!(live.state.duration_ms, 0);
        assert!(live.pending_seek.is_none());
    }
}

#[cfg(test)]
mod external_tests {
    use super::*;

    const VARIANT: crate::ui::palette::Variant = crate::ui::palette::Variant::TonalSpot;

    fn shared_with(state: PlayerState) -> Shared {
        let mut live = Live::new(state, Instant::now());
        live.created_at = Instant::now() - Duration::from_secs(10);
        Arc::new(Mutex::new(Some(live)))
    }

    fn channel() -> (UpdateTx, tokio::sync::mpsc::UnboundedReceiver<PlayerUpdate>) {
        tokio::sync::mpsc::unbounded_channel()
    }

    fn track(title: &str, artist: &str) -> TrackMeta {
        TrackMeta {
            title: title.into(),
            artist: artist.into(),
            album: "Album".into(),
            duration_ms: 179_000,
            ..Default::default()
        }
    }

    fn snapshot(shared: &Shared) -> PlayerState {
        locks::lock(shared).as_ref().unwrap().state.clone()
    }

    fn queue_playing() -> Shared {
        shared_with(PlayerState {
            playing: true,
            title: "Teardrop".into(),
            artist: "Massive Attack".into(),
            album: "Mezzanine".into(),
            codec: "FLAC".into(),
            lossless: true,
            duration_ms: 330_000,
            position_ms: 59_000,
            has_media: true,
            ..Default::default()
        })
    }

    fn airplay() -> ExternalSource {
        ExternalSource {
            name: "AirPlay".into(),
            caps: SourceCaps {
                play_pause: false,
                seek: false,
                next_previous: false,
                shuffle: false,
                repeat: false,
                passive: true,
            },
            shuffle: false,
            repeat: RepeatMode::Off,
        }
    }

    fn external(shared: &Shared, tx: &UpdateTx, t: Option<&TrackMeta>, elapsed: Option<f64>) {
        apply_external(
            shared,
            tx,
            &mut String::new(),
            &airplay(),
            t,
            elapsed,
            VARIANT,
            Instant::now(),
        );
    }

    #[test]
    fn the_sources_caps_and_own_options_replace_the_queues_while_it_plays() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        assert_eq!(snapshot(&shared).source_caps, SourceCaps::default());
        let spotify = ExternalSource {
            name: "Spotify Connect".into(),
            caps: SourceCaps {
                play_pause: true,
                seek: true,
                next_previous: true,
                shuffle: true,
                repeat: false,
                passive: false,
            },
            shuffle: true,
            repeat: RepeatMode::All,
        };
        apply_external(
            &shared,
            &tx,
            &mut String::new(),
            &spotify,
            Some(&track("Song", "Band")),
            Some(1.0),
            VARIANT,
            Instant::now(),
        );
        let state = snapshot(&shared);
        assert_eq!(state.source_caps, spotify.caps);
        assert!(state.shuffle);
        assert_eq!(state.repeat, RepeatMode::All);
        // a stray queue event does not say what the source does
        apply_options(&shared, &tx, false, RepeatMode::Off, Instant::now());
        let state = snapshot(&shared);
        assert!(state.shuffle);
        assert_eq!(state.repeat, RepeatMode::All);
    }

    #[test]
    fn a_source_turning_a_capability_on_later_updates_the_state() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(&shared, &tx, None, None);
        assert!(!snapshot(&shared).source_caps.next_previous);
        let mut richer = airplay();
        richer.caps.next_previous = true;
        apply_external(
            &shared,
            &tx,
            &mut String::new(),
            &richer,
            None,
            None,
            VARIANT,
            Instant::now(),
        );
        assert!(snapshot(&shared).source_caps.next_previous);
    }

    #[test]
    fn the_queue_gets_all_its_abilities_back_when_the_source_ends() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(&shared, &tx, None, None);
        assert!(!snapshot(&shared).source_caps.play_pause);
        apply_external_ended(&shared, &tx, &mut String::new(), Instant::now());
        let state = snapshot(&shared);
        assert_eq!(state.source_caps, SourceCaps::default());
        apply_options(&shared, &tx, true, RepeatMode::One, Instant::now());
        assert!(snapshot(&shared).shuffle);
    }

    #[test]
    fn an_external_track_replaces_the_queue_metadata_and_names_its_source() {
        let shared = queue_playing();
        let (tx, mut rx) = channel();
        external(
            &shared,
            &tx,
            Some(&track("Stand by Me", "Ben E. King")),
            Some(12.0),
        );
        let state = snapshot(&shared);
        assert_eq!(state.external.as_deref(), Some("AirPlay"));
        assert_eq!(state.title, "Stand by Me");
        assert_eq!(state.artist, "Ben E. King");
        assert_eq!(state.album, "Album");
        assert_eq!(state.duration_ms, 179_000);
        assert_eq!(state.position_ms, 12_000);
        assert!(state.has_media);
        // the queue's quality line does not describe AirPlay audio
        assert!(state.codec.is_empty() && !state.lossless);
        assert!(matches!(
            rx.try_recv().unwrap().toast,
            Some(Toast::Track(text)) if text == "Stand by Me — Ben E. King"
        ));
    }

    #[test]
    fn a_placeholder_source_shows_no_media_but_still_names_the_source() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(&shared, &tx, None, None);
        let state = snapshot(&shared);
        assert_eq!(state.external.as_deref(), Some("AirPlay"));
        assert!(!state.has_media, "the queue's track must not linger");
        assert!(state.title.is_empty() && state.artist.is_empty());
    }

    #[tokio::test]
    async fn a_track_without_artwork_shows_the_placeholder_art() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(&shared, &tx, Some(&track("Song", "Band")), Some(0.0));
        assert!(snapshot(&shared).cover_missing);
        // artwork announced but not landed yet: still the placeholder,
        // never the queue's cover
        let with_art = TrackMeta {
            image_url: "http://127.0.0.1:9/imageproxy/x".into(),
            ..track("Song 2", "Band")
        };
        external(&shared, &tx, Some(&with_art), Some(0.0));
        assert!(snapshot(&shared).cover_missing);
    }

    #[test]
    fn the_queue_cannot_overwrite_the_external_track() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(
            &shared,
            &tx,
            Some(&track("Stand by Me", "Ben E. King")),
            Some(40.0),
        );
        // a stray queue event (a web client editing the queue)
        apply_track(
            &shared,
            &tx,
            &mut String::new(),
            &track("Teardrop", "Massive Attack"),
            3.0,
            VARIANT,
            Instant::now(),
        );
        apply_position(&shared, &tx, 3.0, Instant::now());
        apply_queue(
            &shared,
            &tx,
            &mut String::new(),
            Vec::new(),
            usize::MAX,
            Instant::now(),
        );
        let state = snapshot(&shared);
        assert_eq!(state.title, "Stand by Me");
        assert_eq!(state.position_ms, 40_000);
        assert!(
            state.has_media,
            "a cleared queue says nothing about AirPlay"
        );
    }

    #[test]
    fn a_skip_without_a_fresh_reading_restarts_the_bar() {
        // MA reports the new title seconds before the new elapsed
        // time: until it arrives, the old track's clock must not run on
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(&shared, &tx, Some(&track("First", "A")), Some(100.0));
        external(&shared, &tx, Some(&track("Second", "B")), None);
        let state = snapshot(&shared);
        assert_eq!(state.title, "Second");
        assert_eq!(state.position_ms, 0);
        // the unchanged track without a reading leaves the clock alone
        external(&shared, &tx, Some(&track("Second", "B")), None);
        assert_eq!(snapshot(&shared).position_ms, 0);
    }

    #[test]
    fn a_new_reading_is_authoritative_so_a_seek_on_the_phone_lands_at_once() {
        // the queue's clock is replayed stale and needs the outlier
        // guard; an external clock is read once per change
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(&shared, &tx, Some(&track("Song", "A")), Some(10.0));
        external(&shared, &tx, Some(&track("Song", "A")), Some(90.0));
        assert_eq!(snapshot(&shared).position_ms, 90_000);
        // a local seek request never outlives the source it was for
        locks::lock(&shared).as_mut().unwrap().pending_seek = Some((Instant::now(), 5_000));
        external(&shared, &tx, Some(&track("Song", "A")), Some(95.0));
        assert!(locks::lock(&shared)
            .as_ref()
            .unwrap()
            .pending_seek
            .is_none());
    }

    #[test]
    fn leaving_the_external_source_clears_it_and_hands_over_to_the_queue() {
        let shared = queue_playing();
        // the queue skipped to this very title a moment before AirPlay
        // took over: the stale-replay guard must not swallow the re-seed
        locks::lock(&shared).as_mut().unwrap().recent_switch =
            Some((Instant::now(), "Teardrop".into()));
        let (tx, _rx) = channel();
        external(&shared, &tx, Some(&track("Stand by Me", "B")), Some(1.0));
        apply_external_ended(&shared, &tx, &mut String::new(), Instant::now());
        let state = snapshot(&shared);
        assert_eq!(state.external, None);
        assert!(!state.has_media, "the external track must not linger");
        assert!(state.title.is_empty());

        apply_queue_seed(
            &shared,
            &tx,
            &mut String::new(),
            &track("Teardrop", "Massive Attack"),
            59.0,
            VARIANT,
            Instant::now(),
        );
        apply_position(&shared, &tx, 59.0, Instant::now());
        let state = snapshot(&shared);
        assert_eq!(state.title, "Teardrop");
        assert_eq!(state.position_ms, 59_000);
        assert!(state.has_media);
        // AirPlay's cover must not sit under the queue's title while
        // the queue's own is fetched
        assert!(state.cover_missing);
    }

    /// A→B on the queue, a short external episode, back: the re-seed
    /// shows B, and the queue replaying A (the item before the
    /// episode) right after it is stale
    #[test]
    fn a_late_queue_replay_after_the_reseed_is_dropped() {
        let shared = shared_with(PlayerState {
            title: "A".into(),
            has_media: true,
            ..Default::default()
        });
        let (tx, _rx) = channel();
        let mut cover = String::new();
        apply_track(
            &shared,
            &tx,
            &mut cover,
            &track("B", "X"),
            0.0,
            VARIANT,
            Instant::now(),
        );
        external(&shared, &tx, Some(&track("Stand by Me", "Ben")), Some(1.0));
        apply_external_ended(&shared, &tx, &mut cover, Instant::now());
        apply_queue_seed(
            &shared,
            &tx,
            &mut cover,
            &track("B", "X"),
            5.0,
            VARIANT,
            Instant::now(),
        );
        assert_eq!(snapshot(&shared).title, "B");
        // queue_updated still carries the item from before the episode
        apply_track(
            &shared,
            &tx,
            &mut cover,
            &track("A", "X"),
            5.5,
            VARIANT,
            Instant::now(),
        );
        assert_eq!(snapshot(&shared).title, "B");
        // a genuine skip is not a replay
        apply_track(
            &shared,
            &tx,
            &mut cover,
            &track("C", "X"),
            0.0,
            VARIANT,
            Instant::now(),
        );
        assert_eq!(snapshot(&shared).title, "C");
    }

    /// the queue's guard survives the episode, yet the re-seed is
    /// authoritative: it is never rejected as a "revert"
    #[test]
    fn the_reseed_is_never_rejected_by_the_replay_guard() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        let mut cover = String::new();
        // the queue had just skipped away from "Earlier" when AirPlay
        // took over, and it is back on exactly that title
        locks::lock(&shared).as_mut().unwrap().recent_switch =
            Some((Instant::now(), "Earlier".into()));
        external(&shared, &tx, Some(&track("Stand by Me", "Ben")), Some(1.0));
        apply_external_ended(&shared, &tx, &mut cover, Instant::now());
        apply_queue_seed(
            &shared,
            &tx,
            &mut cover,
            &track("Earlier", "X"),
            9.0,
            VARIANT,
            Instant::now(),
        );
        let state = snapshot(&shared);
        assert_eq!(state.title, "Earlier");
        assert_eq!(state.position_ms, 9_000);
    }

    /// a cover that lands after the shown track changed belongs to the
    /// old track: no flip of `cover_missing`, no publish
    #[tokio::test]
    async fn a_late_cover_of_the_previous_external_track_is_dropped() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        let url_a = "http://127.0.0.1:9/imageproxy/a";
        let with_art = TrackMeta {
            image_url: url_a.into(),
            ..track("Song A", "Band")
        };
        external(&shared, &tx, Some(&with_art), Some(0.0));
        // the fetch for A is in flight; AirPlay moves on to B, no artwork
        external(&shared, &tx, Some(&track("Song B", "Band")), Some(0.0));
        assert!(land_cover(&shared, url_a, true, true).is_none());
        let state = snapshot(&shared);
        assert_eq!(state.title, "Song B");
        assert!(state.cover_missing, "B keeps the placeholder");
    }

    #[test]
    fn a_cover_of_the_shown_track_still_lands() {
        let shared = queue_playing();
        let (tx, _rx) = channel();
        external(&shared, &tx, Some(&track("Song", "Band")), Some(0.0));
        locks::lock(&shared).as_mut().unwrap().cover_wanted = "http://x/a".into();
        let state = land_cover(&shared, "http://x/a", true, true).unwrap();
        assert!(!state.cover_missing);
    }

    #[test]
    fn leaving_a_source_that_was_never_active_changes_nothing() {
        let shared = queue_playing();
        let (tx, mut rx) = channel();
        apply_external_ended(&shared, &tx, &mut String::new(), Instant::now());
        assert_eq!(snapshot(&shared).title, "Teardrop");
        assert!(rx.try_recv().is_err());
    }

    /// the lag protection (the queue replaying the previous item
    /// after a skip) stays in force while no external source runs
    #[test]
    fn a_queue_skip_still_drops_the_replayed_previous_item() {
        let shared = shared_with(PlayerState {
            title: "Old".into(),
            has_media: true,
            ..Default::default()
        });
        let (tx, _rx) = channel();
        let mut cover = String::new();
        apply_track(
            &shared,
            &tx,
            &mut cover,
            &track("New", "A"),
            0.0,
            VARIANT,
            Instant::now(),
        );
        // ~1.5 s later the queue replays the previous item
        apply_track(
            &shared,
            &tx,
            &mut cover,
            &track("Old", "A"),
            9.0,
            VARIANT,
            Instant::now(),
        );
        let state = snapshot(&shared);
        assert_eq!(state.title, "New");
        assert_eq!(state.external, None);
    }
}
