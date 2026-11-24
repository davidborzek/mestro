//! `reduce` on its own: what the end-to-end MA scenarios (in the
//! binary crate) do not pin down.

use super::*;

fn live(state: PlayerState) -> Live {
    let mut live = Live::new(state, Instant::now());
    live.created_at = Instant::now() - Duration::from_secs(10);
    live
}

fn queue_playing() -> Live {
    live(PlayerState {
        playing: true,
        title: "Teardrop".into(),
        artist: "Massive Attack".into(),
        album: "Mezzanine".into(),
        codec: "FLAC".into(),
        lossless: true,
        duration_ms: 330_000,
        position_ms: 59_000,
        has_media: true,
        ..Default::default()
    })
}

fn track(title: &str, artist: &str) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artist: artist.into(),
        album: "Album".into(),
        duration_ms: 179_000,
        ..Default::default()
    }
}

fn airplay() -> ExternalSource {
    ExternalSource {
        name: "AirPlay".into(),
        caps: SourceCaps {
            play_pause: false,
            seek: false,
            next_previous: false,
            shuffle: false,
            repeat: false,
            passive: true,
        },
        shuffle: false,
        repeat: RepeatMode::Off,
    }
}

fn external(track: TrackMeta, elapsed_sec: f64) -> Event {
    Event::External {
        source: airplay(),
        track: Some(track),
        elapsed_sec: Some(elapsed_sec),
    }
}

#[test]
fn an_external_reading_ends_a_local_seek_request() {
    let mut live = queue_playing();
    let now = Instant::now();
    reduce(&mut live, external(track("Song", "A"), 10.0), now);
    reduce(&mut live, Event::SeekRequested { seconds: 5 }, now);
    assert!(live.pending_seek.is_none(), "an external seek has no echo");
    live.pending_seek = Some((now, 5_000));
    reduce(&mut live, external(track("Song", "A"), 95.0), now);
    assert!(live.pending_seek.is_none());
    assert_eq!(live.state.position_ms, 95_000);
}

#[test]
fn the_reseed_is_never_rejected_by_the_replay_guard() {
    let mut live = queue_playing();
    let now = Instant::now();
    // the queue had just skipped away from "Earlier" when AirPlay took
    // over, and it is back on exactly that title
    live.recent_switch = Some((now, "Earlier".into()));
    reduce(&mut live, external(track("Stand by Me", "Ben"), 1.0), now);
    reduce(&mut live, Event::ExternalEnded, now);
    reduce(
        &mut live,
        Event::QueueSeed {
            track: track("Earlier", "X"),
            elapsed_sec: 9.0,
        },
        now,
    );
    assert_eq!(live.state.title, "Earlier");
    assert_eq!(live.state.position_ms, 9_000);
}

#[test]
fn the_queues_options_apply_again_once_the_source_ended() {
    let mut live = queue_playing();
    let now = Instant::now();
    reduce(&mut live, external(track("Song", "A"), 1.0), now);
    let options = || Event::Options {
        shuffle: true,
        repeat: RepeatMode::One,
    };
    reduce(&mut live, options(), now);
    assert!(!live.state.shuffle, "a source owns the options");
    reduce(&mut live, Event::ExternalEnded, now);
    reduce(&mut live, options(), now);
    assert!(live.state.shuffle);
    assert_eq!(live.state.repeat, RepeatMode::One);
}
