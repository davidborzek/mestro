//! mestro configuration.
//!
//! Precedence (low to high): compiled-in defaults, TOML, env
//! overrides. `$MESTRO_CONFIG` (or `$XDG_CONFIG_HOME/mestro`, falling
//! back to `$HOME/.config/mestro`) may be a FILE or a DIRECTORY:
//! a directory deep-merges every `*.toml` inside in name order —
//! the recommended thematic split is `mestro.toml` (window, clock,
//! music-assistant), `theme.toml` ([theme], [osd]),
//! `keyboard.toml` ([keyboard]) and `vu.toml` ([vu], [vu.*]);
//! collisions warn and the later file wins. A missing config is
//! not an error. Secrets (MESTRO_TOKEN) are env-only and never go
//! into the file. The in-app settings page (later) will write
//! this same file.
//!
//! The code mirrors that split: one module per section
//! (window.rs, music.rs, theme.rs, osd.rs, clock.rs,
//! background.rs, ui.rs, vu.rs), this file owns the Config
//! struct, loading and validation.

mod background;
mod clock;
mod display;
mod hub;
pub mod lenient;
mod music;
mod nowplaying;
mod osd;
mod remote;
mod system;
mod theme;
mod ui;
mod vu;
mod wifi;
mod window;

pub use background::Background;
pub use clock::{
    BackgroundSource, Clock, StandbyBackground, BUILTIN_STYLES as CLOCK_BUILTIN_STYLES,
};
pub use display::Display;
pub use hub::{Hub, HubEntry, HubLayout};
pub use music::MusicAssistant;
pub use nowplaying::{InlineVu, NowPlaying, NpFont, NpLayout, QualityPart, TransportControl};
pub use osd::{Osd, OsdAnchor, OsdFlags, OsdStyle};
pub use remote::Remote;
pub use system::System;
pub use theme::{Theme, ThemePalette, ThemeVariant};
pub use ui::{RailStyle, Ui, OSK_LAYOUTS};
pub use vu::{
    AnalyzerOptions, BarsOptions, CreamOptions, MetadataField, NightOptions, RoundOptions, Vu,
    VuMode, WallLightOptions, WallOptions, BUILTIN_SKINS as VU_BUILTIN_SKINS,
};
pub use wifi::{ApNet, Wifi, DEFAULT_AP_ADDRESS};
pub use window::{parse_size, Window};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub window: Window,
    pub display: Display,
    pub theme: Theme,
    /// [keyboard]: key name -> control command (src/remote/control.rs);
    /// overrides the built-in defaults, e.g. right = "cycle-forward"
    #[serde(default)]
    pub keyboard: std::collections::BTreeMap<String, String>,
    /// [osd]: what the notification layer shows and where
    #[serde(default)]
    pub osd: Osd,
    pub clock: Clock,
    pub background: Background,
    pub ui: Ui,
    #[serde(default)]
    pub hub: Hub,
    #[serde(rename = "now-playing", default)]
    pub now_playing: NowPlaying,
    pub vu: Vu,
    #[serde(rename = "music-assistant")]
    pub music_assistant: MusicAssistant,
    /// [remote]: the embedded web endpoint (setup wizard + WS)
    #[serde(default)]
    pub remote: Remote,
    /// [wifi]: the setup hotspot's device-level knobs (config-file
    /// only, feature `wifi-setup` on the kiosk)
    #[serde(default)]
    pub wifi: Wifi,
    /// [system]: the timezone (a device setting) and the files it is
    /// applied through (feature `timezone` on the kiosk)
    #[serde(default)]
    pub system: System,
}

/// load defaults, then the TOML file if present, then env overrides.
/// Broken settings never stop the kiosk: an unknown or invalid key is
/// dropped, an unreadable file falls back to the defaults — and both
/// are recorded (see [`problems`]) so the UI can say what was
/// ignored instead of failing silently.
pub fn load(explicit: Option<&std::path::Path>) -> Result<Config, String> {
    Ok(load_parts(explicit)?.0)
}

