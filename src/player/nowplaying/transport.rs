//! Transport wiring for the now-playing surface: play/pause,
//! next/prev, shuffle, repeat, volume, power. Kept beside the np
//! update pipeline — the callbacks flip optimistic loading dots
//! (Event::MarkLoading) and speak SourceCommands; the queue
//! event confirms with the real state, the UI only renders what
//! MA reports.

use crate::locks;
use crate::player::shell::{Shared, Shell};
use crate::player::state::{self, Event};
use crate::providers::{RepeatMode, SourceCommand};
use crate::Mestro;

/// all transport callbacks (np overlay + queue header). Needs
/// the player shell for the optimistic loading dots, the seek
/// bookkeeping and the local-volume origin mark, and the command
/// channel to MA.
pub fn wire(ui: &Mestro, cmd_tx: &crate::ui::navigation::CmdTx, shell: &Shell) {
    ui.on_toggle_play({
        let shell = shell.clone();
        let cmd_tx = cmd_tx.clone();
        move || send_transport(&shell, &cmd_tx, Action::PlayPause)
    });
    ui.on_next_track({
        let shell = shell.clone();
        let cmd_tx = cmd_tx.clone();
        move || send_transport(&shell, &cmd_tx, Action::Next)
    });
    ui.on_prev_track({
        let shell = shell.clone();
        let cmd_tx = cmd_tx.clone();
        move || send_transport(&shell, &cmd_tx, Action::Prev)
    });
    // playback options: shuffle toggles, repeat cycles
    // off → all → one → off (the event confirms with the new
    // state — the UI only renders what MA reports)
    ui.on_shuffle_toggle({
        let shared = shell.shared().clone();
        let cmd_tx = cmd_tx.clone();
        move || {
            if let Some(cmd) = route(&shared, Action::Shuffle) {
                let _ = cmd_tx.send(cmd);
            }
        }
    });
    ui.on_repeat_cycle({
        let shared = shell.shared().clone();
        let cmd_tx = cmd_tx.clone();
        move || {
            if let Some(cmd) = route(&shared, Action::Repeat) {
                let _ = cmd_tx.send(cmd);
            }
        }
    });
    ui.on_seek({
        let shell = shell.clone();
        let cmd_tx = cmd_tx.clone();
        move |fraction, track_key| {
            shell.dispatch_with(std::time::Instant::now(), |state| {
                let seconds = seek_position(state, fraction, track_key.as_str())?;
                let cmd = command(state, Action::Seek(seconds))?;
                cmd_tx
                    .send(cmd)
                    .is_ok()
                    .then_some(Event::SeekRequested { seconds })
            });
        }
    });
    ui.on_volume_set({
        let cmd_tx = cmd_tx.clone();
        let shell = shell.clone();
        move |v| {
            // local origin: the transport echo of THIS change must
            // not pop the remote-volume OSD
            shell.dispatch(Event::MarkVolumeLocal(v), std::time::Instant::now());
            let _ = cmd_tx.send(crate::providers::SourceCommand::Volume(v));
        }
    });
    ui.on_set_power({
        let cmd_tx = cmd_tx.clone();
        move |powered| {
            let _ = cmd_tx.send(crate::providers::SourceCommand::Power(powered));
        }
    });
}

/// what the transport can ask for
#[derive(Debug, Clone, Copy)]
enum Action {
    PlayPause,
    Next,
    Prev,
    Seek(u64),
    Shuffle,
    Repeat,
}

