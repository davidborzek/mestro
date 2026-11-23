//! The abstract control channel: every command source (keyboard,
//! the ctl socket, the web endpoint, the settings page) lands here.
//! Transport and navigation route to the UI; settings edits merge
//! into the overlay and re-sync exactly the families they touch.

use super::clock::StandbyClock;
use super::theme::{self, PaletteCell};
use mestro::config::{Config, Osd};
use mestro::display::Display;
use mestro::player::nowplaying::QualityVals;
use mestro::providers::{ma, SourceCommand};
use mestro::remote::control::Command;
use mestro::remote::web::Events;
use mestro::settings::{Handle, NpState, VuState};
use mestro::ui::navigation::Navigation;
use mestro::vu::wire::Vui;
use mestro::{player::nowplaying, Mestro};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, RwLock};

/// everything a command can touch
pub struct Dispatch {
    pub ui: slint::Weak<Mestro>,
    pub nav: Navigation,
    pub osd: Osd,
    pub settings: Rc<Handle>,
    pub np_state: Rc<RefCell<NpState>>,
    pub quality_vals: Rc<RefCell<QualityVals>>,
    pub theme_cover: Rc<Cell<bool>>,
    pub palette: PaletteCell,
    pub display: Rc<Display>,
    pub clock: Rc<StandbyClock>,
    pub vui: Vui,
    pub vu_state: Rc<RefCell<VuState>>,
    /// the provider connection, re-read by the session on every
    /// (re)connect attempt
    pub ma_cfg: Arc<RwLock<ma::Config>>,
    pub cmd_tx: tokio::sync::mpsc::UnboundedSender<SourceCommand>,
    /// the settings push: every successful settings command (from
    /// any source — web, slint, ctl) tells the sockets to refetch
    pub events: Events,
}

/// which settings families an edit touched
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Families {
    pub now_playing: bool,
    pub theme_ui: bool,
    pub connection: bool,
    pub audio: bool,
    pub vu: bool,
    pub display: bool,
    pub clock: bool,
}

impl Families {
    /// every family (reset-all)
    pub const ALL: Self = Self {
        now_playing: true,
        theme_ui: true,
        connection: true,
        audio: true,
        vu: true,
        display: true,
        clock: true,
    };

    /// the families a dotted settings key belongs to
    pub fn of_key(key: &str) -> Self {
        let has = |prefix: &str| key.starts_with(prefix);
        Self {
            now_playing: has("now-playing."),
            // the theme/ui sync also renders clock, osd and hub
            // values on the settings page
            theme_ui: has("theme.")
                || has("ui.")
                || has("background.")
                || has("clock.")
                || has("osd.")
                || has("hub."),
            connection: has("music-assistant."),
            audio: has("audio."),
            vu: has("vu."),
            display: has("display."),
            clock: has("clock."),
        }
    }
}

impl Dispatch {
    pub fn run(&self, cmd: &Command) {
        let Some(ui) = self.ui.upgrade() else { return };
        match cmd {
            Command::CycleForward => {
                if ui.get_vu_open() {
                    ui.invoke_vu_cycle();
                }
            }
            Command::CycleBack => {
                if ui.get_vu_open() {
                    ui.invoke_vu_cycle_back();
                }
            }
            Command::Back => ui.invoke_nav_back(),
            Command::Home => ui.invoke_nav_home(),
            Command::NowPlaying => self.nav.show_np(&ui),
            // "show X" semantics (vs. the UI buttons' push
            // semantics): an open overlay gets replaced — the
            // commands work from anywhere, incl. from VU
            Command::Queue => self.nav.show_queue(&ui),
            Command::Vu => self.nav.show_vu(&ui),
            Command::VuSelect(index) => {
                // the web ctl op answers out-of-range with an error
                // before it gets here; the other sources land here
                if self.vui.select(&ui, *index) {
                    ui.set_vu_overview_open(false);
                    self.nav.show_vu(&ui);
                } else {
                    tracing::warn!("vu-select:{index}: outside the cycle");
                }
            }
            // standby = the player powered off via the source; the
            // state update lands back on the UI, toggle reads the
            // last known flag
            Command::StandbyOn => ui.invoke_set_power(false),
            Command::StandbyOff => ui.invoke_set_power(true),
            Command::StandbyToggle => ui.invoke_set_power(!ui.get_powered()),
            Command::PlayPause => ui.invoke_toggle_play(),
            Command::NextTrack => ui.invoke_next_track(),
            Command::PrevTrack => ui.invoke_prev_track(),
            Command::VolumeUp | Command::VolumeDown => {
                self.volume_step(&ui, matches!(cmd, Command::VolumeUp));
            }
            Command::SetSetting { key, value } => match self.settings.set(key, value.clone()) {
                Ok(effective) => {
                    self.apply(&ui, &effective, Families::of_key(key));
                    self.events.publish_settings();
                }
                Err(e) => tracing::warn!("settings: {e}"),
            },
            Command::ResetSetting { key } => match self.settings.reset(key) {
                Ok(effective) => {
                    self.apply(&ui, &effective, Families::of_key(key));
                    self.events.publish_settings();
                }
                Err(e) => tracing::warn!("settings: {e}"),
            },
            Command::ResetAllSettings => match self.settings.reset_all() {
                Ok(effective) => {
                    self.apply(&ui, &effective, Families::ALL);
                    self.events.publish_settings();
                }
                Err(e) => tracing::warn!("settings: {e}"),
            },
        }
    }

