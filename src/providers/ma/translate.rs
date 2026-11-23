//! Wire frames → `SourceEvent`s: the one place both the session loop
//! and the scenario harness turn parsed MA payloads into what the
//! core sees. Matching a frame to the display's player or queue
//! stays with the caller.

use super::external::ExternalTracker;
use super::parse::{parse_queue_item, ActiveQueue, PlayerSnapshot};
use crate::providers::{QueueItem, RepeatMode, SourceEvent, TrackMeta};
use serde_json::Value;

pub(super) fn transport(player: &PlayerSnapshot) -> SourceEvent {
    SourceEvent::Transport {
        playing: player.playing,
        volume: player.volume,
        powered: player.powered,
    }
}

/// a `player_updated` of the display's player: the transport, then
/// whatever the external tracker resolves. The flag says the player
/// just left an external source (the queue needs a re-seed).
pub(super) fn player_updated(
    player: &PlayerSnapshot,
    external: &mut ExternalTracker,
    now_unix: f64,
) -> (Vec<SourceEvent>, bool) {
    let mut events = vec![transport(player)];
    let step = external.observe(player, now_unix);
    events.extend(step.events);
    (events, step.back_on_queue)
}

/// a `queue_updated` of the display's queue; `track` is None on a
/// cleared queue. Shuffle/repeat ride every queue event.
pub(super) fn queue_updated(
    track: Option<TrackMeta>,
    elapsed_sec: f64,
    shuffle: bool,
    repeat: RepeatMode,
) -> Vec<SourceEvent> {
    let mut events = Vec::new();
    if let Some(track) = track {
        events.push(SourceEvent::Track { track, elapsed_sec });
    }
    events.push(SourceEvent::Options { shuffle, repeat });
    events
}

/// the reply to `player_queues/get_active_queue`
pub(super) fn queue_seed(active: &ActiveQueue) -> Vec<SourceEvent> {
    vec![
        SourceEvent::Position(active.elapsed_sec),
        SourceEvent::QueueSeed {
            track: active.track.clone(),
            elapsed_sec: active.elapsed_sec,
        },
        SourceEvent::Options {
            shuffle: active.shuffle,
            repeat: active.repeat,
        },
    ]
}

/// the reply to `player_queues/items`. `current_index` counts positions
/// in exactly this array — the queue position IS the index, never MA's
/// sort_index (insertion order: it diverges under shuffle/reorders,
/// putting history rows above the current row and losing or
/// multi-marking the active row)
pub(super) fn queue_items(items: &[Value], img_base: &str, current_index: usize) -> SourceEvent {
    let items = items
        .iter()
        .enumerate()
        .filter_map(|(pos, it)| {
            let mut q = parse_queue_item(it, img_base)?;
            q.sort_index = pos;
            Some(q)
        })
        .collect::<Vec<QueueItem>>();
    SourceEvent::Queue {
        items,
        current_index,
    }
}
