//! Scenarios 10–14 and the cover rules: AirPlay (and a Spotify-like
//! source) taking the display over from the queue and handing it back.
//! The AirPlay frames are the captures in `tests/fixtures/ma/`.

use super::harness::{a, b, c, connect, cover_url, fixture, skipped_to, QueueUpdate, Scenario};
use crate::player::state::{PlayerState, Toast};
use crate::providers::{ExternalSource, RepeatMode, SourceCaps, SourceEvent, TrackMeta};
use serde_json::{json, Value};

const SKIP_TITLE: &str = "I Was Made for Lovin’ You";

const AIRPLAY_ART: &str = "http://ma.test:8095/imageproxy/7c47ad991a1d0d057e5952472824556d2fd55efcd016123ca8e99ec723be0704?size=512&fmt=jpg";

fn nothing_can_be_done() -> SourceCaps {
    SourceCaps {
        play_pause: false,
        seek: false,
        next_previous: false,
        shuffle: false,
        repeat: false,
        passive: true,
    }
}

/// the frame after the user skipped on the phone: the new title
/// arrives with the previous track's clock still attached
fn skipped_title_old_clock() -> Value {
    let mut frame = fixture("airplay_track");
    frame["data"]["current_media"]["title"] = SKIP_TITLE.into();
    frame["data"]["current_media"]["artist"] = "KISS".into();
    frame
}

/// ... and a moment later the clock catches up
fn skipped_fresh_clock() -> Value {
    let mut frame = skipped_title_old_clock();
    frame["data"]["current_media"]["elapsed_time"] = 1.into();
    frame["data"]["current_media"]["elapsed_time_last_updated"] = 1795350824.3236344.into();
    frame
}

fn without_artwork(mut frame: Value) -> Value {
    frame["data"]["current_media"]["image_url"] = Value::Null;
    frame
}

/// the source the AirPlay fixtures translate to — what a scenario-built
/// `SourceEvent::External` must name to be the SAME source, not a new one
fn airplay_receiver() -> ExternalSource {
    ExternalSource {
        name: "AirPlay Receiver".into(),
        caps: nothing_can_be_done(),
        shuffle: false,
        repeat: RepeatMode::Off,
    }
}

/// the track the skip fixture carries, as a scenario-built event
fn kiss_track() -> TrackMeta {
    TrackMeta {
        title: SKIP_TITLE.into(),
        artist: "KISS".into(),
        album: "Stand by Me".into(),
        image_url: AIRPLAY_ART.into(),
        duration_ms: 179_000,
        ..Default::default()
    }
}

fn powered(mut frame: Value, powered: bool) -> Value {
    frame["data"]["powered"] = powered.into();
    frame
}

/// the queue's player switched to a Spotify-Connect-like source
fn spotify_like(title: &str) -> Value {
    let mut frame = fixture("airplay_track");
    let data = &mut frame["data"];
    data["active_source"] = "spotify_connect://audio_source/x".into();
    data["source_list"].as_array_mut().unwrap().push(json!({
        "id": "spotify_connect://audio_source/x",
        "name": "Spotify Connect (HiFi)",
        "passive": false,
        "can_play_pause": true, "can_seek": true, "can_next_previous": true,
        "can_shuffle": true, "can_repeat": false,
        "shuffle_enabled": true, "repeat_mode": "all",
    }));
    data["current_media"]["title"] = title.into();
    data["current_media"]["artist"] = "Band".into();
    data["current_media"]["album"] = "Record".into();
    frame
}

fn titles(s: &mut Scenario) -> Vec<String> {
    s.published()
        .iter()
        .map(|u| u.state.title.clone())
        .collect()
}

fn assert_near(actual: u64, expected: u64) {
    assert!(actual.abs_diff(expected) <= 1, "{actual} ≉ {expected}");
}

// 10. AirPlay: placeholder, track, skip --------------------------------

