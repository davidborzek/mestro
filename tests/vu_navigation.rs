use mestro::{ui::navigation::Navigation, Mestro};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

struct Headless {
    clock: Rc<Cell<Duration>>,
    window: Rc<MinimalSoftwareWindow>,
}

struct InstanceGuard(Rc<Cell<usize>>);
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.clock.get()
    }
}

#[test]
fn vu_visibility_syncs_before_navigation_returns_and_only_on_transitions() {
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(573, 344));
    slint::platform::set_platform(Box::new(Headless {
        clock: clock.clone(),
        window: window.clone(),
    }))
    .unwrap();
    let ui = Mestro::new().unwrap();
    let nav = Navigation::new();
    let transitions = Rc::new(RefCell::new(Vec::new()));
    ui.on_vu_sync({
        let weak = ui.as_weak();
        let transitions = transitions.clone();
        move || {
            transitions
                .borrow_mut()
                .push(weak.upgrade().unwrap().get_vu_open())
        }
    });
    nav.show_vu(&ui);
    assert_eq!(&*transitions.borrow(), &[true]);
    nav.show_vu(&ui);
    assert_eq!(&*transitions.borrow(), &[true]);
    nav.show_queue(&ui);
    assert_eq!(&*transitions.borrow(), &[true, false]);
    nav.show_vu(&ui);
    ui.set_vu_overview_open(true);
    nav.back(&ui);
    assert!(ui.get_vu_open());
    assert!(!ui.get_vu_overview_open());
    assert_eq!(&*transitions.borrow(), &[true, false, true]);
    nav.back(&ui);
    assert_eq!(&*transitions.borrow(), &[true, false, true, false]);
    nav.show_np(&ui);
    assert_eq!(&*transitions.borrow(), &[true, false, true, false]);

    ui.invoke_vu_present(0);
    assert!(!ui.get_vu_transitioning());
    ui.invoke_vu_present(1);
    assert!(ui.get_vu_transitioning());
    slint::platform::update_timers_and_animations();
    clock.set(Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
    assert!(ui.get_vu_transitioning());
    clock.set(Duration::from_millis(300));
    slint::platform::update_timers_and_animations();
    assert!(!ui.get_vu_transitioning());
    ui.invoke_vu_present(-1);
    assert!(ui.get_vu_transitioning());
    ui.invoke_vu_clear();
    assert!(!ui.get_vu_transitioning());

    // a skin is a Rectangle, not a Window: the plain Compiler skips
    // non-Window exports, skins compile like the app compiles them
    let definition = mestro::vu::skin::compile_source(
        "test-vu",
        "export component TestSkin inherits Rectangle { callback ping(); background: #ff0000; }",
    )
    .expect("test skin compiles")
    .definition;
    let live = Rc::new(Cell::new(0));
    let factory = slint::ComponentFactory::new({
        let live = live.clone();
        move |ctx| {
            let instance = definition.create_embedded(ctx).unwrap();
            live.set(live.get() + 1);
            let guard = InstanceGuard(live.clone());
            instance
                .set_callback("ping", move |_| {
                    let _keep_alive = &guard;
                    slint_interpreter::Value::Void
                })
                .unwrap();
            Some(instance)
        }
    });
    ui.set_powered(true);
    ui.set_vu_open(true);
    ui.set_vu_skin_factory(factory.clone());
    ui.invoke_vu_present(0);
    ui.show().unwrap();
    let mut pixels = vec![slint::Rgb8Pixel::default(); 573 * 344];
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 573);
    });
    assert_eq!(live.get(), 1);
    ui.set_vu_skin_factory(factory);
    ui.invoke_vu_present(1);
    slint::platform::update_timers_and_animations();
    clock.set(Duration::from_millis(320));
    slint::platform::update_timers_and_animations();
    clock.set(Duration::from_millis(360));
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 573);
    });
    assert_eq!(live.get(), 2);
    clock.set(Duration::from_millis(650));
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 573);
    });
    assert_eq!(live.get(), 1);
    ui.set_vu_open(false);
    ui.invoke_vu_clear();
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 573);
    });
    assert_eq!(live.get(), 0);
}
