//! runtime VU skins
//!
//! A skin is a .slint file implementing the VuSkin contract
//! (README): in-properties level-l/level-r (float 0..1 across
//! -20..+3 VU), title/artist/codec/quality (string),
//! lossless/has-media/show-info (bool) and a back() callback. It
//! renders fullscreen inside the VU overlay via a
//! ComponentContainer. Skins compile once at boot; a broken skin
//! falls back to the preset.
//!
//! With [vu] cycle the views switch at runtime: activating a
//! skin swaps the ComponentContainer factory (an empty factory
//! leaves the built-in presets visible), so cycling costs
//! nothing but a factory set.

use crate::ui::navigation::Navigation;
use crate::Mestro;
use slint::{ComponentFactory, ComponentHandle};
use slint_interpreter::{Compiler, ComponentDefinition, ComponentInstance};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// per-instance setup hook: manifest skins register their
/// callbacks on each fresh instance
pub type SkinSetup = std::rc::Rc<dyn Fn(&ComponentInstance)>;

/// a compiled skin: name, interpreter definition, whether its
/// plate is light (flips the ghost-arrow ink), and a per-instance
/// setup hook
pub struct Skin {
    pub name: String,
    pub definition: ComponentDefinition,
    pub light: bool,
    pub setup: Option<SkinSetup>,
}

/// where the active skin instance lives (the factory stores it
/// here as a weak handle for push_skin)
pub type SkinSlot = Rc<RefCell<Option<slint::Weak<ComponentInstance>>>>;

/// compile a skin from a .slint path
pub fn compile(name: &str, path: &Path) -> Option<Skin> {
    let Ok(source) = std::fs::read_to_string(path) else {
        tracing::warn!("vu: cannot read skin {name} at {}", path.display());
        return None;
    };
    compile_source(name, &source)
}

/// compile a skin from an embedded source string
pub fn compile_source(name: &str, source: &str) -> Option<Skin> {
    let definition = {
        let mut compiler = Compiler::new();
        crate::vu::select_last_exported(&mut compiler);
        let result =
            spin_on::spin_on(compiler.build_from_source(source.to_string(), PathBuf::from(name)));
        for d in result.diagnostics() {
            tracing::warn!("vu skin: {d}");
        }
        let component = result.components().next();
        match component {
            Some(c) => c,
            None => {
                tracing::warn!("vu: skin {name} failed to compile, skipping it");
                return None;
            }
        }
    };
    Some(Skin {
        name: name.to_string(),
        definition,
        light: false,
        setup: None,
    })
}

/// activate a view: a skin embeds through a fresh factory (and
/// its instance lands in the slot for push_skin); None clears the
/// factory so the built-in presets render again
pub fn activate(ui: &Mestro, slot: &SkinSlot, nav: &Navigation, skin: Option<&Skin>) {
    activate_direction(ui, slot, nav, skin, 0);
}

pub fn activate_direction(
    ui: &Mestro,
    slot: &SkinSlot,
    nav: &Navigation,
    skin: Option<&Skin>,
    direction: i32,
) {
    let Some(skin) = skin else {
        ui.set_vu_skin_factory(ComponentFactory::default());
        ui.invoke_vu_clear();
        return;
    };
    let definition = skin.definition.clone();
    let setup = skin.setup.clone();
    let slot2 = slot.clone();
    let ui_weak = ui.as_weak();
    let nav = nav.clone();
    let factory = ComponentFactory::new(move |ctx| {
        let instance = match definition.create_embedded(ctx) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("vu: skin failed to embed: {e}");
                return None;
            }
        };
        // the skin's back() closes the overlay like the presets
        let ui_weak = ui_weak.clone();
        let nav = nav.clone();
        instance
            .set_callback("back", move |_args| {
                if let Some(ui) = ui_weak.upgrade() {
                    nav.back(&ui);
                }
                slint_interpreter::Value::Void
            })
            .ok();
        if let Some(setup) = &setup {
            setup(&instance);
        }
        // attach the spectrum models once per instance — later
        // pushes only update rows in place (a fresh model per
        // tick re-instantiates the whole segment grid)
        let (bands, peaks) = spectrum_models();
        BANDS_MODEL.with(|b| *b.borrow_mut() = Some(bands.clone()));
        PEAKS_MODEL.with(|b| *b.borrow_mut() = Some(peaks.clone()));
        if let Err(e) = instance.set_property("bands", slint_interpreter::Value::Model(bands)) {
            tracing::warn!("vu: bands attach failed: {e}");
        }
        if let Err(e) = instance.set_property("peaks", slint_interpreter::Value::Model(peaks)) {
            tracing::warn!("vu: peaks attach failed: {e}");
        }
        *slot2.borrow_mut() = Some(instance.as_weak());
        // a fresh instance starts at property defaults: the
        // change-gating cache must not skip the first push
        // (equal option maps across views made meters vanish on
        // direct analyzer switches)
        LAST.with(|l| *l.borrow_mut() = PushState::default());
        Some(instance)
    });
    ui.set_vu_skin_factory(factory);
    ui.invoke_vu_present(direction);
}

