//! The fallback hotspot while the player idles in standby: the
//! overlay must paint OVER the standby clock (mount order) and the
//! backlight must not sit at the standby brightness under it —
//! `standby-showing`, which Rust wires to the backlight, stays
//! false while the QR shows. Measurement, not vision: the QR plate
//! is the only large white area.

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

/// bright pixels: the QR plate's white — the standby clock never
/// reaches these counts
fn white(img: &image::RgbImage) -> usize {
    img.pixels()
        .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200)
        .count()
}

#[test]
fn the_hotspot_paints_over_standby_and_keeps_the_backlight_awake() {
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
    // the player idles in standby (powered off) when the fallback
    // hotspot starts — the panel showed the standby clock
    ui.set_powered(false);
    ui.set_hotspot_ssid("mestro-6BEC".into());
    ui.set_hotspot_qr(mestro::remote::qr::image(
        "WIFI:T:nopass;S:mestro-6BEC;;",
        260,
    ));
    ui.show().unwrap();
    assert!(ui.get_standby_showing(), "plain standby dims the backlight");
    let standby = white(&render(&window));

    ui.set_hotspot_reason(HotspotReason::Fallback);
    ui.set_hotspot_saved_ssid("FritzBox".into());
    ui.set_hotspot_active(true);
    let hotspot = white(&render(&window));
    assert!(
        hotspot > standby + 20_000,
        "the QR plate must paint over the standby clock ({hotspot} vs {standby})"
    );
    assert!(
        !ui.get_standby_showing(),
        "the backlight must not dim to the standby brightness while the QR shows"
    );

    ui.set_hotspot_active(false);
    assert!(
        ui.get_standby_showing(),
        "standby returns when the hotspot ends"
    );
    let back = white(&render(&window));
    assert!(
        back < standby + 5_000,
        "the standby clock is back ({back} vs {standby})"
    );
}
