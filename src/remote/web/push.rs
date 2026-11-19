//! The push pipeline: the UI thread owns the state (a `Push` hub
//! fed by the update loop and a view-prop poll), the web thread
//! owns the sockets. The seam is [`Events`]: the hub publishes
//! serialized state frames into a broadcast channel (plus a
//! latest-slot for the snapshot a fresh socket needs), and the
//! cover slot carries the artwork the `/cover` endpoint encodes
//! per version. Frames coalesce — at most one per 100 ms — and
//! position corrections alone never push (the frame carries no
//! position).

use super::state::{
    self, RemoteArt, RemoteContext, RemoteDisplay, RemoteState, RemoteView, Throttle,
};
use crate::player::cover::CoverData;
use crate::player::state::PlayerState;
use crate::providers::ma;
use crate::ui::palette::ArtworkPalette;
use crate::Mestro;
use slint::ComponentHandle as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;

/// how many frames may pile up before a slow client lags
const CHANNEL_CAPACITY: usize = 16;
/// the coalescing window
const MIN_PUSH_INTERVAL: Duration = Duration::from_millis(100);
/// the kiosk view-prop poll (they are plain slint properties,
/// readable only on the UI thread, with no event stream)
const VIEW_POLL: Duration = Duration::from_millis(250);

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
        *self.inner.latest.lock().unwrap() = frame.clone();
        let _ = self.inner.tx.send(frame);
    }

    /// the settings-changed event ([`super::state::SETTINGS_EVENT`]):
    /// broadcast only — never the latest slot, that holds the
    /// player state frame a fresh or lagged socket starts with
    pub fn publish_settings(&self) {
        let _ = self.inner.tx.send(super::state::SETTINGS_EVENT.to_string());
    }

    /// the last published frame, for the snapshot a fresh or
    /// lagged socket needs (None before the first state exists)
    pub fn snapshot(&self) -> Option<String> {
        let latest = self.inner.latest.lock().unwrap();
        (!latest.is_empty()).then(|| latest.clone())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.inner.tx.subscribe()
    }

    /// the latest cover + its pre-blurred backdrop, both
    /// versioned (0 = none yet)
    pub fn set_cover(&self, version: u64, cover: CoverData, backdrop: Option<CoverData>) {
        *self.inner.cover.lock().unwrap() = Some((version, cover));
        *self.inner.backdrop.lock().unwrap() = backdrop.map(|bg| (version, bg));
    }

    /// the current cover as JPEG (quality ~85), encoded at most
    /// once per version; None when no cover exists yet
    pub fn cover_jpeg(&self) -> Option<Vec<u8>> {
        let (version, cover) = self.inner.cover.lock().unwrap().clone()?;
        let mut jpeg = self.inner.jpeg.lock().unwrap();
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
        let (version, backdrop) = self.inner.backdrop.lock().unwrap().clone()?;
        let mut jpeg = self.inner.backdrop_jpeg.lock().unwrap();
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
        if let Some(bytes) = self.inner.previews.lock().unwrap().get(&key) {
            return Some(bytes.clone());
        }
        // render outside the lock: the UI-thread roundtrip can
        // take a while, a concurrent request for another preview
        // must not queue behind it
        let bytes = render()?;
        let mut cache = self.inner.previews.lock().unwrap();
        if cache.len() >= PREVIEW_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, bytes.clone());
        Some(bytes)
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

/// the UI-thread hub: accumulates the display's state, the
/// kiosk view flags and the cover version, and pushes frames
/// into [`Events`] whenever something changed. Frames are small
/// and the throttle caps the send rate, so every PlayerUpdate
/// can afford the roundtrip.
pub struct Push {
    inner: Rc<RefCell<Hub>>,
    events: Events,
    ma_cfg: Arc<RwLock<ma::Config>>,
}