thread_local! {
    static PROBLEMS: std::cell::RefCell<Vec<lenient::Problem>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// what the last load ignored (file + key + reason), empty when the
/// config was clean
pub fn problems() -> Vec<lenient::Problem> {
    PROBLEMS.with(|p| p.borrow().clone())
}

fn record(problems: impl IntoIterator<Item = lenient::Problem>) {
    PROBLEMS.with(|p| p.borrow_mut().extend(problems));
}

/// a file that does not even parse as TOML: keep going on the
/// defaults, remember why
fn tree_or_problem(result: Result<toml::Value, String>, source: &str) -> toml::Value {
    result.unwrap_or_else(|e| {
        tracing::error!("config: {e} — using the defaults instead");
        record([lenient::Problem {
            key: source.to_string(),
            reason: e,
        }]);
        toml::Value::Table(toml::map::Map::new())
    })
}

/// load + the merge inputs: the settings handle re-merges these
/// after every UI edit (base ⊕ overlay → effective). The fourth
/// element is the resolved base path (None = defaults only) —
/// the settings page names the file its edits override.
pub fn load_parts(
    explicit: Option<&std::path::Path>,
) -> Result<
    (
        Config,
        toml::Value,
        crate::settings::Overlay,
        Option<std::path::PathBuf>,
    ),
    String,
> {
    PROBLEMS.with(|p| p.borrow_mut().clear());
    let base = explicit.map(Into::into).or_else(file_path);
    let base_path = base.clone();
    let base_tree = match base {
        // a directory: every *.toml inside deep-merges in name
        // order — config.toml is one of them, never the only one
        // (a lone config.toml used to hide its siblings silently)
        Some(p) if p.is_dir() => tree_or_problem(tree_from_dir(&p), "config file"),
        Some(p) if p.exists() => tree_or_problem(tree_from_file(&p), "config file"),
        _ => toml::Value::Table(toml::map::Map::new()),
    };
    // the settings overlay (docs/configuration.md): a sparse state-dir
    // tree over the read-only base; a corrupt overlay warns and
    // drops, the device still boots
    let overlay = crate::settings::Overlay::load();
    let mut merged = base_tree.clone();
    if let Some(tree) = &overlay.tree {
        crate::settings::merge(&mut merged, tree);
    }
    // per-key leniency: a bad key is cut, everything else stays —
    // a single stale key no longer resets the whole device
    let mut cfg: Config = match lenient::parse::<Config>(merged) {
        Ok((cfg, problems)) => {
            for p in &problems {
                tracing::warn!("config: ignoring {} ({})", p.key, p.reason);
            }
            record(problems);
            cfg
        }
        Err(e) => {
            tracing::error!("config: unusable ({e}) — using the defaults instead");
            record([lenient::Problem {
                key: "config".into(),
                reason: e,
            }]);
            Config::default()
        }
    };

    // the MESTRO_* env overlays (docs/configuration.md): env beats
    // the file and the device settings
    apply_env(&mut cfg);

    cfg.theme.validate();
    cfg.osd.validate();
    cfg.now_playing.validate();
    cfg.hub.validate();

    let wifi_problems = cfg.wifi.validate();
    for problem in &wifi_problems {
        tracing::warn!("config: ignoring {} ({})", problem.key, problem.reason);
    }
    record(wifi_problems);

    if let Err(e) = parse_size(&cfg.window.size) {
        tracing::warn!("config: ignoring window.size ({e})");
        record([lenient::Problem {
            key: "window.size".into(),
            reason: e,
        }]);
        cfg.window.size = Window::default().size;
    }

    Ok((cfg, base_tree, overlay, base_path))
}

impl Config {
    /// the section validators (fonts clamp, duplicates drop,
    /// ...). Also the settings handle's post-merge step
    pub fn validate(&mut self) {
        self.theme.validate();
        self.osd.validate();
        self.now_playing.validate();
        self.hub.validate();
        self.display.validate();
        self.clock.validate();
        // wifi problems were reported at load; a settings merge
        // cannot add new ones, so the fallbacks run quietly here
        let _ = self.wifi.validate();
    }
}

/// the MESTRO_* env overlays: env beats the config file and the
/// device settings (docs/configuration.md). A bad value is dropped
/// with a warning — the file's value stays in force
fn apply_env(cfg: &mut Config) {
    // a built-in name or a [clock.skins] entry; validate() drops
    // unknown names back to digital
    if let Ok(style) = std::env::var("MESTRO_CLOCK") {
        cfg.clock.style = style;
    }
    if let Ok(name) = std::env::var("MESTRO_PLAYER") {
        cfg.music_assistant.player = name;
    }
    if let Ok(url) = std::env::var("MESTRO_MA_URL") {
        cfg.music_assistant.url = url;
    }
    if let Ok(port) = std::env::var("MESTRO_REMOTE_PORT") {
        match port.parse::<u16>() {
            Ok(p) => cfg.remote.port = p,
            Err(_) => {
                tracing::warn!("config: ignoring MESTRO_REMOTE_PORT ({port} is not a port number)")
            }
        }
    }
}

/// $MESTRO_CONFIG is handled by clap (--config env; file or
/// directory); this is the XDG fallback. None when HOME-less
fn file_path() -> Option<PathBuf> {
    let dir = match std::env::var("XDG_CONFIG_HOME") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
    };
    Some(dir.join("mestro"))
}

