//! The nav rail auto-hides on the VU faceplate ([vu]
//! rail-autohide, seconds, 0 = always shown). Rust owns the
//! timer on the slint clock — the fake platform clock drives it
//! headless, like the OSD auto-hide test. While the rail is
//! hidden the first tap only reveals it (it must not cycle the
//! view); swipes keep cycling. Leaving the VU shows the rail
//! again.

use mestro::config::Config;
use mestro::{ui::navigation::Navigation, Mestro};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

/// the kiosk panel: 800x480 at scale 1 → logical == physical
const W: u32 = 800;
const H: u32 = 480;

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

fn advance(clock: &Rc<Cell<Duration>>, ms: u64) {
    clock.set(clock.get() + Duration::from_millis(ms));
    slint::platform::update_timers_and_animations();
}

/// lay the window out and paint once: hit-testing (and the
/// software renderer) work off the rendered item tree
fn draw(window: &MinimalSoftwareWindow, pixels: &mut [slint::Rgb8Pixel], w: u32) {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.render(pixels, w as usize);
    });
}

#[test]
fn the_rail_hides_on_its_timer_and_a_tap_only_reveals_it() {
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W, H));
    slint::platform::set_platform(Box::new(Headless {
        clock: clock.clone(),
        window: window.clone(),
    }))
    .unwrap();
    let ui = Mestro::new().unwrap();
    let mut config = Config::default();
    config.vu.cycle = vec!["night".into(), "cream".into()];
    let nav = Navigation::new();
    let palette = Rc::new(RefCell::new(None));
    let vui = mestro::vu::wire::Vui::new(&config, &nav, palette);
    vui.wire(&ui, ui.as_weak());
    ui.set_powered(true);
    ui.show().unwrap();
    let mut pixels = vec![slint::Rgb8Pixel::default(); (W * H) as usize];

    let tap = |x: f32, y: f32| {
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        });
    };

    // entering the VU starts the timer: the rail shows, hides no
    // earlier than the setting says
    ui.set_vu_rail_autohide(5.0);
    nav.show_vu(&ui);
    draw(&window, &mut pixels, W);
    assert!(!ui.get_vu_rail_hidden(), "the rail starts visible");
    advance(&clock, 4_000);
    assert!(!ui.get_vu_rail_hidden(), "the rail hides too early");
    advance(&clock, 1_100);
    assert!(ui.get_vu_rail_hidden(), "5 s without touch tucks the rail");
    // let the slide-out finish: until then the fading rail still
    // owns the right edge's taps
    advance(&clock, 400);
    draw(&window, &mut pixels, W);

    // a tap on the (invisible) edge strip only reveals — the view
    // must not cycle. The strip rides the zone's right edge, now
    // the full width
    tap(772.0, 240.0);
    assert!(!ui.get_vu_rail_hidden(), "a tap must reveal the rail");
    assert_eq!(
        ui.get_vu_view_index(),
        0,
        "the revealing tap must not cycle"
    );
    // the tap restarted the timer
    advance(&clock, 4_000);
    assert!(!ui.get_vu_rail_hidden());
    advance(&clock, 1_100);
    assert!(ui.get_vu_rail_hidden());

    // a swipe only switches the faceplate: it keeps cycling while
    // the rail is hidden, and the rail stays tucked
    draw(&window, &mut pixels, W);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: slint::LogicalPosition::new(600.0, 240.0),
        button: PointerEventButton::Left,
    });
    assert!(
        ui.get_vu_rail_hidden(),
        "the touch-down of a swipe must not flash the rail"
    );
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(480.0, 240.0),
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: slint::LogicalPosition::new(480.0, 240.0),
        button: PointerEventButton::Left,
    });
    assert!(ui.get_vu_rail_hidden(), "a swipe must not reveal the rail");
    assert_eq!(ui.get_vu_view_index(), 1, "swipes keep cycling");
    // the next plain tap reveals the rail again
    advance(&clock, 400);
    advance(&clock, 400);
    draw(&window, &mut pixels, W);
    tap(100.0, 240.0);
    assert!(!ui.get_vu_rail_hidden(), "a tap after the swipe reveals");
    assert_eq!(
        ui.get_vu_view_index(),
        1,
        "the revealing tap must not cycle"
    );
    // the view slide (staging 16ms + slide 220ms + cleanup 240ms)
    // must finish before the next tap — a transitioning page
    // swallows cycles by design. Each update pass fires at most
    // one link of the timer chain, so: two advances
    advance(&clock, 400);
    advance(&clock, 400);
    draw(&window, &mut pixels, W);

    // while the rail is visible the strip behaves as today: a tap
    // cycles (the rail occupies the right 56px again)
    tap(716.0, 240.0);
    assert_eq!(ui.get_vu_view_index(), 0, "the visible strip still cycles");
    assert!(!ui.get_vu_rail_hidden());

    // a plain plate tap never cycles — it only reveals
    advance(&clock, 6_100);
    assert!(ui.get_vu_rail_hidden());
    draw(&window, &mut pixels, W);
    tap(100.0, 240.0);
    assert!(!ui.get_vu_rail_hidden());
    assert_eq!(ui.get_vu_view_index(), 0);

    // the skin selection (overview) is a menu, not the faceplate:
    // opening it shows the rail and keeps it shown however long
    // it stays open; closing it starts a fresh timer
    advance(&clock, 6_100);
    assert!(ui.get_vu_rail_hidden());
    ui.invoke_vu_open_overview();
    assert!(
        !ui.get_vu_rail_hidden(),
        "opening the selection must show the rail"
    );
    advance(&clock, 10_000);
    assert!(
        !ui.get_vu_rail_hidden(),
        "the rail must stay while the selection is open"
    );
    ui.invoke_vu_select_preview(0);
    assert!(!ui.get_vu_overview_open());
    // the 33 ms tick notices the close (on the device within a
    // frame; here the fake clock needs a nudge)
    ui.invoke_vu_sync();
    advance(&clock, 4_000);
    assert!(
        !ui.get_vu_rail_hidden(),
        "closing the selection restarts the timer"
    );
    advance(&clock, 1_100);
    assert!(ui.get_vu_rail_hidden());
    // a pending hide from before the selection must not fire
    // inside it either
    ui.invoke_vu_activity();
    advance(&clock, 3_000);
    ui.invoke_vu_open_overview();
    advance(&clock, 3_000);
    assert!(
        !ui.get_vu_rail_hidden(),
        "an older hide timer must not fire inside the selection"
    );
    ui.set_vu_overview_open(false);
    ui.invoke_vu_sync();

    // 0 = always shown
    ui.set_vu_rail_autohide(0.0);
    ui.invoke_vu_activity();
    advance(&clock, 10_000);
    assert!(!ui.get_vu_rail_hidden(), "0 must pin the rail visible");

    // leaving the VU always shows the rail again
    ui.set_vu_rail_autohide(5.0);
    ui.invoke_vu_activity();
    advance(&clock, 6_000);
    assert!(ui.get_vu_rail_hidden());
    nav.show_np(&ui);
    assert!(
        !ui.get_vu_rail_hidden(),
        "leaving the VU must show the rail"
    );
}
