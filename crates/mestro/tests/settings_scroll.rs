//! Settings remember the scroll position per page: back from a
//! subpage lands where you were, a freshly opened page starts at
//! the top.

use mestro::{ui::navigation::Navigation, Mestro, Page};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::rc::Rc;

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

/// `changed` handlers are evaluated lazily: drive one frame (timers
/// + a render pass) so they run, as the event loop would
fn settle(window: &MinimalSoftwareWindow) {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut buffer = vec![slint::Rgb8Pixel::default(); 573 * 344];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, 573);
    });
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

#[test]
fn back_restores_the_scroll_position() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(573, 344));
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }))
    .unwrap();
    let ui = Mestro::new().unwrap();
    let nav = Navigation::new();
    // the settings page pushes subviews through this callback (the
    // app wires it in Navigation::wire, which needs the library)
    ui.on_push_subview({
        let nav = nav.clone();
        let weak = ui.as_weak();
        move |sub| nav.push_sub(&weak.upgrade().unwrap(), sub)
    });
    ui.set_powered(true);
    ui.show().unwrap();

    nav.go(&ui, Page::Settings);
    settle(&window);
    // scrolled down the index (viewport-y is negative when scrolled)
    ui.set_settings_scroll_y(-180.0);

    // open a subpage the way a row tap does: it starts at the top
    ui.invoke_push_subview(12);
    settle(&window);
    assert_eq!(ui.get_subview(), 12);
    assert_eq!(
        ui.get_settings_scroll_y(),
        0.0,
        "a subpage opens at the top"
    );
    ui.set_settings_scroll_y(-60.0);

    // back: the index is where it was
    nav.back(&ui);
    settle(&window);
    assert_eq!(ui.get_subview(), 0);
    assert_eq!(
        ui.get_settings_scroll_y(),
        -180.0,
        "back restores the index position"
    );

    // reopening the subpage starts fresh, not at its old -60
    ui.invoke_push_subview(12);
    settle(&window);
    assert_eq!(
        ui.get_settings_scroll_y(),
        0.0,
        "a reopened page starts at the top"
    );
}
