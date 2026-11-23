//! The scenario rig: feeds MA wire frames through the same steps the
//! running session takes (parse → `translate` → `PlayerFeed`), records every published `PlayerUpdate` and every
//! cover request, and owns the clock.
//!
//! Time is explicit: every call into the state machine gets the rig's
//! `now`, which only moves with `advance`. MA's wall clock (the
//! stamps on external readings) moves with it.
//!
//! The frame → `SourceEvent` translation is `translate`, shared with
//! `session::run_session`; only the matching of a frame to the display's
//! player and queue (which needs the websocket session) is the rig's own.

use super::super::external::ExternalTracker;
use super::super::parse::{parse_active_queue, parse_event, MaEvent};
use super::super::translate;
use crate::player::cover::{CoverData, CoverFetcher, CoverLanded, CoverSource};
use crate::player::shell::{Shared, Shell};
use crate::player::state::{Event, Live, PlayerState, PlayerUpdate};
use crate::providers::session::PlayerFeed;
use crate::providers::SourceEvent;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const BASE: &str = "http://ma.test:8095";
pub const PLAYER_ID: &str = "02:00:00:00:00:01";

/// MA's wall clock at the start of every scenario (the fixtures'
/// external readings are stamped around here)
const WALL_START: f64 = 1795350806.0;

/// records what the state machine asks for instead of doing HTTP
#[derive(Default)]
struct Recorder {
    requests: Mutex<Vec<(String, Option<CoverLanded>)>>,
}

impl CoverFetcher for Recorder {
    fn fetch(&self, url: String, landed: CoverLanded) {
        self.requests.lock().unwrap().push((url, Some(landed)));
    }
}

pub struct Scenario {
    pub shared: Shared,
    /// what the UI thread holds: same state, its updates go nowhere here
    ui_shell: Shell,
    feed: PlayerFeed,
    tracker: ExternalTracker,
    recorder: Arc<Recorder>,
    updates: tokio::sync::mpsc::UnboundedReceiver<PlayerUpdate>,
    start: Instant,
    elapsed: Duration,
    wall: f64,
    /// the last player frame asked for the queue to be re-read
    /// (`get_active_queue`) — the scenario answers it with `seed`
    pub reseed_requested: bool,
}

impl Scenario {
    pub fn new() -> Self {
        let (tx, updates) = tokio::sync::mpsc::unbounded_channel();
        let recorder = Arc::new(Recorder::default());
        let shared: Shared = Arc::new(Mutex::new(None));
        let covers = || CoverSource {
            fetcher: recorder.clone(),
            variant: crate::ui::palette::Variant::TonalSpot,
        };
        let (ui_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let ui_shell = Shell::new(shared.clone(), ui_tx, covers());
        let feed = PlayerFeed::new(Shell::new(shared.clone(), tx, covers()));
        Self {
            shared,
            ui_shell,
            feed,
            tracker: ExternalTracker::default(),
            recorder,
            updates,
            start: Instant::now(),
            elapsed: Duration::ZERO,
            wall: WALL_START,
            reseed_requested: false,
        }
    }

    /// the connection dropped and came back: a fresh session starts with a
    /// fresh tracker, the core keeps its state
    pub fn reconnect(&mut self) {
        self.tracker = ExternalTracker::default();
    }

    pub fn now(&self) -> Instant {
        self.start + self.elapsed
    }

    /// MA's wall clock now (unix seconds)
    pub fn wall_now(&self) -> f64 {
        self.wall
    }

    pub fn advance(&mut self, ms: u64) {
        self.elapsed += Duration::from_millis(ms);
        self.wall += ms as f64 / 1000.0;
    }

    // ---- what the state machine published ----

    /// everything published since the last call
    pub fn published(&mut self) -> Vec<PlayerUpdate> {
        let mut all = Vec::new();
        while let Ok(update) = self.updates.try_recv() {
            all.push(update);
        }
        all
    }

    /// the state as the shared handle holds it now
    pub fn state(&self) -> PlayerState {
        state_of(&self.shared)
    }

    pub fn live<R>(&self, read: impl FnOnce(&Live) -> R) -> R {
        read(self.shared.lock().unwrap().as_ref().unwrap())
    }

    /// every cover url the state machine asked for, in order
    pub fn cover_requests(&self) -> Vec<String> {
        self.recorder
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|(url, _)| url.clone())
            .collect()
    }

