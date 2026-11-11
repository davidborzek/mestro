//! [theme]

use serde::{Deserialize, Serialize};

use crate::ui::palette::Variant;

/// named palette: amber (gruvbox-toned) or cover (the accent
/// follows the artwork's dominant color)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePalette {
    #[default]
    Amber,
    Cover,
}

/// config-domain wrapper for the M3 Variant (which has no
/// serde/Debug impls) — serializes through palette::VARIANTS
#[derive(Clone, PartialEq, Eq)]
pub struct ThemeVariant(Variant);

impl ThemeVariant {
    pub fn raw(&self) -> Variant {
        self.0.clone()
    }
}

impl std::fmt::Debug for ThemeVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(crate::ui::palette::variant_as_str(self.0.clone()))
    }
}

impl Default for ThemeVariant {
    fn default() -> Self {
        Self(Variant::TonalSpot)
    }
}

impl<'de> Deserialize<'de> for ThemeVariant {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        crate::ui::palette::variant_from_str(&s)
            .map(ThemeVariant)
            .ok_or_else(|| serde::de::Error::unknown_variant(&s, &crate::ui::palette::VARIANTS))
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub palette: ThemePalette,
    /// the M3 scheme variant for palette = "cover": how far the
    /// tonalized colors may drift from the seed
    /// (tonal-spot .. vibrant; see src/palette.rs VARIANTS)
    #[serde(default)]
    pub variant: ThemeVariant,
}

impl Theme {
    pub fn validate(&mut self) {
        // the enums already enforce the vocabularies
    }
}
