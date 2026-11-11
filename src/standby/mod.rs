//! runtime standby clock skins
//!
//! A skin is a .slint file implementing the StandbySkin contract
//! (docs/standby.md): in-properties hour/minute (int), time-label/
//! date-label (string), accent (color) and a power-on() callback.
//! It renders fullscreen inside the standby view via a
//! ComponentContainer; mestro owns the black ground, the tap-to-
//! wake fallback and the burn-in jitter. Skins compile once at
//! startup; a broken skin logs and the digital face takes over.

mod wake;
pub use wake::StandbyWake;

use crate::Mestro;
use slint::{ComponentFactory, ComponentHandle};
use slint_interpreter::{Compiler, ComponentDefinition, ComponentInstance, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// the built-in faces as standalone sources — only the settings
/// previews compile these; standby itself renders the compiled-in
/// components
const BUILTIN_SOURCES: &[(&str, &str)] = &[
    ("digital", include_str!("../../ui/standby/digital.slint")),
    ("analog", include_str!("../../ui/standby/analog.slint")),
];

/// Preview size: the picker grid shows two columns of these
pub const PREVIEW_WIDTH: u32 = 240;
pub const PREVIEW_HEIGHT: u32 = 144;

/// compiled skins by name ([clock.skins])
pub struct Skins {
    compiled: BTreeMap<String, ComponentDefinition>,
    /// every face wrapped for previews (black Window ground)
    preview: BTreeMap<String, ComponentDefinition>,
    /// the live instance, for the per-minute pushes
    live: Rc<RefCell<Option<slint::Weak<ComponentInstance>>>>,
}

/// the values every skin receives
#[derive(Clone, Default)]
pub struct Tick {
    pub hour: i32,
    pub minute: i32,
    pub time_label: String,
    pub date_label: String,
}

impl Skins {
    /// compile every registered skin; failures log and are skipped
    pub fn compile(skins: &BTreeMap<String, PathBuf>) -> Self {
        // (name, source, path) of every face: built-ins (previews
        // only), then registered skins
        let mut sources: Vec<(String, String, PathBuf)> = BUILTIN_SOURCES
            .iter()
            .map(|(name, src)| {
                (
                    name.to_string(),
                    src.to_string(),
                    PathBuf::from(format!("ui/standby/{name}.slint")),
                )
            })
            .collect();
        for (name, path) in skins {
            match std::fs::read_to_string(path) {
                Ok(src) => sources.push((name.clone(), src, path.clone())),
                Err(_) => tracing::warn!("standby: cannot read skin {name} at {}", path.display()),
            }
        }

        let mut compiled = BTreeMap::new();
        let mut preview = BTreeMap::new();
        for (name, source, path) in sources {
            let builtin = BUILTIN_SOURCES.iter().any(|(n, _)| *n == name);
            let Some(definition) = compile_source(&name, source.clone(), path.clone()) else {
                continue;
            };
            if let Some(wrapped) = compile_preview(&name, &source, &path, &definition) {
                preview.insert(name.clone(), wrapped);
            }
            // built-ins render compiled-in; only skins embed at runtime
            if !builtin {
                compiled.insert(name, definition);
            }
        }
        Self {
            compiled,
            preview,
            live: Rc::new(RefCell::new(None)),
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.compiled.contains_key(name)
    }

    /// embed `name` (or clear with None — the built-in faces
    /// render again). Returns whether a skin is active.
    pub fn activate(&self, ui: &Mestro, name: Option<&str>, tick: &Tick) -> bool {
        let Some(definition) = name.and_then(|n| self.compiled.get(n)).cloned() else {
            *self.live.borrow_mut() = None;
            ui.set_standby_skin_factory(ComponentFactory::default());
            ui.set_standby_skin_active(false);
            return false;
        };
        let live = self.live.clone();
        let ui_weak = ui.as_weak();
        let first = tick.clone();
        let factory = ComponentFactory::new(move |ctx| {
            let instance = match definition.create_embedded(ctx) {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!("standby: skin failed to embed: {e}");
                    return None;
                }
            };
            let ui_weak = ui_weak.clone();
            instance
                .set_callback("power-on", move |_| {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.invoke_standby_power_on();
                    }
                    Value::Void
                })
                .ok();
            push_to(&instance, &first);
            *live.borrow_mut() = Some(instance.as_weak());
            Some(instance)
        });
        ui.set_standby_skin_factory(factory);
        ui.set_standby_skin_active(true);
        true
    }

    /// minute tick + accent: cheap no-op without a live skin
    pub fn push(&self, tick: &Tick) {
        if let Some(instance) = self.live.borrow().as_ref().and_then(|w| w.upgrade()) {
            push_to(&instance, tick);
        }
    }

    /// render-once previews for the style picker, in `styles`
    /// order: a fixed 10:10 pose (the watch-ad classic: hands
    /// apart, symmetric) on a black ground
    pub fn previews(&self, styles: &[String], accent: slint::Color) -> Vec<(String, slint::Image)> {
        let pose = Tick {
            hour: 10,
            minute: 10,
            time_label: "10:10".into(),
            date_label: "MON 09 NOV".into(),
        };
        styles
            .iter()
            .filter_map(|name| {
                let definition = self.preview.get(name)?;
                Some((name.clone(), render_preview(definition, &pose, accent)))
            })
            .collect()
    }

    pub fn push_accent(&self, accent: slint::Color) {
        if let Some(instance) = self.live.borrow().as_ref().and_then(|w| w.upgrade()) {
            set(
                &instance,
                "accent",
                Value::Brush(slint::Brush::SolidColor(accent)),
            );
        }
    }
}

