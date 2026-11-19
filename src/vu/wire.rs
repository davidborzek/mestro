//! The VU overlay wiring: the view registry, skin activation and
//! the meter tick.
//!
//! Every cycle entry is a preset name or a [vu.skins] name; the
//! built-ins resolve to embedded manifest/slint skins (see
//! [`registry`]). [`Vui`] owns the shared state between the
//! plate-cycle callbacks and the 33ms tick — clone-cheap (one
//! Rc).
//!
//! Tick rules (perf lessons live here): everything feeds the VU
//! overlay — with it closed there is nothing to push, and the
//! pushes re-layout the hidden layers' text 30×/s (a whole core
//! of constant CPU). On close the skin instance is DETACHED: a
//! hidden instance kept evaluating its needle bindings at 30fps.
//! The info-lines model updates in place — a fresh model per
//! tick re-laid-out the text 30×/s (ICU4X segmentation).

use super::skin::{self, Skin, SkinSlot};
use super::{Shared, SharedBands};
use crate::config::{Config, MetadataField, ThemePalette};
use crate::player::nowplaying;
use crate::player::queue;
use crate::ui::navigation::Navigation;
use crate::ui::palette::{self, ArtworkPalette};
use crate::Mestro;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// one cycle entry: an index into the compiled skin registry
type View = usize;

#[derive(Clone)]
pub struct Vui(Rc<Inner>);

struct Inner {
    views: Vec<View>,
    compiled: Vec<Skin>,
    view_state: Rc<Cell<usize>>,
    skin_slot: SkinSlot,
    nav: Navigation,
    hint_until: Rc<RefCell<Instant>>,
    // render-once images for the skin picker, indexed like compiled
    preview_cache: RefCell<Vec<Option<slint::Image>>>,
    // built-in skin indices for the info-field + options tables
    b: Builtins,
    spectrum_options: BTreeMap<String, toml::Value>,
    bars_options: BTreeMap<String, toml::Value>,
    skin_options: BTreeMap<String, toml::Value>,
    night_info: Vec<MetadataField>,
    cream_info: Vec<MetadataField>,
    round_info: Vec<MetadataField>,
    spectrum_info: Vec<MetadataField>,
    bars_info: Vec<MetadataField>,
    theme_cover: bool,
    palette_cell: Rc<RefCell<Option<ArtworkPalette>>>,
    info_lines_model: Rc<RefCell<slint::ModelRc<slint::SharedString>>>,
    last_info_rows: Rc<RefCell<Vec<slint::SharedString>>>,
    vu_shared: Shared,
    vu_bands: SharedBands,
}

impl Vui {
    /// spawn the level reader, compile the registry and resolve
    /// the [vu.cycle] views. Cheap to clone (one Rc).
    pub fn new(
        config: &Config,
        nav: &Navigation,
        palette_cell: Rc<RefCell<Option<ArtworkPalette>>>,
    ) -> Self {
        let (vu_shared, vu_bands) = super::spawn(
            config.vu.mode,
            &config.vu.shm_prefix,
            nowplaying::playing(),
            config.vu.reference_dbfs,
        );
        let (compiled, b) = registry(config);
        let views = config
            .vu
            .cycle
            .iter()
            .filter_map(|name| {
                // built-in names resolve to their skins
                let builtin = match name.as_str() {
                    "night" => b.night,
                    "cream" => b.cream,
                    "round" => b.round,
                    "wall" => b.wall,
                    "wall-light" => b.wall_light,
                    "arc" => b.arc,
                    "orbit" => b.orbit,
                    "modern" => b.modern,
                    "spectrum" => b.spectrum,
                    "bars" => b.bars,
                    _ => None,
                };
                builtin.or_else(|| compiled.iter().position(|s| &s.name == name))
            })
            .collect::<Vec<_>>();

        let skin_options = config.vu.skin_options.clone();
        // the analyzers' meters knob rides the same passthrough as
        // [vu.skin-options] — per built-in, resolved once
        let mut spectrum_options = skin_options.clone();
        spectrum_options.insert(
            "meters".into(),
            toml::Value::Boolean(config.vu.spectrum.meters),
        );
        spectrum_options.insert(
            "gradient".into(),
            toml::Value::Boolean(config.vu.spectrum.gradient),
        );
        let mut bars_options = skin_options.clone();
        bars_options.insert("meters".into(), toml::Value::Boolean(config.vu.bars.meters));
        let preview_cache = RefCell::new(vec![None; compiled.len()]);

        Self(Rc::new(Inner {
            views,
            compiled,
            view_state: Rc::new(Cell::new(0)),
            skin_slot: Rc::new(RefCell::new(None)),
            nav: nav.clone(),
            hint_until: Rc::new(RefCell::new(Instant::now() - Duration::from_secs(10))),
            preview_cache,
            b,
            spectrum_options,
            bars_options,
            skin_options,
            night_info: config.vu.night.info.clone(),
            cream_info: config.vu.cream.info.clone(),
            round_info: config.vu.round.info.clone(),
            spectrum_info: config.vu.spectrum.info.clone(),
            bars_info: config.vu.bars.info.clone(),
            theme_cover: matches!(config.theme.palette, ThemePalette::Cover),
            palette_cell,
            info_lines_model: Rc::new(RefCell::new(slint::ModelRc::default())),
            last_info_rows: Rc::new(RefCell::new(Vec::new())),
            vu_shared,
            vu_bands,
        }))
    }

