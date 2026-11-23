//! Scenarios 1–9: the MA queue as the source of what plays.

use super::harness::{
    a, b, c, connect, cover_url, fixture, item, skipped_to, QueueUpdate, Scenario,
};
use crate::player::state::{PlayerUpdate, Toast};
use crate::providers::RepeatMode;

fn toast(update: &PlayerUpdate) -> String {
    match &update.toast {
        None => "-".into(),
        Some(Toast::Transport(text)) => format!("transport: {text}"),
        Some(Toast::Track(text)) => format!("track: {text}"),
        Some(Toast::Queue(text)) => format!("queue: {text}"),
    }
}

/// the 500 ms ticker in main, for `ms`
fn run(s: &mut Scenario, ms: u64) {
    for _ in 0..ms / 500 {
        s.advance(500);
        s.tick();
    }
}

// 1. initial seed ------------------------------------------------------

#[test]
fn the_initial_seed_shows_the_track_and_asks_for_one_cover() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 12.5);
    let state = s.state();
    assert_eq!(state.title, "Teardrop");
    assert_eq!(state.artist, "Massive Attack");
    assert_eq!(state.album, "Album of Teardrop");
    assert_eq!(state.position_ms, 12_500);
    assert_eq!(state.duration_ms, 200_000);
    assert!(state.has_media);
    assert!(state.playing);
    assert_eq!(state.external, None);
    assert_eq!(state.codec, "FLAC");
    assert!(state.lossless);
    assert_eq!(
        (
            state.rate.as_str(),
            state.depth.as_str(),
            state.channels.as_str()
        ),
        ("44.1 kHz", "16 bit", "2 ch")
    );
    assert_eq!(state.bitrate, "");
    assert_eq!(state.queue.len(), 3);
    assert_eq!(state.queue_current, 0);
    // the queue's cover is not fetched yet: the placeholder stays
    // until it lands
    assert_eq!(s.cover_requests(), [cover_url("a")]);
}

#[test]
fn the_initial_events_publish_in_wire_order() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 12.5);
    let published = s.published();
    // transport, position, seed, options, queue list
    assert_eq!(published.len(), 5);
    let titles: Vec<&str> = published.iter().map(|u| u.state.title.as_str()).collect();
    assert_eq!(titles, ["", "", "Teardrop", "Teardrop", "Teardrop"]);
    assert_eq!(
        published.iter().map(toast).collect::<Vec<_>>(),
        ["-", "-", "track: Teardrop — Massive Attack", "-", "-"]
    );
    // CHARACTERIZED: looks wrong because the startup seed raises a
    // "track changed" toast for the track that was playing all along
    // (the OSD suppresses it only while now playing is open)
}

#[test]
fn the_position_before_the_seed_is_dropped_into_an_empty_display() {
    let mut s = Scenario::new();
    s.player_frame(&fixture("queue_back"), true);
    s.seed(QueueUpdate::playing(a(), 12.5));
    let published = s.published();
    // the seed's Position event lands before the track exists: the
    // bar value is set on a state without media
    assert_eq!(published[1].state.position_ms, 12_500);
    assert!(!published[1].state.has_media);
}

#[test]
fn a_track_before_any_other_event_is_dropped() {
    // CHARACTERIZED: looks wrong because nothing parks it: without a
    // transport, options or queue event there is no state yet, and the
    // track (and its cover request) is lost. The session never meets
    // this: players/all always answers first
    let mut s = Scenario::new();
    s.send(crate::providers::SourceEvent::Track {
        track: crate::providers::TrackMeta {
            title: "Lost".into(),
            image_url: cover_url("lost"),
            ..Default::default()
        },
        elapsed_sec: 0.0,
    });
    assert!(s.published().is_empty());
    assert!(s.cover_requests().is_empty());
    assert!(s.shared.lock().is_none());
}

