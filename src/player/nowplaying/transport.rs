//! Transport wiring for the now-playing surface: play/pause,
//! next/prev, shuffle, repeat, volume, power. Kept beside the np
//! update pipeline — the callbacks flip optimistic loading dots
//! (state::mark_loading) and speak SourceCommands; the queue
//! event confirms with the real state, the UI only renders what
//! MA reports.

use crate::player::state::{self, Shared, UpdateTx};
use crate::Mestro;

/// all transport callbacks (np overlay + queue header). Needs
/// the state shared slot for the optimistic loading dots and the
/// local-volume origin mark, and the command channel to MA.
pub fn wire(
    ui: &Mestro,
    cmd_tx: &crate::ui::navigation::CmdTx,
    update_tx: &UpdateTx,
    shared: &Shared,
) {
    ui.on_toggle_play({
        let shared = shared.clone();
        let update_tx = update_tx.clone();
        let cmd_tx = cmd_tx.clone();
        move || {
            let playing = shared
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|l| l.state.playing);
            let cmd = if playing {
                crate::providers::SourceCommand::Pause
            } else {
                state::mark_loading(&shared, &update_tx);
                crate::providers::SourceCommand::Play
            };
            let _ = cmd_tx.send(cmd);
        }
    });
    ui.on_next_track({
        let shared = shared.clone();
        let update_tx = update_tx.clone();
        let cmd_tx = cmd_tx.clone();
        move || {
            state::mark_loading(&shared, &update_tx);
            let _ = cmd_tx.send(crate::providers::SourceCommand::Next);
        }
    });
    ui.on_prev_track({
        let shared = shared.clone();
        let update_tx = update_tx.clone();
        let cmd_tx = cmd_tx.clone();
        move || {
            state::mark_loading(&shared, &update_tx);
            let _ = cmd_tx.send(crate::providers::SourceCommand::Prev);
        }
    });
    // queue-level playback options: shuffle toggles, repeat
    // cycles off → all → one → off (the queue event confirms
    // with the new state — the UI only renders what MA reports)
    ui.on_shuffle_toggle({
        let shared = shared.clone();
        let cmd_tx = cmd_tx.clone();
        move || {
            let on = shared
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|l| l.state.shuffle);
            let _ = cmd_tx.send(crate::providers::SourceCommand::Shuffle(!on));
        }
    });
    ui.on_repeat_cycle({
        let shared = shared.clone();
        let cmd_tx = cmd_tx.clone();
        move || {
            let next = shared
                .lock()
                .unwrap()
                .as_ref()
                .map(|l| match l.state.repeat {
                    crate::providers::RepeatMode::Off => crate::providers::RepeatMode::All,
                    crate::providers::RepeatMode::All => crate::providers::RepeatMode::One,
                    crate::providers::RepeatMode::One => crate::providers::RepeatMode::Off,
                })
                .unwrap_or(crate::providers::RepeatMode::All);
            let _ = cmd_tx.send(crate::providers::SourceCommand::Repeat(next));
        }
    });
    ui.on_seek({
        let shared = shared.clone();
        let cmd_tx = cmd_tx.clone();
        move |fraction, track_key| {
            let mut guard = shared.lock().unwrap();
            let Some(live) = guard.as_mut() else { return };
            let Some(seconds) = seek_position(&live.state, fraction, track_key.as_str()) else {
                return;
            };
            if cmd_tx
                .send(crate::providers::SourceCommand::Seek(seconds))
                .is_ok()
            {
                live.pending_seek = Some((std::time::Instant::now(), seconds * 1000));
                live.pending_pos = None;
            }
        }
    });
    ui.on_volume_set({
        let cmd_tx = cmd_tx.clone();
        let shared = shared.clone();
        move |v| {
            // local origin: the transport echo of THIS change must
            // not pop the remote-volume OSD
            state::mark_volume_local(&shared, v);
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
}