    /// view count/index, the initial activation, the plate-cycle
    /// callbacks and the meter tick
    pub fn wire(&self, ui: &Mestro, ui_weak: slint::Weak<Mestro>) {
        // no pre-activation: with the overlay closed the skin
        // would sit in the hidden container evaluating (the tick
        // attaches it on open, and pins the start view otherwise)
        ui.set_vu_view_count(self.0.views.len() as i32);
        ui.set_vu_view_index(0);
        if !self.0.views.is_empty() && ui.get_vu_open() {
            self.activate(ui, self.0.views[0]);
        }
        // a tap on the plate cycles to the next view (the ghost
        // arrow stays the back affordance)
        let step = {
            let vui = self.clone();
            let ui_weak = ui_weak.clone();
            move |dir: i32| {
                let Some(ui) = ui_weak.upgrade() else { return };
                let inner = &vui.0;
                if inner.views.len() < 2 || !ui.get_vu_open() || ui.get_vu_transitioning() {
                    return;
                }
                let n = inner.views.len() as i32;
                let next = (inner.view_state.get() as i32 + dir).rem_euclid(n) as usize;
                inner.view_state.set(next);
                ui.set_vu_view_index(next as i32);
                vui.activate_direction(&ui, inner.views[next], dir);
            }
        };
        ui.on_vu_cycle({
            let step = step.clone();
            move || step(1)
        });
        ui.on_vu_cycle_back({
            let step = step.clone();
            move || step(-1)
        });
        {
            let vui = self.clone();
            let ui_weak = ui_weak.clone();
            ui.on_vu_open_overview(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                let previews = vui.previews();
                ui.set_vu_previews(slint::ModelRc::new(slint::VecModel::from(previews)));
                ui.set_vu_overview_open(true);
            });
        }
        {
            let vui = self.clone();
            let ui_weak = ui_weak.clone();
            ui.on_vu_select_preview(move |index| {
                let Some(ui) = ui_weak.upgrade() else { return };
                if ui.get_vu_transitioning() {
                    return;
                }
                let index = index.max(0) as usize;
                if let Some(view) = vui.0.views.get(index).copied() {
                    let previous = vui.0.view_state.get();
                    if previous != index {
                        vui.0.view_state.set(index);
                        ui.set_vu_view_index(index as i32);
                        vui.activate_direction(&ui, view, if index > previous { 1 } else { -1 });
                    }
                }
                ui.set_vu_overview_open(false);
            });
        }
        // the ghost-arrow hint: reveal on entering the layer or
        // on back-zone touch, hide after 2s
        ui.on_vu_activity({
            let hint_until = self.0.hint_until.clone();
            move || {
                *hint_until.borrow_mut() = Instant::now() + Duration::from_secs(2);
            }
        });
        self.start_tick(ui, ui_weak);
    }

    /// activate a view: every view embeds its skin through a
    /// fresh factory; light plates flip the ghost-arrow ink
    fn activate(&self, ui: &Mestro, view: View) {
        self.activate_direction(ui, view, 0);
    }

    fn activate_direction(&self, ui: &Mestro, view: View, direction: i32) {
        ui.set_vu_arrow_dark(self.0.compiled[view].light);
        skin::activate_direction(
            ui,
            &self.0.skin_slot,
            &self.0.nav,
            Some(&self.0.compiled[view]),
            direction,
        );
    }

    /// Refresh the edited skin snapshot after a settings change.
    pub fn refresh_settings_preview(&self, name: &str, config: &Config) -> Option<slint::Image> {
        let inner = &self.0;
        let view = inner.compiled.iter().position(|skin| skin.name == name)?;
        let (info, glow, meters, gradient) = preview_inputs(name, config);
        let image = super::preview::render_with_options(
            &inner.compiled[view],
            info,
            glow,
            meters,
            gradient,
        );
        inner.preview_cache.borrow_mut()[view] = Some(image.clone());
        Some(image)
    }

    /// one skin's preview as PNG bytes (the web settings
    /// thumbnails): rendered on demand from the CURRENT effective
    /// config, so the thumbnails track info/glow/meter edits
    pub fn preview_png(&self, name: &str, config: &Config) -> Option<Vec<u8>> {
        let inner = &self.0;
        let view = inner.compiled.iter().position(|skin| skin.name == name)?;
        let (info, glow, meters, gradient) = preview_inputs(name, config);
        super::preview::render_png(&inner.compiled[view], info, glow, meters, gradient)
    }

    /// Render each cycle view once and retain its image until the
    /// process exits. The picker itself is not animated. Info
    /// lines follow the per-skin config tables.
    pub fn previews(&self) -> Vec<crate::VuPreviewData> {
        let inner = &self.0;
        let mut cache = inner.preview_cache.borrow_mut();
        inner
            .views
            .iter()
            .enumerate()
            .map(|(index, &view)| {
                let info: &[MetadataField] = match Some(view) {
                    Some(i) if Some(i) == inner.b.night => &inner.night_info,
                    Some(i) if Some(i) == inner.b.cream => &inner.cream_info,
                    Some(i) if Some(i) == inner.b.round => &inner.round_info,
                    Some(i) if Some(i) == inner.b.spectrum => &inner.spectrum_info,
                    Some(i) if Some(i) == inner.b.bars => &inner.bars_info,
                    _ => &[],
                };
                let image = cache[view].get_or_insert_with(|| {
                    let options = if Some(view) == inner.b.spectrum {
                        Some(&inner.spectrum_options)
                    } else if Some(view) == inner.b.bars {
                        Some(&inner.bars_options)
                    } else {
                        None
                    };
                    let bool_option = |key: &str| {
                        options
                            .and_then(|o| o.get(key))
                            .and_then(toml::Value::as_bool)
                    };
                    super::preview::render_with_options(
                        &inner.compiled[view],
                        info,
                        None,
                        bool_option("meters"),
                        bool_option("gradient"),
                    )
                });
                let name = inner.compiled[view].name.clone();
                let kind = if inner.b.spectrum == Some(view) || inner.b.bars == Some(view) {
                    "FFT"
                } else {
                    "VU"
                };
                crate::VuPreviewData {
                    image: image.clone(),
                    name: name.into(),
                    kind: kind.into(),
                    active: index == inner.view_state.get(),
                }
            })
            .collect()
    }

    /// the meter timer always runs: with mode = off the levels
    /// stay zero, but the wall metadata still tracks the track
    fn start_tick(&self, ui: &Mestro, ui_weak: slint::Weak<Mestro>) {
        let vui = self.clone();
        let vu_was_open = Cell::new(ui.get_vu_open());
        let queue_was_open = Cell::new(ui.get_queue_open());
        let update = Rc::new(move |ui: &Mestro| {
            let mut vu_open = vu_was_open.get();
            let mut queue_open = queue_was_open.get();
            vui.tick(ui, &mut vu_open, &mut queue_open);
            vu_was_open.set(vu_open);
            queue_was_open.set(queue_open);
        });
        ui.on_vu_sync({
            let update = update.clone();
            let weak = ui_weak.clone();
            move || {
                if let Some(ui) = weak.upgrade() {
                    update(&ui);
                }
            }
        });
        let vu_timer = slint::Timer::default();
        vu_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(33),
            move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                update(&ui);
            },
        );
        // keep the timer alive for the app's lifetime
        std::mem::forget(vu_timer);
    }

    fn tick(&self, ui: &Mestro, vu_was_open: &mut bool, queue_was_open: &mut bool) {
        let inner = &self.0;
        // the matrix inline VU lives in the np overlay — its bands
        // must flow while the VU overlay (and thus the rest of
        // this tick) is gated off. np_open is false while vu sits
        // on top of it, so no double pushes
        if ui.get_np_open() && ui.get_np_inline_vu() == 3 {
            let bands = inner.vu_bands.lock().unwrap().0;
            ui.set_np_bands(slint::ModelRc::new(slint::VecModel::from(bands.to_vec())));
        }
        // everything else in this tick feeds the VU overlay —
        // with it closed there is nothing to push, and the pushes
        // re-layout the hidden layers' text 30×/s (a whole core
        // of constant CPU)
        let vu_open = ui.get_vu_open();
        if vu_open != *vu_was_open {
            *vu_was_open = vu_open;
            if vu_open {
                *inner.hint_until.borrow_mut() = Instant::now() + Duration::from_secs(2);
                // (re)attach the view's skin — the container was
                // emptied when the overlay closed
                if let Some(v) = inner.views.get(inner.view_state.get()) {
                    self.activate(ui, *v);
                }
            } else {
                // empty the container: a hidden skin instance kept
                // evaluating its needle bindings at 30fps (tens
                // of percent of CPU while nowplaying shows)
                skin::activate(ui, &inner.skin_slot, &inner.nav, None);
            }
        }
        // queue model sync: apply() only syncs it while the queue
        // is open — on open, rebuild from the cache
        let queue_open = ui.get_queue_open();
        if queue_open != *queue_was_open {
            *queue_was_open = queue_open;
            if queue_open {
                queue::rebuild_cache(ui);
            }
        }
        let level = *inner.vu_shared.lock().unwrap();
        // the nowplaying mini-meters read the levels too — cheap
        // now that no hidden skin instance evaluates
        ui.set_vu_l(level[0]);
        ui.set_vu_r(level[1]);
        if !vu_open {
            return;
        }
        // the active view's info lines, by skin identity
        // (built-in manifest skins map to [vu.<name>])
        let active = inner.views.get(inner.view_state.get());
        let active_fields: &[MetadataField] = match active {
            Some(i) if Some(*i) == inner.b.night => &inner.night_info,
            Some(i) if Some(*i) == inner.b.cream => &inner.cream_info,
            Some(i) if Some(*i) == inner.b.round => &inner.round_info,
            Some(i) if Some(*i) == inner.b.spectrum => &inner.spectrum_info,
            Some(i) if Some(*i) == inner.b.bars => &inner.bars_info,
            _ => &[],
        };
        let value = |f: &MetadataField| -> String {
            match f {
                MetadataField::Title => ui.get_track_title().to_string(),
                MetadataField::Artist => ui.get_track_artist().to_string(),
                MetadataField::Codec => ui.get_codec().to_string(),
                MetadataField::Quality => ui.get_quality().to_string(),
                MetadataField::Time => ui.get_pos_label().to_string(),
            }
        };
        let rows: Vec<slint::SharedString> =
            active_fields.iter().map(|f| value(f).into()).collect();
        let rows_n = rows.len();
        // a fresh model per tick re-laid-out the info text 30×/s
        // (ICU4X segmentation) — update in place, and only when
        // the strings actually changed
        {
            use slint::Model as _;
            let prev_count = inner.info_lines_model.borrow().row_count();
            let strings_changed = {
                let prev = inner.last_info_rows.borrow();
                prev.len() == rows_n && prev.iter().zip(rows.iter()).any(|(a, b)| a != b)
            };
            // the field list changes with the active view —
            // rebuild the (fixed-size) model then
            if prev_count != rows_n {
                let model = slint::ModelRc::new(slint::VecModel::from(rows.clone()));
                ui.set_vu_info_lines(model.clone());
                ui.set_vu_info_count(rows_n as i32);
                *inner.info_lines_model.borrow_mut() = model;
                *inner.last_info_rows.borrow_mut() = rows.clone();
            } else if strings_changed {
                let model = inner.info_lines_model.borrow().clone();
                for (i, r) in rows.iter().enumerate() {
                    model.set_row_data(i, r.clone());
                }
                *inner.last_info_rows.borrow_mut() = rows.clone();
            }
        }
        ui.set_vu_hint(Instant::now() < *inner.hint_until.borrow());
        // one lock, copied out — a second lock on the same mutex
        // in one statement deadlocks the UI thread
        let (bands, peaks) = *inner.vu_bands.lock().unwrap();
        // [theme] palette = "cover": the M3 accent/secondary pair
        // rides the skins, else they keep their designed defaults;
        // danger always rides (it stays red in the amber fallback)
        let (palette, danger) = if inner.theme_cover {
            let p = *inner.palette_cell.borrow();
            let (accent, secondary, danger) = p
                .map(|p| p.to_colors())
                .unwrap_or_else(palette::amber_fallback);
            (Some((accent, secondary)), danger)
        } else {
            (None, slint::Color::from_rgb_u8(0xe1, 0x4c, 0x44))
        };
        // the same meter data feeds the runtime skin (the
        // spectrum reads its models through push_skin)
        skin::push_skin(
            &inner.skin_slot,
            level,
            &ui.get_track_title(),
            &ui.get_track_artist(),
            &ui.get_codec(),
            &ui.get_quality(),
            ui.get_lossless(),
            ui.get_has_media(),
            rows_n > 0,
            &rows,
            &bands,
            &peaks,
            // [theme] palette = "cover": primary + secondary cover
            // colors, else the skins keep their designed defaults.
            // The danger tone harmonizes with the accent (red hue,
            // accent saturation/value)
            palette,
            danger,
            match active {
                Some(i) if Some(*i) == inner.b.spectrum => &inner.spectrum_options,
                Some(i) if Some(*i) == inner.b.bars => &inner.bars_options,
                _ => &inner.skin_options,
            },
        );
    }
}