#[test]
fn queue_additions_toast_only_after_the_first_snapshot() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 0.0);
    // the first snapshot (startup) raised no queue toast
    assert!(s
        .published()
        .iter()
        .all(|u| !matches!(u.toast, Some(Toast::Queue(_)))));
    let d = item("d", "Group Four", "Massive Attack", 200);
    let e = item("e", "Exchange", "Massive Attack", 200);
    s.queue_items(&[a(), b(), c(), d.clone()], 0);
    assert_eq!(
        s.published().iter().map(toast).collect::<Vec<_>>(),
        ["queue: Group Four — Massive Attack"]
    );
    s.queue_items(
        &[
            a(),
            b(),
            c(),
            d.clone(),
            e.clone(),
            item("f", "Man Next Door", "Massive Attack", 1),
        ],
        0,
    );
    assert_eq!(
        s.published().iter().map(toast).collect::<Vec<_>>(),
        ["queue: Exchange — Massive Attack (+1)"]
    );
    // a reorder is silent
    s.queue_items(&[b(), a(), c(), d, e], 1);
    assert_eq!(s.published().iter().map(toast).collect::<Vec<_>>(), ["-"]);
}

// 2. natural advance ---------------------------------------------------

#[test]
fn a_natural_advance_seeds_the_bar_from_the_events_own_elapsed() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 190.0);
    run(&mut s, 10_000);
    // the bar stops at the end of the track, it does not run past it
    assert_eq!(s.tick(), (200_000, 200_000, false));
    s.published();
    // MA reports the next item seconds after it started
    s.queue_updated(skipped_to(b(), 4.0, 1));
    let state = s.state();
    assert_eq!(state.title, "Angel");
    assert_eq!(state.position_ms, 4_000);
    assert_eq!(state.duration_ms, 180_000);
    assert_eq!(s.cover_requests(), [cover_url("a"), cover_url("b")]);
    let published = s.published();
    assert_eq!(toast(&published[0]), "track: Angel — Massive Attack");
    // the list follows with its own current index
    s.queue_items(&[a(), b(), c()], 1);
    assert_eq!(s.state().queue_current, 1);
    // the ticker carries on from the seeded position
    run(&mut s, 2_000);
    assert_eq!(s.tick(), (6_000, 180_000, false));
}

#[test]
fn an_unchanged_track_event_keeps_the_bar_and_the_cover() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 5_000);
    s.published();
    // queue_updated fires for many reasons (volume of the queue,
    // shuffle ...): the same item again must not restart anything
    s.queue_updated(skipped_to(a(), 0.0, 0));
    assert_eq!(s.state().position_ms, 15_000);
    assert_eq!(s.cover_requests(), [cover_url("a")]);
    assert_eq!(toast(&s.published()[0]), "-");
}

// 3. skip with stale replay -------------------------------------------

#[test]
fn a_replay_of_the_previous_item_after_a_skip_is_dropped() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    run(&mut s, 5_000);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    s.published();
    run(&mut s, 1_500);
    // ~1.5 s after the skip MA replays A with A's clock
    s.queue_updated(skipped_to(a(), 36.5, 0));
    let state = s.state();
    assert_eq!(state.title, "Angel");
    assert_eq!(state.position_ms, 1_500);
    assert_eq!(s.cover_requests(), [cover_url("a"), cover_url("b")]);
    // the replay still carried the options: they publish, the track doesn't
    let published = s.published();
    assert_eq!(published.len(), 1);
    assert_eq!(toast(&published[0]), "-");
    assert_eq!(published[0].state.title, "Angel");
    // and A's clock replays on queue_time: an outlier, held back
    s.queue_time(37.0);
    assert!(s.published().is_empty());
    assert_eq!(s.state().position_ms, 1_500);
    // B's own clock lets the held reading go
    s.queue_time(2.1);
    assert_eq!(s.state().position_ms, 2_100);
    assert_eq!(s.live(|live| live.pending_pos), None);
}

#[test]
fn the_replay_guard_runs_out_after_three_seconds() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    run(&mut s, 5_000);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    s.advance(2_999);
    s.queue_updated(skipped_to(a(), 33.0, 0));
    assert_eq!(s.state().title, "Angel", "2999 ms: still a replay");
    s.advance(1);
    s.queue_updated(skipped_to(a(), 33.0, 0));
    // the same replay, 3 s after the skip: accepted as a real change
    let state = s.state();
    assert_eq!(state.title, "Teardrop");
    assert_eq!(state.position_ms, 33_000);
    assert_eq!(
        s.cover_requests(),
        [cover_url("a"), cover_url("b"), cover_url("a")]
    );
    // ... and now B is the item to distrust
    s.advance(500);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    assert_eq!(s.state().title, "Teardrop");
}

