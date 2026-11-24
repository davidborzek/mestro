//! [vu]: meter source, the faceplate cycle, per-preset options

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Vu {
    /// VU meter source: shmem reads squeezelite's visualizer ring
    /// (needs a squeezelite built with -DVISEXPORT and started with
    /// -v; the file lands at /dev/shm/squeezelite-<MAC>), mock
    /// synthesizes levels for UI work, off skips it
    pub mode: VuMode,
    /// 0 VU alignment in dBFS. Classic meters assume ~-20 dBFS
    /// program level; modern loud masters sit around -14, which
    /// centers the needle around 0 VU
    pub reference_dbfs: f64,
    /// squeezelite names its shm file <prefix>-<MAC> in /dev/shm
    /// (fixed by its output_vis.c); mestro resolves it by prefix
    pub shm_prefix: String,
    /// named registry of runtime skin paths (.slint files
    /// implementing the VuSkin contract — README documents it);
    /// list entries in `cycle` to show them
    #[serde(default)]
    pub skins: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// the faceplate views, in order: preset names (night, cream,
    /// round, wall, wall-light, arc, orbit, modern) and/or [vu.skins]
    /// names; a tap on the plate cycles through, the first entry
    /// is the start view. Defaults to every built-in preset; a
    /// single entry pins one view
    #[serde(default = "default_cycle")]
    pub cycle: Vec<String>,
    /// free-form options for the runtime skin, pushed onto the
    /// embedded component as properties (unknown names on the
    /// skin side are ignored)
    #[serde(rename = "skin-options", default)]
    pub skin_options: std::collections::BTreeMap<String, toml::Value>,
    /// per-preset option tables; only the active preset's is read
    #[serde(default)]
    pub night: NightOptions,
    #[serde(default)]
    pub cream: CreamOptions,
    #[serde(default)]
    pub round: RoundOptions,
    #[serde(default)]
    pub wall: WallOptions,
    #[serde(rename = "wall-light", default)]
    pub wall_light: WallLightOptions,
    /// the analyzers (spectrum, bars)
    #[serde(default)]
    pub spectrum: AnalyzerOptions,
    #[serde(default)]
    pub bars: BarsOptions,
}

/// the built-in faceplates, in design order — the default
/// cycle and the schema's option base ([vu.skins] names append)
pub const BUILTIN_SKINS: &[&str] = &[
    "night",
    "cream",
    "round",
    "wall",
    "wall-light",
    "arc",
    "orbit",
    "modern",
    "spectrum",
    "bars",
];

/// the default cycle: every built-in skin, in design order
fn default_cycle() -> Vec<String> {
    BUILTIN_SKINS.iter().map(|s| s.to_string()).collect()
}

impl Default for Vu {
    fn default() -> Self {
        Self {
            mode: VuMode::default(),
            reference_dbfs: -14.0,
            shm_prefix: "squeezelite".into(),
            skins: Default::default(),
            cycle: default_cycle(),
            skin_options: Default::default(),
            night: Default::default(),
            cream: Default::default(),
            round: Default::default(),
            wall: Default::default(),
            wall_light: Default::default(),
            spectrum: Default::default(),
            bars: Default::default(),
        }
    }
}

/// spectrum/bars analyzer options: info lines under the
/// analyzer and a pair of small LED level meters, both optional
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnalyzerOptions {
    /// info lines under the analyzer (same fields as the VU
    /// presets); an empty list hides the block
    #[serde(default)]
    pub info: Vec<MetadataField>,
    /// full-width horizontal LED level meters (L/R) under the
    /// analyzer — opt-in
    #[serde(default)]
    pub meters: bool,
    /// the LED matrix renders a vertical gradient (primary at
    /// the bottom → tertiary at the top, meters blend toward
    /// danger) instead of flat two-tone zones
    #[serde(default)]
    pub gradient: bool,
}

/// bars options — same knobs as the analyzer, but the bar
/// gradient is the skin's built-in two-tone look and stays on
/// unless the config flattens it
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarsOptions {
    pub info: Vec<MetadataField>,
    pub meters: bool,
    pub gradient: bool,
}

impl Default for BarsOptions {
    fn default() -> Self {
        Self {
            info: Vec::new(),
            meters: false,
            gradient: true,
        }
    }
}

/// night preset options
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NightOptions {
    /// info lines under the meters, one field per line in a
    /// fixed hierarchy (16px bold, 13px dim, 12px muted); an
    /// empty list hides the block
    #[serde(default)]
    pub info: Vec<MetadataField>,
}

/// cream preset options
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CreamOptions {
    #[serde(default)]
    pub info: Vec<MetadataField>,
    /// warm backlight wash over the dark plate: 0 (off) to 1
    #[serde(default)]
    pub glow: f64,
}

/// round preset options
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoundOptions {
    #[serde(default)]
    pub info: Vec<MetadataField>,
    /// warm backlight wash over the black plate: 0 (off) to 1
    #[serde(default)]
    pub glow: f64,
}

/// wall preset options (none — the wall is meters only)
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WallOptions {}

/// wall-light preset options
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WallLightOptions {
    /// backlight intensity of the orange plate glow, 0 (plain
    /// cream) to 1 (full hot orange)
    pub glow: f64,
}

impl Default for WallLightOptions {
    fn default() -> Self {
        Self { glow: 1.0 }
    }
}

/// one selectable info line
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataField {
    Title,
    Artist,
    Codec,
    Quality,
    /// elapsed time (mm:ss)
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VuMode {
    Off,
    #[default]
    Shmem,
    Mock,
}
