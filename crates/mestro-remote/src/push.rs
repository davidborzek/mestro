//! The push pipeline: the hub is fed by the update loop (the
//! player state, the covers) on the UI thread and by the view
//! watcher on the web thread; the web thread owns the sockets.
//! The seam is [`Events`]: the hub publishes serialized state
//! frames into a broadcast channel (plus a latest-slot for the
//! snapshot a fresh socket needs), and the cover slot carries
//! the artwork the `/cover` endpoint encodes per version. The
//! view state (kiosk view flags, backlight, connection) arrives
//! through a `tokio` watch channel — the binary publishes a
//! snapshot whenever one of the view properties changes, this
//! side only reacts. Frames coalesce — at most one per 100 ms —
//! and position corrections alone never push (the frame carries
//! no position).

use super::state::{
    self, RemoteArt, RemoteContext, RemoteDisplay, RemoteState, RemoteView, Throttle,
};
use mestro_player::cover::CoverData;
use mestro_player::palette::ArtworkPalette;
use mestro_player::state::PlayerState;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};

/// how many frames may pile up before a slow client lags
const CHANNEL_CAPACITY: usize = 16;
/// the coalescing window
const MIN_PUSH_INTERVAL: Duration = Duration::from_millis(100);

/// how many preview PNGs the slot keeps before it resets —
/// settings edits churn versions, one render re-fills it
const PREVIEW_CACHE_MAX: usize = 64;

/// the shared side of the pipeline: the hub is the only writer,
/// the web thread reads the latest frame and the cover
#[derive(Clone)]
pub struct Events {
    inner: Arc<Inner>,
}

struct Inner {
    tx: broadcast::Sender<String>,
    latest: Mutex<String>,
    cover: Mutex<Option<(u64, CoverData)>>,
    /// the pre-blurred backdrop (the kiosk's own) per cover
    /// version — the phone paints it with no CSS filter
    backdrop: Mutex<Option<(u64, CoverData)>>,
    /// the JPEG per cover version — encoded once per artwork
    jpeg: Mutex<Option<(u64, Vec<u8>)>>,
    /// the dimmed backdrop JPEG per cover version
    backdrop_jpeg: Mutex<Option<(u64, Vec<u8>)>>,
    /// preview PNGs by (kind, name, version) — the /vu-preview
    /// and /clock-preview endpoints cache here so repeat requests
    /// never touch the UI thread
    previews: Mutex<HashMap<(String, String, String), Vec<u8>>>,
}

impl Events {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Inner {
                tx,
                latest: Mutex::new(String::new()),
                cover: Mutex::new(None),
                backdrop: Mutex::new(None),
                jpeg: Mutex::new(None),
                backdrop_jpeg: Mutex::new(None),
                previews: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// the hub side: publish a frame (broadcast + latest slot)
    pub fn publish(&self, frame: String) {
        *self.inner.latest.lock() = frame.clone();
        let _ = self.inner.tx.send(frame);
    }

    /// the settings-changed event (`state::SETTINGS_EVENT`):
    /// broadcast only — never the latest slot, that holds the
    /// player state frame a fresh or lagged socket starts with
    pub fn publish_settings(&self) {
        let _ = self.inner.tx.send(super::state::SETTINGS_EVENT.to_string());
    }

    /// the last published frame, for the snapshot a fresh or
    /// lagged socket needs (None before the first state exists)
    pub fn snapshot(&self) -> Option<String> {
        let latest = self.inner.latest.lock();
        (!latest.is_empty()).then(|| latest.clone())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.inner.tx.subscribe()
    }

    /// the latest cover + its pre-blurred backdrop, both
    /// versioned (0 = none yet)
    pub fn set_cover(&self, version: u64, cover: CoverData, backdrop: Option<CoverData>) {
        *self.inner.cover.lock() = Some((version, cover));
        *self.inner.backdrop.lock() = backdrop.map(|bg| (version, bg));
    }

    /// the current cover as JPEG (quality ~85), encoded at most
    /// once per version; None when no cover exists yet
    pub fn cover_jpeg(&self) -> Option<Vec<u8>> {
        let (version, cover) = self.inner.cover.lock().clone()?;
        let mut jpeg = self.inner.jpeg.lock();
        if let Some((at, bytes)) = jpeg.as_ref() {
            if *at == version {
                return Some(bytes.clone());
            }
        }
        let encoded = encode_jpeg(&cover)?;
        *jpeg = Some((version, encoded.clone()));
        Some(encoded)
    }

