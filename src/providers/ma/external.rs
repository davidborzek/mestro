//! External sources (AirPlay, Spotify Connect, ...) as the session
//! sees them: the player's `active_source` leaves the queue, the
//! queue goes silent and `current_media` becomes the one place that
//! knows the track. The tracker turns each player snapshot into the
//! resolved events for the core (`SourceEvent::External` /
//! `ExternalEnded`) — what the core sees is never a placeholder, a
//! stale clock or a repeat of the last event.

use super::parse::{ExternalMedia, PlayerSnapshot, PlayerSource};
use crate::providers::{ExternalSource, SourceEvent, TrackMeta};

/// what the last emitted External event said
struct Shown {
    source: ExternalSource,
    track: Option<TrackMeta>,
    /// the MA clock reading that event was built from
    reading: (u64, u64),
}

pub(super) struct ExternalStep {
    pub(super) events: Vec<SourceEvent>,
    /// the player just left an external source: the session
    /// re-seeds the display from the queue
    pub(super) back_on_queue: bool,
}

#[derive(Default)]
pub(super) struct ExternalTracker {
    shown: Option<Shown>,
    /// the core has been told what kind of source plays — a fresh
    /// session starts unsettled: the core may still show an external
    /// source from before the reconnect
    settled: bool,
}

fn same_track(a: &TrackMeta, b: &TrackMeta) -> bool {
    a.title == b.title
        && a.artist == b.artist
        && a.album == b.album
        && a.image_url == b.image_url
        && a.duration_ms == b.duration_ms
}

impl ExternalTracker {
    /// one snapshot of the display's player; `now_unix` dates the
    /// carried-forward position
    pub(super) fn observe(&mut self, snapshot: &PlayerSnapshot, now_unix: f64) -> ExternalStep {
        let nothing = ExternalStep {
            events: Vec::new(),
            back_on_queue: false,
        };
        let PlayerSource::External(media) = &snapshot.source else {
            let was_external = self.shown.take().is_some();
            let first = !std::mem::replace(&mut self.settled, true);
            return if was_external || first {
                ExternalStep {
                    events: vec![SourceEvent::ExternalEnded],
                    // the startup path seeds the queue itself
                    back_on_queue: was_external,
                }
            } else {
                nothing
            };
        };
        let Some(shown) = self
            .shown
            .as_ref()
            .filter(|s| s.source.name == media.source.name)
        else {
            // a new source: whatever the queue showed is wiped first
            return self.emit(media, snapshot.playing, now_unix, None);
        };
        let unchanged = match (&shown.track, &media.track) {
            (Some(a), Some(b)) => same_track(a, b),
            (None, None) => true,
            _ => false,
        };
        // (a source whose caps or own state moved is a change too)
        if unchanged && shown.reading == media.reading_key() && shown.source == media.source {
            return nothing;
        }
        // a placeholder in the middle of a playing source is MA
        // reshuffling its own state, not the end of the track
        if media.track.is_none() && shown.track.is_some() && snapshot.playing {
            return nothing;
        }
        let previous_reading = shown.reading;
        self.emit(media, snapshot.playing, now_unix, Some(previous_reading))
    }