    /// remote-style steps: the slider state is the source of truth,
    /// MA follows via the update
    fn volume_step(&self, ui: &Mestro, up: bool) {
        let v = ui.get_volume();
        let next = if up { (v + 5).min(100) } else { (v - 5).max(0) };
        if next != v {
            ui.set_volume(next);
            ui.invoke_volume_set(next);
            // remote feedback: the volume OSD ([osd] volume gates it;
            // base flag only — the context override tables target
            // toasts)
            if self.osd.volume {
                ui.invoke_show_osd_volume(next);
            }
        }
    }

    /// live rewire after a settings edit: only the touched families
    fn apply(&self, ui: &Mestro, effective: &Config, touched: Families) {
        if touched.now_playing {
            mestro::settings::apply_effective(ui, &self.settings, &self.np_state, effective);
            // the quality line renders from the cached values —
            // instant, no player event needed
            nowplaying::apply_quality(
                ui,
                &self.quality_vals.borrow(),
                &self.np_state.borrow().quality,
            );
        }
        if touched.theme_ui {
            mestro::settings::sync_theme_ui(ui, &self.settings, effective);
            theme::apply_live(ui, effective, &self.theme_cover, &self.palette);
        }
        if touched.connection {
            mestro::settings::sync_connection(ui, effective);
            // the session reconnects with the fresh values (its
            // factory re-reads them per attempt)
            *mestro::locks::write(&self.ma_cfg) =
                ma::Config::from_music_assistant(&effective.music_assistant);
            let _ = self.cmd_tx.send(SourceCommand::Reconnect);
        }
        // squeezelite's env file follows the audio settings and the
        // url its SERVER derives from
        #[cfg(feature = "audio-output")]
        if touched.audio || touched.connection {
            mestro::system::squeezelite::settings_changed(
                &effective.audio,
                &effective.music_assistant.url,
            );
        }
        if touched.vu {
            mestro::settings::sync_vu(ui, &self.settings, effective, &self.vui, &self.vu_state);
        }
        if touched.display {
            super::display::apply(&self.display, effective);
            super::display::sync(ui, &self.settings, effective, &self.display);
        }
        if touched.clock {
            self.clock.apply(ui, &effective.clock);
        }
        mestro::settings::sync_overlay_state(ui, &self.settings);
    }
}

#[cfg(test)]
mod tests {
    use super::Families;

    #[test]
    fn keys_map_to_their_families() {
        let np = Families::of_key("now-playing.font.title-size");
        assert!(np.now_playing && !np.theme_ui && !np.clock);

        let clock = Families::of_key("clock.background.dim");
        assert!(
            clock.clock && clock.theme_ui,
            "clock edits also re-sync the theme page"
        );
        assert!(!clock.display);

        assert!(Families::of_key("music-assistant.url").connection);
        assert!(Families::of_key("audio.output").audio);
        assert!(!Families::of_key("audio.output").connection);
        assert!(Families::of_key("display.brightness").display);
        assert!(Families::of_key("vu.cycle").vu);
        assert!(Families::of_key("osd.style").theme_ui);

        let unknown = Families::of_key("nothing.here");
        assert_eq!(
            unknown,
            Families {
                now_playing: false,
                theme_ui: false,
                connection: false,
                audio: false,
                vu: false,
                display: false,
                clock: false
            }
        );
    }
}