#[test]
fn a_real_previous_within_the_window_is_dropped_like_a_replay() {
    // CHARACTERIZED: looks wrong because "previous" right after "next"
    // is the same event as MA's replay of the old item, so it is
    // swallowed: the display keeps showing B while A plays, until the
    // next queue_updated after the window
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    run(&mut s, 5_000);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    run(&mut s, 1_000);
    s.queue_updated(skipped_to(a(), 0.0, 0));
    assert_eq!(s.state().title, "Angel");
    run(&mut s, 1_000);
    s.queue_time(1.0);
    let state = s.state();
    assert_eq!(state.title, "Angel");
    assert_eq!(state.position_ms, 1_000);
}

#[test]
fn a_third_item_during_the_window_is_a_genuine_skip() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 30.0);
    run(&mut s, 5_000);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    run(&mut s, 500);
    s.queue_updated(skipped_to(c(), 0.0, 2));
    assert_eq!(s.state().title, "Inertia Creeps");
    assert_eq!(s.state().position_ms, 0);
}

// 4. position corrections ---------------------------------------------

#[test]
fn small_corrections_move_the_bar_and_outliers_wait_for_a_second_opinion() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 5_000);
    s.published();

    // drift: any correction within 2.5 s of the interpolation is taken
    s.queue_time(16.2);
    assert_eq!(s.state().position_ms, 16_200);
    assert_eq!(s.published().len(), 1);
    // the boundary: 2500 ms off is still drift
    s.queue_time(18.7);
    assert_eq!(s.state().position_ms, 18_700);
    s.published();
    // 2600 ms off: held back, nothing published, the bar keeps its own time
    s.queue_time(21.3);
    assert!(s.published().is_empty());
    assert_eq!(s.state().position_ms, 18_700);
    // the next reading disagrees with the held one: it was an outlier
    s.queue_time(18.9);
    assert_eq!(s.state().position_ms, 18_900);
    assert_eq!(s.live(|live| live.pending_pos), None);
}

#[test]
fn two_agreeing_far_readings_adopt_the_new_position() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 5_000);
    s.published();
    s.queue_time(120.0);
    assert!(s.published().is_empty());
    assert_eq!(s.state().position_ms, 15_000);
    s.advance(1_000);
    s.queue_time(121.0);
    assert_eq!(s.state().position_ms, 121_000);
    assert_eq!(s.published().len(), 1);
    assert_eq!(s.live(|live| live.pending_pos), None);
}

#[test]
fn a_held_reading_is_confirmed_by_one_within_two_and_a_half_seconds() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 5_000);
    s.queue_time(120.0);
    s.advance(500);
    // 2500 ms from the held one: the same new clock
    s.queue_time(122.5);
    assert_eq!(s.state().position_ms, 122_500);

    s.queue_time(20.0);
    s.advance(500);
    // 2600 ms from the held one: a different outlier, held in its turn
    s.queue_time(22.6);
    assert_eq!(s.state().position_ms, 122_500);
    assert!(s.live(|live| live.pending_pos).is_some());
}

#[test]
fn a_held_reading_expires_after_five_seconds() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 5_000);
    s.queue_time(120.0);
    s.published();
    s.advance(5_000);
    // too late to confirm: held as a fresh outlier instead
    s.queue_time(120.5);
    assert!(s.published().is_empty());
    assert_eq!(s.state().position_ms, 15_000);
    assert!(s.live(|live| live.pending_pos).is_some());
}

#[test]
fn the_first_two_seconds_accept_any_position() {
    // the state can be seeded from arbitrarily stale snapshots
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    s.advance(1_999);
    s.queue_time(150.0);
    assert_eq!(s.state().position_ms, 150_000);
    s.advance(1);
    s.queue_time(20.0);
    // 2 s after bootstrap the guard is on
    assert_eq!(s.state().position_ms, 150_000);
}

#[test]
fn a_paused_queue_does_not_interpolate_but_still_takes_corrections() {
    let mut s = Scenario::new();
    connect(&mut s, false, a(), 10.0);
    run(&mut s, 5_000);
    assert_eq!(s.tick(), (10_000, 200_000, false));
    s.queue_time(11.0);
    assert_eq!(s.state().position_ms, 11_000);
}