#[test]
fn a_placeholder_names_the_source_and_shows_no_media() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.land_cover(&cover_url("a"), true);
    s.published();
    s.player_frame(&fixture("airplay_placeholder"), false);
    let state = s.state();
    assert_eq!(state.external.as_deref(), Some("AirPlay Receiver"));
    assert!(!state.has_media, "the queue's track must not linger");
    assert_eq!((state.title.as_str(), state.artist.as_str()), ("", ""));
    assert_eq!(state.codec, "");
    assert!(!state.lossless);
    assert_eq!((state.duration_ms, state.position_ms), (0, 0));
    assert_eq!(state.source_caps, nothing_can_be_done());
    assert!(
        state.cover_missing,
        "the queue's cover is not this source's"
    );
    // the placeholder frame is powered off + idle: the transport says so
    assert!(!state.powered);
    assert!(!state.playing);
    // no artwork, nothing to fetch
    assert_eq!(s.cover_requests(), [cover_url("a")]);
    // the ticker has nothing to run
    assert_eq!(s.tick(), (0, 0, false));
}

#[test]
fn a_real_track_brings_metadata_artwork_and_a_running_bar() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.land_cover(&cover_url("a"), true);
    s.player_frame(&fixture("airplay_placeholder"), false);
    s.advance(1_000);
    s.published();
    s.player_frame(&fixture("airplay_track"), true);
    let published = s.published();
    // the transport snapshot first, then the source's track
    assert_eq!(published.len(), 2);
    assert_eq!(published[0].state.title, "");
    let state = &published[1].state;
    assert_eq!(state.external.as_deref(), Some("AirPlay Receiver"));
    assert_eq!(
        (state.title.as_str(), state.artist.as_str()),
        ("Stand by Me", "Ben E. King")
    );
    assert_eq!(state.album, "Stand by Me");
    assert_eq!(state.duration_ms, 179_000);
    // the capture is stamped 1.71 s before this reading: carried forward
    assert_near(state.position_ms, 1_712);
    assert!(state.has_media);
    assert!(state.playing);
    assert_eq!(state.source_caps, nothing_can_be_done());
    assert!(!state.shuffle && state.repeat == RepeatMode::Off);
    assert_eq!(
        (state.codec.as_str(), state.rate.as_str(), state.lossless),
        ("", "", false)
    );
    assert!(matches!(
        &published[1].toast,
        Some(Toast::Track(text)) if text == "Stand by Me — Ben E. King"
    ));
    // the artwork is requested through the configured MA host, and the
    // placeholder holds until it lands
    assert_eq!(
        s.cover_requests(),
        [cover_url("a"), AIRPLAY_ART.to_string()]
    );
    assert!(state.cover_missing);
    let landed = s.land_cover(AIRPLAY_ART, true);
    assert_eq!(landed.len(), 1);
    assert!(landed[0].cover.is_some() && landed[0].bg.is_some());
    assert!(!landed[0].state.cover_missing);
    // the bar runs on its own between readings
    s.advance(10_000);
    let (position, duration, loading) = s.tick();
    assert_near(position, 11_712);
    assert_eq!((duration, loading), (179_000, false));
}

#[test]
fn repeated_frames_publish_only_transport() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.published();
    for _ in 0..3 {
        s.advance(1_000);
        s.player_frame(&fixture("airplay_track"), true);
    }
    let published = s.published();
    assert_eq!(published.len(), 3, "one transport snapshot per frame");
    assert!(published.iter().all(|u| u.toast.is_none()));
    assert_eq!(s.cover_requests().len(), 2);
}

#[test]
fn a_skip_restarts_the_bar_until_the_fresh_clock_arrives() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(15_000);
    assert_near(s.tick().0, 15_712);
    s.published();

    // the title moves first, the clock still belongs to the old track
    s.player_frame(&skipped_title_old_clock(), true);
    let published = s.published();
    let state = &published[1].state;
    assert_eq!(state.title, SKIP_TITLE);
    assert_eq!(state.artist, "KISS");
    assert_eq!(state.position_ms, 0);
    assert!(matches!(
        &published[1].toast,
        Some(Toast::Track(text)) if text == &format!("{SKIP_TITLE} — KISS")
    ));
    // the same artwork url: no second fetch
    assert_eq!(s.cover_requests().len(), 2);

    // the bar runs from zero meanwhile
    s.advance(3_000);
    assert_eq!(s.tick().0, 3_000);

    // MA's fresh reading (stamped 1795350824.32, read at ...824.8)
    s.advance(0);
    let seen = s.wall_now();
    assert!((seen - 1795350824.0).abs() < 1.0, "{seen}");
    s.player_frame(&skipped_fresh_clock(), true);
    let state = s.state();
    assert_eq!(state.title, SKIP_TITLE);
    // 1 s + the reading's age at that moment
    assert_near(
        state.position_ms,
        1_000 + ((seen - 1795350824.3236344) * 1000.0) as u64,
    );
    let published = s.published();
    assert!(published[1].toast.is_none(), "same title: no second toast");
}

