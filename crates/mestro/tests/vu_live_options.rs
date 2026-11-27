//! Settings edits must reach the LIVE faceplate: [vu.<skin>]
//! options (glow, the analyzers' meters/gradient, info lines)
//! apply without a restart, through the same handle → sync_vu
//! path the settings page, web endpoint and ctl socket use.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mestro::config::{Config, VuMode};
use mestro::settings::{sync_vu, Handle, Overlay, VuState};
use mestro::ui::navigation::Navigation;
use mestro::vu::wire::Vui;
use mestro::Mestro;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Model};
use slint_interpreter::Value;
use std::cell::RefCell;
use std::rc::Rc;

const W: usize = 573;
const H: usize = 344;

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn setup(config: &Config) -> (Mestro, Rc<MinimalSoftwareWindow>, Vui, Navigation) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W as u32, H as u32));
    let _ = slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }));
    let ui = Mestro::new().unwrap();
    ui.set_powered(true);
    let nav = Navigation::new();
    let vui = Vui::new(config, &nav, Rc::new(RefCell::new(None)));
    vui.wire(&ui, ui.as_weak());
    (ui, window, vui, nav)
}

/// render one frame — the ComponentContainer instantiates the
/// freshly activated skin
fn draw(window: &Rc<MinimalSoftwareWindow>) {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut pixels = vec![slint::Rgb8Pixel::default(); W * H];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, W);
    });
}

/// the device's settings handle: a sparse overlay over the
/// built-in defaults, persisted into the test's tempdir
fn handle(dir: &std::path::Path) -> Rc<Handle> {
    Rc::new(Handle::new(
        Overlay {
            tree: None,
            path: dir.join("settings.toml"),
        },
        toml::Value::Table(Default::default()),
        None,
    ))
}

/// one settings edit through the handle → sync_vu path
fn edit(
    ui: &Mestro,
    handle: &Handle,
    vui: &Vui,
    vu_state: &Rc<RefCell<VuState>>,
    key: &str,
    value: toml::Value,
) {
    let effective = handle.set(key, value).expect("the edit is valid");
    sync_vu(ui, handle, &effective, vui, vu_state);
}

fn live_number(vui: &Vui, name: &str) -> f64 {
    match vui.live_property(name) {
        Some(Value::Number(n)) => n,
        other => panic!("the live skin's {name} must be a number, got {other:?}"),
    }
}

fn live_bool(vui: &Vui, name: &str) -> bool {
    match vui.live_property(name) {
        Some(Value::Bool(b)) => b,
        other => panic!("the live skin's {name} must be a bool, got {other:?}"),
    }
}

fn info_rows(vui: &Vui) -> Vec<String> {
    match vui.live_property("info-lines") {
        Some(Value::Model(model)) => {
            let mut rows = Vec::new();
            for i in 0..model.row_count() {
                match model.row_data(i) {
                    Some(Value::String(s)) => rows.push(s.to_string()),
                    other => panic!("info row {i} must be a string, got {other:?}"),
                }
            }
            rows
        }
        other => panic!("the live skin must carry the info-lines model, got {other:?}"),
    }
}

#[test]
fn settings_edits_reach_the_live_faceplate() {
    let mut config = Config::default();
    config.vu.mode = VuMode::Off;
    config.vu.cycle = vec!["wall-light".into(), "spectrum".into(), "night".into()];
    let (ui, window, vui, nav) = setup(&config);
    let vu_state = Rc::new(RefCell::new(VuState::from(&config)));
    let dir = tempfile::tempdir().unwrap();
    let handle = handle(dir.path());

    // open the VU on wall-light: the tick attaches the skin, the
    // draw instantiates it
    nav.show_vu(&ui);
    ui.show().unwrap();
    ui.invoke_vu_sync();
    draw(&window);
    assert_eq!(live_number(&vui, "glow"), 1.0);

    // a glow edit must reach the attached instance now, not after
    // a restart
    edit(
        &ui,
        &handle,
        &vui,
        &vu_state,
        "vu.wall-light.glow",
        0.0.into(),
    );
    assert_eq!(live_number(&vui, "glow"), 0.0);

    // and the NEXT activation (close → reopen) must start at the
    // edited value, not the boot one
    ui.set_vu_open(false);
    ui.invoke_vu_sync();
    draw(&window);
    ui.set_vu_open(true);
    ui.invoke_vu_sync();
    draw(&window);
    assert_eq!(live_number(&vui, "glow"), 0.0);

    edit(
        &ui,
        &handle,
        &vui,
        &vu_state,
        "vu.wall-light.glow",
        1.0.into(),
    );
    assert_eq!(live_number(&vui, "glow"), 1.0);

    // the analyzer knobs ride the tick's options push
    assert!(vui.select(&ui, 1));
    draw(&window);
    assert!(!live_bool(&vui, "meters"));
    edit(
        &ui,
        &handle,
        &vui,
        &vu_state,
        "vu.spectrum.meters",
        true.into(),
    );
    edit(
        &ui,
        &handle,
        &vui,
        &vu_state,
        "vu.spectrum.gradient",
        true.into(),
    );
    ui.invoke_vu_sync();
    assert!(live_bool(&vui, "meters"));
    assert!(live_bool(&vui, "gradient"));

    // info lines: a fresh block, then a same-count field swap —
    // the in-place update path must not swallow the change
    ui.set_track_title("Blue in Green".into());
    ui.set_track_artist("Miles Davis".into());
    assert!(vui.select(&ui, 2));
    draw(&window);
    assert_eq!(live_number(&vui, "info-count"), 0.0);
    edit(
        &ui,
        &handle,
        &vui,
        &vu_state,
        "vu.night.info",
        toml::Value::Array(vec!["title".into()]),
    );
    ui.invoke_vu_sync();
    assert_eq!(live_number(&vui, "info-count"), 1.0);
    assert_eq!(info_rows(&vui), ["Blue in Green"]);
    edit(
        &ui,
        &handle,
        &vui,
        &vu_state,
        "vu.night.info",
        toml::Value::Array(vec!["artist".into()]),
    );
    ui.invoke_vu_sync();
    assert_eq!(live_number(&vui, "info-count"), 1.0);
    assert_eq!(info_rows(&vui), ["Miles Davis"]);
}
