//! The `player` op on the UI thread: absolute volume, the one
//! transport value the ctl vocabulary cannot express. It maps
//! onto the same SourceCommand the slint slider uses, with the
//! same optimistic bookkeeping — the volume local-origin mark
//! lives in the shared state, guarded there.

use super::super::PlayerCtx;
use crate::player::state::Event;
use crate::providers::SourceCommand;
use crate::remote::web::protocol::PlayerAction;
use crate::remote::web::Response;

/// one player round on the UI thread: dispatch + ack
pub(crate) fn handle(id: u64, action: &PlayerAction, ctx: &PlayerCtx) -> Response {
    let PlayerAction::Volume { value } = action;
    let value = (*value).clamp(0, 100);
    // local origin: the transport echo of THIS change must
    // not pop the remote-volume OSD (same as the slider)
    ctx.shell
        .dispatch(Event::MarkVolumeLocal(value), std::time::Instant::now());
    if ctx.cmd_tx.send(SourceCommand::Volume(value)).is_ok() {
        Response::ack(id)
    } else {
        Response::err(id, "player gone")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::cover::{CoverSource, HttpCovers};
    use crate::player::shell::Shell;
    use crate::player::state::Live;
    use crate::player::state::PlayerState;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn shell(live: Option<Live>) -> Shell {
        let (update_tx, _) = tokio::sync::mpsc::unbounded_channel();
        Shell::new(
            Arc::new(Mutex::new(live)),
            update_tx,
            CoverSource {
                fetcher: Arc::new(HttpCovers),
                variant: crate::ui::palette::Variant::TonalSpot,
            },
        )
    }

    fn ctx() -> (
        PlayerCtx,
        tokio::sync::mpsc::UnboundedReceiver<SourceCommand>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            PlayerCtx {
                shell: shell(None),
                cmd_tx: tx,
            },
            rx,
        )
    }

    fn live_ctx() -> (
        PlayerCtx,
        tokio::sync::mpsc::UnboundedReceiver<SourceCommand>,
    ) {
        let (mut ctx, rx) = ctx();
        let live = Live::new(
            PlayerState {
                has_media: true,
                powered: true,
                duration_ms: 120_000,
                volume: 20,
                ..Default::default()
            },
            std::time::Instant::now(),
        );
        ctx.shell = shell(Some(live));
        (ctx, rx)
    }

    #[test]
    fn volume_clamps_and_marks_the_local_origin() {
        let (ctx, mut rx) = live_ctx();
        let response = handle(1, &PlayerAction::Volume { value: 150 }, &ctx);
        assert!(response.ok);
        assert_eq!(rx.try_recv().unwrap(), SourceCommand::Volume(100));
        let shared = ctx.shell.shared().lock();
        let live = shared.as_ref().unwrap();
        assert_eq!(
            live.volume_local.map(|(_, v)| v),
            Some(100),
            "the echo is ours"
        );
    }

    #[test]
    fn a_dead_command_channel_answers_player_gone() {
        let (tx, dead) = tokio::sync::mpsc::unbounded_channel();
        drop(dead);
        let shell = shell(Some(Live::new(
            PlayerState {
                has_media: true,
                duration_ms: 60_000,
                ..Default::default()
            },
            std::time::Instant::now(),
        )));
        let ctx = PlayerCtx { shell, cmd_tx: tx };
        let response = handle(5, &PlayerAction::Volume { value: 40 }, &ctx);
        assert!(!response.ok);
        assert_eq!(response.error.as_deref(), Some("player gone"));
    }
}
