//! The session protocol loop: auth, request/response matching by
//! message_id, push-event dispatch and the player/queue matching
//! to the display's player.

use super::command::{command_for, command_frame};
use super::external::ExternalTracker;
use super::parse::{
    http_base_from_ws, parse_active_queue, parse_event, parse_media_entry, parse_player,
    parse_queue_item, MaEvent, PlayerSnapshot,
};
use super::AuthError;
use super::Config;
use crate::providers::{Enqueue, MediaEntry, QueueItem, SourceCommand, SourceEvent};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio_tungstenite::tungstenite::Message;

pub(super) async fn run_session(
    cfg: &Config,
    cmd_rx: &mut tokio::sync::mpsc::UnboundedReceiver<SourceCommand>,
    events: &tokio::sync::mpsc::UnboundedSender<SourceEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // (an unconfigured source never reaches this — the retry loop
    // idles on `configured()` instead of dialing)
    // bounded dial: a black-holed route must not swallow
    // Reconnects for minutes
    let (ws, _) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::connect_async(cfg.ws_url.clone()),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"))??;
    tracing::info!("ma: connected to {}", cfg.ws_url);
    let (mut sink, mut stream) = ws.split();
    let img_base = http_base_from_ws(&cfg.ws_url);

    let mut msg_id: u64 = 0;
    let mut matched_id: Option<String> = None;
    let mut players = Players::new();
    let mut active_queue_req: Option<u64> = None;
    let mut matched_queue: Option<String> = None;
    // pending player_queues/items request + the index it must ship
    let mut items_req: Option<u64> = None;
    let mut current_index: usize = usize::MAX;
    // pending library request: (message_id, reply key)
    let mut browse_req: Option<(u64, String)> = None;
    // pending music/search request + its query (the stale key)
    let mut search_req: Option<(u64, String)> = None;
    // chained artists/get alongside artist_albums: delivers the
    // artist's own image as a late SubjectImage event
    let mut artist_get_req: Option<(u64, String)> = None;
    let mut shuffle_play = super::shuffle::ShufflePlay::default();
    // the display player's external source (AirPlay & co.) — see external.rs
    let mut external = ExternalTracker::default();

    // first message must be auth (with a message_id — see module docs)
    sink.send(Message::Text(
        command_frame(msg_id, "auth", json!({ "token": cfg.token })).into(),
    ))
    .await?;
    msg_id += 1;
    sink.send(Message::Text(
        command_frame(msg_id, "players/all", json!({})).into(),
    ))
    .await?;
    msg_id += 1;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                // live rewire: the connection settings changed —
                // end the attempt, the retry loop re-opens with
                // the fresh config (make() reads it per attempt)
                if matches!(cmd, SourceCommand::Reconnect) { return Ok(()); }
                let Some(player_id) = matched_id.clone() else { continue };
                let queue_id = matched_queue.clone().unwrap_or_default();
                if let SourceCommand::PlayMediaShuffled { uri } = &cmd {
                    if queue_id.is_empty() || uri.is_empty() { continue; }
                    // Older MA ignores play_media's shuffle argument and reads queue state.
                    // Await its shuffle reply so resolution cannot race the setting change.
                    let (command, args) = command_for(&SourceCommand::Shuffle(true), &player_id, &queue_id);
                    shuffle_play.start(msg_id, queue_id, uri.clone());
                    sink.send(Message::Text(command_frame(msg_id, command, args).into())).await?;
                    msg_id += 1;
                    continue;
                }
                if matches!(&cmd, SourceCommand::Play | SourceCommand::Pause | SourceCommand::Next
                    | SourceCommand::Prev | SourceCommand::ClearQueue | SourceCommand::PlayMedia { enqueue: Enqueue::Default, .. }
                    | SourceCommand::PlayMediaFrom { .. } | SourceCommand::PlayQueueItem(_)
                    | SourceCommand::Power(false) | SourceCommand::Shuffle(_)) {
                    shuffle_play.cancel();
                }
                // library requests answer out of band and can be
                // slow (provider roundtrips) — remember message_id
                // and reply key so stale answers drop
                match &cmd {
                    SourceCommand::Browse(path) => {
                        browse_req = Some((msg_id, path.clone()));
                    }
                    SourceCommand::Search(query) => {
                        search_req = Some((msg_id, query.clone()));
                    }
                    SourceCommand::ArtistAlbums { item_id, provider } => {
                        browse_req = Some((msg_id, format!("artist-albums:{provider}:{item_id}")));
                    }                    SourceCommand::AlbumTracks { item_id, provider } => {
                        browse_req = Some((msg_id, format!("album-tracks:{provider}:{item_id}")));
                    }
                    SourceCommand::PlaylistTracks { item_id, provider } => {
                        browse_req = Some((msg_id, format!("playlist-tracks:{provider}:{item_id}")));
                    }
                    _ => {}
                }
                let (command, args) = command_for(&cmd, &player_id, &queue_id);
                sink.send(Message::Text(
                    command_frame(msg_id, command, args).into(),
                ))
                .await?;
                msg_id += 1;

                // artist drills chain artists/get for the artist's
                // own image — it arrives as a late SubjectImage,
                // keyed like the albums reply
                if let SourceCommand::ArtistAlbums { item_id, provider } = &cmd {
                    let key = format!("artist-albums:{provider}:{item_id}");
                    sink.send(Message::Text(
                        command_frame(
                            msg_id,
                            "music/artists/get",
                            json!({
                                "item_id": item_id,
                                "provider_instance_id_or_domain": provider,
                            }),
                        )
                        .into(),
                    ))
                    .await?;
                    artist_get_req = Some((msg_id, key));
                    msg_id += 1;
                }
            }
            msg = stream.next() => {
                let Some(msg) = msg else { return Ok(()) };
                let Message::Text(txt) = msg? else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };

                // every reply carries the request's message_id; auth
                // is id 0. A bad token never closes the socket — it
                // answers the auth frame with error_code 23 ("The
                // access token is invalid or has expired.") and 20
                // ("Authentication is required.") on all following
                // commands, while nothing else flows. Bail out so
                // the caller can surface it instead of hanging in a
                // zombie session
                if let Some(id) = v
                    .get("message_id")
                    .and_then(|m| m.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    if let Some(code) = v.get("error_code").and_then(|c| c.as_i64()) {
                        shuffle_play.reply(id, matched_queue.as_deref().unwrap_or(""), true);
                        let details = v
                            .get("details")
                            .and_then(|d| d.as_str())
                            .unwrap_or("unknown error")
                            .to_string();
                        if id == 0 || code == 20 {
                            return Err(Box::new(AuthError(details)));
                        }
                        tracing::warn!("ma: command {id} failed (code {code}): {details}");
                        // a failed request must not leave its UI
                        // spinner hanging: resolve the pending
                        // request slots with empty results
                        if let Some((pending_id, key)) = browse_req.clone() {
                            if id == pending_id {
                                browse_req = None;
                                let _ = events.send(SourceEvent::Browse {
                                    key,
                                    entries: Vec::new(),
                                });
                            }
                        }
                        if let Some((pending_id, key)) = search_req.clone() {
                            if id == pending_id {
                                search_req = None;
                                let _ = events.send(SourceEvent::Search {
                                    key,
                                    entries: Vec::new(),
                                });
                            }
                        }
                        continue;
                    }
                    if id == 0
                        && !v.pointer("/result/authenticated")
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false)
                    {
                        return Err(Box::new(AuthError(
                            "server rejected the token".into(),
                        )));
                    }
                    if let Some(command) = shuffle_play.reply(id, matched_queue.as_deref().unwrap_or(""), false) {
                        let queue_id = matched_queue.as_deref().unwrap_or("");
                        let (command, args) = command_for(&command, matched_id.as_deref().unwrap_or(""), queue_id);
                        sink.send(Message::Text(command_frame(msg_id, command, args).into())).await?;
                        msg_id += 1;
                    }
                }

                if let Some(ev) = parse_event(&v, &img_base) {
                    match ev {
                        MaEvent::Player(p) => {
                            remember_player(&mut players, &p);
                            let pick = try_pick(&mut matched_id, &players, &cfg.player_name, cfg.own_player_id.as_deref());
                            if pick == Pick::Switched {
                                // the old player's queue, source and pending
                                // requests are void: wipe the display, then
                                // seed it from the new player like a first pick
                                matched_queue = None;
                                items_req = None;
                                current_index = usize::MAX;
                                shuffle_play.cancel();
                                external = ExternalTracker::default();
                                forget_shown_player(events);
                                if let Some(pid) = matched_id.as_deref() {
                                    sink.send(Message::Text(command_frame(
                                        msg_id,
                                        "player_queues/get_active_queue",
                                        json!({ "player_id": pid }),
                                    ).into()))
                                    .await?;
                                    active_queue_req = Some(msg_id);
                                    msg_id += 1;
                                }
                                // the new player's transport and source: the
                                // players/all reply emits them for the match
                                sink.send(Message::Text(
                                    command_frame(msg_id, "players/all", json!({})).into(),
                                ))
                                .await?;
                                msg_id += 1;
                            }
                            let relevant = matched_id.as_deref() == Some(p.player_id.as_str());
                            if relevant {
                                let _ = events.send(SourceEvent::Transport {
                                    playing: p.playing,
                                    volume: p.volume,
                                    powered: p.powered,
                                });
                                let step = external.observe(&p, unix_now());
                                for event in step.events {
                                    let _ = events.send(event);
                                }
                                // back on the queue: its current item
                                // and list were not pushed while the
                                // source played — ask again, the reply
                                // re-seeds the display at once
                                if step.back_on_queue {
                                    if let Some(pid) = matched_id.as_deref() {
                                        sink.send(Message::Text(command_frame(
                                            msg_id,
                                            "player_queues/get_active_queue",
                                            json!({ "player_id": pid }),
                                        ).into()))
                                        .await?;
                                        active_queue_req = Some(msg_id);
                                        msg_id += 1;
                                    }
                                }
                            }
                        }
                        MaEvent::QueueTime { queue_id, elapsed_sec } => {
                            if matched_queue.as_deref() == Some(queue_id.as_str()) {
                                let _ = events.send(SourceEvent::Position(elapsed_sec));
                            }
                        }
                        MaEvent::Queue { queue_id, track, elapsed_sec, current_index: idx, shuffle, repeat } => {
                            if matched_queue.as_deref() == Some(queue_id.as_str()) {
                                current_index = idx;
                                if let Some(track) = track {
                                    let _ = events.send(SourceEvent::Track { track, elapsed_sec });
                                }
                                // shuffle/repeat state rides every
                                // queue event — keep the transport lit
                                tracing::debug!("ma: queue event shuffle={shuffle} repeat={repeat:?}");
                                let _ = events.send(SourceEvent::Options { shuffle, repeat });
                                // queue changed — the item list may
                                // have moved with it; refetch (one request
                                // in flight at a time)
                                if items_req.is_none() {
                                    sink.send(Message::Text(command_frame(
                                        msg_id,
                                        "player_queues/items",
                                        json!({ "queue_id": queue_id }),
                                    ).into()))
                                    .await?;
                                    items_req = Some(msg_id);
                                    msg_id += 1;
                                }
                            }
                        }
                    }
                    continue;
                }

                // reply to player_queues/get_active_queue: resolve the
                // queue of the matched player and seed the full state
                if let Some(want_id) = active_queue_req {
                    let replied = v
                        .get("message_id")
                        .and_then(|m| m.as_str())
                        .and_then(|s| s.parse::<u64>().ok());
                    if replied == Some(want_id) {
                        active_queue_req = None;
                        if let Some(aq) = parse_active_queue(&v, &img_base) {
                            matched_queue = Some(aq.queue_id.clone());
                            current_index = aq.current_index;
                            let _ = events.send(SourceEvent::Position(aq.elapsed_sec));
                            let _ = events.send(SourceEvent::QueueSeed {
                                track: aq.track,
                                elapsed_sec: aq.elapsed_sec,
                            });
                            let _ = events.send(SourceEvent::Options {
                                shuffle: aq.shuffle,
                                repeat: aq.repeat,
                            });
                            // seed the queue view: one full snapshot
                            sink.send(Message::Text(command_frame(
                                msg_id,
                                "player_queues/items",
                                json!({ "queue_id": aq.queue_id }),
                            ).into()))
                            .await?;
                            items_req = Some(msg_id);
                            msg_id += 1;
                        }
                    }
                }

                // chained artists/get: the artist's own image, sent
                // as a late event keyed like the albums reply. The
                // reply is an ARTIST object — no media_type — so
                // parse_media_entry (a track/album parser) yields
                // None here; read the image proxy directly
                if let Some((want_id, key)) = artist_get_req.clone() {
                    let replied = v
                        .get("message_id")
                        .and_then(|m| m.as_str())
                        .and_then(|s| s.parse::<u64>().ok());
                    if replied == Some(want_id) {
                        artist_get_req = None;
                        let pid = v
                            .pointer("/result/image/proxy_id")
                            .and_then(|p| p.as_str())
                            .or_else(|| {
                                v.pointer("/result/metadata/images/0/proxy_id")
                                    .and_then(|p| p.as_str())
                            })
                            .or_else(|| {
                                v.pointer("/result/images/0/proxy_id")
                                    .and_then(|p| p.as_str())
                            });
                        match pid {
                            Some(pid) => {
                                let _ = events.send(SourceEvent::SubjectImage {
                                    key: key.clone(),
                                    image_url: format!(
                                        "{img_base}/imageproxy/{pid}?size=256&fmt=jpg"
                                    ),
                                });
                            }
                            None => tracing::warn!(
                                "artists/get reply for {key} carries no image proxy_id"
                            ),
                        }
                    }
                }

                // reply to music/search: the result carries the
                // categories in one payload (tracks, albums,
                // artists — radio dropped); keyed by the query so
                // stale answers drop silently
                if let Some((want_id, key)) = search_req.clone() {
                    let replied = v
                        .get("message_id")
                        .and_then(|m| m.as_str())
                        .and_then(|s| s.parse::<u64>().ok());
                    if replied == Some(want_id) {
                        search_req = None;
                        if let Some(result) = v.get("result") {
                            let keep = |it: &Value| {
                                let kind =
                                    it.get("media_type").and_then(|k| k.as_str()).unwrap_or("");
                                matches!(kind, "track" | "album" | "artist")
                            };
                            let mut parsed: Vec<MediaEntry> = Vec::new();
                            for cat in ["tracks", "albums", "artists"] {
                                if let Some(items) =
                                    result.get(cat).and_then(|r| r.as_array())
                                {
                                    parsed.extend(
                                        items
                                            .iter()
                                            .filter(|it| keep(it))
                                            .filter_map(|it| parse_media_entry(it, &img_base)),
                                    );
                                }
                            }
                            let _ = events.send(SourceEvent::Search {
                                key: key.clone(),
                                entries: parsed,
                            });
                        }
                    }
                }

                // reply to a library browse/drill: keyed so stale
                // answers (slow provider!) drop silently
                if let Some((want_id, key)) = browse_req.clone() {
                    let replied = v
                        .get("message_id")
                        .and_then(|m| m.as_str())
                        .and_then(|s| s.parse::<u64>().ok());
                    if replied == Some(want_id) {
                        browse_req = None;
                        if let Some(items) = v.get("result").and_then(|r| r.as_array()) {
                            let parsed: Vec<MediaEntry> = items
                                .iter()
                                .filter_map(|it| parse_media_entry(it, &img_base))
                                .collect();
                            let _ = events.send(SourceEvent::Browse {
                                key: key.clone(),
                                entries: parsed,
                            });
                        }
                    }
                }

                // reply to player_queues/items: the queue snapshot
                if let Some(want_id) = items_req {
                    let replied = v
                        .get("message_id")
                        .and_then(|m| m.as_str())
                        .and_then(|s| s.parse::<u64>().ok());
                    if replied == Some(want_id) {
                        items_req = None;
                        if let Some(items) = v.get("result").and_then(|r| r.as_array()) {
                            // current_index counts positions in exactly
                            // this array — the queue position IS the index,
                            // never MA's sort_index (insertion order: it
                            // diverges under shuffle/reorders, putting
                            // history rows above the current row and
                            // losing or multi-marking the active row)
                            let parsed: Vec<QueueItem> = items
                                .iter()
                                .enumerate()
                                .filter_map(|(pos, it)| {
                                    let mut q = parse_queue_item(it, &img_base)?;
                                    q.sort_index = pos;
                                    Some(q)
                                })
                                .collect();
                            // an empty reply is valid — it is how a
                            // cleared queue reaches the UI
                            let _ = events.send(SourceEvent::Queue {
                                items: parsed,
                                current_index,
                            });
                        }
                    }
                }

                // reply to players/all: discovery + transport snapshot
                if let Some(result) = v.get("result").and_then(|r| r.as_array()) {
                    let snaps: Vec<PlayerSnapshot> =
                        result.iter().filter_map(|p| parse_player(p, &img_base)).collect();
                    for p in &snaps {
                        remember_player(&mut players, p);
                        let relevant = matched_id.as_deref() == Some(p.player_id.as_str());
                        if relevant {
                            let _ = events.send(SourceEvent::Transport {
                                playing: p.playing,
                                volume: p.volume,
                                powered: p.powered,
                            });
                        }
                    }
                    let pick = try_pick(&mut matched_id, &players, &cfg.player_name, cfg.own_player_id.as_deref());
                    if pick == Pick::Switched {
                        // wipe the old player; the block below emits the new
                        // one's state and the queue request after this follows
                        matched_queue = None;
                        active_queue_req = None;
                        items_req = None;
                        current_index = usize::MAX;
                        shuffle_play.cancel();
                        external = ExternalTracker::default();
                        forget_shown_player(events);
                    }
                    // initial fetch: the pick above happened only now, so
                    // the loop skipped the matched player — emit it once
                    if let Some(id) = matched_id.clone() {
                        if let Some(p) = snaps.iter().find(|p| p.player_id == id) {
                            let _ = events.send(SourceEvent::Transport {
                                playing: p.playing,
                                volume: p.volume,
                                powered: p.powered,
                            });
                            for event in external.observe(p, unix_now()).events {
                                let _ = events.send(event);
                            }
                        }
                    }
                }

                // after a player is matched, resolve its queue so the
                // queue events can drive metadata and position
                if let (Some(pid), None) = (&matched_id, &matched_queue) {
                    if active_queue_req.is_none() {
                        let mid = msg_id;
                        active_queue_req = Some(mid);
                        sink.send(Message::Text(
                            command_frame(
                                mid,
                                "player_queues/get_active_queue",
                                json!({ "player_id": pid }),
                            )
                            .into(),
                        ))
                        .await?;
                        msg_id += 1;
                    }
                }
            }
        }
    }
}

