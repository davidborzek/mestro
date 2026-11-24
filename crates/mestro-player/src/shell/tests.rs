//! The shell's own contract: publishes leave in event order, under
//! the shared lock — what the end-to-end scenarios cannot pin down,
//! being single-threaded.

use super::*;
use crate::cover::{CoverFetcher, CoverLanded};
use std::sync::{Arc, Barrier};
use std::thread;
use tokio::sync::mpsc::UnboundedReceiver;

/// never asked for anything: these events carry no artwork
struct NoFetch;

impl CoverFetcher for NoFetch {
    fn fetch(&self, _url: String, _landed: CoverLanded) {}
}

fn shell() -> (Shell, UnboundedReceiver<PlayerUpdate>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let covers = CoverSource {
        fetcher: Arc::new(NoFetch),
        variant: crate::palette::Variant::TonalSpot,
    };
    (Shell::new(Arc::new(Mutex::new(None)), tx, covers), rx)
}

fn transport(playing: bool) -> Event {
    Event::Transport {
        playing,
        volume: 40,
        powered: true,
    }
}

fn drain(rx: &mut UnboundedReceiver<PlayerUpdate>) -> Vec<PlayerUpdate> {
    let mut all = Vec::new();
    while let Ok(update) = rx.try_recv() {
        all.push(update);
    }
    all
}

#[test]
fn publishes_leave_in_event_order() {
    let (shell, mut rx) = shell();
    let now = Instant::now();
    shell.dispatch(transport(false), now);
    shell.dispatch(Event::MarkLoading, now);
    shell.dispatch(transport(true), now);
    let published = drain(&mut rx);
    assert_eq!(published.len(), 3, "one snapshot per event");
    assert_eq!(
        (published[0].state.playing, published[0].state.loading),
        (false, false)
    );
    assert!(published[1].state.loading);
    assert_eq!(
        (published[2].state.playing, published[2].state.loading),
        (true, false)
    );
}

/// MarkLoading (the UI thread) races a transport event reporting
/// "playing" (the provider thread). The loading snapshot may be taken
/// first, but its publish must not slip past the lock: otherwise it
/// overtakes the transport snapshot and the UI stays on a stale
/// playing=false while the player already plays.
#[test]
fn a_loading_publish_never_overtakes_a_later_transport() {
    let (shell, mut rx) = shell();
    let now = Instant::now();
    shell.dispatch(transport(false), now);
    drain(&mut rx);

    for _ in 0..200 {
        let start = Arc::new(Barrier::new(2));
        let loading = {
            let shell = shell.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                shell.dispatch(Event::MarkLoading, now);
            })
        };
        let transports = {
            let shell = shell.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                shell.dispatch(transport(true), now);
            })
        };
        loading.join().unwrap();
        transports.join().unwrap();
        let published = drain(&mut rx);
        assert_eq!(published.len(), 2, "one snapshot per event");
        assert!(
            published[1].state.playing,
            "the playing=true snapshot is the last one, whichever event reduced first"
        );
        // back to paused for the next round
        shell.dispatch(transport(false), now);
        drain(&mut rx);
    }
}