/// the preview pose inputs per skin: the built-ins' option
/// tables from the effective config, empty for everything else
fn preview_inputs<'c>(
    name: &str,
    config: &'c Config,
) -> (&'c [MetadataField], Option<f64>, Option<bool>, Option<bool>) {
    match name {
        "night" => (&config.vu.night.info, None, None, None),
        "cream" => (
            &config.vu.cream.info,
            Some(config.vu.cream.glow),
            None,
            None,
        ),
        "round" => (
            &config.vu.round.info,
            Some(config.vu.round.glow),
            None,
            None,
        ),
        "wall-light" => (&[], Some(config.vu.wall_light.glow), None, None),
        "spectrum" => (
            &config.vu.spectrum.info,
            None,
            Some(config.vu.spectrum.meters),
            Some(config.vu.spectrum.gradient),
        ),
        "bars" => (
            &config.vu.bars.info,
            None,
            Some(config.vu.bars.meters),
            Some(config.vu.bars.gradient),
        ),
        _ => (&[], None, None, None),
    }
}

/// the compiled skin registry: [vu.skins] entries plus the
/// built-ins (manifest VUs are pure data under skins/ — eat your
/// own dogfood; the spectrum/bars are embedded slint skins —
/// they draw, not layers). The built-in indices feed the
/// view-resolution and the info-field/options tables.
struct Builtins {
    night: Option<usize>,
    cream: Option<usize>,
    round: Option<usize>,
    wall: Option<usize>,
    wall_light: Option<usize>,
    arc: Option<usize>,
    orbit: Option<usize>,
    modern: Option<usize>,
    spectrum: Option<usize>,
    bars: Option<usize>,
}