// 5. seek -------------------------------------------------------------

#[test]
fn a_requested_seek_ignores_old_echoes_until_the_target_is_confirmed() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 10_000);
    s.published();
    s.request_seek(120);
    s.queue_time(20.5);
    s.advance(1_000);
    s.queue_time(21.5);
    assert!(s.published().is_empty(), "pre-seek echoes are dropped");
    assert_eq!(s.state().position_ms, 20_000);
    // within 2.5 s of the target counts as the confirmation
    s.advance(1_000);
    s.queue_time(118.0);
    assert_eq!(s.state().position_ms, 118_000);
    assert_eq!(s.live(|live| live.pending_seek), None);
    s.advance(1_000);
    s.queue_time(119.0);
    assert_eq!(s.state().position_ms, 119_000);
}

#[test]
fn a_seek_nobody_confirms_times_out_without_rewriting_the_position() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 10_000);
    s.request_seek(120);
    s.advance(4_999);
    s.queue_time(20.0);
    assert!(s.live(|live| live.pending_seek).is_some());
    s.advance(1);
    // 5 s: the request is forgotten, the reading is judged like any other
    s.queue_time(20.0);
    assert_eq!(s.live(|live| live.pending_seek), None);
    assert_eq!(s.state().position_ms, 20_000);
    assert_ne!(s.state().position_ms, 120_000);
}

#[test]
fn a_track_change_forgets_a_pending_seek() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 10_000);
    s.request_seek(120);
    s.queue_updated(skipped_to(b(), 0.0, 1));
    assert_eq!(s.live(|live| live.pending_seek), None);
    s.queue_time(0.5);
    assert_eq!(s.state().position_ms, 500);
}

// 6. no duration ------------------------------------------------------

#[test]
fn a_stream_without_duration_has_an_empty_bar_and_a_still_position() {
    let mut s = Scenario::new();
    let radio = item("r", "Radio Paradise", "Various", 0);
    connect(&mut s, true, radio, 42.0);
    let state = s.state();
    assert_eq!(state.title, "Radio Paradise");
    assert_eq!(state.duration_ms, 0);
    assert!(state.has_media);
    // the ticker reports duration 0 (the bar draws empty) and never
    // runs the position on
    run(&mut s, 5_000);
    assert_eq!(s.tick(), (42_000, 0, false));
    // corrections still land in the state
    s.queue_time(43.0);
    assert_eq!(s.state().position_ms, 43_000);
    assert_eq!(s.tick(), (43_000, 0, false));
}

#[test]
fn a_stream_after_a_track_does_not_inherit_its_duration() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 190.0);
    run(&mut s, 2_000);
    s.queue_updated(skipped_to(
        item("r", "Radio Paradise", "Various", 0),
        7.0,
        1,
    ));
    let state = s.state();
    assert_eq!(state.duration_ms, 0);
    assert_eq!(state.position_ms, 7_000);
    // the same stream reported again without a duration changes nothing
    s.queue_updated(skipped_to(
        item("r", "Radio Paradise", "Various", 0),
        9.0,
        1,
    ));
    assert_eq!(s.state().duration_ms, 0);
    assert_eq!(s.state().position_ms, 7_000);
}

#[test]
fn a_duration_that_arrives_later_is_adopted() {
    let mut s = Scenario::new();
    connect(&mut s, true, item("r", "Podcast", "Host", 0), 3.0);
    s.queue_updated(skipped_to(item("r", "Podcast", "Host", 1800), 3.0, 0));
    assert_eq!(s.state().duration_ms, 1_800_000);
    assert_eq!(s.state().position_ms, 3_000);
}

// 7. cleared queue ----------------------------------------------------

