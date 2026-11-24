use mestro::config::Config;
use mestro::{ui::navigation::Navigation, Mestro};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, rc::Rc};

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

#[test]
fn the_cycle_names_reach_the_ui_and_select_stays_inside_it() {
    slint::platform::set_platform(Box::new(Headless {
        window: MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer),
    }))
    .unwrap();
    let ui = Mestro::new().unwrap();
    let mut config = Config::default();
    config.vu.cycle = vec!["cream".into(), "night".into(), "round".into()];
    let nav = Navigation::new();
    let palette = Rc::new(RefCell::new(None));
    let vui = mestro::vu::wire::Vui::new(&config, &nav, palette);
    vui.wire(&ui, ui.as_weak());

    let names: Vec<String> = ui
        .get_vu_view_names()
        .iter()
        .map(|n| n.to_string())
        .collect();
    assert_eq!(names, ["cream", "night", "round"]);
    assert_eq!(ui.get_vu_view_count(), 3);

    assert!(vui.select(&ui, 2));
    assert_eq!(ui.get_vu_view_index(), 2);

    // out of range: refused, the index stays
    assert!(!vui.select(&ui, 3));
    assert!(!vui.select(&ui, usize::MAX));
    assert_eq!(ui.get_vu_view_index(), 2);
}
