#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! Offscreen snapshots of the VU faceplate with the nav rail
//! visible vs. tucked away by [vu] rail-autohide — the zone
//! takes the full width while the rail is hidden. Rendered on
//! the software renderer, no window, nothing on the desktop.
//! Opt-in (ignored by default):
//!
//!   MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test vu_rail_snapshot -- --ignored
//!
//! The fake platform clock drives both the hide timer and the
//! tuck animation. The software renderer cannot rotate (slint
//! #6068), so the shot uses the wall faceplate — meters only,
//! no needle rotation.

use mestro::config::Config;
use mestro::{ui::navigation::Navigation, Mestro};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

struct Headless {
    clock: Rc<Cell<Duration>>,
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.clock.get()
    }
}

/// the 5" kiosk panel at scale 1.5 → 533x320 logical
const W: u32 = 800;
const H: u32 = 480;
const SCALE: f32 = 1.5;

fn advance(clock: &Rc<Cell<Duration>>, ms: u64) {
    clock.set(clock.get() + Duration::from_millis(ms));
    slint::platform::update_timers_and_animations();
}

fn shot(window: &MinimalSoftwareWindow, path: &PathBuf) {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(W, H);
    window.draw_if_needed(|renderer| {
        renderer.render(buffer.make_mut_slice(), W as usize);
    });
    image::RgbImage::from_raw(W, H, buffer.as_bytes().to_vec())
        .expect("buffer size")
        .save(path)
        .expect("write png");
}

#[test]
#[ignore = "visual review: MESTRO_SNAPSHOT_DIR=… cargo test --test vu_rail_snapshot -- --ignored"]
fn vu_with_the_rail_visible_and_tucked() {
    let dir = PathBuf::from(std::env::var("MESTRO_SNAPSHOT_DIR").expect("set MESTRO_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&dir).unwrap();

    let clock = Rc::new(Cell::new(Duration::ZERO));
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W, H));
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE,
    });
    slint::platform::set_platform(Box::new(Headless {
        clock: clock.clone(),
        window: window.clone(),
    }))
    .unwrap();

    let ui = Mestro::new().unwrap();
    let mut config = Config::default();
    config.vu.cycle = vec!["wall".into()];
    let nav = Navigation::new();
    let palette: Rc<RefCell<Option<mestro::ui::palette::ArtworkPalette>>> =
        Rc::new(RefCell::new(None));
    let vui = mestro::vu::wire::Vui::new(&config, &nav, palette);
    vui.wire(&ui, ui.as_weak());
    ui.set_powered(true);
    ui.set_has_media(true);
    ui.set_track_title("Nightfall".into());
    ui.set_track_artist("The Meters".into());

    // entering the VU: the rail shows next to the faceplate
    ui.set_vu_rail_autohide(5.0);
    nav.show_vu(&ui);
    ui.show().unwrap();
    advance(&clock, 100);
    assert!(!ui.get_vu_rail_hidden());
    shot(&window, &dir.join("vu-rail-visible.png"));

    // 5 s without touch: the rail tucks away, the faceplate zone
    // takes the full width (plus the tuck animation's 250 ms)
    advance(&clock, 5_100);
    assert!(ui.get_vu_rail_hidden());
    advance(&clock, 400);
    shot(&window, &dir.join("vu-rail-hidden.png"));
}
