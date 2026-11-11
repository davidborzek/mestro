//! Waking from standby reveals the page the user left — standby
//! is an overlay over the page stack, not a navigation target.

use mestro::standby::StandbyWake;
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

fn setup() -> Mestro {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(573, 344));
    // tests share one thread per binary only when run serially; the
    // platform can be set once per process
    let _ = slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }));
    Mestro::new().unwrap()
}

#[test]
fn waking_from_standby_keeps_the_page() {
    let ui = setup();
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

/// playback started elsewhere (MA web UI, Home Assistant) powers the
/// player on: with auto-open, standby hands over to now playing
#[test]
fn remote_power_on_opens_now_playing() {
    let ui = setup();
    let nav = Navigation::new();
    let wake = StandbyWake::new(true);

    nav.go(&ui, Page::Library);
    ui.set_powered(false);
    wake.observe(&ui, &nav);

    ui.set_powered(true);
    wake.observe(&ui, &nav);

    assert!(ui.get_np_open(), "now playing opens on a remote start");
}

#[test]
fn remote_power_on_without_auto_open_keeps_the_page() {
    let ui = setup();
    let nav = Navigation::new();
    let wake = StandbyWake::new(false);

    nav.go(&ui, Page::Library);
    ui.set_powered(false);
    wake.observe(&ui, &nav);
    ui.set_powered(true);
    wake.observe(&ui, &nav);

    assert!(!ui.get_np_open());
    assert_eq!(ui.get_page(), Page::Library);
}

/// a tap on the standby clock is the user's own wake: back to the page
#[test]
fn local_wake_keeps_the_page_even_with_auto_open() {
    let ui = setup();
    let nav = Navigation::new();
    let wake = StandbyWake::new(true);
    ui.on_set_power(|_| {});

    nav.go(&ui, Page::Library);
    ui.set_powered(false);
    wake.observe(&ui, &nav);

    wake.local_wake();
    ui.invoke_standby_power_on();
    ui.set_powered(true);
    wake.observe(&ui, &nav);

    assert!(!ui.get_np_open());
    assert_eq!(ui.get_page(), Page::Library);
}

/// a power-on that is not a standby exit (already on) changes nothing
#[test]
fn updates_while_powered_do_not_open_now_playing() {
    let ui = setup();
    let nav = Navigation::new();
    let wake = StandbyWake::new(true);

    nav.go(&ui, Page::Library);
    ui.set_powered(true);
    wake.observe(&ui, &nav);
    wake.observe(&ui, &nav);

    assert!(!ui.get_np_open());
}
