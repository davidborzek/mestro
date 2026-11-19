//! [ui]

use serde::{Deserialize, Serialize};

/// the OskLayouts registry keys known to the UI — anything else
/// resolves to qwertz-de in the slint registry
pub const OSK_LAYOUTS: &[&str] = &["de", "en"];

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RailStyle {
    Dark,
    Glass,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    /// open the now-playing overlay when playback starts from a
    /// library tap or from outside while in standby
    pub auto_open_now_playing: bool,
    /// nav rail look: dark bezel glass or translucent glass
    /// (the blurred-cover backdrop runs under the rail, so the
    /// glass option shows it through)
    pub rail: RailStyle,
    /// which OSK registry layout the search keyboard renders —
    /// one of the resolve() keys in ui/osk.slint
    /// ("de" = qwertz + numbers-de, "en" = qwerty + numbers-en)
    pub osk_layout: String,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            auto_open_now_playing: true,
            rail: RailStyle::Dark,
            osk_layout: "de".to_string(),
        }
    }
}
