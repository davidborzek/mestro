//! Offscreen snapshots of the settings pages for visual review.
//! Renders the real Mestro window with the software renderer into
//! PNGs — no window, nothing on the desktop. Opt-in (ignored by
//! default):
//!
//!   MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test settings_snapshot -- --ignored
//!
//! The software renderer cannot rotate (slint #6068): rotated
//! elements come out wrong. The settings pages use none; the VU
//! overview's needles arrive pre-rotated in their SVG (the
//! preview path bakes the rotation, see src/vu/manifest.rs).

use mestro::{ui::navigation::Navigation, Mestro, Page};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

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

/// saturated stripes: makes any backdrop bleed obvious
fn loud_cover() -> slint::Image {
    let (w, h) = (320u32, 320u32);
    let mut px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    for (i, p) in px.make_mut_slice().iter_mut().enumerate() {
        let x = i as u32 % w;
        *p = match (x / 40) % 4 {
            0 => slint::Rgba8Pixel {
                r: 230,
                g: 40,
                b: 60,
                a: 255,
            },
            1 => slint::Rgba8Pixel {
                r: 40,
                g: 200,
                b: 90,
                a: 255,
            },
            2 => slint::Rgba8Pixel {
                r: 50,
                g: 90,
                b: 240,
                a: 255,
            },
            _ => slint::Rgba8Pixel {
                r: 250,
                g: 220,
                b: 40,
                a: 255,
            },
        };
    }
    slint::Image::from_rgba8(px)
}

#[test]
#[ignore = "visual review: MESTRO_SNAPSHOT_DIR=… cargo test --test settings_snapshot -- --ignored"]
fn settings_pages() {
    let dir = PathBuf::from(std::env::var("MESTRO_SNAPSHOT_DIR").expect("set MESTRO_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    // hermetic: never read the developer's own settings overlay
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::env::set_var("XDG_STATE_HOME", &state);

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
    // the effective config + overlay state, as main pushes them at
    // startup (defaults: no config file, empty overlay)
    let (config, base_tree, _overlay, base_path) =
        mestro::config::load_parts(Some(std::path::Path::new("/dev/null"))).unwrap();
    let handle =
        mestro::settings::Handle::new(mestro::settings::Overlay::default(), base_tree, base_path);
    let np_state = Rc::new(RefCell::new(mestro::settings::NpState::from(&config)));
    mestro::settings::sync_initial(&ui, &handle, &np_state);
    mestro::settings::sync_theme_ui(&ui, &handle, &config);
    mestro::settings::sync_connection(&ui, &config);
    // the hermetic config has no server: the first-run overlay
    // would cover every page — shot on its own below
    ui.set_first_run(false);
    mestro::settings::sync_overlay_state(&ui, &handle);
    ui.set_powered(true);
    ui.set_app_version("0.1.0".into());
    // a real pairing QR, as main bakes it
    let url = "http://192.168.1.50:8787/?pin=4821";
    ui.set_settings_remote_url(url.into());
    ui.set_first_run_qr(mestro::remote::qr::image(url, 260));
    // a loud cover behind the pages: the backdrop must not bleed
    // through the settings tiles
    ui.set_has_media(true);
    ui.set_backdrop_enabled(true);
    ui.set_bg_cover(loud_cover());

    let nav = Navigation::new();
    nav.go(&ui, Page::Settings);
    ui.show().unwrap();
    shot(&window, &dir.join("settings-0-index.png"));

    for (sub, name) in [
        (1, "nowplaying"),
        (2, "fonts"),
        (6, "theme"),
        (10, "standby"),
        (11, "hub"),
        (12, "osd"),
        (7, "connection"),
        (13, "remote-setup"),
        (3, "about"),
    ] {
        nav.go(&ui, Page::Settings);
        nav.push_sub(&ui, sub);
        shot(&window, &dir.join(format!("settings-{sub}-{name}.png")));
    }
    // the first-run wizard screen, as a fresh device shows it
    ui.set_first_run(true);
    shot(&window, &dir.join("first-run.png"));
    ui.set_first_run(false);

    // config errors: the settings banner (connection fine) …
    let problems = slint::ModelRc::new(slint::VecModel::from(vec![
        mestro::ConfigProblem {
            key: "display.future-key".into(),
            reason: "unknown field `future-key`, expected `brightness` or `standby-brightness`"
                .into(),
        },
        mestro::ConfigProblem {
            key: "clock.background.source".into(),
            reason: "unknown variant `sparkles`, expected one of `black`, `cover`, `folder`".into(),
        },
    ]));
    ui.set_config_problems(problems);
    nav.go(&ui, Page::Settings);
    nav.back(&ui);
    nav.go(&ui, Page::Settings);
    shot(&window, &dir.join("settings-0-index-config-errors.png"));

    // … and the full-screen notice when the connection is gone
    ui.set_first_run(true);
    shot(&window, &dir.join("config-error-screen.png"));
    ui.set_first_run(false);

    // the VU overview: the same pre-rotated preview path the
    // device's picker uses (the software renderer cannot rotate,
    // the needles are baked into the SVG — see src/vu/manifest.rs).
    // The overview lives INSIDE the VU layer, so vu-open must be
    // up too
    let palette: Rc<RefCell<Option<mestro::ui::palette::ArtworkPalette>>> =
        Rc::new(RefCell::new(None));
    let vui = mestro::vu::wire::Vui::new(&config, &nav, palette);
    ui.set_vu_previews(slint::ModelRc::new(slint::VecModel::from(vui.previews())));
    ui.set_vu_open(true);
    ui.set_vu_overview_open(true);
    shot(&window, &dir.join("device-vu-overview.png"));

    // measurement, not vision: a synthetic magenta preview must
    // reach the screen through the grid, and only while the
    // overview is open
    let magenta_tile = || {
        let mut px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(286, 172);
        for p in px.make_mut_slice().iter_mut() {
            *p = slint::Rgba8Pixel {
                r: 255,
                g: 0,
                b: 255,
                a: 255,
            };
        }
        slint::Image::from_rgba8(px)
    };
    ui.set_vu_previews(slint::ModelRc::new(slint::VecModel::from(vec![
        mestro::VuPreviewData {
            image: magenta_tile(),
            name: "measure".into(),
            kind: "VU".into(),
            active: true,
        },
    ])));
    let magenta = |img: &image::RgbImage| {
        img.pixels()
            .filter(|p| p[0] > 200 && p[1] < 80 && p[2] > 200)
            .count()
    };
    let open = magenta(&render(&window));
    ui.set_vu_overview_open(false);
    let closed = magenta(&render(&window));
    assert!(
        open > 1000,
        "the preview images did not reach the overview ({open} magenta pixels)"
    );
    assert_eq!(closed, 0, "the magenta must come from the overview tiles");
    ui.set_vu_open(false);
    let _ = ui.as_weak();
}