    /// the fetch for `url` finishes (with a picture, or failed) now;
    /// returns what it published, if anything
    pub fn land_cover(&mut self, url: &str, found: bool) -> Vec<PlayerUpdate> {
        let landed = self
            .recorder
            .requests
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(requested, landed)| requested == url && landed.is_some())
            .and_then(|(_, landed)| landed.take())
            .unwrap_or_else(|| panic!("no pending cover fetch for {url}"));
        landed(found.then(picture));
        self.published()
    }

    // ---- events in ----

    /// a `SourceEvent` the scenario builds itself
    pub fn send(&mut self, event: SourceEvent) {
        let leftover = self.feed.apply(event, self.now());
        assert!(leftover.is_none(), "not a player event");
    }

    /// a `player_updated` frame (a fixture, usually) for the display's
    /// player: transport, then whatever the external tracker resolves
    pub fn player_frame(&mut self, frame: &Value, playing: bool) {
        let mut frame = frame.clone();
        frame["data"]["playback_state"] = if playing { "playing" } else { "idle" }.into();
        let Some(MaEvent::Player(player)) = parse_event(&frame, BASE) else {
            panic!("not a player_updated frame");
        };
        let (events, back_on_queue) =
            translate::player_updated(&player, &mut self.tracker, self.wall);
        if back_on_queue {
            self.reseed_requested = true;
        }
        for event in events {
            self.send(event);
        }
    }

    /// a `queue_updated` push
    pub fn queue_updated(&mut self, update: QueueUpdate) {
        let Some(MaEvent::Queue {
            track,
            elapsed_sec,
            shuffle,
            repeat,
            ..
        }) = parse_event(&update.frame(), BASE)
        else {
            panic!("not a queue_updated frame");
        };
        for event in translate::queue_updated(track, elapsed_sec, shuffle, repeat) {
            self.send(event);
        }
    }

    /// a `queue_time_updated` push
    pub fn queue_time(&mut self, secs: f64) {
        let frame = json!({"event": "queue_time_updated", "object_id": PLAYER_ID, "data": secs});
        let Some(MaEvent::QueueTime { elapsed_sec, .. }) = parse_event(&frame, BASE) else {
            panic!("not a queue_time_updated frame");
        };
        self.send(SourceEvent::Position(elapsed_sec));
    }

    /// the reply to `player_queues/get_active_queue` — at startup and
    /// after an external source ended
    pub fn seed(&mut self, update: QueueUpdate) {
        let reply = json!({"message_id": "1", "result": update.data()});
        let active = parse_active_queue(&reply, BASE).expect("an active queue reply");
        self.reseed_requested = false;
        for event in translate::queue_seed(&active) {
            self.send(event);
        }
    }

    /// the reply to `player_queues/items`; `current_index` is what the
    /// session remembered from the queue event or seed
    pub fn queue_items(&mut self, items: &[Value], current_index: usize) {
        self.send(translate::queue_items(items, BASE, current_index));
    }

    // ---- what the user does on the player ----

    /// the seek bookkeeping of a tap on the progress bar (the queue's
    /// branch of `transport::on_seek`)
    pub fn request_seek(&mut self, seconds: u64) {
        self.ui_shell
            .dispatch(Event::SeekRequested { seconds }, self.now());
    }

    pub fn set_volume_locally(&mut self, volume: i32) {
        self.ui_shell
            .dispatch(Event::MarkVolumeLocal(volume), self.now());
    }

    pub fn press_transport(&mut self) {
        self.ui_shell.dispatch(Event::MarkLoading, self.now());
    }

    /// one ticker beat (the 500 ms timer in main): position, duration, loading
    pub fn tick(&self) -> (u64, u64, bool) {
        self.ui_shell.tick(self.now()).unwrap()
    }
}

