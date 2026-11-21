//! The standby clock: built-in faces, custom skins with their
//! previews, the image backdrop, and the per-tick clock values.

use mestro::config::{Clock, StandbyBackground};
use mestro::standby::{Backdrop, ClockValues, Skins};
use mestro::Mestro;
use slint::ComponentHandle;
use std::cell::Cell;
use std::rc::Rc;

/// the values every face receives, for now (the placeholder while
/// the system time is not trustworthy yet)
pub fn tick_now() -> ClockValues {
    ClockValues::at(chrono::Local::now().naive_local())
}

/// the clock properties of the window: the topbar label, the
/// standby faces and their validity
pub fn show(ui: &Mestro, values: &ClockValues) {
    ui.set_clock_label(values.time_label.as_str().into());
    ui.set_clock_date(values.date_label.as_str().into());
    ui.set_clock_minute(values.minute);
    ui.set_clock_hour(values.hour);
    ui.set_clock_valid(values.valid);
}

/// skins + backdrop, activated from the config and re-activated
/// on every clock.* settings edit
pub struct StandbyClock {
    skins: Rc<Skins>,
    backdrop: Rc<Backdrop>,
    /// previews render once per accent: the gallery is static and a
    /// full render set costs a few hundred ms on a Pi
    preview_accent: Cell<Option<slint::Color>>,
    ui: slint::Weak<Mestro>,
}

impl StandbyClock {
    /// compile [clock.skins] (a failed skin falls back to the
    /// built-in digital face) and wire the backdrop's image slots
    pub fn new(ui: &Mestro, clock: &Clock) -> Rc<Self> {
        let me = Rc::new(Self {
            skins: Rc::new(Skins::compile(&clock.skins)),
            backdrop: Backdrop::new(ui),
            preview_accent: Cell::new(None),
            ui: ui.as_weak(),
        });
        ui.on_standby_bg_present({
            let backdrop = me.backdrop.clone();
            let weak = ui.as_weak();
            move |image| {
                if let Some(ui) = weak.upgrade() {
                    backdrop.present(&ui, image);
                }
            }
        });
        me.apply(ui, clock);
        me
    }

    /// the face (built-in or skin), the gallery previews and the
    /// backdrop for this [clock] section
    pub fn apply(&self, ui: &Mestro, clock: &Clock) {
        let skin = clock.skin().filter(|name| self.skins.has(name));
        let active = self.skins.activate(ui, skin, &tick_now());
        // a configured but uncompiled skin shows the digital face
        ui.set_clock_analog(!active && clock.is_analog());
        let accent = ui.get_accent_color();
        if active {
            self.skins.push_accent(accent);
        }
        if self.preview_accent.replace(Some(accent)) != Some(accent) {
            let previews: Vec<mestro::ClockPreview> = self
                .skins
                .previews(&clock.styles(), accent)
                .into_iter()
                .map(|(name, preview)| mestro::ClockPreview {
                    name: name.into(),
                    preview,
                })
                .collect();
            ui.set_clock_previews(slint::ModelRc::new(slint::VecModel::from(previews)));
        }
        self.configure_backdrop(&clock.background);
    }

    pub fn configure_backdrop(&self, background: &StandbyBackground) {
        self.backdrop.configure(background);
    }

    /// one face as PNG (the web settings thumbnail), at the live
    /// accent like the gallery
    pub fn preview_png(&self, name: &str) -> Option<Vec<u8>> {
        let ui = self.ui.upgrade()?;
        self.skins.preview_png(name, ui.get_accent_color())
    }

    /// the cover source follows the player's cover
    pub fn cover_changed(&self) {
        self.backdrop.cover_changed();
    }

    /// once per ticker round: the clock values for the built-in
    /// faces and the running skin
    pub fn tick(&self, ui: &Mestro) {
        // cheap: slint change-tracking only repaints when the minute
        // (or label) actually changes
        let values = tick_now();
        show(ui, &values);
        // runtime skins: values + the (cover-following) accent
        self.skins.push(&values);
        self.skins.push_accent(ui.get_accent_color());
    }
}