#[test]
fn a_repeated_unchanged_event_without_a_reading_does_not_restart_the_bar() {
    // MA can repeat the shown track while its clock still lags the
    // skip; the tracker drops such frames as unchanged, so this event
    // is fed to the core directly — it must leave the running bar alone
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(15_000);
    s.published();

    s.player_frame(&skipped_title_old_clock(), true);
    assert_eq!(s.state().title, SKIP_TITLE);
    assert_eq!(s.state().position_ms, 0, "the skip restarts the bar");
    s.published();
    s.advance(3_000);
    assert_eq!(s.tick().0, 3_000);

    s.send(SourceEvent::External {
        source: airplay_receiver(),
        track: Some(kiss_track()),
        elapsed_sec: None,
    });
    let published = s.published();
    let state = s.state();
    assert_eq!(state.title, SKIP_TITLE);
    assert_eq!(
        state.position_ms, 3_000,
        "the unchanged track without a reading does not restart the bar"
    );
    assert!(
        published.iter().all(|u| u.toast.is_none()),
        "same title: no toast"
    );
    // nothing new to fetch either
    assert_eq!(s.cover_requests().len(), 2);
    // and the bar keeps running on its own
    s.advance(2_000);
    assert_eq!(s.tick().0, 5_000);
}

#[test]
fn the_queues_events_say_nothing_while_a_source_plays() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(5_000);
    s.published();
    // a web client edits the queue: events keep arriving for it
    s.queue_updated(QueueUpdate {
        shuffle: true,
        repeat: "all",
        ..skipped_to(b(), 3.0, 1)
    });
    s.queue_time(3.0);
    let state = s.state();
    assert_eq!(state.title, "Stand by Me");
    assert!(!state.shuffle);
    assert_eq!(state.repeat, RepeatMode::Off);
    assert_near(state.position_ms, 712);
    // the queue's cover is not asked for either
    assert_eq!(s.cover_requests().len(), 2);
    // a cleared queue says nothing about AirPlay
    s.queue_updated(QueueUpdate::cleared());
    s.queue_items(&[], usize::MAX);
    assert!(s.state().has_media);
    assert_eq!(s.state().title, "Stand by Me");
}

#[test]
fn the_source_name_comes_without_the_players_suffix() {
    let mut s = Scenario::new();
    s.player_frame(&fixture("airplay_placeholder"), false);
    assert_eq!(s.state().external.as_deref(), Some("AirPlay Receiver"));
}

// 11. back to the queue -------------------------------------------------

#[test]
fn back_on_the_queue_the_reseed_shows_its_track_at_once() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.land_cover(&cover_url("a"), true);
    s.player_frame(&fixture("airplay_track"), true);
    s.land_cover(AIRPLAY_ART, true);
    s.advance(60_000);
    s.published();

    s.player_frame(&fixture("queue_back"), false);
    assert!(s.reseed_requested, "the session asks for the queue again");
    let state = s.state();
    assert_eq!(state.external, None);
    assert!(!state.has_media, "the external track must not linger");
    assert_eq!(state.title, "");
    assert_eq!(state.source_caps, SourceCaps::default());
    assert!(!state.shuffle && state.repeat == RepeatMode::Off);
    assert!(state.cover_missing, "AirPlay's cover is not the queue's");

    s.published();
    s.seed(QueueUpdate::playing(a(), 90.0));
    assert!(!s.reseed_requested);
    // CHARACTERIZED: looks wrong because the seed's Position event comes
    // first and meets the wiped bar (0 ms): 90 s away, it is held back as
    // an outlier and publishes nothing; the QueueSeed right after sets
    // the bar. Harmless today, but the guard works against the seed here
    let published = s.published();
    assert_eq!(published.len(), 2, "seed + options, no position update");
    assert_eq!(published[0].state.position_ms, 90_000);
    let state = s.state();
    assert_eq!(state.title, "Teardrop");
    assert_eq!(state.position_ms, 90_000);
    assert_eq!(state.duration_ms, 200_000);
    assert!(state.has_media);
    assert_eq!(state.codec, "FLAC");
    assert!(state.cover_missing, "until the queue's cover lands");
    // the queue's cover is asked for again, whatever came before
    assert_eq!(
        s.cover_requests(),
        [cover_url("a"), AIRPLAY_ART.to_string(), cover_url("a")]
    );
    let landed = s.land_cover(&cover_url("a"), true);
    assert!(!landed[0].state.cover_missing);
}