fn registry(config: &Config) -> (Vec<Skin>, Builtins) {
    let mut compiled: Vec<Skin> = config
        .vu
        .skins
        .iter()
        .filter_map(|(name, path)| {
            // .toml entries are manifest skins, everything else
            // compiles as a slint file (the classic full-control
            // skin contract)
            if path.extension().is_some_and(|e| e == "toml") {
                super::manifest::load(name, path)
            } else {
                skin::compile(name, path)
            }
        })
        .collect();
    let mut builtin_idx = |name: &'static str| -> Option<usize> {
        if name == "spectrum" || name == "bars" {
            let source = match name {
                "spectrum" => include_str!("../../skins/spectrum/spectrum.slint"),
                _ => include_str!("../../skins/spectrum-bars/spectrum-bars.slint"),
            };
            return skin::compile_source(name, source).map(|s| {
                compiled.push(s);
                compiled.len() - 1
            });
        }
        let glow = match name {
            "night" => 0.0,
            "cream" => config.vu.cream.glow as f32,
            "round" => config.vu.round.glow as f32,
            "wall" => 0.0,
            "wall-light" => config.vu.wall_light.glow as f32,
            "arc" => 0.0,
            "orbit" => 0.0,
            "modern" => 0.0,
            _ => return None,
        };
        super::manifest::builtin_named(name, glow).map(|s| {
            compiled.push(s);
            compiled.len() - 1
        })
    };
    let night = builtin_idx("night");
    let cream = builtin_idx("cream");
    let round = builtin_idx("round");
    let wall = builtin_idx("wall");
    let wall_light = builtin_idx("wall-light");
    let arc = builtin_idx("arc");
    let orbit = builtin_idx("orbit");
    let modern = builtin_idx("modern");
    let spectrum = builtin_idx("spectrum");
    let bars = builtin_idx("bars");
    (
        compiled,
        Builtins {
            night,
            cream,
            round,
            wall,
            wall_light,
            arc,
            orbit,
            modern,
            spectrum,
            bars,
        },
    )
}
