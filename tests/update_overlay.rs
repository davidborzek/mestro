//! The system-update overlay: any status text shows it above the
//! whole UI, an empty status hides it.

use mestro::Mestro;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use std::rc::Rc;

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

#[test]
fn status_text_shows_the_overlay_and_empty_hides_it() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(573, 344));
    slint::platform::set_platform(Box::new(Headless { window })).unwrap();
    let ui = Mestro::new().unwrap();

    assert!(!ui.get_update_overlay_visible(), "hidden by default");

    ui.set_update_status("Receiving update … 42 %".into());
    assert!(ui.get_update_overlay_visible());

    ui.set_update_status("".into());
    assert!(!ui.get_update_overlay_visible());
}