#[test]
fn a_cleared_queue_resets_once_its_item_list_arrives() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    s.land_cover(&cover_url("a"), true);
    s.published();
    // queue_updated without current_item: only the options flow
    s.queue_updated(QueueUpdate::cleared());
    let state = s.state();
    assert_eq!(state.title, "Teardrop");
    assert!(state.has_media);
    // CHARACTERIZED: looks wrong because the old track stays on screen
    // until the (async) item list answers — the cleared queue_updated
    // itself resets nothing
    // the refetched, empty list is what wipes the display
    s.queue_items(&[], usize::MAX);
    let state = s.state();
    assert!(!state.has_media);
    assert_eq!(state.title, "");
    assert_eq!(state.artist, "");
    assert_eq!(state.album, "");
    assert_eq!(state.codec, "");
    assert!(!state.lossless);
    assert_eq!(state.duration_ms, 0);
    assert_eq!(state.position_ms, 0);
    assert!(state.queue.is_empty());
    assert!(!state.cover_missing);
    // the next play of the same track fetches its cover again
    s.queue_updated(skipped_to(a(), 0.0, 0));
    assert_eq!(s.state().title, "Teardrop");
    assert_eq!(s.cover_requests(), [cover_url("a"), cover_url("a")]);
}

#[test]
fn an_empty_seed_resets_the_display_and_the_bar() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    run(&mut s, 3_000);
    s.request_seek(100);
    s.seed(QueueUpdate::cleared());
    let state = s.state();
    assert!(!state.has_media);
    assert_eq!(state.title, "");
    assert_eq!(state.position_ms, 0);
    assert_eq!(s.live(|live| live.pending_seek), None);
    assert_eq!(s.tick(), (0, 0, false));
    // CHARACTERIZED: looks wrong because `playing` survives a cleared
    // queue; only a transport event turns it off
    assert!(state.playing);
}

// 8. transport --------------------------------------------------------

fn transport(playing: bool, volume: f64, powered: bool) -> serde_json::Value {
    let mut frame = fixture("queue_back");
    frame["data"]["volume_level"] = volume.into();
    frame["data"]["powered"] = powered.into();
    frame["data"]["playback_state"] = if playing { "playing" } else { "idle" }.into();
    frame
}

#[test]
fn play_pause_and_power_follow_the_player_with_a_toast_on_transitions() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    s.published();
    s.player_frame(&transport(false, 0.0, true), false);
    let published = s.published();
    assert_eq!(toast(&published[0]), "transport: Paused");
    assert!(!s.state().playing);
    // the same state again: no toast
    s.player_frame(&transport(false, 0.0, true), false);
    assert_eq!(toast(&s.published()[0]), "-");
    s.player_frame(&transport(true, 0.0, true), true);
    assert_eq!(toast(&s.published()[0]), "transport: Playing");
    // powered rides along, the media stays
    s.player_frame(&transport(false, 0.0, false), false);
    let state = s.state();
    assert!(!state.powered);
    assert_eq!(state.title, "Teardrop");
    assert!(state.has_media);
}

#[test]
fn no_transport_toast_before_there_is_media() {
    let mut s = Scenario::new();
    s.player_frame(&transport(true, 0.0, true), true);
    s.player_frame(&transport(false, 0.0, true), false);
    assert!(s.published().iter().all(|u| toast(u) == "-"));
}

#[test]
fn a_remote_volume_change_raises_the_osd_and_the_state_follows() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    s.published();
    s.player_frame(&transport(true, 35.0, true), true);
    let published = s.published();
    assert_eq!(published[0].osd_volume, Some(35));
    assert_eq!(s.state().volume, 35);
    // the same volume again is not news
    s.player_frame(&transport(true, 35.0, true), true);
    assert_eq!(s.published()[0].osd_volume, None);
    // MA reports fractional levels: rounded
    s.player_frame(&transport(true, 35.6, true), true);
    assert_eq!(s.published()[0].osd_volume, Some(36));
}

#[test]
fn the_echo_of_a_local_volume_change_stays_silent_once() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    s.player_frame(&transport(true, 20.0, true), true);
    s.published();
    s.set_volume_locally(40);
    s.advance(300);
    s.player_frame(&transport(true, 40.0, true), true);
    assert_eq!(s.published()[0].osd_volume, None);
    assert_eq!(s.live(|live| live.volume_local), None, "consumed");
    // a later remote change to the same value is a remote change
    s.player_frame(&transport(true, 10.0, true), true);
    s.published();
    s.player_frame(&transport(true, 40.0, true), true);
    assert_eq!(s.published()[0].osd_volume, Some(40));
}

