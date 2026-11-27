//! The glow must reach the LIVE faceplate's pixels, not only the
//! preview: the device renders the attached skin instance, so the
//! check draws the whole player window with the VU open.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mestro::config::{Config, VuMode};
use mestro::ui::navigation::Navigation;
use mestro::vu::wire::Vui;
use mestro::Mestro;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

const W: usize = 800;
const H: usize = 480;

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn frame(window: &Rc<MinimalSoftwareWindow>) -> Vec<slint::Rgb8Pixel> {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut pixels = vec![slint::Rgb8Pixel::default(); W * H];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, W);
    });
    pixels
}

/// warmth of a pixel: red minus blue
fn warmth(p: slint::Rgb8Pixel) -> i32 {
    p.r as i32 - p.b as i32
}

#[test]
fn the_live_wall_light_plate_carries_the_glow() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W as u32, H as u32));
    let _ = slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }));

    let render = |glow: f64| {
        let mut config = Config::default();
        config.vu.mode = VuMode::Off;
        config.vu.cycle = vec!["wall-light".into()];
        config.vu.wall_light.glow = glow;
        let ui = Mestro::new().unwrap();
        ui.set_powered(true);
        let nav = Navigation::new();
        let vui = Vui::new(&config, &nav, Rc::new(RefCell::new(None)));
        vui.wire(&ui, ui.as_weak());
        nav.show_vu(&ui);
        ui.show().unwrap();
        ui.invoke_vu_sync();
        frame(&window);
        ui.invoke_vu_sync();
        frame(&window);
        let pixels = frame(&window);
        ui.hide().unwrap();
        pixels
    };

    let off = render(0.0);
    let on = render(1.0);
    // the plate's middle, above the scales (the zone sits left of
    // the 56px rail): the lamp's warmest spot
    let at = |px: &[slint::Rgb8Pixel], x: usize, y: usize| px[y * W + x];
    let (cx, cy) = ((W - 56) / 2, H / 6);
    let gain = warmth(at(&on, cx, cy)) - warmth(at(&off, cx, cy));
    assert!(
        gain > 40,
        "the live plate must glow: off {:?} on {:?} (gain {gain})",
        at(&off, cx, cy),
        at(&on, cx, cy)
    );
}
