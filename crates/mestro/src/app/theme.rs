//! Accent and backdrop: the amber default, the cover-following
//! palette and the live theme switch from the settings.

use mestro::config::{Config, RailStyle, ThemePalette};
use mestro::ui::palette::ArtworkPalette;
use mestro::Mestro;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// the extracted cover palette, shared by the update loop, the VU
/// tick and the settings
pub type PaletteCell = Rc<RefCell<Option<ArtworkPalette>>>;

/// accent, optional second accent, danger
type Colors = (slint::Color, Option<slint::Color>, slint::Color);

/// the fixed theme for covers without a dominant color
pub fn amber_fallback() -> Colors {
    let [r, g, b] = mestro::ui::palette::AMBER;
    let [rd, gd, bd] = mestro::ui::palette::DANGER;
    (
        slint::Color::from_rgb_u8(r, g, b),
        None,
        slint::Color::from_rgb_u8(rd, gd, bd),
    )
}

/// the accent the cover asks for — amber when there is no dominant
/// color (gray covers, no artwork)
pub fn cover_accent(palette: &PaletteCell) -> Colors {
    palette
        .borrow()
        .map(|p| p.to_colors())
        .unwrap_or_else(amber_fallback)
}

pub fn rail_style(rail: RailStyle) -> i32 {
    match rail {
        RailStyle::Glass => 1,
        RailStyle::Dark => 0,
    }
}

/// theme + ui live class: palette flips the update loop's
/// cover-following and re-themes instantly (cover → tonal or
/// amber fallback; amber → the fixed amber trio — the update loop
/// no longer touches the accent), rail re-skins the nav bar.
/// Variant/OSK/auto-open stay restart-class (the provider session
/// and the OSK registry captured them)
pub fn apply_live(
    ui: &Mestro,
    effective: &Config,
    theme_cover: &Cell<bool>,
    palette: &PaletteCell,
) {
    let cover = matches!(effective.theme.palette, ThemePalette::Cover);
    theme_cover.set(cover);
    ui.set_rail_style(rail_style(effective.ui.rail));
    ui.set_backdrop_enabled(effective.background.blurred_cover);
    let (accent, _, danger) = if cover {
        cover_accent(palette)
    } else {
        amber_fallback()
    };
    ui.invoke_set_accent(accent, danger);
}