/// push the meter data into the runtime skin; missing properties
/// are ignored — skins may implement a subset of the contract.
/// `options` ([vu.skin-options]) pass through as properties.
#[allow(clippy::too_many_arguments)]
pub fn push_skin(
    slot: &SkinSlot,
    level: [f32; 2],
    title: &slint::SharedString,
    artist: &slint::SharedString,
    codec: &slint::SharedString,
    quality: &slint::SharedString,
    lossless: bool,
    has_media: bool,
    show_info: bool,
    info: &[slint::SharedString],
    // the spectrum analyzer bands 0..1 and their peak caps
    // (vu::BANDS entries each); skins declare
    // `in property <[float]> bands` / `peaks` to receive them
    bands: &[f32],
    peaks: &[f32],
    // the cover's colors ([theme] palette = "cover"): primary
    // and optional hue-distant secondary; skins declare
    // `in property <color> palette` / `palette-2` to receive them.
    // `danger` is the harmonized overload tone (fixed red hue,
    // the accent's saturation/value) — skins declare
    // `in property <color> danger` for red zones and meter peaks
    palette: Option<(slint::Color, Option<slint::Color>)>,
    danger: slint::Color,
    options: &std::collections::BTreeMap<String, toml::Value>,
) {
    let Some(skin) = slot.borrow().as_ref().and_then(|w| w.upgrade()) else {
        return;
    };
    let set = |name: &str, v: slint_interpreter::Value| {
        let _ = skin.set_property(name, v);
    };
    set("level-l", (level[0] as f64).into());
    set("level-r", (level[1] as f64).into());
    // unchanged strings would re-layout the skin's text 30×/s
    // (interpreter sets always notify) — push only what changed
    if LAST.with(|l| l.borrow().title != title.as_str()) {
        set("title", slint_interpreter::Value::String(title.clone()));
    }
    if LAST.with(|l| l.borrow().artist != artist.as_str()) {
        set("artist", slint_interpreter::Value::String(artist.clone()));
    }
    if LAST.with(|l| l.borrow().codec != codec.as_str()) {
        set("codec", slint_interpreter::Value::String(codec.clone()));
    }
    if LAST.with(|l| l.borrow().quality != quality.as_str()) {
        set("quality", slint_interpreter::Value::String(quality.clone()));
    }
    if LAST.with(|l| l.borrow().lossless != lossless) {
        set("lossless", lossless.into());
    }
    if LAST.with(|l| l.borrow().has_media != has_media) {
        set("has-media", has_media.into());
    }
    if LAST.with(|l| l.borrow().show_info != show_info) {
        set("show-info", show_info.into());
    }
    {
        let strings_differ = LAST.with(|l| {
            let last = l.borrow();
            last.info.len() != info.len()
                || last
                    .info
                    .iter()
                    .zip(info.iter())
                    .any(|(a, b)| a != b.as_str())
        });
        if strings_differ {
            set(
                "info-lines",
                slint_interpreter::Value::Model(slint::ModelRc::new(slint::VecModel::from(
                    info.iter()
                        .map(|r| slint_interpreter::Value::String(r.clone()))
                        .collect::<Vec<_>>(),
                ))),
            );
            set("info-count", (info.len() as f64).into());
        }
    }
    LAST.with(|l| {
        let mut last = l.borrow_mut();
        last.title = title.to_string();
        last.artist = artist.to_string();
        last.codec = codec.to_string();
        last.quality = quality.to_string();
        last.lossless = lossless;
        last.has_media = has_media;
        last.show_info = show_info;
        last.info = info.iter().map(|r| r.to_string()).collect();
    });

    if let Some((p, p2)) = palette {
        if LAST.with(|l| l.borrow().palette != Some((p, p2))) {
            set(
                "palette",
                slint_interpreter::Value::Brush(slint::Brush::SolidColor(p)),
            );
            if let Some(p2) = p2 {
                set(
                    "palette-2",
                    slint_interpreter::Value::Brush(slint::Brush::SolidColor(p2)),
                );
            }
            LAST.with(|l| l.borrow_mut().palette = Some((p, p2)));
        }
    }
    if LAST.with(|l| l.borrow().danger != danger) {
        set(
            "danger",
            slint_interpreter::Value::Brush(slint::Brush::SolidColor(danger)),
        );
        LAST.with(|l| l.borrow_mut().danger = danger);
    }

    // the spectrum models: attached per instance (see activate),
    // rows updated in place — only changed rows re-evaluate
    for (values, cache) in [(bands, &BANDS_MODEL), (peaks, &PEAKS_MODEL)] {
        use slint::Model as _;
        let model = cache.with(|b| b.borrow().clone());
        let model = match model {
            Some(m) if m.row_count() == values.len() => m,
            _ => {
                let m = slint::ModelRc::new(slint::VecModel::from(vec![
                    slint_interpreter::Value::Number(0.0);
                    values.len()
                ]));
                cache.with(|b| *b.borrow_mut() = Some(m.clone()));
                m
            }
        };
        for (i, v) in values.iter().enumerate() {
            model.set_row_data(i, (*v as f64).into());
        }
    }

    // [vu.skin-options] → plain-value properties, gated like
    // the strings (interpreter sets always notify)
    if LAST.with(|l| l.borrow().options.as_ref() != Some(options)) {
        for (key, value) in options {
            let v = match value {
                toml::Value::String(s) => Some(slint_interpreter::Value::String(s.clone().into())),
                toml::Value::Integer(i) => Some((*i as f64).into()),
                toml::Value::Float(f) => Some((*f).into()),
                toml::Value::Boolean(b) => Some((*b).into()),
                _ => None, // arrays/tables: not a plain slint value
            };
            if let Some(v) = v {
                let _ = skin.set_property(key, v);
            }
        }
        LAST.with(|l| l.borrow_mut().options = Some(options.clone()));
    }
}

