//! The outbound vocabulary: SourceCommand -> MA JSON frames.

use crate::providers::{Enqueue, SourceCommand};
use serde_json::{json, Value};

pub(super) fn command_frame(msg_id: u64, command: &str, args: Value) -> String {
    json!({ "message_id": msg_id.to_string(), "command": command, "args": args }).to_string()
}

pub(super) fn command_for(
    cmd: &SourceCommand,
    player_id: &str,
    queue_id: &str,
) -> (&'static str, Value) {
    match cmd {
        // never framed — the session loop intercepts it (live
        // rewire of the connection settings)
        SourceCommand::Reconnect => ("", json!({})),
        SourceCommand::Play => ("players/cmd/play", json!({ "player_id": player_id })),
        SourceCommand::Pause => ("players/cmd/pause", json!({ "player_id": player_id })),
        SourceCommand::Next => ("players/cmd/next", json!({ "player_id": player_id })),
        SourceCommand::Prev => ("players/cmd/previous", json!({ "player_id": player_id })),
        SourceCommand::Volume(v) => (
            "players/cmd/volume_set",
            json!({
                "player_id": player_id,
                "volume_level": (*v).clamp(0, 100),
            }),
        ),
        // an external source has no queue: seek/shuffle/repeat go to the player
        SourceCommand::PlayerSeek(position) => (
            "players/cmd/seek",
            json!({ "player_id": player_id, "position": position }),
        ),
        SourceCommand::PlayerShuffle(enabled) => (
            "players/cmd/shuffle",
            json!({ "player_id": player_id, "shuffle_enabled": enabled }),
        ),
        SourceCommand::PlayerRepeat(mode) => (
            "players/cmd/repeat",
            json!({ "player_id": player_id, "repeat_mode": mode.wire() }),
        ),
        SourceCommand::Seek(position) => (
            "player_queues/seek",
            json!({ "queue_id": queue_id, "position": position }),
        ),
        SourceCommand::Power(powered) => (
            "players/cmd/power",
            json!({
                "player_id": player_id,
                "powered": powered,
            }),
        ),
        // queue commands address the queue, not the player; play_index
        // takes an index or a queue_item_id — the id survives reorders
        SourceCommand::PlayQueueItem(item_id) => (
            "player_queues/play_index",
            json!({
                "queue_id": queue_id,
                "index": item_id,
            }),
        ),
        SourceCommand::RemoveQueueItem(item_id) => (
            "player_queues/delete_item",
            json!({
                "queue_id": queue_id,
                "item_id_or_index": item_id,
            }),
        ),
        SourceCommand::ClearQueue => ("player_queues/clear", json!({ "queue_id": queue_id })),
        // queue-level playback options
        SourceCommand::Shuffle(enabled) => (
            "player_queues/shuffle",
            json!({ "queue_id": queue_id, "shuffle_enabled": enabled }),
        ),
        SourceCommand::Repeat(mode) => (
            "player_queues/repeat",
            json!({ "queue_id": queue_id, "repeat_mode": mode.wire() }),
        ),
        // library browsing — path-based tree walk
        SourceCommand::Browse(path) => ("music/browse", json!({ "path": path })),
        // library-wide search: one payload, grouped server-side
        SourceCommand::Search(query) => (
            "music/search",
            json!({ "search_query": query, "limit": 30 }),
        ),
        // drill-downs via dedicated commands
        SourceCommand::ArtistAlbums { item_id, provider } => (
            "music/artists/artist_albums",
            json!({
                "item_id": item_id,
                "provider_instance_id_or_domain": provider,
            }),
        ),
        SourceCommand::AlbumTracks { item_id, provider } => (
            "music/albums/album_tracks",
            json!({
                "item_id": item_id,
                "provider_instance_id_or_domain": provider,
            }),
        ),
        SourceCommand::PlaylistTracks { item_id, provider } => (
            "music/playlists/playlist_tracks",
            json!({
                "item_id": item_id,
                "provider_instance_id_or_domain": provider,
            }),
        ),
        // media is a URI string or an item object; the URI alone
        // works. The option drives the enqueue mode (next/add); MA's
        // default (play/replace) applies when omitted
        SourceCommand::PlayMedia { uri, enqueue } => {
            let option = match enqueue {
                Enqueue::Default => None,
                Enqueue::Next => Some("next"),
                Enqueue::Add => Some("add"),
            };
            let mut args = json!({ "queue_id": queue_id, "media": uri });
            if let Some(o) = option {
                args["option"] = json!(o);
            }
            ("player_queues/play_media", args)
        }
        SourceCommand::PlayMediaShuffled { uri } => (
            "player_queues/play_media",
            json!({ "queue_id": queue_id, "media": uri, "option": "replace", "shuffle": true }),
        ),
        // play a container from a given item: the tapped track keeps
        // its album/playlist context
        SourceCommand::PlayMediaFrom { media, start_item } => (
            "player_queues/play_media",
            json!({
                "queue_id": queue_id,
                "media": media,
                "start_item": start_item,
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::RepeatMode;
    #[test]
    fn shuffled_play_replaces_queue_and_randomizes_the_first_track() {
        let (command, args) = command_for(
            &SourceCommand::PlayMediaShuffled {
                uri: "library://album/1".into(),
            },
            "player",
            "queue",
        );
        assert_eq!(command, "player_queues/play_media");
        assert_eq!(
            args,
            json!({ "queue_id": "queue", "media": "library://album/1", "option": "replace", "shuffle": true })
        );
    }
    #[test]
    fn an_external_source_is_controlled_on_the_player_never_on_the_queue() {
        let cases = [
            (
                SourceCommand::Play,
                "players/cmd/play",
                json!({ "player_id": "player" }),
            ),
            (
                SourceCommand::Pause,
                "players/cmd/pause",
                json!({ "player_id": "player" }),
            ),
            (
                SourceCommand::Next,
                "players/cmd/next",
                json!({ "player_id": "player" }),
            ),
            (
                SourceCommand::Prev,
                "players/cmd/previous",
                json!({ "player_id": "player" }),
            ),
            (
                SourceCommand::PlayerSeek(75),
                "players/cmd/seek",
                json!({ "player_id": "player", "position": 75 }),
            ),
            (
                SourceCommand::PlayerShuffle(true),
                "players/cmd/shuffle",
                json!({ "player_id": "player", "shuffle_enabled": true }),
            ),
            (
                SourceCommand::PlayerRepeat(RepeatMode::One),
                "players/cmd/repeat",
                json!({ "player_id": "player", "repeat_mode": "one" }),
            ),
        ];
        for (cmd, command, args) in cases {
            assert_eq!(
                command_for(&cmd, "player", "queue"),
                (command, args),
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn seek_addresses_the_active_queue_in_seconds() {
        let (command, args) = command_for(&SourceCommand::Seek(75), "player", "queue");
        assert_eq!(command, "player_queues/seek");
        assert_eq!(args, json!({ "queue_id": "queue", "position": 75 }));
    }
}