    /// the backdrop as JPEG: the blur the kiosk already made,
    /// darkened and saturated HERE so the phone paints it as a
    /// plain image (iOS recomputes CSS filters on every repaint
    /// above them); None while no cover exists
    pub fn backdrop_jpeg(&self) -> Option<Vec<u8>> {
        let (version, backdrop) = self.inner.backdrop.lock().clone()?;
        let mut jpeg = self.inner.backdrop_jpeg.lock();
        if let Some((at, bytes)) = jpeg.as_ref() {
            if *at == version {
                return Some(bytes.clone());
            }
        }
        let encoded = encode_backdrop(&backdrop)?;
        *jpeg = Some((version, encoded.clone()));
        Some(encoded)
    }

    /// one preview thumbnail as PNG, cached per (kind, name,
    /// version): `render` runs only on a miss (the UI-thread
    /// roundtrip); its result is not cached when it fails
    pub fn preview_png(
        &self,
        kind: &str,
        name: &str,
        version: &str,
        render: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        let key = (kind.to_string(), name.to_string(), version.to_string());
        if let Some(bytes) = self.inner.previews.lock().get(&key) {
            return Some(bytes.clone());
        }
        // render outside the lock: the UI-thread roundtrip can
        // take a while, a concurrent request for another preview
        // must not queue behind it
        let bytes = render()?;
        let mut cache = self.inner.previews.lock();
        if cache.len() >= PREVIEW_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, bytes.clone());
        Some(bytes)
    }

    /// test-only: the cover slot's version (0 = no image)
    #[cfg(test)]
    pub(crate) fn cover_version(&self) -> u64 {
        self.inner.cover.lock().as_ref().map_or(0, |(v, _)| *v)
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

fn encode_jpeg(cover: &CoverData) -> Option<Vec<u8>> {
    let rgba = image::RgbaImage::from_raw(cover.width, cover.height, cover.rgba.clone())?;
    let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
    let mut bytes = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(bytes.into_inner())
}

/// the backdrop encode: the blur is already in the pixels, this
/// adds the saturation boost (1.2×, away from the luma) and the
/// darkening (~0.35) the web used to paint with a CSS filter —
/// iOS recomputes `filter: blur(...)` on every repaint above
/// the layer, a plain image costs nothing
fn encode_backdrop(backdrop: &CoverData) -> Option<Vec<u8>> {
    let mut rgba =
        image::RgbaImage::from_raw(backdrop.width, backdrop.height, backdrop.rgba.clone())?;
    for pixel in rgba.pixels_mut() {
        let [r, g, b, ..] = pixel.0;
        let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        for channel in pixel.0.iter_mut().take(3) {
            let saturated = luma + (*channel as f32 - luma) * 1.2;
            *channel = (saturated * 0.35).round().clamp(0.0, 255.0) as u8;
        }
    }
    encode_jpeg(&CoverData {
        rgba: rgba.into_raw(),
        width: backdrop.width,
        height: backdrop.height,
    })
}

/// the UI view state as the binary publishes it: a plain-data
/// snapshot of the kiosk view flags, the VU cycle, the backlight,
/// the provider connection and the configured MA ws endpoint.
/// The binary samples it from the slint properties (UI thread)
/// and sends it into the watch whenever one of them changes —
/// that side owns every change hook, this side only reacts
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiView {
    pub np_open: bool,
    pub queue_open: bool,
    pub vu_open: bool,
    pub vu_index: i32,
    pub vu_count: i32,
    /// the cycle's skin names, in `vu_index` order
    pub vu_skins: Vec<String>,
    pub standby: bool,
    pub display_supported: bool,
    /// 1..100, rounded
    pub brightness: i32,
    /// kiosk ↔ Music Assistant
    pub connected: bool,
    /// the configured MA ws endpoint ("" = unconfigured) — the
    /// frame's `ma_url` derives from it
    pub ma_ws_url: String,
}

impl Default for UiView {
    fn default() -> Self {
        Self {
            np_open: false,
            queue_open: false,
            vu_open: false,
            vu_index: 0,
            vu_count: 0,
            vu_skins: Vec::new(),
            standby: false,
            display_supported: false,
            // the frame contract: 100 = the untouched default
            brightness: 100,
            connected: false,
            ma_ws_url: String::new(),
        }
    }
}

/// the UI-thread-independent half of the pipeline: accumulates
/// the display's state, the kiosk view flags and the cover
/// version, and pushes frames into [`Events`] whenever
/// something changed. Frames are small and the throttle caps the
/// send rate, so every PlayerUpdate can afford a roundtrip.
/// Cross-thread (UI thread writes, web thread flushes) behind a
/// parking_lot lock.
pub struct Push {
    inner: Arc<Mutex<Hub>>,
    events: Events,
    views: watch::Receiver<UiView>,
}

impl Clone for Push {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            events: self.events.clone(),
            views: self.views.clone(),
        }
    }
}

