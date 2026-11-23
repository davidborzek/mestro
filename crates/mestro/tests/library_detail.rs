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
fn detail_context_menu_opens_for_albums_and_playlists() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }))
    .unwrap();
    window.set_size(slint::PhysicalSize::new(400, 300));
    let compiler = Compiler::default();
    let result = spin_on::spin_on(
        compiler.build_from_source(
            r#"
        import { LibraryPage } from "ui/library/library.slint";
        import { LibEntry } from "ui/library/entry.slint";
        export component LibraryFixture inherits Window {
            width: 400px; height: 300px;
            in-out property <string> mode: "album";
            in-out property <LibEntry> subject;
            in-out property <bool> menu-open <=> library.menu-open;
            callback menu-requested(string);
            library := LibraryPage {
                width: 100%; height: 100%; mode: root.mode; subject: root.subject;
                menu(entry) => { root.menu-requested(entry.uri); return []; }
            }
        }
    "#
            .into(),
            "library-fixture.slint".into(),
        ),
    );
    let definition = result
        .components()
        .next()
        .unwrap_or_else(|| panic!("{:?}", result.diagnostics().collect::<Vec<_>>()));
    let ui = definition.create().unwrap();
    let menus = Rc::new(RefCell::new(Vec::new()));
    ui.set_callback("menu-requested", {
        let menus = menus.clone();
        move |args| {
            if let [Value::String(uri)] = args {
                menus.borrow_mut().push(uri.to_string());
            }
            Value::Void
        }
    })
    .unwrap();
    ui.show().unwrap();
    let mut pixels = vec![slint::Rgb8Pixel::default(); 400 * 300];
    let tap = |x| {
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(x, 42.0),
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(x, 42.0),
            button: PointerEventButton::Left,
        });
    };
    for kind in ["album", "playlist"] {
        let Value::Struct(mut subject) = ui.get_property("subject").unwrap() else {
            panic!("subject struct missing")
        };
        let uri = format!("library://{kind}/1");
        subject.set_field("kind".into(), Value::String((*kind).into()));
        subject.set_field("name".into(), Value::String("Test".into()));
        subject.set_field("uri".into(), Value::String(uri.clone().into()));
        ui.set_property("subject", Value::Struct(subject)).unwrap();
        ui.set_property("mode", Value::String((*kind).into()))
            .unwrap();
        ui.set_property("menu-open", Value::Bool(false)).unwrap();
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 400);
        });
        tap(306.0);
        assert_eq!(ui.get_property("menu-open").unwrap(), Value::Bool(true));
        assert_eq!(menus.borrow().last().unwrap(), &uri);
    }
    ui.set_property("menu-open", Value::Bool(false)).unwrap();
    ui.set_property("mode", Value::String("artist".into()))
        .unwrap();
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 400);
    });
    tap(306.0);
    assert_eq!(menus.borrow().len(), 2);
    assert_eq!(ui.get_property("menu-open").unwrap(), Value::Bool(false));
}
