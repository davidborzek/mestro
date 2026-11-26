//! Render-once previews for the VU skin overview.
//!
//! The main window keeps the linuxkms/winit renderer; preview
//! instances render against a private MinimalSoftwareWindow and
//! become plain Slint images. The overview reuses these images
//! instead of keeping multiple live skins attached.

use super::skin::Skin;
use crate::config::MetadataField;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint_interpreter::{ComponentInstance, Value};

/// Preview resolution: high enough for the three-column grid,
/// small enough to render every skin once on a Pi.
pub const WIDTH: u32 = 286;
pub const HEIGHT: u32 = 172;

/// deterministic pose for all previews: the same track and
/// meter position make skins visually comparable
pub const LEVEL_L: f64 = 0.72;
pub const LEVEL_R: f64 = 0.48;

pub fn render(skin: &Skin, info: &[MetadataField]) -> slint::Image {
    render_with_options(skin, info, None, None, None)
}

pub fn render_with_options(
    skin: &Skin,
    info: &[MetadataField],
    glow: Option<f64>,
    meters: Option<bool>,
    gradient: Option<bool>,
) -> slint::Image {
    render_buffer(skin, info, glow, meters, gradient)
        .map_or_else(slint::Image::default, slint::Image::from_rgb8)
}

/// one preview as PNG bytes (the web settings thumbnails)
pub fn render_png(
    skin: &Skin,
    info: &[MetadataField],
    glow: Option<f64>,
    meters: Option<bool>,
    gradient: Option<bool>,
) -> Option<Vec<u8>> {
    let buffer = render_buffer(skin, info, glow, meters, gradient)?;
    let image = image::RgbImage::from_raw(WIDTH, HEIGHT, buffer.as_bytes().to_vec())?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .ok()?;
    Some(bytes.into_inner())
}

fn render_buffer(
    skin: &Skin,
    info: &[MetadataField],
    glow: Option<f64>,
    meters: Option<bool>,
    gradient: Option<bool>,
) -> Option<slint::SharedPixelBuffer<slint::Rgb8Pixel>> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    let instance = match skin.definition.create_with_existing_window(&window) {
        Ok(instance) => instance,
        Err(error) => {
            tracing::warn!("vu preview {}: {error}", skin.name);
            return None;
        }
    };
    apply_pose(&instance, info);
    // the preview setup pre-rotates manifest needles (the
    // software renderer cannot rotate); skins without one take
    // the live setup
    let setup = skin.preview_setup.as_ref().or(skin.setup.as_ref());
    if let Some(setup) = setup {
        setup(&instance);
    }
    // baked skins carry the glow in their art (femtovg-safe);
    // the glow property covers everything else (modern's tinted
    // lamp)
    if let Some(bake) = &skin.bake {
        bake.apply(&instance, glow.unwrap_or_else(|| bake.default_glow()), true);
    }
    if let Some(value) = glow {
        let _ = instance.set_property("glow", Value::Number(value));
    }
    if let Some(value) = meters {
        let _ = instance.set_property("meters", Value::Bool(value));
    }
    if let Some(value) = gradient {
        let _ = instance.set_property("gradient", Value::Bool(value));
    }
    window.request_redraw();
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(WIDTH, HEIGHT);
    window.draw_if_needed(|renderer| {
        renderer.render(pixels.make_mut_slice(), WIDTH as usize);
    });
    Some(pixels)
}

fn apply_pose(instance: &ComponentInstance, info: &[MetadataField]) {
    let set = |name: &str, value: Value| {
        if let Err(error) = instance.set_property(name, value) {
            tracing::debug!("vu preview: {name}: {error}");
        }
    };
    set("level-l", Value::Number(LEVEL_L));
    set("level-r", Value::Number(LEVEL_R));
    set("title", Value::String("Blue in Green".into()));
    set("artist", Value::String("Miles Davis".into()));
    set("codec", Value::String("FLAC".into()));
    set("quality", Value::String("24/96".into()));
    set("lossless", Value::Bool(true));
    set("has-media", Value::Bool(true));
    set("show-info", Value::Bool(true));
    let rows: Vec<String> = info
        .iter()
        .map(|f| match f {
            MetadataField::Title => "Blue in Green".to_string(),
            MetadataField::Artist => "Miles Davis".to_string(),
            MetadataField::Codec => "FLAC".to_string(),
            MetadataField::Quality => "24/96".to_string(),
            MetadataField::Time => "03:42".to_string(),
        })
        .collect();
    set("info-lines", string_model(rows));
    set("info-count", Value::Number(info.len() as f64));
    set("bands", number_model(preview_bands()));
    set("peaks", number_model(preview_peaks()));
}