fn compile_source(name: &str, source: String, path: PathBuf) -> Option<ComponentDefinition> {
    let mut compiler = Compiler::new();
    crate::vu::select_last_exported(&mut compiler);
    let result = spin_on::spin_on(compiler.build_from_source(source, path));
    for d in result.diagnostics() {
        tracing::warn!("standby skin {name}: {d}");
    }
    let component = result.components().next();
    if component.is_none() {
        tracing::warn!("standby: skin {name} failed to compile, using digital");
    }
    component
}

/// The face wrapped in a Window: the software renderer fills only
/// a Window's background, and a standalone face gets the palette's
/// white one. The wrapper forwards the contract into the face —
/// only the properties the face declares (binding a missing one
/// would not compile). The built-in faces use their own names
/// (clock-hour, date, …), so those map too.
fn compile_preview(
    name: &str,
    source: &str,
    path: &Path,
    face: &ComponentDefinition,
) -> Option<ComponentDefinition> {
    const FORWARD: &[(&str, &str)] = &[
        ("hour", "root.hour"),
        ("minute", "root.minute"),
        ("clock-hour", "root.hour"),
        ("clock-minute", "root.minute"),
        ("time-label", "root.time-label"),
        ("date-label", "root.date-label"),
        ("date", "root.date-label"),
        ("accent", "root.accent"),
    ];
    let declared: Vec<String> = face.properties().map(|(prop, _)| prop).collect();
    let bindings: String = FORWARD
        .iter()
        .filter(|(prop, _)| declared.iter().any(|d| d == prop))
        .map(|(prop, expr)| format!("        {prop}: {expr};\n"))
        .collect();
    let wrapper = format!(
        r#"{source}

export component MestroClockPreview inherits Window {{
    background: #000;
    in property <int> hour;
    in property <int> minute;
    in property <string> time-label;
    in property <string> date-label;
    in property <color> accent: #d8a657;
    {face_name} {{
        width: parent.width;
        height: parent.height;
{bindings}    }}
}}
"#,
        face_name = face.name(),
    );
    let mut compiler = Compiler::new();
    crate::vu::select_last_exported(&mut compiler);
    let result = spin_on::spin_on(compiler.build_from_source(wrapper, path.to_path_buf()));
    let component = result.components().next();
    if component.is_none() {
        for d in result.diagnostics() {
            tracing::warn!("standby preview {name}: {d}");
        }
    }
    component
}

