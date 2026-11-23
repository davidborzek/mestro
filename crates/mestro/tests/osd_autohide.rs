//! OSD auto-hide: the toast and the volume overlay hide on their
//! own timers. A show on one channel must not cancel the other
//! channel's pending hide (a shared generation counter left the
//! toast stuck whenever a volume change overlapped it).

use mestro::Mestro;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use std::{cell::Cell, rc::Rc, time::Duration};

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

fn advance(clock: &Cell<Duration>, ms: u64) {
    clock.set(clock.get() + Duration::from_millis(ms));
    slint::platform::update_timers_and_animations();
}

#[test]
fn toast_and_volume_hide_independently() {
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(573, 344));
    slint::platform::set_platform(Box::new(Headless {
        clock: clock.clone(),
        window: window.clone(),
    }))
    .unwrap();
    let ui = Mestro::new().unwrap();
    mestro::ui::osd::wire(&ui);
    ui.set_osd_toast_duration(1000.0);
    ui.set_osd_volume_duration(1000.0);
    slint::platform::update_timers_and_animations();

    // a volume change lands while the toast is up
    ui.invoke_show_toast("Added to queue".into());
    advance(&clock, 300);
    ui.invoke_show_osd_volume(40);
    assert!(ui.get_toast_visible());
    assert!(ui.get_osd_visible());

    // the toast's own time is up: it hides, the volume stays
    advance(&clock, 800);
    assert!(
        !ui.get_toast_visible(),
        "toast stuck after an overlapping volume show"
    );
    assert!(ui.get_osd_visible());

    // the volume's time is up too
    advance(&clock, 600);
    assert!(!ui.get_osd_visible());

    // the other way round: a toast during the volume overlay
    ui.invoke_show_osd_volume(50);
    advance(&clock, 300);
    ui.invoke_show_toast("Added to queue".into());
    advance(&clock, 800);
    assert!(
        !ui.get_osd_visible(),
        "volume overlay stuck after an overlapping toast"
    );
    assert!(ui.get_toast_visible());

    // a newer toast still extends its own channel
    ui.invoke_show_toast("Added again".into());
    advance(&clock, 600);
    assert!(ui.get_toast_visible());
    advance(&clock, 500);
    assert!(!ui.get_toast_visible());
}