/// wall-clock unix seconds: MA stamps its external clock readings with it
fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn remember_player(players: &mut Players, p: &PlayerSnapshot) {
    players.insert(
        p.player_id.clone(),
        (p.name.clone(), p.playing, p.available, p.has_media),
    );
}

/// the old player's media must not stay on screen after a switch: an
/// ended external source and an empty queue wipe it; the new player's
/// own events and queue seed fill the display again
fn forget_shown_player(events: &tokio::sync::mpsc::UnboundedSender<SourceEvent>) {
    let _ = events.send(SourceEvent::ExternalEnded);
    let _ = events.send(SourceEvent::Queue {
        items: Vec::new(),
        current_index: usize::MAX,
    });
}

type Players = HashMap<String, (String, bool, bool, bool)>;

/// what a pick pass did to the matched player
#[derive(Debug, PartialEq)]
enum Pick {
    Kept,
    First,
    Switched,
}

/// ranks up to here (configured name, own squeezelite) are worth
/// leaving a fallback for
const PREFERRED_RANK: u8 = 1;

/// pick the display's player, best first: the configured name (exact),
/// the device's own squeezelite, playing, has media and available,
/// available, the rest — deterministic by id.
/// The first pick takes the best one; afterwards only a preferred
/// player (configured name, own squeezelite) that has appeared and
/// outranks the current match replaces it — never down, never between
/// equal ranks (no flicker between fallbacks)
fn try_pick(
    matched_id: &mut Option<String>,
    players: &Players,
    want: &str,
    own: Option<&str>,
) -> Pick {
    let rank_of = |id: &String, (name, playing, avail, media): &(String, bool, bool, bool)| {
        if !want.is_empty() && name == want {
            0
        } else if own == Some(id.as_str()) {
            PREFERRED_RANK
        } else if *playing {
            2
        } else if *media && *avail {
            3
        } else if *avail {
            4
        } else {
            5
        }
    };
    let best = players
        .iter()
        .min_by_key(|(id, entry)| (rank_of(id, entry), (*id).clone()));
    let Some((best_id, best_entry)) = best else {
        return Pick::Kept;
    };
    let best_rank = rank_of(best_id, best_entry);
    let outcome = match matched_id.as_ref() {
        None => Pick::First,
        Some(current) => {
            let current_rank = players
                .get(current)
                .map_or(u8::MAX, |entry| rank_of(current, entry));
            if best_rank <= PREFERRED_RANK && best_rank < current_rank {
                Pick::Switched
            } else {
                return Pick::Kept;
            }
        }
    };
    let name = &best_entry.0;
    match outcome {
        Pick::Switched => tracing::info!("switching to player {name:?} ({best_id})"),
        _ => tracing::info!("showing player {name:?} ({best_id})"),
    }
    *matched_id = Some(best_id.clone());
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &str = "d8:3a:dd:bc:6b:ec";

    /// (id, name, playing, available, has_media)
    fn players(entries: &[(&str, &str, bool, bool, bool)]) -> Players {
        entries
            .iter()
            .map(|&(id, name, playing, avail, media)| {
                (id.to_string(), (name.to_string(), playing, avail, media))
            })
            .collect()
    }

    fn idle(id: &str, name: &str) -> (String, (String, bool, bool, bool)) {
        (id.to_string(), (name.to_string(), false, true, false))
    }

    fn pick_once(players: &Players, want: &str, own: Option<&str>) -> Option<String> {
        let mut matched = None;
        try_pick(&mut matched, players, want, own);
        matched
    }

    #[test]
    fn the_own_player_wins_over_a_playing_fallback() {
        let all = players(&[
            ("aa:web", "Web (Firefox on Linux)", true, true, true),
            (OWN, "HiFi", false, true, false),
        ]);
        assert_eq!(pick_once(&all, "", Some(OWN)).as_deref(), Some(OWN));
    }

    #[test]
    fn a_fallback_is_replaced_when_the_own_player_appears() {
        let mut all = players(&[("aa:web", "Web (Firefox on Linux)", true, true, true)]);
        let mut matched = None;
        assert_eq!(try_pick(&mut matched, &all, "", Some(OWN)), Pick::First);
        assert_eq!(matched.as_deref(), Some("aa:web"));
        all.extend([idle(OWN, "HiFi")]);
        assert_eq!(try_pick(&mut matched, &all, "", Some(OWN)), Pick::Switched);
        assert_eq!(matched.as_deref(), Some(OWN));
        // settled: nothing moves on the next pass
        assert_eq!(try_pick(&mut matched, &all, "", Some(OWN)), Pick::Kept);
    }

    #[test]
    fn the_configured_name_wins_over_the_own_player() {
        let all = players(&[
            (OWN, "HiFi", false, true, false),
            ("bb:kitchen", "Kitchen", false, true, false),
        ]);
        assert_eq!(
            pick_once(&all, "Kitchen", Some(OWN)).as_deref(),
            Some("bb:kitchen")
        );
    }

    #[test]
    fn the_configured_name_replaces_the_own_player_when_it_appears() {
        let mut all = players(&[(OWN, "HiFi", false, true, false)]);
        let mut matched = None;
        try_pick(&mut matched, &all, "Kitchen", Some(OWN));
        assert_eq!(matched.as_deref(), Some(OWN));
        all.extend([idle("bb:kitchen", "Kitchen")]);
        assert_eq!(
            try_pick(&mut matched, &all, "Kitchen", Some(OWN)),
            Pick::Switched
        );
        assert_eq!(matched.as_deref(), Some("bb:kitchen"));
    }

    #[test]
    fn the_configured_name_is_never_left_for_the_own_player() {
        let all = players(&[
            (OWN, "HiFi", true, true, true),
            ("bb:kitchen", "Kitchen", false, false, false),
        ]);
        let mut matched = Some("bb:kitchen".to_string());
        assert_eq!(
            try_pick(&mut matched, &all, "Kitchen", Some(OWN)),
            Pick::Kept
        );
        assert_eq!(matched.as_deref(), Some("bb:kitchen"));
    }

    #[test]
    fn without_an_own_id_the_auto_rank_decides_as_before() {
        let all = players(&[
            ("aa:idle", "Idle", false, true, false),
            ("bb:media", "Media", false, true, true),
            ("cc:playing", "Playing", true, true, true),
        ]);
        assert_eq!(pick_once(&all, "", None).as_deref(), Some("cc:playing"));
        let all = players(&[
            ("aa:idle", "Idle", false, true, false),
            ("bb:media", "Media", false, true, true),
        ]);
        assert_eq!(pick_once(&all, "", None).as_deref(), Some("bb:media"));
        let all = players(&[
            ("bb:gone", "Gone", false, false, false),
            ("aa:idle", "Idle", false, true, false),
        ]);
        assert_eq!(pick_once(&all, "", None).as_deref(), Some("aa:idle"));
    }

    #[test]
    fn an_own_id_that_is_not_listed_falls_back_to_the_auto_rank() {
        let all = players(&[("aa:web", "Web", false, true, false)]);
        assert_eq!(pick_once(&all, "", Some(OWN)).as_deref(), Some("aa:web"));
    }

    #[test]
    fn fallbacks_never_replace_each_other() {
        let mut all = players(&[("zz:idle", "Idle", false, true, false)]);
        let mut matched = None;
        try_pick(&mut matched, &all, "", Some(OWN));
        // a better-ranked fallback shows up, and a "playing" one too
        all.extend([
            (
                "aa:playing".to_string(),
                ("Playing".to_string(), true, true, true),
            ),
            idle("bb:idle", "Other"),
        ]);
        assert_eq!(try_pick(&mut matched, &all, "", Some(OWN)), Pick::Kept);
        assert_eq!(matched.as_deref(), Some("zz:idle"));
    }

    fn player_json(id: &str, name: &str) -> Value {
        json!({
            "player_id": id,
            "name": name,
            "playback_state": "idle",
            "available": true,
            "current_media": null,
        })
    }

    /// the next command frame of the session: (command, args)
    async fn next_command<S>(ws: &mut S) -> (String, Value)
    where
        S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("the session sent nothing")
            .expect("socket closed")
            .expect("socket error");
        let v: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        (
            v["command"].as_str().unwrap().to_string(),
            v["args"].clone(),
        )
    }

    #[tokio::test]
    async fn the_own_player_appearing_late_takes_over_and_the_old_state_is_wiped() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = Config {
            ws_url: format!("ws://{}/ws", listener.local_addr().unwrap()),
            token: String::new(),
            player_name: String::new(),
            own_player_id: Some(OWN.to_string()),
        };
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let session = tokio::spawn(async move {
            let _ = run_session(&cfg, &mut cmd_rx, &events_tx).await;
        });

        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(next_command(&mut ws).await.0, "auth");
        assert_eq!(next_command(&mut ws).await.0, "players/all");
        let send = |value: Value| Message::Text(value.to_string().into());
        ws.send(send(
            json!({"message_id": "0", "result": {"authenticated": true}}),
        ))
        .await
        .unwrap();
        // MA is still restarting squeezelite: only the web player is listed
        ws.send(send(json!({
            "message_id": "1",
            "result": [player_json("aa:web", "Web (Firefox on Linux)")],
        })))
        .await
        .unwrap();
        let (command, args) = next_command(&mut ws).await;
        assert_eq!(command, "player_queues/get_active_queue");
        assert_eq!(args["player_id"], "aa:web");

        // squeezelite is back
        ws.send(send(json!({
            "event": "player_updated",
            "object_id": OWN,
            "data": player_json(OWN, "HiFi"),
        })))
        .await
        .unwrap();
        let (command, args) = next_command(&mut ws).await;
        assert_eq!(command, "player_queues/get_active_queue");
        assert_eq!(args["player_id"], OWN);

        // the old player's queue is gone from the display
        drop(cmd_tx);
        session.await.unwrap();
        let mut seen = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            seen.push(event);
        }
        assert!(
            seen.iter().any(|e| matches!(
                e,
                SourceEvent::Queue { items, .. } if items.is_empty()
            )),
            "an empty queue must wipe the old player's list"
        );
        assert!(
            seen.iter().any(|e| matches!(e, SourceEvent::ExternalEnded)),
            "an external source of the old player must not linger"
        );
    }
}