/// the command for an action, or None when the playing source cannot
/// do it. On the queue everything is possible and goes to the queue
/// (next/previous and play/pause are player commands MA resolves
/// itself); an external source has no queue, so seek, shuffle and
/// repeat go to the player — and only what its caps allow
fn command(state: &state::PlayerState, action: Action) -> Option<SourceCommand> {
    let caps = state.source_caps;
    let external = state.external.is_some();
    match action {
        Action::PlayPause => caps.play_pause.then_some(if state.playing {
            SourceCommand::Pause
        } else {
            SourceCommand::Play
        }),
        Action::Next => caps.next_previous.then_some(SourceCommand::Next),
        Action::Prev => caps.next_previous.then_some(SourceCommand::Prev),
        Action::Seek(seconds) => caps.seek.then_some(if external {
            SourceCommand::PlayerSeek(seconds)
        } else {
            SourceCommand::Seek(seconds)
        }),
        Action::Shuffle => caps.shuffle.then_some(if external {
            SourceCommand::PlayerShuffle(!state.shuffle)
        } else {
            SourceCommand::Shuffle(!state.shuffle)
        }),
        Action::Repeat => caps.repeat.then(|| {
            let next = match state.repeat {
                RepeatMode::Off => RepeatMode::All,
                RepeatMode::All => RepeatMode::One,
                RepeatMode::One => RepeatMode::Off,
            };
            if external {
                SourceCommand::PlayerRepeat(next)
            } else {
                SourceCommand::Repeat(next)
            }
        }),
    }
}

/// `command` on the shared state; a refusal is logged (the buttons
/// are greyed, so it only comes from the keyboard, ctl or the web)
fn route(shared: &Shared, action: Action) -> Option<SourceCommand> {
    let guard = locks::lock(shared);
    let state = guard
        .as_ref()
        .map(|live| live.state.clone())
        .unwrap_or_default();
    let cmd = command(&state, action);
    if cmd.is_none() {
        tracing::info!(
            "{action:?} refused: {} cannot do it",
            state.external.as_deref().unwrap_or("the queue")
        );
    }
    cmd
}

/// play/pause, next, previous: refused or sent; on the queue the
/// buttons show the optimistic loading dots until MA confirms (an
/// external source may keep "playing" through a skip — nothing would
/// clear them there)
fn send_transport(shell: &Shell, cmd_tx: &crate::ui::navigation::CmdTx, action: Action) {
    let Some(cmd) = route(shell.shared(), action) else {
        return;
    };
    let on_queue = locks::lock(shell.shared())
        .as_ref()
        .is_none_or(|live| live.state.external.is_none());
    if on_queue && cmd != SourceCommand::Pause {
        shell.dispatch(Event::MarkLoading, std::time::Instant::now());
    }
    let _ = cmd_tx.send(cmd);
}

pub fn seek_track_key(state: &state::PlayerState) -> String {
    let item_id = state
        .queue
        .get(state.queue_current)
        .map(|item| item.queue_item_id.as_str())
        .unwrap_or("");
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        item_id, state.title, state.artist, state.album
    )
}

