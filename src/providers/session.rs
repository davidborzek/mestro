//! The provider session: the retry loop that keeps a playback
//! source connected and the core event loop that fans provider
//! events out into the app state. Provider-agnostic — it speaks
//! only the SourceCommand/SourceEvent vocabulary; the concrete
//! wire protocol (Music Assistant today, LMS/MPD tomorrow)
//! plugs in through the PlaybackSource trait: `make` constructs
//! a fresh source per attempt, `failure` classifies its errors
//! (auth failures never heal in-process — poll slowly at 60s;
//! transport errors retry fast at 5s).
//!
//! The event channel survives attempts (the core state stays
//! intact). The banner only clears once real events flow again —
//! the retry loop raises it, the core loop lowers it.

use super::{PlaybackSource, SourceCommand, SourceEvent};
use crate::config::ThemeVariant;
use crate::library;
use crate::library::search::Search;
use crate::player::state::{self, Shared, UpdateTx};
use crate::Mestro;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn set_status(ui_weak: &slint::Weak<Mestro>, text: &str) {
    let text = text.to_string();
    let weak = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_provider_status(text.into());
        }
    });
}

/// the verified-connection flag: true once real events flow, false
/// whenever an attempt fails or the source is unconfigured. The
/// first-run overlay dismisses on this — the (ambiguous) empty
/// status string is the initial value too.
fn set_connected(ui_weak: &slint::Weak<Mestro>, connected: bool) {
    let weak = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_provider_connected(connected);
        }
    });
}

/// spawn the provider thread: connection retry loop + core
/// event dispatch (state machine, library routing, search
/// results). Consumes the command receiver — the session owns
/// it. `make` constructs a fresh playback source per attempt
/// (the factory captures the provider's own config).
#[allow(clippy::too_many_arguments)]
pub fn spawn<P, F>(
    make: F,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<SourceCommand>,
    shared: Shared,
    update_tx: UpdateTx,
    lib_feed: library::LibTx,
    theme_variant: ThemeVariant,
    ui_weak: slint::Weak<Mestro>,
) where
    P: PlaybackSource + Send + 'static,
    F: Fn() -> P + Send + 'static,
{
    let banner_up = Arc::new(AtomicBool::new(false));
    let connected_up = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<SourceEvent>();
            let provider_loop = async {
                loop {
                    let mut provider = make();
                    // unconfigured: never dial (first-run state) —
                    // idle-poll and keep the connected flag honest
                    if !provider.configured() {
                        if connected_up.swap(false, Ordering::Relaxed) {
                            set_connected(&ui_weak, false);
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                    if let Err(e) = provider.run(&mut cmd_rx, evt_tx.clone()).await {
                        let retry = provider.failure(e);
                        set_status(&ui_weak, &retry.message);
                        banner_up.store(true, Ordering::Relaxed);
                        if connected_up.swap(false, Ordering::Relaxed) {
                            set_connected(&ui_weak, false);
                        }
                        tokio::time::sleep(retry.backoff).await;
                    }
                }
            };
            let core = async {
                let mut last_cover = String::new();
                while let Some(ev) = evt_rx.recv().await {
                    // the first event proves auth + subscription —
                    // drop the banner before applying it
                    if banner_up.swap(false, Ordering::Relaxed) {
                        set_status(&ui_weak, "");
                    }
                    if !connected_up.swap(true, Ordering::Relaxed) {
                        set_connected(&ui_weak, true);
                    }
                    match ev {
                        SourceEvent::Transport {
                            playing,
                            volume,
                            powered,
                        } => state::apply_transport(&shared, &update_tx, playing, volume, powered),
                        SourceEvent::Track { track, elapsed_sec } => state::apply_track(
                            &shared,
                            &update_tx,
                            &mut last_cover,
                            &track,
                            elapsed_sec,
                            theme_variant.raw().clone(),
                        ),
                        SourceEvent::Position(secs) => {
                            state::apply_position(&shared, &update_tx, secs)
                        }
                        SourceEvent::Queue {
                            items,
                            current_index,
                        } => state::apply_queue(
                            &shared,
                            &update_tx,
                            &mut last_cover,
                            items,
                            current_index,
                        ),
                        SourceEvent::Options { shuffle, repeat } => {
                            state::apply_options(&shared, &update_tx, shuffle, repeat)
                        }
                        // library events route through the lib module
                        ev @ (SourceEvent::Browse { .. } | SourceEvent::SubjectImage { .. }) => {
                            library::route(&lib_feed, ev);
                        }
                        SourceEvent::Search { key, entries } => {
                            Search::apply_results(&ui_weak, &key, entries);
                        }
                    }
                }
            };
            tokio::join!(provider_loop, core);
        });
    });
}