    fn emit(
        &mut self,
        media: &ExternalMedia,
        playing: bool,
        now_unix: f64,
        previous_reading: Option<(u64, u64)>,
    ) -> ExternalStep {
        let reading = media.reading_key();
        // MA updates the title first and the clock seconds later: a
        // changed track still carrying the previous clock reading must
        // not take it for its position (the core restarts the bar)
        let clock_is_new = previous_reading != Some(reading);
        let elapsed_sec = clock_is_new.then(|| media.position_at(now_unix, playing));
        self.settled = true;
        self.shown = Some(Shown {
            source: media.source.clone(),
            track: media.track.clone(),
            reading,
        });
        ExternalStep {
            events: vec![SourceEvent::External {
                source: media.source.clone(),
                track: media.track.clone(),
                elapsed_sec,
            }],
            back_on_queue: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ma::parse::parse_player;
    use serde_json::Value;

    const BASE: &str = "http://ma.test:8095";

    fn frame(json: &str) -> Value {
        let frame: Value = serde_json::from_str(json).unwrap();
        frame["data"].clone()
    }

    fn placeholder() -> Value {
        frame(include_str!(
            "../../../tests/fixtures/ma/airplay_placeholder.json"
        ))
    }

    fn track() -> Value {
        frame(include_str!(
            "../../../tests/fixtures/ma/airplay_track.json"
        ))
    }

    fn queue_back() -> Value {
        frame(include_str!("../../../tests/fixtures/ma/queue_back.json"))
    }

    /// the frame after the user skipped: the new title arrives with
    /// the previous track's clock still attached
    fn skipped_title_old_clock() -> Value {
        let mut v = track();
        v["current_media"]["title"] = "I Was Made for Lovin’ You".into();
        v["current_media"]["artist"] = "KISS".into();
        v
    }

    /// ... and a moment later the clock catches up
    fn skipped_fresh_clock() -> Value {
        let mut v = skipped_title_old_clock();
        v["current_media"]["elapsed_time"] = 1.into();
        v["current_media"]["elapsed_time_last_updated"] = 1795350824.3236344.into();
        v
    }

    fn step(tracker: &mut ExternalTracker, v: &Value, playing: bool, now: f64) -> ExternalStep {
        let mut v = v.clone();
        v["playback_state"] = if playing { "playing" } else { "idle" }.into();
        tracker.observe(&parse_player(&v, BASE).unwrap(), now)
    }

    fn external(step: &ExternalStep) -> (&str, Option<&str>, Option<f64>) {
        match step.events.as_slice() {
            [SourceEvent::External {
                source,
                track,
                elapsed_sec,
            }] => (
                source.name.as_str(),
                track.as_ref().map(|t| t.title.as_str()),
                *elapsed_sec,
            ),
            other => panic!("expected one External event, got {other:?}"),
        }
    }

    #[test]
    fn placeholder_track_skip_and_back_emit_the_resolved_events() {
        let mut tracker = ExternalTracker::default();

        // 1. the source starts with MA's placeholder: named, no metadata
        let first = step(&mut tracker, &placeholder(), false, 1795350321.5);
        assert_eq!(external(&first).0, "AirPlay Receiver");
        assert_eq!(external(&first).1, None);
        assert!(!first.back_on_queue);

        // 2. the real track: title, a position carried by the reading's age
        let second = step(&mut tracker, &track(), true, 1795350807.0);
        let (source, title, elapsed) = external(&second);
        assert_eq!((source, title), ("AirPlay Receiver", Some("Stand by Me")));
        assert!((elapsed.unwrap() - 1.71).abs() < 0.01, "{elapsed:?}");

        // 3. the skip: the title moves first, the clock still belongs
        // to the previous track — the core must not take it
        let skip = step(&mut tracker, &skipped_title_old_clock(), true, 1795350821.4);
        let (_, title, elapsed) = external(&skip);
        assert_eq!(title, Some("I Was Made for Lovin’ You"));
        assert_eq!(elapsed, None, "the previous track's clock is stale");

        // 4. the fresh clock arrives
        let clock = step(&mut tracker, &skipped_fresh_clock(), true, 1795350824.8);
        let (_, title, elapsed) = external(&clock);
        assert_eq!(title, Some("I Was Made for Lovin’ You"));
        assert!((elapsed.unwrap() - 1.48).abs() < 0.01, "{elapsed:?}");

        // 5. back on the queue: the external state ends and the
        // session is told to re-seed from the queue
        let back = step(&mut tracker, &queue_back(), false, 1795350874.2);
        assert!(matches!(
            back.events.as_slice(),
            [SourceEvent::ExternalEnded]
        ));
        assert!(back.back_on_queue);
    }

    #[test]
    fn repeated_snapshots_emit_nothing() {
        // player_updated fires for volume, power, outputs ... — only a
        // change of the shown source/track/clock is an event
        let mut tracker = ExternalTracker::default();
        assert_eq!(
            step(&mut tracker, &track(), true, 1795350806.0)
                .events
                .len(),
            1
        );
        for _ in 0..3 {
            let again = step(&mut tracker, &track(), true, 1795350810.0);
            assert!(again.events.is_empty());
            assert!(!again.back_on_queue);
        }
        let mut quieter = track();
        quieter["volume_level"] = 10.into();
        assert!(step(&mut tracker, &quieter, true, 1795350811.0)
            .events
            .is_empty());
    }

    #[test]
    fn a_queue_player_tells_the_core_once_and_never_reseeds() {
        // the first snapshot of a session settles a core that may still
        // show an external source from before a reconnect; the startup
        // path seeds the queue itself
        let mut tracker = ExternalTracker::default();
        let first = step(&mut tracker, &queue_back(), true, 1795350874.0);
        assert!(matches!(
            first.events.as_slice(),
            [SourceEvent::ExternalEnded]
        ));
        assert!(!first.back_on_queue);
        for _ in 0..3 {
            let again = step(&mut tracker, &queue_back(), true, 1795350875.0);
            assert!(again.events.is_empty());
            assert!(!again.back_on_queue);
        }
    }

    #[test]
    fn a_placeholder_during_playback_does_not_wipe_the_shown_track() {
        let mut tracker = ExternalTracker::default();
        step(&mut tracker, &track(), true, 1795350806.0);
        let mut flapping = placeholder();
        flapping["playback_state"] = "playing".into();
        assert!(step(&mut tracker, &flapping, true, 1795350807.0)
            .events
            .is_empty());
        // once the source goes idle the placeholder is the truth again
        let idle = step(&mut tracker, &placeholder(), false, 1795350900.0);
        assert_eq!(external(&idle).1, None);
    }

    #[test]
    fn a_change_of_the_sources_caps_is_an_event() {
        let mut tracker = ExternalTracker::default();
        step(&mut tracker, &track(), true, 1795350806.0);
        let mut richer = track();
        richer["source_list"][1]["can_next_previous"] = true.into();
        let changed = step(&mut tracker, &richer, true, 1795350807.0);
        match changed.events.as_slice() {
            [SourceEvent::External { source, .. }] => assert!(source.caps.next_previous),
            other => panic!("expected one External event, got {other:?}"),
        }
        assert!(step(&mut tracker, &richer, true, 1795350808.0)
            .events
            .is_empty());
    }

    #[test]
    fn the_source_flapping_back_and_forth_ends_and_restarts_cleanly() {
        let mut tracker = ExternalTracker::default();
        step(&mut tracker, &track(), true, 1795350806.0);
        let back = step(&mut tracker, &queue_back(), false, 1795350807.0);
        assert!(back.back_on_queue);
        // AirPlay again: a fresh External, the old track is not replayed
        // as "unchanged"
        let again = step(&mut tracker, &placeholder(), false, 1795350808.0);
        assert_eq!(external(&again).1, None);
        assert!(!again.back_on_queue);
    }

    #[test]
    fn a_source_switch_seeds_the_new_sources_position() {
        let mut tracker = ExternalTracker::default();
        step(&mut tracker, &track(), true, 1795350806.0);
        let mut spotify = track();
        spotify["active_source"] = "spotify_connect://audio_source/x".into();
        spotify["source_list"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": "spotify_connect://audio_source/x",
                "name": "Spotify Connect (HiFi)",
            }));
        let switched = step(&mut tracker, &spotify, true, 1795350807.0);
        let (source, _, elapsed) = external(&switched);
        assert_eq!(source, "Spotify Connect");
        assert!(elapsed.is_some());
    }
}
