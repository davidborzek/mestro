//! Palette application: the slint side of the artwork palette.
//! The generation (cover pixels → plain RGB) lives in
//! `mestro_player::palette`, re-exported here under its historical
//! path; this module turns the result into `slint::Color`s and owns
//! the fixed fallbacks' color form.

pub use mestro_player::palette::{
    from_cover, variant_as_str, variant_from_str, ArtworkPalette, Variant, AMBER, DANGER, VARIANTS,
};

/// (accent, Some(tertiary), danger) as slint colors
pub fn to_colors(palette: ArtworkPalette) -> (slint::Color, Option<slint::Color>, slint::Color) {
    let [r, g, b] = palette.accent;
    let [r2, g2, b2] = palette.tertiary;
    let [rd, gd, bd] = palette.danger;
    (
        slint::Color::from_rgb_u8(r, g, b),
        Some(slint::Color::from_rgb_u8(r2, g2, b2)),
        slint::Color::from_rgb_u8(rd, gd, bd),
    )
}

/// the fixed theme for covers without a dominant color
pub fn amber_fallback() -> (slint::Color, Option<slint::Color>, slint::Color) {
    (
        slint::Color::from_rgb_u8(AMBER[0], AMBER[1], AMBER[2]),
        None,
        slint::Color::from_rgb_u8(DANGER[0], DANGER[1], DANGER[2]),
    )
}