fn tree_from_file(path: &Path) -> Result<toml::Value, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    toml::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// one *.toml per theme, deep-merged in name order; collisions
/// warn and the later file wins
fn tree_from_dir(dir: &Path) -> Result<toml::Value, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let mut merged = toml::Value::Table(toml::map::Map::new());
    for f in &files {
        let raw = std::fs::read_to_string(f).map_err(|e| format!("read {}: {e}", f.display()))?;
        let v: toml::Value =
            toml::from_str(&raw).map_err(|e| format!("parse {}: {e}", f.display()))?;
        merge(&mut merged, v, f.to_string_lossy().as_ref(), "");
    }
    Ok(merged)
}

/// deep table merge: tables recurse, anything else overwrites
/// (with a warning when a previous file set the same key)
fn merge(dst: &mut toml::Value, src: toml::Value, file: &str, path: &str) {
    let (Some(da), Some(sa)) = (dst.as_table_mut(), src.as_table()) else {
        if !dst.is_table() {
            tracing::warn!("config: {path} in {file} overrides an earlier value");
            *dst = src;
        }
        return;
    };
    for (k, v) in sa {
        let key = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        match da.get_mut(k) {
            Some(old) => merge(old, v.clone(), file, &key),
            None => {
                da.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the example documents every key — it must parse cleanly,
    /// or the docs and the structs have drifted apart
    #[test]
    fn example_config_parses() {
        let raw = include_str!("../../mestro.example.toml");
        let cfg: Config = toml::from_str(raw).expect("example config must parse");
        // sanity: the defaults survived the roundtrip
        assert_eq!(cfg.now_playing.inline_vu, super::nowplaying::InlineVu::Led);
        assert_eq!(cfg.now_playing.font.title_size, 24.0);
        assert_eq!(cfg.osd.toast_duration, 2600);
        assert_eq!(cfg.display.standby_brightness, 15);
        assert_eq!(
            cfg.clock.background.folder,
            PathBuf::from("/data/backgrounds")
        );
    }

    /// a config written before the image paths became build settings
    /// still loads: the stale keys are reported and dropped, the rest stays
    #[test]
    fn stale_image_path_keys_are_dropped_with_a_warning() {
        let tree: toml::Value = toml::from_str(
            "[system]\ntimezone = \"Europe/Berlin\"\nenv-file = \"/data/mestro.env\"\n\
             zoneinfo-dir = \"/usr/share/zoneinfo\"\nlocaltime = \"/etc/localtime\"\n",
        )
        .unwrap();
        let (cfg, problems) = lenient::parse::<Config>(tree).unwrap();
        assert_eq!(
            cfg.system.timezone, "Europe/Berlin",
            "the real setting stays"
        );
        let mut keys: Vec<_> = problems.iter().map(|p| p.key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["system.env-file", "system.localtime", "system.zoneinfo-dir"]
        );
    }

    #[test]
    fn stale_wifi_path_keys_are_dropped_with_a_warning() {
        let tree: toml::Value = toml::from_str(
            "[wifi]\ninterface = \"wlan1\"\nctrl-dir = \"/run/wpa_supplicant\"\n\
             conf = \"/data/wpa_supplicant.conf\"\ncountry = \"DE\"\n",
        )
        .unwrap();
        let (cfg, problems) = lenient::parse::<Config>(tree).unwrap();
        assert_eq!(cfg.wifi.interface, "wlan1", "the real settings stay");
        assert_eq!(cfg.wifi.country, "DE");
        let mut keys: Vec<_> = problems.iter().map(|p| p.key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["wifi.conf", "wifi.ctrl-dir"]);
    }

    #[test]
    fn remote_port_env_beats_the_file_and_garbage_keeps_it() {
        // precedence like the other MESTRO_* overlays: env wins
        let mut cfg: Config = toml::from_str("[remote]\nport = 9000\n").unwrap();
        std::env::set_var("MESTRO_REMOTE_PORT", "80");
        apply_env(&mut cfg);
        std::env::remove_var("MESTRO_REMOTE_PORT");
        assert_eq!(cfg.remote.port, 80);
        // a value that is not a port number is dropped with a
        // warning — the file's value stays
        let mut cfg: Config = toml::from_str("[remote]\nport = 9000\n").unwrap();
        std::env::set_var("MESTRO_REMOTE_PORT", "not-a-port");
        apply_env(&mut cfg);
        std::env::remove_var("MESTRO_REMOTE_PORT");
        assert_eq!(cfg.remote.port, 9000);
    }
}
