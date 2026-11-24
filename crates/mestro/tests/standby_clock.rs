#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! The standby clock before the time is trustworthy (no RTC, NTP
//! not answered): no hands, no lit ring, no digits — a placeholder
//! instead of a wrong time. Pixel checks on the analog face (shapes,
//! no fonts needed); the digital face is covered by a snapshot for
//! visual review (`--ignored`, needs fonts).
//!
//!   MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test standby_clock -- --ignored

use mestro::standby::ClockValues;
use mestro::Mestro;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle as _;
use std::rc::Rc;

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

const W: u32 = 573;
const H: u32 = 344;

fn setup() -> (Mestro, Rc<MinimalSoftwareWindow>) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W, H));
    let _ = slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }));
    let ui = Mestro::new().unwrap();
    ui.set_powered(false);
    (ui, window)
}

fn show(ui: &Mestro, values: &ClockValues) {
    ui.set_clock_label(values.time_label.as_str().into());
    ui.set_clock_date(values.date_label.as_str().into());
    ui.set_clock_minute(values.minute);
    ui.set_clock_hour(values.hour);
    ui.set_clock_valid(values.valid);
}

fn render(window: &MinimalSoftwareWindow) -> image::RgbImage {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(W, H);
    window.draw_if_needed(|renderer| {
        renderer.render(buffer.make_mut_slice(), W as usize);
    });
    image::RgbImage::from_raw(W, H, buffer.as_bytes().to_vec()).expect("buffer size")
}

/// pixels clearly brighter than the dial's unlit marks (#4a443b is
/// the brightest of those) — the hands (#a59a8a) and the lit ring —
/// outside the centre cap, which stays on a bare dial
fn hand_pixels(img: &image::RgbImage) -> u32 {
    const CAP_RADIUS: i64 = 12;
    let (cx, cy) = (i64::from(W) / 2, i64::from(H) / 2);
    img.enumerate_pixels()
        .filter(|(x, y, p)| {
            let (dx, dy) = (i64::from(*x) - cx, i64::from(*y) - cy);
            dx * dx + dy * dy > CAP_RADIUS * CAP_RADIUS
                && p.0[0] as u32 + p.0[1] as u32 + p.0[2] as u32 > 330
        })
        .count() as u32
}

fn at(year: i32, hour: u32, minute: u32) -> ClockValues {
    let now = chrono::NaiveDate::from_ymd_opt(year, 6, 15)
        .unwrap()
        .and_hms_opt(hour, minute, 0)
        .unwrap();
    ClockValues::at_with(now, 2026)
}

#[test]
fn the_analog_face_without_a_trusted_time_has_no_hands_and_no_lit_ring() {
    let (ui, window) = setup();
    ui.set_clock_analog(true);
    ui.show().unwrap();

    show(&ui, &at(2026, 10, 10));
    let with_time = hand_pixels(&render(&window));
    assert!(with_time > 150, "a trusted time paints hands: {with_time}");

    show(&ui, &at(1970, 0, 0));
    let without_time = hand_pixels(&render(&window));
    assert!(
        without_time < 20,
        "before NTP the dial is bare, {without_time} bright pixels left"
    );

    // the first real tick brings the hands back
    show(&ui, &at(2026, 10, 10));
    assert!(hand_pixels(&render(&window)) > 150);
}

#[test]
fn the_window_carries_the_placeholder_label_for_the_topbar() {
    let (ui, _window) = setup();
    show(&ui, &at(1970, 0, 0));
    assert_eq!(ui.get_clock_label().as_str(), "--:--");
    assert!(!ui.get_clock_valid());
    show(&ui, &at(2026, 14, 30));
    assert_eq!(ui.get_clock_label().as_str(), "14:30");
    assert!(ui.get_clock_valid());
}

/// the digital face and the topbar, both states, for visual review
#[test]
#[ignore = "visual review: MESTRO_SNAPSHOT_DIR=… cargo test --test standby_clock -- --ignored"]
fn clock_snapshots() {
    let dir = std::path::PathBuf::from(std::env::var("MESTRO_SNAPSHOT_DIR").expect("set it"));
    std::fs::create_dir_all(&dir).unwrap();
    let (ui, window) = setup();
    ui.show().unwrap();
    for (name, values) in [
        ("standby-digital-trusted", at(2026, 21, 42)),
        ("standby-digital-placeholder", at(1970, 0, 0)),
    ] {
        show(&ui, &values);
        render(&window)
            .save(dir.join(format!("{name}.png")))
            .unwrap();
    }
    ui.set_clock_analog(true);
    for (name, values) in [
        ("standby-analog-trusted", at(2026, 10, 10)),
        ("standby-analog-placeholder", at(1970, 0, 0)),
    ] {
        show(&ui, &values);
        render(&window)
            .save(dir.join(format!("{name}.png")))
            .unwrap();
    }
    ui.set_powered(true);
    for (name, values) in [
        ("topbar-trusted", at(2026, 21, 42)),
        ("topbar-placeholder", at(1970, 0, 0)),
    ] {
        show(&ui, &values);
        render(&window)
            .save(dir.join(format!("{name}.png")))
            .unwrap();
    }
}
