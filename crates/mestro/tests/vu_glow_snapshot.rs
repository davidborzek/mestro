//! Offscreen snapshots of every glow skin for visual review —
//! the wash shape (the manifest's `radius`) must stay right
//! across engine changes:
//!
//!   MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test vu_glow_snapshot -- --ignored

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mestro::vu::{manifest, preview};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use std::path::PathBuf;
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
#[ignore = "visual review: MESTRO_SNAPSHOT_DIR=… cargo test --test vu_glow_snapshot -- --ignored"]
fn glow_skin_snapshots() {
    let dir = PathBuf::from(std::env::var("MESTRO_SNAPSHOT_DIR").expect("set MESTRO_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    slint::platform::set_platform(Box::new(Headless {
        window: MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer),
    }))
    .unwrap();
    let skins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skins");
    for name in ["cream", "round", "wall-light", "arc", "orbit", "modern"] {
        let skin = manifest::load(name, &skins.join(name).join("manifest.toml"))
            .unwrap_or_else(|| panic!("the {name} skin compiles"));
        // wall-light is the tunable plate: the wash states a
        // reviewer compares; the others show their one lit look
        let glows: &[f64] = if name == "wall-light" {
            &[0.0, 0.5, 1.0, 1.5]
        } else {
            &[1.0]
        };
        for glow in glows {
            let png = preview::render_png(&skin, &[], Some(*glow), None, None)
                .unwrap_or_else(|| panic!("the {name} preview renders"));
            std::fs::write(dir.join(format!("{name}-glow{glow:.1}.png")), png).unwrap();
        }
    }
}