struct Hub {
    player: String,
    state: PlayerState,
    ctx: RemoteContext,
    art: RemoteArt,
    last: Option<RemoteState>,
    throttle: Throttle,
    pending: bool,
}

impl Push {
    pub fn new(events: Events, player: &str, views: watch::Receiver<UiView>) -> Self {
        let ctx = context_of(&views.borrow());
        Self {
            events,
            views,
            inner: Arc::new(Mutex::new(Hub {
                player: player.to_string(),
                state: PlayerState::default(),
                ctx,
                art: RemoteArt {
                    version: 0,
                    accent: String::new(),
                },
                last: None,
                throttle: Throttle::new(MIN_PUSH_INTERVAL),
                // the first frame (an empty player) must reach
                // the client even though nothing "changed"
                pending: true,
            })),
        }
    }

    /// the view watcher: feeds the binary's view snapshots from
    /// the watch into the hub. Consumes one hub handle (clones
    /// are cheap); [`super::serve`] spawns it on the endpoint's
    /// runtime — it reacts to the published changes, never polls
    pub(crate) async fn observe_views(self) {
        let mut views = self.views.clone();
        loop {
            self.set_context(context_of(&views.borrow()));
            if views.changed().await.is_err() {
                // the publisher is gone — nothing left to observe
                return;
            }
        }
    }

    /// the coalescing flush: a throttled change goes out as soon
    /// as the window allows — without a fresh change to trigger
    /// it. Runs on tokio time on the endpoint's runtime; consumes
    /// one hub handle like [`Self::observe_views`]
    pub(crate) async fn run_flush(self) {
        let mut tick = tokio::time::interval(MIN_PUSH_INTERVAL);
        loop {
            tick.tick().await;
            self.flush();
        }
    }

    /// publish the current frame now, change check and throttle
    /// bypassed: it becomes the snapshot every fresh socket gets
    /// (the binary calls it once, before the endpoint serves).
    /// Samples the view watch first — the binary publishes the
    /// current props before the endpoint starts serving
    pub fn publish_snapshot(&self) {
        let mut hub = self.inner.lock();
        hub.ctx = context_of(&self.views.borrow());
        hub.pending = true;
        drop(hub);
        self.push_if_changed(true);
    }

    /// the update loop's player state (every PlayerUpdate)
    pub fn update(&self, state: &PlayerState) {
        let changed = {
            let mut hub = self.inner.lock();
            // the frame carries no position — a correction alone
            // must not push, so the comparison runs position-free
            let mut state = state.clone();
            state.position_ms = 0;
            let changed = hub.state != state;
            hub.state = state;
            changed
        };
        if changed {
            self.push_if_changed(false);
        }
    }

    /// a fresh cover (with its palette accent and its pre-blurred
    /// backdrop): bumps the version the `/cover` URLs carry
    pub fn cover(
        &self,
        cover: Option<&CoverData>,
        backdrop: Option<&CoverData>,
        palette: Option<&ArtworkPalette>,
    ) {
        let Some(cover) = cover else { return };
        {
            let mut hub = self.inner.lock();
            hub.art.version += 1;
            hub.art.accent = palette.map_or_else(String::new, |p| hex(p.accent));
            #[cfg(test)]
            gate_wait(&COVER_GATE);
            // install the image before the new version can reach a
            // frame: /cover is cached forever per version URL
            self.events
                .set_cover(hub.art.version, cover.clone(), backdrop.cloned());
        }
        self.push_if_changed(false);
    }

    /// a fresh view snapshot arrived. A VIEW change bypasses the
    /// throttle — a tile tap must not wait out the coalescing
    /// window; everything else stays throttled
    pub(crate) fn set_context(&self, ctx: RemoteContext) {
        let (changed, view_changed) = {
            let mut hub = self.inner.lock();
            let view_changed = hub.ctx.view != ctx.view;
            let changed = hub.ctx != ctx;
            hub.ctx = ctx;
            (changed, view_changed)
        };
        if changed {
            self.push_if_changed(view_changed);
        }
    }

    /// the timer side: a throttled change goes out as soon as
    /// the window allows — without a fresh change to trigger it
    fn flush(&self) {
        if self.inner.lock().pending {
            self.push_if_changed(false);
        }
    }