pub fn state_of(shared: &Shared) -> PlayerState {
    shared.lock().unwrap().as_ref().unwrap().state.clone()
}

fn picture() -> CoverData {
    CoverData {
        rgba: [200, 60, 40, 255].repeat(32 * 32),
        width: 32,
        height: 32,
    }
}

// ---- wire frames ----

pub fn fixture(name: &str) -> Value {
    let text = match name {
        "airplay_placeholder" => {
            include_str!("../../../../tests/fixtures/ma/airplay_placeholder.json")
        }
        "airplay_track" => include_str!("../../../../tests/fixtures/ma/airplay_track.json"),
        "queue_back" => include_str!("../../../../tests/fixtures/ma/queue_back.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap()
}

/// a queue item as `player_queues/items` and `current_item` carry it
pub fn item(id: &str, title: &str, artist: &str, duration_sec: u64) -> Value {
    json!({
        "queue_item_id": id,
        "name": format!("{artist} - {title}"),
        "duration": duration_sec,
        "image": {"proxy_id": format!("cover-{id}")},
        "media_item": {
            "name": title,
            "provider": "library",
            "artists": [{"name": artist, "item_id": "7", "uri": "library://artist/7"}],
            "album": {"name": format!("Album of {title}"), "item_id": "9", "uri": "library://album/9"},
        },
        "streamdetails": {"audio_format": {
            "content_type": "flac", "sample_rate": 44100, "bit_depth": 16, "bit_rate": 0, "channels": 2,
        }},
    })
}

/// the proxied cover url the state machine will ask for
pub fn cover_url(id: &str) -> String {
    format!("{BASE}/imageproxy/cover-{id}?size=512&fmt=jpg")
}

/// the queue's own view: the payload of `queue_updated` and of the
/// `get_active_queue` reply
pub struct QueueUpdate {
    pub current_item: Option<Value>,
    pub elapsed_sec: f64,
    pub current_index: usize,
    pub shuffle: bool,
    pub repeat: &'static str,
}

impl QueueUpdate {
    pub fn playing(current_item: Value, elapsed_sec: f64) -> Self {
        Self {
            current_item: Some(current_item),
            elapsed_sec,
            current_index: 0,
            shuffle: false,
            repeat: "off",
        }
    }

    /// a cleared queue: no `current_item` at all
    pub fn cleared() -> Self {
        Self {
            current_item: None,
            elapsed_sec: 0.0,
            current_index: 0,
            shuffle: false,
            repeat: "off",
        }
    }

    fn data(&self) -> Value {
        let mut data = json!({
            "queue_id": PLAYER_ID,
            "elapsed_time": self.elapsed_sec,
            "current_index": self.current_index,
            "shuffle_enabled": self.shuffle,
            "repeat_mode": self.repeat,
        });
        if let Some(item) = &self.current_item {
            data["current_item"] = item.clone();
        }
        data
    }

    fn frame(&self) -> Value {
        json!({"event": "queue_updated", "object_id": PLAYER_ID, "data": self.data()})
    }
}

pub fn a() -> Value {
    item("a", "Teardrop", "Massive Attack", 200)
}

pub fn b() -> Value {
    item("b", "Angel", "Massive Attack", 180)
}

pub fn c() -> Value {
    item("c", "Inertia Creeps", "Massive Attack", 240)
}

/// what the session does on connect: the players/all reply (transport,
/// the queue as source), then the active-queue seed and the item list.
/// `playing` is the player's transport state.
pub fn connect(s: &mut Scenario, playing: bool, current: Value, elapsed: f64) {
    s.player_frame(&fixture("queue_back"), playing);
    s.seed(QueueUpdate::playing(current, elapsed));
    s.queue_items(&[a(), b(), c()], 0);
}

/// a queue event for the item at `index`, `elapsed_sec` into it
pub fn skipped_to(item: Value, elapsed_sec: f64, index: usize) -> QueueUpdate {
    QueueUpdate {
        current_index: index,
        ..QueueUpdate::playing(item, elapsed_sec)
    }
}
