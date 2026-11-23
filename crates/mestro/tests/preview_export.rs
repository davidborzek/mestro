//! exports the preview thumbnails the web dev mock serves
//! (web/src/lib/mock-previews/*.png). The mock has no device
//! behind it, so its schema URLs are rewritten to these renders;
//! regenerate after touching the preview path or the skins:
//!
//!   cargo test --test preview_export -- --ignored
//!
//! dev-only artifacts: nothing in web/ imports them outside the
//! mock, so the production bundle never contains them.

use mestro::config::Config;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use std::collections::BTreeMap;
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
#[ignore = "repo export: cargo test --test preview_export -- --ignored"]
fn exports_the_mock_preview_pngs() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/src/lib/mock-previews");
    std::fs::create_dir_all(&dir).unwrap();

    // slint 1.19 requires a platform context before any component
    // instance is created — each preview renders onto its own private
    // software window (src/vu/preview.rs), this one only satisfies the
    // context
    slint::platform::set_platform(Box::new(Headless {
        window: MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer),
    }))
    .unwrap();

    // the VU skins: every option the cycle picker offers
    let nav = mestro::ui::navigation::Navigation::new();
    let palette: Rc<std::cell::RefCell<Option<mestro::ui::palette::ArtworkPalette>>> =
        Rc::new(std::cell::RefCell::new(None));
    let vui = mestro::vu::wire::Vui::new(&Config::default(), &nav, palette);
    let config = Config::default();
    for name in mestro::config::VU_BUILTIN_SKINS {
        let Some(png) = vui.preview_png(name, &config) else {
            panic!("the {name} preview must render");
        };
        std::fs::write(dir.join(format!("{name}.png")), png).unwrap();
    }

    // the clock styles (the built-ins; [clock.skins] live behind
    // files the test cannot know)
    let skins = mestro::standby::Skins::compile(&BTreeMap::new());
    let accent = slint::Color::from_rgb_u8(0xd8, 0xa6, 0x57);
    for name in ["digital", "analog"] {
        let Some(png) = skins.preview_png(name, accent) else {
            panic!("the {name} clock preview must render");
        };
        std::fs::write(dir.join(format!("{name}.png")), png).unwrap();
    }
}
