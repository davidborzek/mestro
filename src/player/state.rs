//! The display's state machine — provider-agnostic. Guards against
//! incoherent event streams live here (stale replays, seek
//! confirmation, loading timeouts); the provider only supplies
//! resolved Transport/Track/Position events.

use crate::player::cover::{load_cover, make_blur, CoverData};
use crate::providers::{QueueItem, RepeatMode, TrackMeta};
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
    /// (when, value) of a locally initiated volume change — the
    /// transport echo that matches it is OUR command confirming,
    /// anything else is a remote change (OSD material)
    pub volume_local: Option<(Instant, i32)>,
}

impl Live {
    pub(crate) fn new(state: PlayerState) -> Self {
        Self {
            state,
            at: Instant::now(),
            created_at: Instant::now(),
            loading_since: None,
            pending_pos: None,
            pending_seek: None,
            recent_switch: None,
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
) {
    let (st, toast, osd_volume) = {
        let mut g = shared.lock().unwrap();
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
                        at.elapsed() < Duration::from_secs(4) && v == volume
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
                live.at = Instant::now();
                (live.state.clone(), toast, osd_volume)
            }
            None => {
                let state = PlayerState {
                    playing,
                    volume,
                    powered,
                    ..Default::default()
                };
                *g = Some(Live::new(state.clone()));
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
pub fn apply_options(shared: &Shared, update_tx: &UpdateTx, shuffle: bool, repeat: RepeatMode) {
    let st = {
        let mut g = shared.lock().unwrap();
        match g.as_mut() {
            Some(live) => {
                live.state.shuffle = shuffle;
                live.state.repeat = repeat;
                live.at = Instant::now();
                live.state.clone()
            }
            None => {
                let state = PlayerState {
                    shuffle,
                    repeat,
                    ..Default::default()
                };
                *g = Some(Live::new(state.clone()));
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
) {
    // toast on additions: entries whose queue_item_id is new
    // since the last queue snapshot (reorders stay silent); an
    // empty previous queue is the first snapshot (startup) — no
    // toast for everything
    let mut toast = None;
    {
        let g = shared.lock().unwrap();
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
        let mut g = shared.lock().unwrap();
        match g.as_mut() {
            Some(live) => {
                live.state.queue = items;
                live.state.queue_current = current_index;
                // an empty queue means the player went idle (queue
                // cleared): the media state must not linger on the
                // last track
                if live.state.queue.is_empty() {
                    *last_cover = String::new();
                    reset_media(&mut live.state);
                    live.loading_since = None;
                }
                live.at = Instant::now();
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
                *g = Some(Live::new(state.clone()));
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
) {
    let (update, changed) = {
        let mut g = shared.lock().unwrap();
        let Some(live) = g.as_mut() else { return };
        // empty metadata = no media: reset to the idle state; the
        // normal path below only ever sees non-empty tracks
        if track.title.is_empty() && track.artist.is_empty() {
            *last_cover = String::new();
            reset_media(&mut live.state);
            live.loading_since = None;
            live.pending_seek = None;
            live.pending_pos = None;
            live.at = Instant::now();
            (Some(live.state.clone()), false)
        } else {
            let changed = !track.title.is_empty() && live.state.title != track.title;
            if changed {
                // stale-replay guard: ignore a revert to the title we
                // switched away from within the window
                if let Some((since, prev)) = &live.recent_switch {
                    if since.elapsed() < Duration::from_secs(3) && track.title == *prev {
                        return;
                    }
                }
                live.recent_switch = Some((Instant::now(), live.state.title.clone()));
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
            live.state.has_media = true;
            live.at = Instant::now();
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
    // cover: download whenever the url differs from the last one
    // FETCHED — never gate on the shared state, it seeds no download
    if !track.image_url.is_empty() && track.image_url != *last_cover {
        *last_cover = track.image_url.clone();
        let cover_tx = update_tx.clone();
        let url = track.image_url.clone();
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
            if let Some(st) = shared.lock().unwrap().as_ref().map(|l| l.state.clone()) {
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

/// Position events → corrections. Coherence-guarded: a correction
/// disagreeing with the interpolation by >2.5s is held back until the
/// next correction confirms the new position — one-off outliers are
/// stale replays, a consistent new position is a seek.
pub fn apply_position(shared: &Shared, update_tx: &UpdateTx, secs: f64) {
    let st = {
        let mut g = shared.lock().unwrap();
        let Some(live) = g.as_mut() else { return };
        if !secs.is_finite() || secs < 0.0 {
            return;
        }
        let new_ms = (secs * 1000.0) as u64;
        let seek_confirmed = if let Some((since, target)) = live.pending_seek {
            if since.elapsed() < Duration::from_secs(5) {
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
        if seek_confirmed || live.created_at.elapsed() < Duration::from_secs(2) || off <= 2_500 {
            live.pending_pos = None;
            live.state.position_ms = new_ms;
        } else {
            let confirms = live.pending_pos.as_ref().is_some_and(|(since, pending)| {
                since.elapsed() < Duration::from_secs(5) && new_ms.abs_diff(*pending) <= 2_500
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
                live.pending_pos = Some((Instant::now(), new_ms));
                return;
            }
        }
        live.at = Instant::now();
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
pub fn mark_loading(shared: &Shared, update_tx: &UpdateTx) {
    let mut g = shared.lock().unwrap();
    if let Some(live) = g.as_mut() {
        live.state.loading = true;
        live.loading_since = Some(Instant::now());
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
pub fn mark_volume_local(shared: &Shared, v: i32) {
    let mut g = shared.lock().unwrap();
    if let Some(live) = g.as_mut() {
        live.volume_local = Some((Instant::now(), v));
    }
}

/// Ticker mutation: the loading timeout backstop and the position
/// interpolation. Returns the render tuple (pos, dur, loading).
/// Advance only happens with a known duration — min(0) would pin
/// radio streams to zero between corrections.
pub fn advance(shared: &Shared) -> Option<(u64, u64, bool)> {
    let mut g = shared.lock().unwrap();
    let live = g.as_mut()?;
    // loading timeout BEFORE everything else: during loading the
    // duration is often 0, which would skip this check
    if live.state.loading {
        if let Some(ls) = live.loading_since {
            if ls.elapsed() > Duration::from_secs(6) {
                live.state.loading = false;
                live.loading_since = None;
            }
        }
    }
    if live.state.playing && live.state.duration_ms > 0 {
        live.state.position_ms = (live.state.position_ms + live.at.elapsed().as_millis() as u64)
            .min(live.state.duration_ms);
        live.at = Instant::now();
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
        let mut live = Live::new(PlayerState {
            title: "Track".into(),
            has_media: true,
            duration_ms: 120_000,
            position_ms: 10_000,
            ..Default::default()
        });
        live.created_at = Instant::now() - Duration::from_secs(10);
        live.pending_seek = Some((Instant::now(), 60_000));
        live
    }
    #[test]
    fn requested_seek_ignores_old_echo_and_accepts_first_matching_confirmation() {
        let shared = Arc::new(Mutex::new(Some(live())));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        apply_position(&shared, &tx, 11.0);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            shared.lock().unwrap().as_ref().unwrap().state.position_ms,
            10_000
        );
        apply_position(&shared, &tx, 60.0);
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
        apply_position(&shared, &tx, 12.0);
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
        );
        let guard = shared.lock().unwrap();
        let live = guard.as_ref().unwrap();
        assert_eq!(live.state.duration_ms, 0);
        assert!(live.pending_seek.is_none());
    }
}