/// render one face offscreen. Standby renders at full window size
/// (~800x480); the preview renders at that size too and scales
/// down, so faces with fixed lengths keep their proportions
fn render_preview(
    definition: &ComponentDefinition,
    pose: &Tick,
    accent: slint::Color,
) -> slint::Image {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    const RENDER_W: u32 = PREVIEW_WIDTH * 10 / 3;
    const RENDER_H: u32 = PREVIEW_HEIGHT * 10 / 3;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(RENDER_W, RENDER_H));
    let instance = match definition.create_with_existing_window(&window) {
        Ok(instance) => instance,
        Err(error) => {
            tracing::warn!("standby preview: {error}");
            return slint::Image::default();
        }
    };
    push_to(&instance, pose);
    set(
        &instance,
        "accent",
        Value::Brush(slint::Brush::SolidColor(accent)),
    );
    window.request_redraw();
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(RENDER_W, RENDER_H);
    window.draw_if_needed(|renderer| {
        renderer.render(pixels.make_mut_slice(), RENDER_W as usize);
    });
    slint::Image::from_rgb8(pixels)
}

fn push_to(instance: &ComponentInstance, tick: &Tick) {
    set(instance, "hour", Value::Number(tick.hour as f64));
    set(instance, "minute", Value::Number(tick.minute as f64));
    set(
        instance,
        "time-label",
        Value::String(tick.time_label.as_str().into()),
    );
    set(
        instance,
        "date-label",
        Value::String(tick.date_label.as_str().into()),
    );
}

/// skins declare only what they use — a missing property is fine
fn set(instance: &ComponentInstance, name: &str, value: Value) {
    let _ = instance.set_property(name, value);
}

// ── standby backgrounds: the folder slideshow ──

/// image files in `dir` (jpg/jpeg/png, case-insensitive), sorted
/// for a stable base order; a missing folder is just empty
pub fn folder_images(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut images: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png"))
                .unwrap_or(false)
        })
        .collect();
    images.sort();
    images
}

/// next slideshow index: a pseudo-random step that never repeats
/// the current image (a stride coprime to the count would cycle
/// everything, but a fresh random pick feels less mechanical)
pub fn next_index(current: Option<usize>, count: usize, seed: u64) -> Option<usize> {
    match count {
        0 => None,
        1 => Some(0),
        _ => {
            let pick = (seed % count as u64) as usize;
            Some(match current {
                Some(c) if c == pick => (pick + 1) % count,
                _ => pick,
            })
        }
    }
}

/// load one background off the UI thread: decoded, scaled to
/// cover the panel (`max_edge` on the long side) and optionally
/// blurred. Plain RGBA so the caller can hand it to the UI thread
pub fn load_background(
    path: &Path,
    max_edge: u32,
    blur: bool,
) -> Option<crate::player::cover::CoverData> {
    let img = match image::open(path) {
        Ok(img) => img.to_rgba8(),
        Err(e) => {
            tracing::warn!("standby: cannot load background {}: {e}", path.display());
            return None;
        }
    };
    let (width, height) = img.dimensions();
    let mut data = crate::player::cover::CoverData {
        rgba: img.into_raw(),
        width,
        height,
    };
    if blur {
        // the blur helper downsizes first — cheap on a Pi
        return crate::player::cover::make_blur(&Some(data));
    }
    crate::player::cover::cap_size(&mut data, max_edge);
    Some(data)
}

/// Drives the standby backdrop: picks the source, loads folder
/// images off the UI thread, crossfades through the two UI slots.
/// UI-thread only (holds slint timers and weak UI handles).
pub struct Backdrop {
    ui: slint::Weak<Mestro>,
    config: RefCell<crate::config::StandbyBackground>,
    /// folder slideshow state
    images: RefCell<Vec<PathBuf>>,
    current: std::cell::Cell<Option<usize>>,
    /// which slot is in front (false = a, true = b)
    front_b: std::cell::Cell<bool>,
    timer: slint::Timer,
}

/// long edge for decoded backgrounds: covers an 800x480 panel at
/// scale 1.5 with room to spare, keeps uploads small
const BACKGROUND_EDGE: u32 = 1280;

