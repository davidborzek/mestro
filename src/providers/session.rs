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
//! the retry loop raises it, the core loop lowers it. Transient
//! failures raise it only after a grace window (banner.rs).

use super::banner::BannerGate;
use super::startup::{Startup, StartupGate};
use super::{PlaybackSource, SourceCommand, SourceEvent};
use crate::library;
use crate::library::search::Search;
use crate::locks;
use crate::player::shell::Shell;
use crate::player::state::Event;
use crate::Mestro;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn set_startup(ui_weak: &slint::Weak<Mestro>, phase: Startup) {
    let weak = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_startup_phase(match phase {
                Startup::WaitingForNetwork => 1,
                Startup::Connecting => 2,
                Startup::Done => 0,
            });
        }
    });
}

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

/// The player half of the core loop: a provider event becomes a state
/// `Event` for the shell. Library and search events are not the
/// player's — `apply` hands them back to the caller.
pub struct PlayerFeed {
    shell: Shell,
}

impl PlayerFeed {
    pub fn new(shell: Shell) -> Self {
        Self { shell }
    }

    /// apply one event at `now`; None when the player consumed it
    pub fn apply(&mut self, ev: SourceEvent, now: Instant) -> Option<SourceEvent> {
        let event = match ev {
            SourceEvent::Transport {
                playing,
                volume,
                powered,
            } => Event::Transport {
                playing,
                volume,
                powered,
            },
            SourceEvent::Track { track, elapsed_sec } => Event::Track { track, elapsed_sec },
            SourceEvent::QueueSeed { track, elapsed_sec } => {
                Event::QueueSeed { track, elapsed_sec }
            }
            SourceEvent::External {
                source,
                track,
                elapsed_sec,
            } => Event::External {
                source,
                track,
                elapsed_sec,
            },
            SourceEvent::ExternalEnded => Event::ExternalEnded,
            SourceEvent::Position(secs) => Event::Position(secs),
            SourceEvent::Queue {
                items,
                current_index,
            } => Event::Queue {
                items,
                current_index,
            },
            SourceEvent::Options { shuffle, repeat } => Event::Options { shuffle, repeat },
            other @ (SourceEvent::Browse { .. }
            | SourceEvent::SubjectImage { .. }
            | SourceEvent::Search { .. }) => return Some(other),
        };
        self.shell.dispatch(event, now);
        None
    }
}

/// spawn the provider thread: connection retry loop + core
/// event dispatch (state machine, library routing, search
/// results). Consumes the command receiver — the session owns
/// it. `make` constructs a fresh playback source per attempt
/// (the factory captures the provider's own config).
pub fn spawn<P, F>(
    make: F,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<SourceCommand>,
    shell: Shell,
    lib_feed: library::LibTx,
    ui_weak: slint::Weak<Mestro>,
) where
    P: PlaybackSource + Send + 'static,
    F: Fn() -> P + Send + 'static,
{
    let banner_up = Arc::new(AtomicBool::new(false));
    let connected_up = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(Mutex::new(BannerGate::new(Instant::now())));
    let startup = Arc::new(Mutex::new(StartupGate::new(Instant::now())));
    std::thread::spawn(move || {
        #[expect(
            clippy::unwrap_used,
            reason = "multi-thread runtime creation only fails on resource exhaustion"
        )]
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<SourceEvent>();
            // the boot screen: network → connecting → gone
            let boot_screen = async {
                let mut shown = None;
                loop {
                    let network = crate::remote::web::lan_ip() != "127.0.0.1";
                    let phase = locks::lock(&startup).phase(Instant::now(), network);
                    if shown != Some(phase) {
                        set_startup(&ui_weak, phase);
                        shown = Some(phase);
                    }
                    if phase == Startup::Done {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            };
            let provider_loop = async {
                loop {
                    let mut provider = make();
                    // unconfigured: never dial (first-run state) —
                    // idle-poll and keep the connected flag honest
                    if !provider.configured() {
                        // first run: the setup screen, not the boot screen
                        locks::lock(&startup).connected();
                        if connected_up.swap(false, Ordering::Relaxed) {
                            set_connected(&ui_weak, false);
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                    if let Err(e) = provider.run(&mut cmd_rx, evt_tx.clone()).await {
                        let retry = provider.failure(e);
                        let now = Instant::now();
                        let booting =
                            !locks::lock(&startup).show_failure(now, retry.heals_on_retry);
                        if !booting && locks::lock(&gate).on_failure(now, retry.heals_on_retry) {
                            set_status(&ui_weak, &retry.message);
                            banner_up.store(true, Ordering::Relaxed);
                        }
                        if connected_up.swap(false, Ordering::Relaxed) {
                            set_connected(&ui_weak, false);
                        }
                        tokio::time::sleep(retry.backoff).await;
                    }
                }
            };
            let core = async {
                let mut feed = PlayerFeed::new(shell);
                while let Some(ev) = evt_rx.recv().await {
                    // the first event proves auth + subscription —
                    // drop the banner before applying it
                    if banner_up.swap(false, Ordering::Relaxed) {
                        set_status(&ui_weak, "");
                    }
                    if !connected_up.swap(true, Ordering::Relaxed) {
                        set_connected(&ui_weak, true);
                    }
                    locks::lock(&gate).on_connected(Instant::now());
                    locks::lock(&startup).connected();
                    match feed.apply(ev, Instant::now()) {
                        None => {}
                        // library events route through the lib module
                        Some(
                            ev @ (SourceEvent::Browse { .. } | SourceEvent::SubjectImage { .. }),
                        ) => {
                            library::route(&lib_feed, ev);
                        }
                        Some(SourceEvent::Search { key, entries }) => {
                            Search::apply_results(&ui_weak, &key, entries);
                        }
                        // `apply` hands back only the library and search events
                        Some(_) => {}
                    }
                }
            };
            tokio::join!(boot_screen, provider_loop, core);
        });
    });
}
