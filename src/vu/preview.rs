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
const LEVEL_L: f64 = 0.72;
const LEVEL_R: f64 = 0.48;

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
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    match skin.definition.create_with_existing_window(&window) {
        Ok(instance) => {
            apply_pose(&instance, info);
            if let Some(setup) = &skin.setup {
                setup(&instance);
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
            slint::Image::from_rgb8(pixels)
        }
        Err(error) => {
            tracing::warn!("vu preview {}: {error}", skin.name);
            slint::Image::default()
        }
    }
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