#[test]
fn leaving_a_source_that_was_never_active_changes_nothing() {
    // the tracker settles on the first queue frame and never re-emits
    // the end; a stray ExternalEnded against a playing queue must be a
    // no-op, not a wipe of the shown track
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.land_cover(&cover_url("a"), true);
    s.published();

    s.send(SourceEvent::ExternalEnded);
    let state = s.state();
    assert_eq!(state.title, "Teardrop", "the queue's track is kept");
    assert!(state.has_media);
    assert_eq!(state.external, None);
    assert!(s.published().is_empty(), "nothing is published");
    // and the bar keeps running
    s.advance(2_000);
    assert_eq!(s.tick().0, 32_000);
}

#[test]
fn a_late_replay_of_the_item_before_the_episode_is_dropped_for_three_seconds() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.advance(5_000);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    s.advance(5_000);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(20_000);
    s.player_frame(&fixture("queue_back"), false);
    s.seed(skipped_to(b(), 30.0, 1));
    assert_eq!(s.state().title, "Angel");

    // queue_updated still carries the item from before the episode
    s.advance(500);
    s.queue_updated(skipped_to(a(), 12.0, 0));
    assert_eq!(s.state().title, "Angel");
    assert_eq!(s.state().position_ms, 30_000);
    s.queue_time(12.5);
    assert_eq!(s.state().position_ms, 30_000, "the replayed clock is held");

    // a genuine new item is not a replay
    s.advance(500);
    s.queue_updated(skipped_to(c(), 0.0, 2));
    assert_eq!(s.state().title, "Inertia Creeps");
    assert_eq!(s.state().position_ms, 0);
}

#[test]
fn the_replay_guard_after_the_reseed_runs_from_the_reseed() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.advance(5_000);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    s.advance(10_000);
    s.player_frame(&fixture("airplay_track"), true);
    s.player_frame(&fixture("queue_back"), false);
    s.seed(skipped_to(b(), 15.0, 1));
    s.advance(3_000);
    // three seconds after the reseed (not after the original skip)
    // the replay is a real change again
    s.queue_updated(skipped_to(a(), 12.0, 0));
    assert_eq!(s.state().title, "Teardrop");
    assert_eq!(s.state().position_ms, 12_000);
}

#[test]
fn a_reseed_to_the_item_that_was_shown_before_the_source_keeps_the_old_guard() {
    // the queue showed B when AirPlay came; back on B, the item it
    // replaces in the guard is still A
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    s.advance(5_000);
    s.player_frame(&fixture("airplay_track"), true);
    s.player_frame(&fixture("queue_back"), false);
    // MA moved on while AirPlay played: the queue is now at C
    s.seed(skipped_to(c(), 0.0, 2));
    s.advance(500);
    // what a late queue_updated may replay is B (shown before AirPlay)
    s.queue_updated(skipped_to(b(), 20.0, 1));
    assert_eq!(s.state().title, "Inertia Creeps");
    // A is not distrusted any more
    s.queue_updated(skipped_to(a(), 0.0, 0));
    assert_eq!(s.state().title, "Teardrop");
}