#[test]
fn a_local_volume_marker_is_good_for_four_seconds_and_one_value() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    s.player_frame(&transport(true, 20.0, true), true);
    s.published();
    // another value than ours is remote, and leaves the marker alone
    s.set_volume_locally(40);
    s.player_frame(&transport(true, 30.0, true), true);
    assert_eq!(s.published()[0].osd_volume, Some(30));
    assert!(s.live(|live| live.volume_local).is_some());
    // our value after 3999 ms is still the echo ...
    s.advance(3_999);
    s.player_frame(&transport(true, 40.0, true), true);
    assert_eq!(s.published()[0].osd_volume, None);
    // ... after 4 s it is not
    s.player_frame(&transport(true, 20.0, true), true);
    s.published();
    s.set_volume_locally(40);
    s.advance(4_000);
    s.player_frame(&transport(true, 40.0, true), true);
    assert_eq!(s.published()[0].osd_volume, Some(40));
}

#[test]
fn a_local_volume_marker_equal_to_the_current_volume_is_never_consumed() {
    // CHARACTERIZED: looks wrong because setting the volume the player
    // already has leaves the marker behind (no change, no echo to
    // consume it): a remote change back to exactly that value within
    // 4 s is then taken for our own echo and raises no OSD
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 10.0);
    s.player_frame(&transport(true, 20.0, true), true);
    s.published();
    s.set_volume_locally(20);
    s.player_frame(&transport(true, 20.0, true), true);
    assert!(s.live(|live| live.volume_local).is_some());
    s.advance(1_000);
    s.player_frame(&transport(true, 50.0, true), true);
    assert_eq!(s.published().last().unwrap().osd_volume, Some(50));
    s.advance(1_000);
    s.player_frame(&transport(true, 20.0, true), true);
    assert_eq!(s.published().last().unwrap().osd_volume, None);
}

#[test]
fn loading_ends_when_the_player_reports_playing_or_after_six_seconds() {
    let mut s = Scenario::new();
    connect(&mut s, false, a(), 10.0);
    s.press_transport();
    assert!(s.state().loading);
    assert_eq!(s.tick(), (10_000, 200_000, true));
    // a non-playing report does not end it
    s.player_frame(&transport(false, 0.0, true), false);
    assert!(s.state().loading);
    s.player_frame(&transport(true, 0.0, true), true);
    assert!(!s.state().loading);
    assert_eq!(s.live(|live| live.loading_since), None);

    // the timeout backstop: strictly longer than 6 s
    s.press_transport();
    s.advance(6_000);
    assert!(s.tick().2);
    s.advance(1);
    assert!(!s.tick().2);
}

// 9. shuffle / repeat -------------------------------------------------

#[test]
fn shuffle_and_repeat_ride_the_queue_events() {
    let mut s = Scenario::new();
    connect(&mut s, true, a(), 0.0);
    assert!(!s.state().shuffle);
    assert_eq!(s.state().repeat, RepeatMode::Off);
    s.queue_updated(QueueUpdate {
        shuffle: true,
        repeat: "all",
        ..QueueUpdate::playing(a(), 0.0)
    });
    assert!(s.state().shuffle);
    assert_eq!(s.state().repeat, RepeatMode::All);
    s.queue_updated(QueueUpdate {
        repeat: "one",
        ..QueueUpdate::playing(a(), 0.0)
    });
    assert!(!s.state().shuffle);
    assert_eq!(s.state().repeat, RepeatMode::One);
    // a cleared queue still carries them
    s.queue_updated(QueueUpdate {
        shuffle: true,
        ..QueueUpdate::cleared()
    });
    assert!(s.state().shuffle);
    assert_eq!(s.state().repeat, RepeatMode::Off);
    // an unknown repeat word is off
    s.queue_updated(QueueUpdate {
        repeat: "sideways",
        ..QueueUpdate::playing(a(), 0.0)
    });
    assert_eq!(s.state().repeat, RepeatMode::Off);
}

#[test]
fn the_seed_carries_the_options_too() {
    let mut s = Scenario::new();
    s.player_frame(&fixture("queue_back"), true);
    s.seed(QueueUpdate {
        shuffle: true,
        repeat: "all",
        ..QueueUpdate::playing(a(), 0.0)
    });
    assert!(s.state().shuffle);
    assert_eq!(s.state().repeat, RepeatMode::All);
}
