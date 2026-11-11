//! The standby clock: built-in faces, custom skins with their
//! previews, the image backdrop, and the per-tick clock values.

use chrono::Timelike;
use mestro::config::{Clock, StandbyBackground};
use mestro::standby::{Backdrop, Skins, Tick};
use mestro::Mestro;
use slint::ComponentHandle;
use std::cell::Cell;
use std::rc::Rc;

/// the values every face receives, for now
pub fn tick_now() -> Tick {
    let now = chrono::Local::now();
    Tick {
        hour: now.hour() as i32,
        minute: now.minute() as i32,
        time_label: now.format("%H:%M").to_string(),
        date_label: now.format("%a %d %b").to_string().to_uppercase(),
    }
}

/// skins + backdrop, activated from the config and re-activated
/// on every clock.* settings edit
pub struct StandbyClock {
    skins: Rc<Skins>,
    backdrop: Rc<Backdrop>,
    /// previews render once per accent: the gallery is static and a
    /// full render set costs a few hundred ms on a Pi
    preview_accent: Cell<Option<slint::Color>>,
}

impl StandbyClock {
    /// compile [clock.skins] (a failed skin falls back to the
    /// built-in digital face) and wire the backdrop's image slots
    pub fn new(ui: &Mestro, clock: &Clock) -> Rc<Self> {
        let me = Rc::new(Self {
            skins: Rc::new(Skins::compile(&clock.skins)),
            backdrop: Backdrop::new(ui),
            preview_accent: Cell::new(None),
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

    /// the cover source follows the player's cover
    pub fn cover_changed(&self) {
        self.backdrop.cover_changed();
    }

    /// once per ticker round: the clock values for the built-in
    /// faces and the running skin
    pub fn tick(&self, ui: &Mestro) {
        let now = chrono::Local::now();
        // cheap: slint change-tracking only repaints when the minute
        // (or label) actually changes
        ui.set_clock_label(now.format("%H:%M").to_string().into());
        ui.set_clock_date(now.format("%a %d %b").to_string().to_uppercase().into());
        ui.set_clock_minute(now.minute() as i32);
        ui.set_clock_hour(now.hour() as i32);
        // runtime skins: values + the (cover-following) accent
        self.skins.push(&tick_now());
        self.skins.push_accent(ui.get_accent_color());
    }
}
