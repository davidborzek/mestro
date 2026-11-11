//! Waking from standby reveals the page the user left — standby
//! is an overlay over the page stack, not a navigation target.

use mestro::{ui::navigation::Navigation, Mestro, Page};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
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
fn waking_from_standby_keeps_the_page() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(573, 344));
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }))
    .unwrap();
    let ui = Mestro::new().unwrap();
    let nav = Navigation::new();
    let power = Rc::new(RefCell::new(Vec::new()));
    ui.on_set_power({
        let power = power.clone();
        move |on| power.borrow_mut().push(on)
    });

    nav.go(&ui, Page::Library);
    ui.set_powered(false);
    assert!(ui.get_standby_showing());

    // the standby clock's tap (and a skin's power-on())
    ui.invoke_standby_power_on();

    assert_eq!(&*power.borrow(), &[true], "waking powers the player on");
    assert_eq!(ui.get_page(), Page::Library, "the page the user left");
    assert!(!ui.get_np_open(), "no forced now-playing overlay");
}