    /// frame the current state and publish it when something
    /// changed and the throttle permits (a forced push bypasses
    /// the window); otherwise the pending flag keeps it queued
    /// for the next timer tick
    fn push_if_changed(&self, force: bool) {
        let mut hub = self.inner.lock();
        let frame = frame_of(&hub);
        hub.pending |= hub.last.as_ref().is_none_or(|last| *last != frame);
        if !hub.pending || (!force && !hub.throttle.permit(std::time::Instant::now())) {
            return;
        }
        hub.pending = false;
        match serde_json::to_string(&state::PushFrame::state(frame.clone())) {
            Ok(text) => {
                hub.last = Some(frame);
                #[cfg(test)]
                gate_wait(&PUBLISH_GATE);
                // publish under the hub lock: the UI thread and the
                // view watcher both push, and an older frame must
                // never land after a newer one
                self.events.publish(text);
            }
            Err(e) => {
                // try again on the next tick
                hub.pending = true;
                tracing::warn!("web: state frame: {e}");
            }
        }
    }
}

/// the kiosk view flags, the MA connection and the backlight as
/// the binary published them
fn context_of(view: &UiView) -> RemoteContext {
    RemoteContext {
        connected: view.connected,
        ma_url: state::ma_web_url(&view.ma_ws_url),
        view: RemoteView {
            now_playing: view.np_open,
            queue: view.queue_open,
            vu: view.vu_open,
            standby: view.standby,
            vu_index: view.vu_index,
            vu_count: view.vu_count,
            vu_skins: view.vu_skins.clone(),
        },
        display: RemoteDisplay {
            supported: view.display_supported,
            brightness: view.brightness,
        },
    }
}