impl Clone for Push {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
            events: self.events.clone(),
            ma_cfg: Arc::clone(&self.ma_cfg),
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
    pub fn new(events: Events, ma_cfg: Arc<RwLock<ma::Config>>, player: &str) -> Self {
        Self {
            events,
            ma_cfg,
            inner: Rc::new(RefCell::new(Hub {
                player: player.to_string(),
                state: PlayerState::default(),
                ctx: RemoteContext {
                    connected: false,
                    ma_url: String::new(),
                    view: RemoteView {
                        now_playing: false,
                        queue: false,
                        vu: false,
                        standby: false,
                        vu_index: 0,
                        vu_count: 0,
                    },
                    display: RemoteDisplay {
                        supported: false,
                        brightness: 100,
                    },
                },
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

    /// the update loop's player state (every PlayerUpdate)
    pub fn update(&self, state: &PlayerState) {
        let changed = {
            let mut hub = self.inner.borrow_mut();
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
        let version = {
            let mut hub = self.inner.borrow_mut();
            hub.art.version += 1;
            hub.art.accent = palette.map_or_else(String::new, |p| hex(p.accent));
            hub.art.version
        };
        self.events
            .set_cover(version, cover.clone(), backdrop.cloned());
        self.push_if_changed(false);
    }

    /// the kiosk view flags, the MA connection, the web UI URL
    /// and the backlight (the 250 ms poll + the view-changed
    /// callback). A VIEW change bypasses the throttle — a tile
    /// tap must not wait out the coalescing window; everything
    /// else stays throttled
    pub fn set_context(&self, ctx: RemoteContext) {
        let (changed, view_changed) = {
            let mut hub = self.inner.borrow_mut();
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
        if self.inner.borrow().pending {
            self.push_if_changed(false);
        }
    }

    /// frame the current state and publish it when something
    /// changed and the throttle permits (a forced push bypasses
    /// the window); otherwise the pending flag keeps it queued
    /// for the next timer tick
    fn push_if_changed(&self, force: bool) {
        let mut hub = self.inner.borrow_mut();
        let frame = frame_of(&hub);
        hub.pending |= hub.last.as_ref().is_none_or(|last| *last != frame);
        if !hub.pending || (!force && !hub.throttle.permit(std::time::Instant::now())) {
            return;
        }
        hub.pending = false;
        match serde_json::to_string(&state::PushFrame::state(frame.clone())) {
            Ok(text) => {
                hub.last = Some(frame);
                drop(hub);
                self.events.publish(text);
            }
            Err(e) => {
                // try again on the next tick
                hub.pending = true;
                tracing::warn!("web: state frame: {e}");
            }
        }
    }

    /// the background timers: the coalescing flush and the
    /// view-prop poll (slint properties, readable only on this
    /// thread). The poll is the SAFETY NET — the view-changed
    /// callback (wired here) covers the view flips at once.
    /// Call once, on the UI thread, before the event loop runs
    pub fn start_timers(&self, ui: &Mestro) {
        let push = self.clone();
        let flush = slint::Timer::default();
        flush.start(slint::TimerMode::Repeated, MIN_PUSH_INTERVAL, move || {
            push.flush()
        });
        let push = self.clone();
        let weak = ui.as_weak();
        let poll = slint::Timer::default();
        poll.start(slint::TimerMode::Repeated, VIEW_POLL, move || {
            let Some(ui) = weak.upgrade() else { return };
            push.set_context(context_of(&ui, &push.ma_cfg.read().unwrap().ws_url));
        });
        let push = self.clone();
        let weak = ui.as_weak();
        ui.on_view_changed(move || {
            let Some(ui) = weak.upgrade() else { return };
            push.set_context(context_of(&ui, &push.ma_cfg.read().unwrap().ws_url));
        });
        std::mem::forget((flush, poll));
    }
}

/// the kiosk view flags, the MA connection and the backlight as
/// sampled from the UI's properties
fn context_of(ui: &Mestro, ws_url: &str) -> RemoteContext {
    RemoteContext {
        connected: ui.get_provider_connected(),
        ma_url: state::ma_web_url(ws_url),
        view: RemoteView {
            now_playing: ui.get_np_open(),
            queue: ui.get_queue_open(),
            vu: ui.get_vu_open(),
            standby: ui.get_standby_showing(),
            vu_index: ui.get_vu_view_index(),
            vu_count: ui.get_vu_view_count(),
        },
        display: RemoteDisplay {
            supported: ui.get_display_supported(),
            brightness: ui.get_display_brightness().round() as i32,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ma_cfg() -> Arc<RwLock<ma::Config>> {
        Arc::new(RwLock::new(ma::Config {
            ws_url: "ws://ma.local:8095/ws".into(),
            token: String::new(),
            player_name: "wohnzimmer".into(),
        }))
    }

    // a hub with a live player, without the timers; the receiver
    // counts what actually goes on the wire
    fn push() -> (Push, broadcast::Receiver<String>) {
        let events = Events::new();
        let rx = events.subscribe();
        (Push::new(events, ma_cfg(), "wohnzimmer"), rx)
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
        assert_eq!(push.inner.borrow().art.version, 1);
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
        assert_eq!(push.inner.borrow().art.version, 0);
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