/// the last pushed scalar values — push_skin is a 30fps tick and
/// interpreter sets always notify, so unchanged values must not
/// re-fire (each re-fire re-lays-out text)
#[derive(Default)]
struct PushState {
    title: String,
    artist: String,
    codec: String,
    quality: String,
    lossless: bool,
    has_media: bool,
    show_info: bool,
    info: Vec<String>,
    options: Option<std::collections::BTreeMap<String, toml::Value>>,
    palette: Option<(slint::Color, Option<slint::Color>)>,
    danger: slint::Color,
}
thread_local! {
    static LAST: std::cell::RefCell<PushState> = const { std::cell::RefCell::new(
        PushState {
            title: String::new(),
            artist: String::new(),
            codec: String::new(),
            quality: String::new(),
            lossless: false,
            has_media: false,
            show_info: false,
            info: Vec::new(),
            options: None,
            palette: None,
            danger: slint::Color::from_rgb_u8(0xe1, 0x4c, 0x44),
        }
    ) };
}

// the spectrum models — thread_local: push_skin runs on the UI
// thread, and the models aren't Sync
thread_local! {
    static BANDS_MODEL: std::cell::RefCell<
        Option<slint::ModelRc<slint_interpreter::Value>>,
    > = const { std::cell::RefCell::new(None) };
    static PEAKS_MODEL: std::cell::RefCell<
        Option<slint::ModelRc<slint_interpreter::Value>>,
    > = const { std::cell::RefCell::new(None) };
}

/// the cached spectrum models (vu::BANDS zero rows on first use) —
/// activate() attaches them to a fresh skin instance once
pub(crate) fn spectrum_models() -> (
    slint::ModelRc<slint_interpreter::Value>,
    slint::ModelRc<slint_interpreter::Value>,
) {
    let fresh = || {
        slint::ModelRc::new(slint::VecModel::from(vec![
            slint_interpreter::Value::Number(
                0.0
            );
            crate::vu::BANDS
        ]))
    };
    (
        BANDS_MODEL
            .with(|b| b.borrow().clone())
            .unwrap_or_else(&fresh),
        PEAKS_MODEL
            .with(|b| b.borrow().clone())
            .unwrap_or_else(&fresh),
    )
}