impl Backdrop {
    pub fn new(ui: &Mestro) -> Rc<Self> {
        Rc::new(Self {
            ui: ui.as_weak(),
            config: RefCell::new(crate::config::StandbyBackground::default()),
            images: RefCell::new(Vec::new()),
            current: std::cell::Cell::new(None),
            front_b: std::cell::Cell::new(false),
            timer: slint::Timer::default(),
        })
    }

    /// (re)apply the [clock.background] settings
    pub fn configure(self: &Rc<Self>, config: &crate::config::StandbyBackground) {
        use crate::config::BackgroundSource;
        *self.config.borrow_mut() = crate::config::StandbyBackground {
            source: config.source,
            dim: config.dim,
            blur: config.blur,
            folder: config.folder.clone(),
            interval: config.interval,
        };
        self.timer.stop();
        let Some(ui) = self.ui.upgrade() else { return };
        ui.set_standby_bg_dim(config.dim);
        match config.source {
            BackgroundSource::Black => ui.set_standby_bg_shown(false),
            BackgroundSource::Cover => {
                ui.set_standby_bg_shown(true);
                self.show_cover(&ui);
            }
            BackgroundSource::Folder => {
                *self.images.borrow_mut() = folder_images(&config.folder);
                if self.images.borrow().is_empty() {
                    tracing::warn!(
                        "standby: no jpg/png in {} — background stays black",
                        config.folder.display()
                    );
                    ui.set_standby_bg_shown(false);
                    return;
                }
                ui.set_standby_bg_shown(true);
                self.advance();
                if config.interval > 0 {
                    let me = Rc::downgrade(self);
                    self.timer.start(
                        slint::TimerMode::Repeated,
                        std::time::Duration::from_secs(config.interval as u64 * 60),
                        move || {
                            if let Some(me) = me.upgrade() {
                                me.advance();
                            }
                        },
                    );
                }
            }
        }
    }

    /// the cover source follows the player's cover: call on every
    /// cover change (cheap: the images are already uploaded)
    pub fn cover_changed(&self) {
        if self.config.borrow().source != crate::config::BackgroundSource::Cover {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            self.show_cover(&ui);
        }
    }

    fn show_cover(&self, ui: &Mestro) {
        let image = if self.config.borrow().blur {
            ui.get_bg_cover()
        } else {
            ui.get_cover()
        };
        self.present(ui, image);
    }

    /// next folder image: decode on a worker thread, then hand the
    /// pixels back to the UI thread for the crossfade
    fn advance(self: &Rc<Self>) {
        let count = self.images.borrow().len();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let Some(index) = next_index(self.current.get(), count, seed) else {
            return;
        };
        self.current.set(Some(index));
        let path = self.images.borrow()[index].clone();
        let blur = self.config.borrow().blur;
        let ui = self.ui.clone();
        std::thread::spawn(move || {
            let Some(data) = load_background(&path, BACKGROUND_EDGE, blur) else {
                return;
            };
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui.upgrade() else { return };
                let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &data.rgba,
                    data.width,
                    data.height,
                );
                ui.invoke_standby_bg_present(slint::Image::from_rgba8(buffer));
            });
        });
    }

    /// write into the hidden slot, then flip it to the front
    pub fn present(&self, ui: &Mestro, image: slint::Image) {
        let to_b = !self.front_b.get();
        if to_b {
            ui.set_standby_bg_b(image);
        } else {
            ui.set_standby_bg_a(image);
        }
        self.front_b.set(to_b);
        ui.set_standby_bg_front_b(to_b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_index_never_repeats_the_current_image() {
        assert_eq!(next_index(None, 0, 7), None);
        assert_eq!(next_index(Some(0), 1, 7), Some(0));
        for seed in 0..50 {
            let next = next_index(Some(2), 5, seed).unwrap();
            assert_ne!(next, 2);
            assert!(next < 5);
        }
    }

    #[test]
    fn folder_images_filters_by_extension() {
        let dir = std::env::temp_dir().join(format!("mestro-bg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b.JPG", "a.png", "c.jpeg", "notes.txt", "d.webp"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let names: Vec<String> = folder_images(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.png", "b.JPG", "c.jpeg"]);
        assert!(folder_images(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