fn string_model(values: impl IntoIterator<Item = String>) -> Value {
    Value::Model(slint::ModelRc::new(slint::VecModel::from(
        values
            .into_iter()
            .map(|value| Value::String(value.into()))
            .collect::<Vec<_>>(),
    )))
}

fn number_model(values: [f32; super::BANDS]) -> Value {
    Value::Model(slint::ModelRc::new(slint::VecModel::from(
        values
            .into_iter()
            .map(|v| Value::Number(v as f64))
            .collect::<Vec<_>>(),
    )))
}

fn preview_bands() -> [f32; super::BANDS] {
    let mut bands = [0.0; super::BANDS];
    for (i, band) in bands.iter_mut().enumerate() {
        let x = i as f32 / (super::BANDS - 1) as f32;
        *band = 0.2 + 0.65 * (x * std::f32::consts::PI * 2.2).sin().abs();
    }
    bands
}

fn preview_peaks() -> [f32; super::BANDS] {
    let mut peaks = preview_bands();
    for peak in &mut peaks {
        *peak = (*peak + 0.08).min(1.0);
    }
    peaks
}
#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::platform::{Platform, WindowAdapter};

    /// Headless platform: create_with_existing_window still
    /// consults the global platform, which defaults to winit and
    /// needs a display (CI is headless). Installed once per
    /// process; the no-op window is never rendered into.
    struct Headless {
        window: std::rc::Rc<MinimalSoftwareWindow>,
    }

    impl Platform for Headless {
        fn create_window_adapter(
            &self,
        ) -> Result<std::rc::Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.window.clone())
        }
        fn duration_since_start(&self) -> std::time::Duration {
            std::time::Duration::from_millis(0)
        }
    }

    /// a synthetic pointer skin: a white plate with a plain black
    /// needle — exact geometry, no other art to interfere with
    /// the pixel samples
    fn pointer_skin(dir: &std::path::Path) -> crate::vu::skin::Skin {
        std::fs::write(
            dir.join("bg.svg"),
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" fill="#ffffff"/></svg>"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("needle.svg"),
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect x="48" y="10" width="4" height="40" fill="#000000"/></svg>"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("manifest.toml"),
            r##"
glow = 0

[faceplate]
design-w = 286
design-h = 172
plate = "#ffffff"

[[dial]]
side = "left"
x = 0
art-w = 100
art-h = 100
pivot-x = 50
pivot-y = 50
zero-angle = -45
sweep = 90

[[dial.layer]]
role = "face"
source = "bg.svg"

[[dial.layer]]
role = "needle"
source = "needle.svg"
"##,
        )
        .unwrap();
        crate::vu::manifest::load("pointer", &dir.join("manifest.toml")).expect("the skin compiles")
    }

    fn luma(buffer: &slint::SharedPixelBuffer<slint::Rgb8Pixel>, x: u32, y: u32) -> u32 {
        let at = ((y * WIDTH + x) * 3) as usize;
        let px = &buffer.as_bytes()[at..at + 3];
        (px[0] as u32 * 299 + px[1] as u32 * 587 + px[2] as u32 * 114) / 1000
    }

    /// the needle must sit at the preview pose: the design matches
    /// the preview size (fit 1), the dial box centers vertically
    /// (origin y = 36), and LEVEL_L = 0.72 puts the left needle at
    /// -45° + 0.72 × 90° = 19.8° clockwise from up
    #[test]
    fn the_preview_needle_renders_at_the_rotated_pose() {
        let _ = slint::platform::set_platform(Box::new(Headless {
            window: MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer),
        }));
        let dir = std::env::temp_dir().join(format!("mestro-preview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let skin = pointer_skin(&dir);
        let buffer = render_buffer(&skin, &[], None, None, None).expect("the preview renders");
        // the stem midpoint (20 units above the pivot) rotated by
        // 19.8°: (50 + 20·sin, 50 − 20·cos) ≈ (56.8, 31.2) + 36
        assert!(
            luma(&buffer, 56, 67) < 60,
            "the rotated stem midpoint must be black"
        );
        // 35 units up the stem, still on the rotated line
        assert!(
            luma(&buffer, 61, 53) < 60,
            "the rotated stem must reach toward the tip"
        );
        // the UNrotated stem position: without prerotation the
        // needle renders upright exactly here
        assert!(
            luma(&buffer, 50, 66) > 200,
            "the unrotated stem position must be empty"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