#[test]
fn a_reconnect_during_a_source_clears_it_through_the_first_queue_frame() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(10_000);
    // the connection dies; the player went back to the queue meanwhile
    s.reconnect();
    s.player_frame(&fixture("queue_back"), true);
    assert!(
        !s.reseed_requested,
        "the fresh session seeds the queue itself"
    );
    let state = s.state();
    assert_eq!(state.external, None);
    assert!(!state.has_media);
    s.seed(QueueUpdate::playing(a(), 40.0));
    assert_eq!(s.state().title, "Teardrop");
    assert_eq!(s.state().position_ms, 40_000);
}

#[test]
fn a_reconnect_during_a_source_that_still_plays_corrects_the_clock() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.published();
    s.advance(20_000);
    s.reconnect();
    s.player_frame(&fixture("airplay_track"), true);
    // a fresh tracker repeats the event: the reading is taken anew
    assert_eq!(s.state().title, "Stand by Me");
    assert_near(s.state().position_ms, 20_712);
    // not a track change: no second toast
    assert!(s.published().iter().all(|u| u.toast.is_none()));
}

// 12. covers --------------------------------------------------------------

#[test]
fn a_late_cover_of_the_previous_external_track_leaves_the_placeholder() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    // the fetch for A is in flight; AirPlay moves on to B, no artwork
    s.advance(2_000);
    s.player_frame(&without_artwork(skipped_title_old_clock()), true);
    assert_eq!(s.state().title, SKIP_TITLE);
    assert!(s.state().cover_missing);
    s.published();
    let landed = s.land_cover(AIRPLAY_ART, true);
    assert!(landed.is_empty(), "nothing is published for a stale cover");
    assert!(s.state().cover_missing, "B keeps the placeholder");
}

#[test]
fn artwork_that_comes_back_after_a_track_without_is_fetched_again() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(2_000);
    s.player_frame(&without_artwork(skipped_title_old_clock()), true);
    assert_eq!(
        s.cover_requests(),
        [cover_url("a"), AIRPLAY_ART.to_string()]
    );
    let mut again = fixture("airplay_track");
    again["data"]["current_media"]["title"] = "Another One".into();
    s.player_frame(&again, true);
    assert_eq!(
        s.cover_requests(),
        [
            cover_url("a"),
            AIRPLAY_ART.to_string(),
            AIRPLAY_ART.to_string()
        ]
    );
}

#[test]
fn a_failed_external_cover_keeps_the_placeholder() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.published();
    let landed = s.land_cover(AIRPLAY_ART, false);
    assert_eq!(landed.len(), 1);
    assert!(landed[0].cover.is_none());
    assert!(landed[0].state.cover_missing);
}

#[test]
fn a_late_cover_of_the_previous_queue_track_is_dropped() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.advance(5_000);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    s.published();
    assert!(s.land_cover(&cover_url("a"), true).is_empty());
    let landed = s.land_cover(&cover_url("b"), true);
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].state.title, "Angel");
    assert!(!landed[0].state.cover_missing);
}

#[test]
fn a_queue_cover_that_lands_after_an_external_source_took_over_is_dropped() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_placeholder"), false);
    s.published();
    assert!(s.land_cover(&cover_url("a"), true).is_empty());
    assert!(s.state().cover_missing);
}

#[test]
fn a_failed_queue_cover_publishes_but_does_not_raise_the_placeholder() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.published();
    let landed = s.land_cover(&cover_url("a"), false);
    assert_eq!(landed.len(), 1);
    assert!(landed[0].cover.is_none());
    // CHARACTERIZED: looks wrong because a failed fetch on the queue
    // leaves `cover_missing` false, so the previous track's cover
    // stays on screen under the new title (only the external path
    // flips the flag on failure)
    assert!(!landed[0].state.cover_missing);
}

// 13. sources with capabilities ----------------------------------------

#[test]
fn a_source_with_capabilities_enables_them_and_owns_the_options() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&spotify_like("Song"), true);
    let state = s.state();
    assert_eq!(state.external.as_deref(), Some("Spotify Connect"));
    assert_eq!(
        state.source_caps,
        SourceCaps {
            play_pause: true,
            seek: true,
            next_previous: true,
            shuffle: true,
            repeat: false,
            passive: false,
        }
    );
    assert!(state.shuffle);
    assert_eq!(state.repeat, RepeatMode::All);
    assert_eq!(
        (
            state.title.as_str(),
            state.artist.as_str(),
            state.album.as_str()
        ),
        ("Song", "Band", "Record")
    );
    // the queue's options cannot override the source's
    s.queue_updated(QueueUpdate::playing(a(), 0.0));
    assert!(s.state().shuffle);
    assert_eq!(s.state().repeat, RepeatMode::All);
}

