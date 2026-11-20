#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! The hotspot overlay: it renders whenever the wifi manager
//! reports the hotspot up — the first-run setup and the
//! lost-network fallback alike — and leaves the screen when AP
//! mode ends. Measurement, not vision: the QR plate is the only
//! large white area, so bright pixels prove the overlay painted.

use mestro::{HotspotReason, Mestro};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::rc::Rc;

/// the 5" kiosk panel at scale 1.5 → 533x320 logical
const W: u32 = 800;
const H: u32 = 480;
const SCALE: f32 = 1.5;

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
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

/// bright pixels: the QR plate's white — text alone never reaches
/// these counts
fn white(img: &image::RgbImage) -> usize {
    img.pixels()
        .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200)
        .count()
}

#[test]
fn the_hotspot_overlay_shows_for_both_reasons_and_hides_when_inactive() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W, H));
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE,
    });
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }))
    .unwrap();

    let ui = Mestro::new().unwrap();
    // a configured player idles underneath — the fallback case
    ui.set_powered(true);
    ui.set_hotspot_ssid("mestro-6BEC".into());
    ui.set_hotspot_qr(mestro::remote::qr::image(
        "WIFI:T:nopass;S:mestro-6BEC;;",
        260,
    ));
    ui.show().unwrap();
    let baseline = white(&render(&window));

    ui.set_hotspot_reason(HotspotReason::Setup);
    ui.set_hotspot_active(true);
    let setup = white(&render(&window));
    assert!(
        setup > baseline + 20_000,
        "the setup hotspot must paint the QR plate ({setup} vs {baseline})"
    );

    ui.set_hotspot_reason(HotspotReason::Fallback);
    ui.set_hotspot_saved_ssid("FritzBox".into());
    let fallback = white(&render(&window));
    assert!(
        fallback > baseline + 20_000,
        "the fallback hotspot must paint the QR plate ({fallback} vs {baseline})"
    );

    ui.set_hotspot_active(false);
    let hidden = white(&render(&window));
    assert!(
        hidden < baseline + 5_000,
        "the overlay must leave the screen when AP mode ends ({hidden} vs {baseline})"
    );
}