fn seek_position(state: &state::PlayerState, fraction: f32, track_key: &str) -> Option<u64> {
    if !fraction.is_finite()
        || !state.source_caps.seek
        || !state.has_media
        || !state.powered
        || state.loading
        || state.duration_ms < 1000
        || seek_track_key(state) != track_key
    {
        return None;
    }
    let duration = state.duration_ms / 1000;
    Some(((fraction.clamp(0.0, 1.0) as f64 * duration as f64).floor() as u64).min(duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::SourceCaps;
    #[test]
    fn seek_clamps_fraction_and_rejects_unknown_media_or_stale_identity() {
        let mut state = state::PlayerState {
            has_media: true,
            duration_ms: 120_000,
            title: "Track".into(),
            ..Default::default()
        };
        let key = seek_track_key(&state);
        assert_eq!(seek_position(&state, 0.5, &key), Some(60));
        assert_eq!(seek_position(&state, -1.0, &key), Some(0));
        assert_eq!(seek_position(&state, 2.0, &key), Some(120));
        assert_eq!(seek_position(&state, f32::NAN, &key), None);
        assert_eq!(seek_position(&state, 0.5, "previous"), None);
        state.loading = true;
        assert_eq!(seek_position(&state, 0.5, &key), None);
        state.loading = false;
        state.duration_ms = 0;
        assert_eq!(seek_position(&state, 0.5, &key), None);
    }

    fn external(caps: SourceCaps) -> state::PlayerState {
        state::PlayerState {
            has_media: true,
            powered: true,
            duration_ms: 120_000,
            title: "Track".into(),
            external: Some("Spotify Connect".into()),
            source_caps: caps,
            ..Default::default()
        }
    }

    fn nothing() -> SourceCaps {
        SourceCaps {
            play_pause: false,
            seek: false,
            next_previous: false,
            shuffle: false,
            repeat: false,
            passive: true,
        }
    }

    #[test]
    fn the_queue_can_do_everything_and_addresses_the_queue() {
        let mut state = state::PlayerState::default();
        assert_eq!(
            command(&state, Action::PlayPause),
            Some(SourceCommand::Play)
        );
        state.playing = true;
        assert_eq!(
            command(&state, Action::PlayPause),
            Some(SourceCommand::Pause)
        );
        assert_eq!(command(&state, Action::Next), Some(SourceCommand::Next));
        assert_eq!(command(&state, Action::Prev), Some(SourceCommand::Prev));
        assert_eq!(
            command(&state, Action::Seek(9)),
            Some(SourceCommand::Seek(9))
        );
        assert_eq!(
            command(&state, Action::Shuffle),
            Some(SourceCommand::Shuffle(true))
        );
        assert_eq!(
            command(&state, Action::Repeat),
            Some(SourceCommand::Repeat(RepeatMode::All))
        );
    }

    #[test]
    fn an_airplay_like_source_refuses_every_control_play_pause_included() {
        let state = external(nothing());
        for action in [
            Action::PlayPause,
            Action::Next,
            Action::Prev,
            Action::Seek(5),
            Action::Shuffle,
            Action::Repeat,
        ] {
            assert_eq!(command(&state, action), None, "{action:?}");
        }
        let key = seek_track_key(&state);
        assert_eq!(seek_position(&state, 0.5, &key), None);
    }

    #[test]
    fn a_spotify_connect_like_source_gets_exactly_its_controls_as_player_commands() {
        let mut state = external(SourceCaps {
            play_pause: true,
            seek: true,
            next_previous: true,
            shuffle: false,
            repeat: false,
            passive: false,
        });
        assert_eq!(
            command(&state, Action::PlayPause),
            Some(SourceCommand::Play)
        );
        state.playing = true;
        assert_eq!(
            command(&state, Action::PlayPause),
            Some(SourceCommand::Pause)
        );
        assert_eq!(command(&state, Action::Next), Some(SourceCommand::Next));
        assert_eq!(command(&state, Action::Prev), Some(SourceCommand::Prev));
        let key = seek_track_key(&state);
        assert_eq!(seek_position(&state, 0.5, &key), Some(60));
        assert_eq!(
            command(&state, Action::Seek(60)),
            Some(SourceCommand::PlayerSeek(60))
        );
        // not offered: refused
        assert_eq!(command(&state, Action::Shuffle), None);
        assert_eq!(command(&state, Action::Repeat), None);
    }

    #[test]
    fn shuffle_and_repeat_of_an_external_source_go_to_the_player() {
        let mut state = external(SourceCaps {
            shuffle: true,
            repeat: true,
            ..nothing()
        });
        assert_eq!(
            command(&state, Action::Shuffle),
            Some(SourceCommand::PlayerShuffle(true))
        );
        state.shuffle = true;
        state.repeat = RepeatMode::One;
        assert_eq!(
            command(&state, Action::Shuffle),
            Some(SourceCommand::PlayerShuffle(false))
        );
        assert_eq!(
            command(&state, Action::Repeat),
            Some(SourceCommand::PlayerRepeat(RepeatMode::Off))
        );
    }
}