#[test]
fn a_source_gaining_a_capability_later_updates_the_state() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    assert!(!s.state().source_caps.next_previous);
    let mut richer = fixture("airplay_track");
    richer["data"]["source_list"][1]["can_next_previous"] = true.into();
    s.advance(1_000);
    s.player_frame(&richer, true);
    assert!(s.state().source_caps.next_previous);
    assert!(!s.state().source_caps.seek);
}

#[test]
fn switching_from_one_source_to_another_wipes_the_first_ones_media() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(10_000);
    s.published();
    s.player_frame(&spotify_like("Song"), true);
    let state = s.state();
    assert_eq!(state.external.as_deref(), Some("Spotify Connect"));
    assert_eq!(state.title, "Song");
    assert_eq!(state.position_ms, 10_712);
    assert!(state.source_caps.seek);
    // the cover of the new source is fetched (same artwork url as
    // AirPlay's capture: the wipe resets what was fetched last)
    assert_eq!(
        s.cover_requests(),
        [
            cover_url("a"),
            AIRPLAY_ART.to_string(),
            AIRPLAY_ART.to_string()
        ]
    );
    assert!(state.cover_missing);
    // ending the source goes through the queue path like any other
    s.player_frame(&fixture("queue_back"), false);
    assert!(s.reseed_requested);
    assert_eq!(s.state().source_caps, SourceCaps::default());
    assert_eq!(titles(&mut s).last().map(String::as_str), Some(""));
}

// 14. power ----------------------------------------------------------------

#[test]
fn powering_off_while_a_source_plays_keeps_it_on_screen_and_flips_powered() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.player_frame(&fixture("airplay_track"), true);
    s.advance(5_000);
    s.published();
    s.player_frame(&powered(fixture("airplay_track"), false), false);
    let published = s.published();
    // one snapshot: the transport (the source frame itself is unchanged)
    assert_eq!(published.len(), 1);
    let state = &published[0].state;
    assert!(!state.powered);
    assert!(!state.playing);
    assert_eq!(state.external.as_deref(), Some("AirPlay Receiver"));
    assert_eq!(state.title, "Stand by Me");
    assert!(state.has_media);
    assert_eq!(
        published[0]
            .toast
            .as_ref()
            .map(|t| matches!(t, Toast::Transport(text) if text == "Paused")),
        Some(true)
    );
}

#[test]
fn a_remote_airplay_start_wakes_the_player_before_its_track_arrives() {
    // the standby wake (src/standby/wake.rs) acts on the off → on edge
    // of the published `powered`; what the snapshot carries at that
    // moment is what now playing would open on
    let mut s = Scenario::new();
    s.player_frame(&fixture("airplay_placeholder"), false);
    s.advance(1_000);
    s.published();
    s.player_frame(&fixture("airplay_track"), true);
    let published = s.published();
    let powered: Vec<bool> = published.iter().map(|u| u.state.powered).collect();
    assert_eq!(powered, [true, true]);
    // the edge snapshot is the transport event, ahead of the track
    assert_eq!(published[0].state.title, "");
    assert!(!published[0].state.has_media);
    assert_eq!(published[1].state.title, "Stand by Me");
    let state: PlayerState = s.state();
    assert!(state.powered && state.playing);
}

#[test]
fn powering_off_the_queue_keeps_the_track_and_the_bar_stops() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    s.advance(2_000);
    assert_eq!(s.tick().0, 32_000);
    let mut off = fixture("queue_back");
    off["data"]["powered"] = false.into();
    s.published();
    s.player_frame(&off, false);
    let state = s.state();
    assert!(!state.powered);
    assert_eq!(state.title, "Teardrop");
    s.advance(10_000);
    assert_eq!(s.tick().0, 32_000);
}
