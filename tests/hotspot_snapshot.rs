//! Offscreen snapshots of the hotspot overlay for visual review:
//! the first-run setup variant and the lost-network fallback,
//! rendered with the software renderer at the 5" panel's scale.
//! No window, nothing on the desktop. Opt-in (ignored by default):
//!
//!   MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test hotspot_snapshot -- --ignored

use mestro::{HotspotReason, Mestro};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::{path::PathBuf, rc::Rc};

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

/// the 5" kiosk panel at scale 1.5 → 533x320 logical
const W: u32 = 800;
const H: u32 = 480;
const SCALE: f32 = 1.5;

fn shot(window: &MinimalSoftwareWindow, path: &PathBuf) {
    render(window).save(path).expect("write png");
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

#[test]
#[ignore = "visual review: MESTRO_SNAPSHOT_DIR=… cargo test --test hotspot_snapshot -- --ignored"]
fn hotspot_overlays() {
    let dir = PathBuf::from(std::env::var("MESTRO_SNAPSHOT_DIR").expect("set MESTRO_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&dir).unwrap();

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
    // the setup variant: a first-run player that came up as the
    // hotspot (the overlay covers the wizard view underneath)
    ui.set_first_run(true);
    ui.set_hotspot_ssid("mestro-6BEC".into());
    ui.set_hotspot_qr(mestro::remote::qr::image(
        "WIFI:T:nopass;S:mestro-6BEC;;",
        260,
    ));
    ui.set_hotspot_active(true);
    ui.show().unwrap();
    shot(&window, &dir.join("hotspot-setup.png"));

    // the fallback variant: a configured player that lost its
    // network — the app idles underneath, the retry line shows
    ui.set_first_run(false);
    ui.set_powered(true);
    ui.set_hotspot_reason(HotspotReason::Fallback);
    ui.set_hotspot_saved_ssid("FritzBox".into());
    shot(&window, &dir.join("hotspot-fallback.png"));
    let _ = ui.as_weak();
}
