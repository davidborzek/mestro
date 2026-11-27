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
use slint::ComponentHandle as _;
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
    // the rail auto-hide: one generation per (re)schedule — a
    // newer touch cancels the older pending hide (the OSD's
    // pattern, on the slint clock so tests can drive it)
    rail_gen: Rc<Cell<u32>>,
    // render-once images for the skin picker, indexed like compiled
    preview_cache: RefCell<Vec<Option<slint::Image>>>,
    // built-in skin indices for the info-field + options tables
    b: Builtins,
    // the per-skin options derived from the effective config
    // (info lines, meters/gradient, glow) — rebuilt by
    // apply_options on every settings change
    state: RefCell<SkinState>,
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

        let state = SkinState::from_config(config, &b, &compiled);
        let preview_cache = RefCell::new(vec![None; compiled.len()]);

        Self(Rc::new(Inner {
            views,
            compiled,
            view_state: Rc::new(Cell::new(0)),
            skin_slot: Rc::new(RefCell::new(None)),
            nav: nav.clone(),
            hint_until: Rc::new(RefCell::new(Instant::now() - Duration::from_secs(10))),
            rail_gen: Rc::new(Cell::new(0)),
            preview_cache,
            b,
            state: RefCell::new(state),
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
        ui.set_vu_view_names(slint::ModelRc::new(slint::VecModel::from(
            self.0
                .views
                .iter()
                .map(|&view| slint::SharedString::from(self.0.compiled[view].name.as_str()))
                .collect::<Vec<_>>(),
        )));
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
        // a swipe only switches the faceplate: a tucked rail stays
        // tucked, a visible one keeps its full time from now
        let swiped = {
            let vui = self.clone();
            let ui_weak = ui_weak.clone();
            move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                if !ui.get_vu_rail_hidden() {
                    vui.schedule_rail_hide(&ui);
                }
            }
        };
        ui.on_vu_cycle({
            let step = step.clone();
            let swiped = swiped.clone();
            move || {
                swiped();
                step(1)
            }
        });
        ui.on_vu_cycle_back({
            let step = step.clone();
            move || {
                swiped();
                step(-1)
            }
        });
        {
            let vui = self.clone();
            let ui_weak = ui_weak.clone();
            ui.on_vu_open_overview(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                let previews = vui.previews();
                ui.set_vu_previews(slint::ModelRc::new(slint::VecModel::from(previews)));
                ui.set_vu_overview_open(true);
                // the selection is a menu: the rail stays while it
                // is open (schedule_rail_hide skips it)
                vui.reveal_rail(&ui);
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
        // the ghost-arrow hint AND the rail auto-hide: reveal on
        // entering the layer or on any faceplate touch; Rust
        // hides both on their own timers
        ui.on_vu_activity({
            let hint_until = self.0.hint_until.clone();
            let vui = self.clone();
            let ui_weak = ui_weak.clone();
            move || {
                *hint_until.borrow_mut() = Instant::now() + Duration::from_secs(2);
                let Some(ui) = ui_weak.upgrade() else { return };
                vui.reveal_rail(&ui);
                vui.schedule_rail_hide(&ui);
            }
        });
        self.start_tick(ui, ui_weak);
    }

    /// show the nav rail and drop any pending hide (the VU
    /// closed, or the setting turned the auto-hide off)
    pub fn reveal_rail(&self, ui: &Mestro) {
        self.0.rail_gen.set(self.0.rail_gen.get() + 1);
        ui.set_vu_rail_hidden(false);
    }

    /// start/restart the rail's hide timer ([vu.rail-autohide]
    /// seconds; 0, a closed overlay or the open skin selection
    /// never schedules). A newer schedule cancels the older one
    /// via the generation counter
    pub fn schedule_rail_hide(&self, ui: &Mestro) {
        let inner = &self.0;
        let gen = inner.rail_gen.get() + 1;
        inner.rail_gen.set(gen);
        let secs = ui.get_vu_rail_autohide();
        if !ui.get_vu_open() || ui.get_vu_overview_open() || secs <= 0.0 {
            return;
        }
        let ui_weak = ui.as_weak();
        let rail_gen = inner.rail_gen.clone();
        slint::Timer::single_shot(Duration::from_millis((secs * 1000.0) as u64), move || {
            if rail_gen.get() != gen {
                return;
            }
            let Some(ui) = ui_weak.upgrade() else { return };
            if ui.get_vu_open() && !ui.get_vu_overview_open() && ui.get_vu_rail_autohide() > 0.0 {
                ui.set_vu_rail_hidden(true);
            }
        });
    }

    /// remote selection: make the cycle's `index` the active view.
    /// While the overlay is up the skin switches now (sliding
    /// unless a slide is still running); otherwise the next open
    /// attaches it. False when the index is outside the cycle —
    /// nothing changes then
    pub fn select(&self, ui: &Mestro, index: usize) -> bool {
        let inner = &self.0;
        let Some(&view) = inner.views.get(index) else {
            return false;
        };
        let previous = inner.view_state.get();
        inner.view_state.set(index);
        ui.set_vu_view_index(index as i32);
        if ui.get_vu_open() && previous != index {
            let direction = match (ui.get_vu_transitioning(), index > previous) {
                (true, _) => 0,
                (false, true) => 1,
                (false, false) => -1,
            };
            self.activate_direction(ui, view, direction);
        }
        true
    }

    /// apply the effective config's per-skin options: the settings
    /// path (sync_vu) calls this after every edit. Info lines and
    /// the analyzers' meters/gradient ride the next tick's push;
    /// glow lands on the attached faceplate now, and every future
    /// activation starts at the new value
    pub fn apply_options(&self, config: &Config) {
        let inner = &self.0;
        let next = SkinState::from_config(config, &inner.b, &inner.compiled);
        let active = inner.views.get(inner.view_state.get()).copied();
        let glow = active.and_then(|view| next.glow[view]);
        *inner.state.borrow_mut() = next;
        if let (Some(view), Some(glow)) = (active, glow) {
            skin::push_glow(&inner.skin_slot, &inner.compiled[view], glow);
        }
    }

    /// the attached skin instance's property value — the live-apply
    /// tests read the pushed options off the faceplate; None while
    /// no skin is attached or the skin doesn't declare the property
    pub fn live_property(&self, name: &str) -> Option<slint_interpreter::Value> {
        let skin = self
            .0
            .skin_slot
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())?;
        skin.get_property(name).ok()
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
            self.0.state.borrow().glow[view],
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
        let state = inner.state.borrow();
        let mut cache = inner.preview_cache.borrow_mut();
        inner
            .views
            .iter()
            .enumerate()
            .map(|(index, &view)| {
                let info = state.info(Some(view), &inner.b);
                let image = cache[view].get_or_insert_with(|| {
                    let options = state.options(Some(view), &inner.b);
                    let bool_option = |key: &str| options.get(key).and_then(toml::Value::as_bool);
                    super::preview::render_with_options(
                        &inner.compiled[view],
                        info,
                        state.glow[view],
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
        let overview_was_open = Cell::new(ui.get_vu_overview_open());
        let update = Rc::new(move |ui: &Mestro| {
            let mut vu_open = vu_was_open.get();
            let mut queue_open = queue_was_open.get();
            vui.tick(ui, &mut vu_open, &mut queue_open);
            vu_was_open.set(vu_open);
            queue_was_open.set(queue_open);
            // the selection closed by any path (a pick, back, the
            // remote): the faceplate is back, the rail timer
            // starts fresh
            let overview_open = ui.get_vu_overview_open();
            if overview_was_open.replace(overview_open) && !overview_open {
                vui.schedule_rail_hide(ui);
            }
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
            let bands = inner.vu_bands.lock().0;
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
                // the rail starts shown; its hide timer restarts
                // with the overlay
                self.reveal_rail(ui);
                self.schedule_rail_hide(ui);
                // (re)attach the view's skin — the container was
                // emptied when the overlay closed
                if let Some(v) = inner.views.get(inner.view_state.get()) {
                    self.activate(ui, *v);
                }
            } else {
                // leaving the faceplate always shows the rail
                self.reveal_rail(ui);
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
        let level = *inner.vu_shared.lock();
        // the nowplaying mini-meters read the levels too — cheap
        // now that no hidden skin instance evaluates
        ui.set_vu_l(level[0]);
        ui.set_vu_r(level[1]);
        if !vu_open {
            return;
        }
        // the active view's info lines, by skin identity
        // (built-in manifest skins map to [vu.<name>])
        let state = inner.state.borrow();
        let active = inner.views.get(inner.view_state.get()).copied();
        let active_fields: &[MetadataField] = state.info(active, &inner.b);
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
        let (bands, peaks) = *inner.vu_bands.lock();
        // [theme] palette = "cover": the M3 accent/secondary pair
        // rides the skins, else they keep their designed defaults;
        // danger always rides (it stays red in the amber fallback)
        let (palette, danger) = if inner.theme_cover {
            let p = *inner.palette_cell.borrow();
            let (accent, secondary, danger) = p
                .map(palette::to_colors)
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
            state.options(active, &inner.b),
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

/// the per-skin option tables derived from the effective config:
/// info lines, the analyzers' meters/gradient, the passthrough
/// [vu.skin-options] and the built-in glow values. One source of
/// truth: [`Vui::new`] derives it from the boot config,
/// [`Vui::apply_options`] rebuilds it on every settings change
struct SkinState {
    night_info: Vec<MetadataField>,
    cream_info: Vec<MetadataField>,
    round_info: Vec<MetadataField>,
    spectrum_info: Vec<MetadataField>,
    bars_info: Vec<MetadataField>,
    skin_options: BTreeMap<String, toml::Value>,
    spectrum_options: BTreeMap<String, toml::Value>,
    bars_options: BTreeMap<String, toml::Value>,
    /// per compiled view: the built-in glow dials' current value
    /// (cream, round, wall-light); custom manifest skins keep
    /// their manifest default (None — nothing pushes over it)
    glow: Vec<Option<f64>>,
}

impl SkinState {
    fn from_config(config: &Config, b: &Builtins, compiled: &[Skin]) -> Self {
        let skin_options = config.vu.skin_options.clone();
        // the analyzers' meters/gradient knobs ride the same
        // passthrough as [vu.skin-options] — per built-in
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
        bars_options.insert(
            "gradient".into(),
            toml::Value::Boolean(config.vu.bars.gradient),
        );
        // the effective glow per view: the config knobs (cream,
        // round, wall-light), every other baked skin's manifest
        // default — a [vu.skin-options] glow key overrides both,
        // like any pushed property used to
        let skin_glow = skin_options.get("glow").and_then(toml::Value::as_float);
        let mut glow = vec![None; compiled.len()];
        for (view, skin) in compiled.iter().enumerate() {
            let configured = match Some(view) {
                v if v == b.cream => Some(config.vu.cream.glow),
                v if v == b.round => Some(config.vu.round.glow),
                v if v == b.wall_light => Some(config.vu.wall_light.glow),
                _ => skin.bake.as_ref().map(|bake| bake.default_glow()),
            };
            if let Some(default) = configured {
                glow[view] = Some(skin_glow.unwrap_or(default));
            }
        }
        Self {
            night_info: config.vu.night.info.clone(),
            cream_info: config.vu.cream.info.clone(),
            round_info: config.vu.round.info.clone(),
            spectrum_info: config.vu.spectrum.info.clone(),
            bars_info: config.vu.bars.info.clone(),
            skin_options,
            spectrum_options,
            bars_options,
            glow,
        }
    }

    /// the view's info fields — empty for skins without a config
    /// table (the cycle may hold custom skins)
    fn info(&self, view: Option<usize>, b: &Builtins) -> &[MetadataField] {
        match view {
            Some(i) if Some(i) == b.night => &self.night_info,
            Some(i) if Some(i) == b.cream => &self.cream_info,
            Some(i) if Some(i) == b.round => &self.round_info,
            Some(i) if Some(i) == b.spectrum => &self.spectrum_info,
            Some(i) if Some(i) == b.bars => &self.bars_info,
            _ => &[],
        }
    }

    /// the view's passthrough options table ([vu.skin-options],
    /// with the analyzers' meters/gradient folded in)
    fn options(&self, view: Option<usize>, b: &Builtins) -> &BTreeMap<String, toml::Value> {
        match view {
            Some(i) if Some(i) == b.spectrum => &self.spectrum_options,
            Some(i) if Some(i) == b.bars => &self.bars_options,
            _ => &self.skin_options,
        }
    }
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
        // the config's [vu.<name>] glow rides the live state
        // (SkinState), not the compiled skin — edits apply
        // without a recompile, custom skins keep their default
        super::manifest::builtin_named(name).map(|s| {
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
