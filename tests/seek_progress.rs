use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use slint_interpreter::{Compiler, Value};
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
fn progress_tap_drag_clamping_and_disabled_or_changed_track() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }))
    .unwrap();
    window.set_size(slint::PhysicalSize::new(200, 80));
    let compiler = Compiler::default();
    let result = spin_on::spin_on(
        compiler.build_from_source(
            r#"
        import { ProgressBar } from "ui/nowplaying/progressbar.slint";
        export component SeekFixture inherits Window {
            width: 200px; height: 80px;
            in-out property <bool> seek-enabled: true;
            in-out property <int> duration: 100;
            in-out property <string> track-key: "track";
            callback seek(float, string);
            ProgressBar {
                x: 0; y: 20px; width: 200px; height: 26px;
                progress: 0.1; pos-label: "0:10"; dur-label: "1:40";
                enabled: root.seek-enabled; duration-seconds: root.duration;
                track-key: root.track-key;
                seek(fraction, key) => { root.seek(fraction, key); }
            }
        }
    "#
            .into(),
            "seek-fixture.slint".into(),
        ),
    );
    let definition = result
        .components()
        .next()
        .unwrap_or_else(|| panic!("{:?}", result.diagnostics().collect::<Vec<_>>()));
    let ui = definition.create().unwrap();
    let requests = Rc::new(RefCell::new(Vec::new()));
    ui.set_callback("seek", {
        let requests = requests.clone();
        move |args| {
            if let [Value::Number(fraction), Value::String(key)] = args {
                requests.borrow_mut().push((*fraction, key.to_string()));
            } else {
                panic!("unexpected seek arguments");
            }
            Value::Void
        }
    })
    .unwrap();
    ui.show().unwrap();
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    slint::platform::update_timers_and_animations();
    let mut pixels = vec![slint::Rgb8Pixel::default(); 200 * 80];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 200);
    });
    let before = pixels[22 * 200 + 100];
    let press = |x, y| {
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        })
    };
    let release = |x, y| {
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        })
    };
    press(50.0, 23.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(150.0, 23.0),
    });
    assert!(requests.borrow().is_empty());
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 200);
    });
    assert_ne!(pixels[22 * 200 + 100], before);
    release(150.0, 23.0);
    assert_eq!(&*requests.borrow(), &[(0.75, "track".into())]);

    press(100.0, 12.0);
    release(100.0, 12.0);
    assert_eq!(requests.borrow().last().unwrap().0, 0.5);
    assert_eq!(requests.borrow().len(), 2);
    press(100.0, 23.0);
    release(350.0, 23.0);
    assert_eq!(requests.borrow().last().unwrap().0, 1.0);
    press(100.0, 23.0);
    release(-20.0, 23.0);
    assert_eq!(requests.borrow().last().unwrap().0, 0.0);
    assert_eq!(requests.borrow().len(), 4);

    press(100.0, 23.0);
    ui.set_property("track-key", Value::String("next".into()))
        .unwrap();
    release(150.0, 23.0);
    assert_eq!(requests.borrow().len(), 4);
    slint::platform::update_timers_and_animations();
    ui.set_property("duration", Value::Number(0.0)).unwrap();
    press(100.0, 23.0);
    release(100.0, 23.0);
    assert_eq!(requests.borrow().len(), 4);
    ui.set_property("duration", Value::Number(100.0)).unwrap();
    ui.set_property("seek-enabled", Value::Bool(false)).unwrap();
    press(100.0, 23.0);
    release(100.0, 23.0);
    assert_eq!(requests.borrow().len(), 4);
}
