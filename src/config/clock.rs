//! [clock]: the standby clock face

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// the built-in faces; anything else in `style` names a
/// [clock.skins] entry
pub const BUILTIN_STYLES: &[&str] = &["digital", "analog"];

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clock {
    /// "digital", "analog" or a [clock.skins] name
    pub style: String,
    /// custom faces: name -> .slint file implementing the
    /// StandbySkin contract (docs/standby.md)
    pub skins: BTreeMap<String, PathBuf>,
    /// what sits behind the clock
    pub background: StandbyBackground,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            style: "digital".into(),
            skins: BTreeMap::new(),
            background: StandbyBackground::default(),
        }
    }
}

/// [clock.background]: the standby backdrop behind the clock
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StandbyBackground {
    pub source: BackgroundSource,
    /// darkening over the image, 0 (image as is) ..= 1 (black)
    pub dim: f32,
    /// blurred instead of sharp (the cover source has a blurred
    /// variant ready; folder images blur on load)
    pub blur: bool,
    /// folder source: image directory (jpg/png), shown in random
    /// order
    pub folder: PathBuf,
    /// folder source: minutes per image (0 = never change)
    pub interval: u32,
}

impl Default for StandbyBackground {
    fn default() -> Self {
        Self {
            source: BackgroundSource::Black,
            dim: 0.6,
            blur: false,
            folder: PathBuf::from("/data/backgrounds"),
            interval: 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundSource {
    /// plain black (darkest, the night setting)
    #[default]
    Black,
    /// the last played cover
    Cover,
    /// a folder of own pictures, slideshow
    Folder,
}

impl BackgroundSource {
    pub const ALL: [Self; 3] = [Self::Black, Self::Cover, Self::Folder];

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Cover => "cover",
            Self::Folder => "folder",
        }
    }
}

impl Clock {
    pub fn is_analog(&self) -> bool {
        self.style == "analog"
    }

    /// the custom skin `style` names, if any
    pub fn skin(&self) -> Option<&str> {
        (!BUILTIN_STYLES.contains(&self.style.as_str())).then_some(self.style.as_str())
    }

    /// every selectable style: built-ins, then registered skins
    /// (a skin named like a built-in does not repeat it)
    pub fn styles(&self) -> Vec<String> {
        let mut styles: Vec<String> = BUILTIN_STYLES.iter().map(|s| s.to_string()).collect();
        for name in self.skins.keys() {
            if !styles.contains(name) {
                styles.push(name.clone());
            }
        }
        styles
    }

    /// an unknown style falls back to the digital face (a skin
    /// that was renamed or removed must not leave standby blank)
    pub fn validate(&mut self) {
        self.background.dim = self.background.dim.clamp(0.0, 1.0);
        let known =
            BUILTIN_STYLES.contains(&self.style.as_str()) || self.skins.contains_key(&self.style);
        if !known {
            tracing::warn!("clock: unknown style {:?}, using digital", self.style);
            self.style = "digital".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_skins_follow_the_builtins() {
        let mut clock = Clock::default();
        clock.skins.insert("mine".into(), "/tmp/mine.slint".into());
        clock
            .skins
            .insert("analog".into(), "/tmp/analog.slint".into());
        assert_eq!(clock.styles(), ["digital", "analog", "mine"]);
        clock.style = "mine".into();
        clock.validate();
        assert_eq!(clock.skin(), Some("mine"));
    }

    #[test]
    fn background_defaults_to_black_and_parses() {
        assert_eq!(Clock::default().background.source, BackgroundSource::Black);
        let clock: Clock =
            toml::from_str("[background]\nsource = \"folder\"\ndim = 1.5\ninterval = 3\n").unwrap();
        let mut clock = clock;
        clock.validate();
        assert_eq!(clock.background.source, BackgroundSource::Folder);
        assert_eq!(clock.background.dim, 1.0);
        assert_eq!(clock.background.interval, 3);
    }

    #[test]
    fn unknown_style_falls_back_to_digital() {
        let mut clock = Clock {
            style: "nope".into(),
            ..Clock::default()
        };
        clock.validate();
        assert_eq!(clock.style, "digital");
    }
}