/// one state frame from the hub's latest player state
fn frame_of(hub: &Hub) -> RemoteState {
    state::from_player(&hub.state, &hub.player, &hub.ctx, &hub.art)
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// test-only seam: freezes the calling publisher at one exact
/// point of the pipeline so a test can interleave a second
/// publisher — the window the hub lock keeps closed in production
/// (each gate is one-shot and thread-local: parallel tests never
/// see each other's, production threads carry `None`)
#[cfg(test)]
struct Gate {
    arrived: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
thread_local! {
    /// [`Push::push_if_changed`]: just before the frame goes on
    /// the wire — under the hub lock, so a gated publisher also
    /// holds the lock (what the fix guarantees)
    static PUBLISH_GATE: std::cell::RefCell<Option<Gate>> =
        const { std::cell::RefCell::new(None) };
    /// [`Push::cover`]: between the version bump and the image
    /// install
    static COVER_GATE: std::cell::RefCell<Option<Gate>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn gate_install(
    key: &'static std::thread::LocalKey<std::cell::RefCell<Option<Gate>>>,
    arrived: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    key.with(|gate| *gate.borrow_mut() = Some(Gate { arrived, release }));
}

#[cfg(test)]
fn gate_wait(key: &'static std::thread::LocalKey<std::cell::RefCell<Option<Gate>>>) {
    key.with(|gate| {
        if let Some(gate) = gate.borrow_mut().take() {
            let _ = gate.arrived.send(());
            let _ = gate.release.recv();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // a hub with a live player, without the background tasks; the
    // receiver counts what actually goes on the wire
    fn push() -> (Push, broadcast::Receiver<String>) {
        let events = Events::new();
        let rx = events.subscribe();
        (Push::new(events, "wohnzimmer", watch_views()), rx)
    }

    /// a watch at the default view (the binary replaces the value
    /// before the endpoint serves)
    fn watch_views() -> watch::Receiver<UiView> {
        watch::channel(UiView::default()).1
    }

    fn playing(title: &str) -> PlayerState {
        PlayerState {
            playing: true,
            title: title.into(),
            has_media: true,
            duration_ms: 100_000,
            volume: 30,
            ..Default::default()
        }
    }

    fn context(ma_url: &str) -> RemoteContext {
        RemoteContext {
            connected: true,
            ma_url: ma_url.into(),
            view: RemoteView {
                now_playing: false,
                queue: false,
                vu: true,
                standby: false,
                vu_index: 1,
                vu_count: 3,
                vu_skins: vec!["night".into(), "cream".into(), "round".into()],
            },
            display: RemoteDisplay {
                supported: true,
                brightness: 60,
            },
        }
    }

    fn next_frame(rx: &mut broadcast::Receiver<String>) -> String {
        for _ in 0..200 {
            if let Ok(frame) = rx.try_recv() {
                return frame;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("no frame on the wire");
    }

    #[test]
    fn changes_publish_and_position_alone_does_not() {
        let (push, mut rx) = push();
        push.update(&playing("Track"));
        assert!(next_frame(&mut rx).contains("\"title\":\"Track\""));

        // a position-only PlayerUpdate (the state machine emits
        // them for every correction) must not publish
        let mut moved = playing("Track");
        moved.position_ms = 50_000;
        push.update(&moved);
        assert!(rx.try_recv().is_err(), "position alone never pushes");

        // a real change publishes once the coalescing window passed
        std::thread::sleep(MIN_PUSH_INTERVAL + Duration::from_millis(10));
        push.update(&playing("Other"));
        assert!(next_frame(&mut rx).contains("\"title\":\"Other\""));
    }

    #[test]
    fn context_changes_publish() {
        let (push, mut rx) = push();
        push.update(&playing("Track"));
        assert!(next_frame(&mut rx).contains("\"title\":\"Track\""));
        std::thread::sleep(MIN_PUSH_INTERVAL + Duration::from_millis(10));
        push.set_context(context("http://ma.local:8095"));
        let frame = next_frame(&mut rx);
        assert!(
            frame.contains("\"ma_url\":\"http://ma.local:8095\""),
            "{frame}"
        );
        assert!(frame.contains("\"vu\":true"), "{frame}");
        assert!(frame.contains("\"vu_index\":1"), "{frame}");
        assert!(frame.contains("\"vu_count\":3"), "{frame}");
        assert!(
            frame.contains("\"vu_skins\":[\"night\",\"cream\",\"round\"]"),
            "{frame}"
        );
        assert!(
            frame.contains("\"display\":{\"supported\":true,\"brightness\":60}"),
            "{frame}"
        );
    }

    #[test]
    fn a_view_change_publishes_at_once_inside_the_coalescing_window() {
        let (push, mut rx) = push();
        push.update(&playing("Track"));
        assert!(next_frame(&mut rx).contains("\"title\":\"Track\""));
        // no sleep: the 100 ms window is still closed, the view
        // flip goes out anyway — a tile tap must not queue
        let mut view = context("http://ma.local:8095");
        view.view.vu = true;
        let start = std::time::Instant::now();
        push.set_context(view);
        let frame = next_frame(&mut rx);
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "throttled: {} ms",
            start.elapsed().as_millis()
        );
        assert!(frame.contains("\"vu\":true"), "{frame}");
        // the throttle still holds for player changes: the next
        // one waits for the window
        push.update(&playing("Other"));
        assert!(rx.try_recv().is_err(), "player changes stay throttled");
    }

    #[test]
    fn the_initial_snapshot_is_a_minimal_valid_state_frame() {
        let events = Events::new();
        let push = Push::new(events.clone(), "wohnzimmer", watch_views());
        assert!(events.snapshot().is_none(), "nothing before the publish");
        push.publish_snapshot();
        let frame: serde_json::Value =
            serde_json::from_str(&events.snapshot().expect("the snapshot exists")).unwrap();
        assert_eq!(frame["event"], "state");
        assert_eq!(frame["data"]["connected"], false);
        assert_eq!(frame["data"]["player"]["name"], "wohnzimmer");
        assert_eq!(frame["data"]["player"]["has_media"], false);
        assert_eq!(frame["data"]["view"]["vu_skins"], serde_json::json!([]));
        // the first real update still publishes (nothing is held back)
        let mut rx = events.subscribe();
        push.update(&playing("Track"));
        assert!(next_frame(&mut rx).contains("\"title\":\"Track\""));
    }

    #[test]
    fn the_initial_snapshot_samples_the_view_watch() {
        // the binary publishes the current props into the watch
        // before the endpoint serves; the initial frame is built
        // from that snapshot
        let events = Events::new();
        let (_views_tx, views_rx) = watch::channel(UiView {
            vu_open: true,
            vu_index: 1,
            vu_count: 3,
            vu_skins: vec!["night".into(), "cream".into(), "round".into()],
            display_supported: true,
            brightness: 60,
            connected: true,
            ma_ws_url: "ws://ma.local:8095/ws".into(),
            ..UiView::default()
        });
        let push = Push::new(events.clone(), "wohnzimmer", views_rx);
        push.publish_snapshot();
        let frame: serde_json::Value =
            serde_json::from_str(&events.snapshot().expect("the snapshot exists")).unwrap();
        assert_eq!(frame["data"]["connected"], true);
        assert_eq!(frame["data"]["ma_url"], "http://ma.local:8095");
        assert_eq!(frame["data"]["view"]["vu"], true);
        assert_eq!(frame["data"]["view"]["vu_index"], 1);
        assert_eq!(frame["data"]["view"]["vu_count"], 3);
        assert_eq!(
            frame["data"]["view"]["vu_skins"],
            serde_json::json!(["night", "cream", "round"])
        );
        assert_eq!(
            frame["data"]["display"],
            serde_json::json!({"supported": true, "brightness": 60})
        );
        // nothing changed since the snapshot: no further frame
        let mut rx = events.subscribe();
        push.update(&playing("Track"));
        assert!(next_frame(&mut rx).contains("\"title\":\"Track\""));
    }

    #[test]
    fn an_unchanged_context_does_not_publish() {
        let (push, mut rx) = push();
        push.update(&playing("Track"));
        assert!(next_frame(&mut rx).contains("\"title\":\"Track\""));
        std::thread::sleep(MIN_PUSH_INTERVAL + Duration::from_millis(10));
        push.set_context(context(""));
        assert!(next_frame(&mut rx).contains("\"ma_url\":\"\""));
        push.set_context(context(""));
        assert!(rx.try_recv().is_err(), "the same context never re-pushes");
    }

    #[test]
    fn covers_bump_the_version_and_reach_the_slot() {
        let (push, mut rx) = push();
        let cover = CoverData {
            rgba: vec![255, 0, 0, 255],
            width: 1,
            height: 1,
        };
        let backdrop = CoverData {
            rgba: vec![255, 255, 255, 255],
            width: 1,
            height: 1,
        };
        let palette = ArtworkPalette {
            accent: [0xf3, 0xbd, 0x6e],
            tertiary: [0; 3],
            danger: [0; 3],
        };
        push.cover(Some(&cover), Some(&backdrop), Some(&palette));
        assert_eq!(push.inner.lock().art.version, 1);
        let frame = next_frame(&mut rx);
        assert!(
            frame.contains("/cover?v=") && frame.contains("-1\""),
            "{frame}"
        );
        assert!(frame.contains("#f3bd6e"), "{frame}");
        assert!(
            push.events.cover_jpeg().is_some(),
            "the slot holds the artwork"
        );
        let jpeg = push
            .events
            .backdrop_jpeg()
            .expect("the slot holds the backdrop");
        assert!(jpeg.starts_with(&[0xFF, 0xD8]), "a JPEG");
        // the darkening happens at encode time: a white backdrop
        // comes out ~0.35 bright, not 1.0
        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let [r, g, b] = decoded.get_pixel(0, 0).0;
        assert!((60..=120).contains(&r), "dimmed: {r} {g} {b}");
    }

    #[test]
    fn without_a_cover_there_is_nothing_to_encode() {
        let events = Events::new();
        assert!(events.snapshot().is_none());
        assert!(events.cover_jpeg().is_none());
        let jpeg = encode_jpeg(&CoverData {
            rgba: vec![10, 20, 30, 255],
            width: 1,
            height: 1,
        });
        // a 1x1 JPEG is a valid SOI…EOI stream
        assert!(jpeg.is_some_and(|j| j.starts_with(&[0xFF, 0xD8]) && j.ends_with(&[0xFF, 0xD9])));
    }

    #[test]
    fn a_missing_cover_stays_none_in_the_slot() {
        let (push, _) = push();
        push.cover(None, None, None);
        assert_eq!(push.inner.lock().art.version, 0);
        assert!(push.events.backdrop_jpeg().is_none());
    }

    #[test]
    fn settings_events_broadcast_without_replacing_the_snapshot() {
        let events = Events::new();
        let mut rx = events.subscribe();
        let state = r#"{"event":"state","data":{}}"#;
        events.publish(state.into());
        events.publish_settings();
        assert_eq!(next_frame(&mut rx), state, "the state frame comes first");
        assert_eq!(next_frame(&mut rx), super::state::SETTINGS_EVENT);
        // the snapshot a fresh socket starts with stays the state
        // frame, not the settings event
        assert_eq!(events.snapshot().as_deref(), Some(state));
    }

    fn playing_at(volume: i32) -> PlayerState {
        PlayerState {
            playing: true,
            title: "Track".into(),
            has_media: true,
            duration_ms: 100_000,
            volume,
            ..Default::default()
        }
    }

    fn context_vu(vu_index: i32) -> RemoteContext {
        RemoteContext {
            connected: true,
            ma_url: "http://ma.local:8095".into(),
            view: RemoteView {
                now_playing: false,
                queue: false,
                vu: true,
                standby: false,
                vu_index,
                vu_count: 3,
                vu_skins: vec!["night".into(), "cream".into(), "round".into()],
            },
            display: RemoteDisplay {
                supported: true,
                brightness: 60,
            },
        }
    }

    fn cover_data(rgba: Vec<u8>) -> CoverData {
        CoverData {
            rgba,
            width: 1,
            height: 1,
        }
    }

    fn palette() -> ArtworkPalette {
        ArtworkPalette {
            accent: [0xf3, 0xbd, 0x6e],
            tertiary: [0; 3],
            danger: [0; 3],
        }
    }

    fn volume_of(frame: &str) -> i32 {
        serde_json::from_str::<serde_json::Value>(frame).unwrap()["data"]["player"]["volume"]
            .as_i64()
            .unwrap() as i32
    }

    fn vu_of(frame: &str) -> i32 {
        serde_json::from_str::<serde_json::Value>(frame).unwrap()["data"]["view"]["vu_index"]
            .as_i64()
            .unwrap() as i32
    }

    /// the cover URL is "/cover?v=<boot tag>-<version>": the
    /// artwork version a frame carries
    fn art_version_of(frame: &str) -> Option<u64> {
        let value: serde_json::Value = serde_json::from_str(frame).ok()?;
        let cover = value["data"]["player"]["cover"].as_str()?;
        cover.rsplit('-').next()?.parse().ok()
    }

    /// the wire's monotone triple: (volume, vu_index, art version)
    /// — the hub's fields a test owns and only ever increases
    fn record_wire(rx: &mut broadcast::Receiver<String>, frames: &Mutex<Vec<(i32, i32, u64)>>) {
        while let Ok(frame) = rx.try_recv() {
            frames.lock().push((
                volume_of(&frame),
                vu_of(&frame),
                art_version_of(&frame).unwrap_or(0),
            ));
        }
    }

    /// regression (review finding: an older frame can overwrite a
    /// newer one): the UI thread (player state, covers) and the
    /// web runtime (view watcher) both publish, and the hub lock
    /// must keep framing and publishing one atomic step — an
    /// older frame may never land after a newer one. The gate
    /// freezes the first publisher right before the wire; before
    /// the fix that point sat after the hub lock was dropped, so
    /// the second publisher's frame went out first and the frozen
    /// one overwrote it (phase 1). The stress run (phase 2) then
    /// hammers both publishers against each other while a
    /// subscriber watches the wire
    #[test]
    fn concurrent_publishers_never_put_an_older_frame_back_on_the_wire() {
        let (push, rx) = push();

        // phase 1 (deterministic): freeze a publisher just before
        // its frame goes on the wire
        let (arrived_tx, arrived_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let push_a = push.clone();
        let a = std::thread::Builder::new()
            .spawn(move || {
                gate_install(&PUBLISH_GATE, arrived_tx, release_rx);
                push_a.update(&playing_at(1));
            })
            .unwrap();
        arrived_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the publisher froze at the wire");

        // the second publisher (the view watcher's role): frames
        // the hub and publishes while the first is frozen — only
        // possible when the hub lock is not held across publish
        let push_b = push.clone();
        let b = std::thread::spawn(move || push_b.set_context(context_vu(1)));
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            if push
                .events
                .snapshot()
                .is_some_and(|frame| vu_of(&frame) >= 1)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        release_tx.send(()).expect("the gate opens");
        a.join().unwrap();
        b.join().unwrap();

        // phase 2 (stress): both sides publish many changes while
        // a subscriber records every frame that goes out
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let frames: Arc<Mutex<Vec<(i32, i32, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let wire = {
            let (done, frames) = (done.clone(), frames.clone());
            let mut rx = rx;
            std::thread::spawn(move || {
                while !done.load(std::sync::atomic::Ordering::SeqCst) {
                    record_wire(&mut rx, &frames);
                    std::thread::sleep(Duration::from_millis(1));
                }
                record_wire(&mut rx, &frames);
            })
        };
        let iterations = 100u32;
        let push_state = push.clone();
        let a2 = std::thread::spawn(move || {
            for i in 1..=iterations {
                push_state.update(&playing_at(i as i32 + 1));
                if i % 2 == 0 {
                    push_state.cover(
                        Some(&cover_data(vec![255, 0, 0, 255])),
                        Some(&cover_data(vec![255, 255, 255, 255])),
                        Some(&palette()),
                    );
                }
            }
        });
        let push_ctx = push.clone();
        let b2 = std::thread::spawn(move || {
            for i in 2..=iterations + 1 {
                push_ctx.set_context(context_vu(i as i32));
            }
        });
        a2.join().unwrap();
        b2.join().unwrap();
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        wire.join().unwrap();

        // the broadcast sequence never carries an older frame
        // after a newer one
        let frames = frames.lock().clone();
        let mut last = (i32::MIN, i32::MIN, 0u64);
        for current in &frames {
            assert!(
                current.0 >= last.0 && current.1 >= last.1 && current.2 >= last.2,
                "the wire went back to an older frame: {last:?} then {current:?}"
            );
            last = *current;
        }

        // the snapshot must equal the frame of the hub's final
        // state — whatever coalesced pend, one flush past the
        // window publishes it
        std::thread::sleep(MIN_PUSH_INTERVAL + Duration::from_millis(10));
        push.flush();
        let hub_frame = frame_of(&push.inner.lock());
        let expected = serde_json::to_string(&state::PushFrame::state(hub_frame)).unwrap();
        assert_eq!(
            push.events.snapshot().as_deref(),
            Some(expected.as_str()),
            "the snapshot must be the hub's final frame"
        );
        assert!(
            push.events.cover_version() >= art_version_of(&expected).unwrap_or(0),
            "every version a frame carried is installed in the cover slot"
        );
    }

    /// regression (review finding: a new cover version can serve
    /// old image data): the version bump and the image install
    /// must be one step under the hub lock — /cover is cached
    /// forever per version URL, so a frame may never carry a
    /// version whose image is not in the slot yet. The gate
    /// freezes the cover publisher between the bump and the
    /// install; before the fix that window sat outside the hub
    /// lock, so the view watcher published a frame with the new
    /// version while the slot still held nothing
    #[test]
    fn a_frame_never_carries_a_cover_version_whose_image_is_not_installed() {
        let (push, _rx) = push();

        let (arrived_tx, arrived_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let push_a = push.clone();
        let a = std::thread::Builder::new()
            .spawn(move || {
                gate_install(&COVER_GATE, arrived_tx, release_rx);
                push_a.cover(
                    Some(&cover_data(vec![255, 0, 0, 255])),
                    Some(&cover_data(vec![255, 255, 255, 255])),
                    Some(&palette()),
                );
            })
            .unwrap();
        arrived_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the cover publisher froze between bump and install");

        // the view watcher's role: frame and publish whatever the
        // hub holds — the version is bumped already
        let push_b = push.clone();
        let b = std::thread::spawn(move || push_b.set_context(context_vu(1)));

        // the invariant: whenever a frame with version N is
        // observed, the slot must already hold at least N
        let mut violation = None;
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            if let Some(n) = push.events.snapshot().as_deref().and_then(art_version_of) {
                let installed = push.events.cover_version();
                if installed < n {
                    violation = Some((n, installed));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        release_tx.send(()).expect("the gate opens");
        a.join().unwrap();
        b.join().unwrap();
        if let Some((n, installed)) = violation {
            panic!("a frame carried cover version {n} while the slot held {installed}");
        }

        // let the window pass and flush whatever pends: the final
        // frame and the slot must agree too
        std::thread::sleep(MIN_PUSH_INTERVAL + Duration::from_millis(10));
        push.flush();
        let frame = push.events.snapshot().expect("a state frame");
        let n = art_version_of(&frame).expect("the frame carries a cover version");
        assert_eq!(push.events.cover_version(), 1);
        assert!(
            push.events.cover_version() >= n,
            "the slot serves every version the frame carries"
        );
        assert!(push.events.cover_jpeg().is_some());
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn preview_pngs_render_once_per_version() {
        let events = Events::new();
        let calls = std::cell::Cell::new(0u32);
        let render = || {
            calls.set(calls.get() + 1);
            Some(vec![0x89, b'P', b'N', b'G'])
        };
        let first = events.preview_png("vu", "round", "v1", render).unwrap();
        assert_eq!(first, vec![0x89, b'P', b'N', b'G']);
        // the same version never re-renders
        let cached = events
            .preview_png("vu", "round", "v1", || panic!("cache miss"))
            .unwrap();
        assert_eq!(cached, first);
        // a new version does
        assert!(events.preview_png("vu", "round", "v2", render).is_some());
        assert_eq!(calls.get(), 2);
        // a failed render is not cached
        assert!(events.preview_png("vu", "night", "v1", || None).is_none());
        assert!(events.preview_png("vu", "night", "v1", render).is_some());
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn the_preview_cache_resets_when_full() {
        let events = Events::new();
        for i in 0..PREVIEW_CACHE_MAX as u32 {
            let version = format!("v{i}");
            assert!(events
                .preview_png("vu", "round", &version, || Some(vec![1, 2]))
                .is_some());
        }
        // one entry over the cap clears the slot: v0 must re-render
        assert!(events
            .preview_png("vu", "round", "v0", || panic!("still cached"))
            .is_some());
    }
}
