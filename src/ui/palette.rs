//! Palette generation: everything that turns a cover into UI
//! colors. Two stages:
//!
//! 1. seed: 5-bit channel buckets over a strided sample,
//!    saturation- and luminance-filtered — that pre-filter is
//!    OURS on purpose: gray/black/white covers yield None so
//!    the caller keeps its amber fallback (the M3 Score alone
//!    would always return a seed, Google Blue as the last
//!    resort). The surviving buckets feed the REAL M3 Score
//!    (HCT-chroma weighted, with google's dislike/target-line
//!    heuristics) via its IndexMap type — constructible from
//!    outside because indexmap+ahash are direct deps now
//! 2. scheme: a real Material You dynamic scheme
//!    (material-colors, [theme] variant, dark) generates the
//!    accent (primary tone 80) and the two-tone partner
//!    (tertiary) from the seed — the M3 lesson: never use the
//!    raw artwork color, reshape it into pleasing tones. The
//!    danger red is M3-harmonized with the seed (same tone,
//!    red hue).

use crate::player::cover::CoverData;
use material_colors::blend;
use material_colors::color::Argb;
pub use material_colors::dynamic_color::Variant;
use material_colors::score;
use material_colors::theme::ThemeBuilder;
use std::hash::BuildHasherDefault;

// the config speaks these names ([theme] variant)
pub const VARIANTS: [&str; 9] = [
    "tonal-spot",
    "content",
    "expressive",
    "fidelity",
    "fruit-salad",
    "monochrome",
    "neutral",
    "rainbow",
    "vibrant",
];

/// the config name of a variant (inverse of variant_from_str)
pub fn variant_as_str(v: Variant) -> &'static str {
    match v {
        Variant::TonalSpot => "tonal-spot",
        Variant::Content => "content",
        Variant::Expressive => "expressive",
        Variant::Fidelity => "fidelity",
        Variant::FruitSalad => "fruit-salad",
        Variant::Monochrome => "monochrome",
        Variant::Neutral => "neutral",
        Variant::Rainbow => "rainbow",
        Variant::Vibrant => "vibrant",
    }
}

pub fn variant_from_str(s: &str) -> Option<Variant> {
    Some(match s {
        "tonal-spot" => Variant::TonalSpot,
        "content" => Variant::Content,
        "expressive" => Variant::Expressive,
        "fidelity" => Variant::Fidelity,
        "fruit-salad" => Variant::FruitSalad,
        "monochrome" => Variant::Monochrome,
        "neutral" => Variant::Neutral,
        "rainbow" => Variant::Rainbow,
        "vibrant" => Variant::Vibrant,
        _ => return None,
    })
}

/// the fixed fallbacks for covers without a dominant color
pub const AMBER: [u8; 3] = [0xd8, 0xa6, 0x57];
pub const DANGER: [u8; 3] = [0xe1, 0x4c, 0x44];

/// the artwork-derived UI colors
#[derive(Clone, Copy, Debug)]
pub struct ArtworkPalette {
    pub accent: [u8; 3],
    /// the two-tone partner for the visualizers (M3 tertiary —
    /// a hue-distant family, tonalized like the accent)
    pub tertiary: [u8; 3],
    /// the semantic overload red, harmonized with the seed
    pub danger: [u8; 3],
}

impl ArtworkPalette {
    /// (accent, Some(tertiary), danger) as slint colors
    pub fn to_colors(self) -> (slint::Color, Option<slint::Color>, slint::Color) {
        let [r, g, b] = self.accent;
        let [r2, g2, b2] = self.tertiary;
        let [rd, gd, bd] = self.danger;
        (
            slint::Color::from_rgb_u8(r, g, b),
            Some(slint::Color::from_rgb_u8(r2, g2, b2)),
            slint::Color::from_rgb_u8(rd, gd, bd),
        )
    }
}

/// the cover's UI colors via the two-stage pipeline. Gray
/// covers yield None — the caller falls back to the fixed
/// amber theme.
pub fn from_cover(c: &CoverData, variant: Variant) -> Option<ArtworkPalette> {
    let n = c.rgba.len();
    if c.width < 8 || c.height < 8 || n < c.width as usize * c.height as usize * 4 {
        return None;
    }
    // bucket key: 5 significant bits per channel
    let mut bins: std::collections::HashMap<u16, (u32, u64, u64, u64)> =
        std::collections::HashMap::new();
    let stride = ((c.width * c.height) / 4096).max(1) as usize;
    let px = |i: usize| {
        let o = i * 4;
        (c.rgba[o] as u32, c.rgba[o + 1] as u32, c.rgba[o + 2] as u32)
    };
    let total = c.width as usize * c.height as usize;
    let mut i = 0;
    while i < total {
        let (r, g, b) = px(i);
        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
        let lum = (r + g + b) / 3;
        // colored pixels only, not too dark, not washed out
        if mx.saturating_sub(mn) >= 28 && (10..=235).contains(&lum) {
            let key = ((r >> 3) as u16) << 10 | ((g >> 3) as u16) << 5 | (b >> 3) as u16;
            let e = bins.entry(key).or_insert((0, 0, 0, 0));
            e.0 += 1;
            e.1 += r as u64;
            e.2 += g as u64;
            e.3 += b as u64;
        }
        i += stride;
    }
    if bins.is_empty() {
        return None;
    }
    // the bucket averages with their populations feed the real
    // M3 Score — its IndexMap alias is private, but the concrete
    // type (indexmap + ahash default hasher) is ours to build
    let avg = |s: u64, c: u32| (s / c as u64) as u8;
    let mut population =
        indexmap::IndexMap::<Argb, u32, BuildHasherDefault<ahash::AHasher>>::default();
    let mut samples = 0u32;
    for (count, sr, sg, sb) in bins.into_values() {
        let argb = Argb {
            alpha: 255,
            red: avg(sr, count),
            green: avg(sg, count),
            blue: avg(sb, count),
        };
        *population.entry(argb).or_insert(0) += count;
        samples += count;
    }
    // gray covers: too few samples survived the saturation
    // filter — keep the amber fallback
    if samples < 32 {
        return None;
    }
    let seed = *score::Score::score(&population, Some(1), None, Some(true)).first()?;
    let theme = ThemeBuilder::with_source(seed).variant(variant).build();
    let dark = theme.schemes.dark;
    let danger = blend::harmonize(
        Argb {
            alpha: 255,
            red: DANGER[0],
            green: DANGER[1],
            blue: DANGER[2],
        },
        seed,
    );
    Some(ArtworkPalette {
        accent: [dark.primary.red, dark.primary.green, dark.primary.blue],
        tertiary: [dark.tertiary.red, dark.tertiary.green, dark.tertiary.blue],
        danger: [danger.red, danger.green, danger.blue],
    })
}

/// the fixed theme for covers without a dominant color
pub fn amber_fallback() -> (slint::Color, Option<slint::Color>, slint::Color) {
    (
        slint::Color::from_rgb_u8(AMBER[0], AMBER[1], AMBER[2]),
        None,
        slint::Color::from_rgb_u8(DANGER[0], DANGER[1], DANGER[2]),
    )
}
