//! The abstract control channel: every input source speaks the
//! same command vocabulary. Today: the touch UI callbacks and
//! the keyboard FocusScope; tomorrow: a WebSocket/IPC server, a
//! USB remote (evdev), an IR receiver — they all parse into
//! [`Command`] and go through the same dispatch in main.rs.
//!
//! The wire format is plain kebab-case strings (`"cycle-forward"`,
//! `"back"`, `"play-pause"`, ...) so future transports can stay
//! dumb and testable from a terminal.

use std::collections::BTreeMap;

/// one atomic user intent
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// next VU view in the cycle
    CycleForward,
    /// previous VU view
    CycleBack,
    /// one level back in the navigation stack
    Back,
    /// straight to the hub
    Home,
    PlayPause,
    NextTrack,
    PrevTrack,
    VolumeUp,
    /// set one settings key (dotted path, string value — the
    /// settings-UI vocabulary, shared with future IPC/web
    /// transports): `setting <key> <value>`
    SetSetting {
        key: String,
        value: toml::Value,
    },
    // remove one overlay key — the config default returns
    ResetSetting {
        key: String,
    },
    /// drop every overlay key at once — config.toml takes over
    ResetAllSettings,
    VolumeDown,
    /// open the now-playing layer
    NowPlaying,
    /// open the queue (on top of the now-playing layer)
    Queue,
    /// open the VU faceplate (on top of the now-playing layer)
    Vu,
    /// power the player off — the standby face
    StandbyOn,
    /// wake the player back up
    StandbyOff,
    /// flip between standby and awake
    StandbyToggle,
}

impl Command {
    /// settings writes ride their own ops — the pure control
    /// transports (ctl socket, web `ctl` op) reject them
    pub fn is_settings(&self) -> bool {
        matches!(
            self,
            Self::SetSetting { .. } | Self::ResetSetting { .. } | Self::ResetAllSettings
        )
    }

    /// parse the kebab-case wire form
    pub fn parse(s: &str) -> Option<Self> {
        if s == "standby-on" {
            return Some(Self::StandbyOn);
        }
        if s == "standby-off" {
            return Some(Self::StandbyOff);
        }
        if s == "standby-toggle" {
            return Some(Self::StandbyToggle);
        }
        if s == "now-playing" {
            return Some(Self::NowPlaying);
        }
        if s == "queue" {
            return Some(Self::Queue);
        }
        if s == "vu" {
            return Some(Self::Vu);
        }
        if s == "reset-all-settings" {
            return Some(Self::ResetAllSettings);
        }
        if let Some(rest) = s.strip_prefix("reset-setting ") {
            let key = rest.trim();
            if key.is_empty() {
                return None;
            }
            return Some(Self::ResetSetting {
                key: key.to_string(),
            });
        }
        if let Some(rest) = s.strip_prefix("setting ") {
            let (key, value) = rest.split_once(' ')?;
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                return None;
            }
            // the text protocol guesses the value type exactly once,
            // at the wire — everything inside is typed
            let value = if let Ok(b) = value.parse::<bool>() {
                toml::Value::Boolean(b)
            } else if let Ok(i) = value.parse::<i64>() {
                toml::Value::Integer(i)
            } else if let Ok(f) = value.parse::<f64>() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(value.to_string())
            };
            return Some(Self::SetSetting {
                key: key.to_string(),
                value,
            });
        }
        Some(match s {
            "cycle-forward" => Self::CycleForward,
            "cycle-back" => Self::CycleBack,
            "back" => Self::Back,
            "home" => Self::Home,
            "play-pause" => Self::PlayPause,
            "next-track" => Self::NextTrack,
            "prev-track" => Self::PrevTrack,
            "volume-up" => Self::VolumeUp,
            "volume-down" => Self::VolumeDown,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CycleForward => "cycle-forward",
            Self::CycleBack => "cycle-back",
            Self::Back => "back",
            Self::Home => "home",
            Self::PlayPause => "play-pause",
            Self::NextTrack => "next-track",
            Self::PrevTrack => "prev-track",
            Self::VolumeUp => "volume-up",
            Self::VolumeDown => "volume-down",
            Self::NowPlaying => "now-playing",
            Self::Queue => "queue",
            Self::Vu => "vu",
            Self::StandbyOn => "standby-on",
            Self::StandbyOff => "standby-off",
            Self::StandbyToggle => "standby-toggle",
            Self::SetSetting { .. } => "setting",
            Self::ResetSetting { .. } => "reset-setting",
            Self::ResetAllSettings => "reset-all-settings",
        }
    }
}

/// a key NAME (config-speak: "left", "escape", "f1", or a
/// single character) to the KeyEvent.text encoding slint
/// delivers — the special-key codepoints come from slint's
/// own key table (internal/common/key_codes.rs)
pub fn key_text(name: &str) -> Option<String> {
    Some(
        match name.to_ascii_lowercase().as_str() {
            "backspace" => "\u{0008}",
            "tab" => "\u{0009}",
            "enter" | "return" => "\u{000a}",
            "escape" => "\u{001b}",
            "delete" => "\u{007f}",
            "space" => "\u{0020}",
            "up" => "\u{F700}",
            "down" => "\u{F701}",
            "left" => "\u{F702}",
            "right" => "\u{F703}",
            "f1" => "\u{F704}",
            "f2" => "\u{F705}",
            "f3" => "\u{F706}",
            "f4" => "\u{F707}",
            "f5" => "\u{F708}",
            "f6" => "\u{F709}",
            "f7" => "\u{F70A}",
            "f8" => "\u{F70B}",
            "f9" => "\u{F70C}",
            "f10" => "\u{F70D}",
            "f11" => "\u{F70E}",
            "f12" => "\u{F70F}",
            "home" => "\u{F729}",
            "end" => "\u{F72B}",
            "pageup" => "\u{F72C}",
            "pagedown" => "\u{F72D}",
            // printable keys: the name is the text
            c if c.chars().count() == 1 => c,
            _ => return None,
        }
        .into(),
    )
}

/// the default key map ([keyboard] overrides entries).
/// Optimized for USB HID remotes, which register as plain
/// keyboards (arrows / OK=Enter / Back=Escape / Home=Home),
/// with keyboard fallbacks borrowed from the conventions
/// people already know: VLC (space play, n/p tracks) and
/// Kodi (+/- volume, PageUp/PageDown skip)
pub fn default_keymap() -> &'static [(&'static str, &'static str)] {
    &[
        ("right", "cycle-forward"),
        ("down", "cycle-forward"),
        ("left", "cycle-back"),
        ("up", "cycle-back"),
        ("escape", "back"),
        ("backspace", "back"),
        ("home", "home"),
        ("h", "home"),
        ("space", "play-pause"),
        ("enter", "play-pause"),
        ("n", "next-track"),
        ("pagedown", "next-track"),
        ("p", "prev-track"),
        ("pageup", "prev-track"),
        ("f12", "volume-up"),
        ("+", "volume-up"),
        ("f11", "volume-down"),
        ("-", "volume-down"),
    ]
}

/// the compiled map: KeyEvent.text -> Command. Config entries
/// override the defaults; invalid names/commands warn and drop
pub fn keymap(overrides: &BTreeMap<String, String>) -> BTreeMap<String, Command> {
    let mut map: BTreeMap<String, Command> = default_keymap()
        .iter()
        .filter_map(|(k, c)| Some((key_text(k)?, Command::parse(c)?)))
        .collect();
    for (k, c) in overrides {
        let (Some(text), Some(cmd)) = (key_text(k), Command::parse(c)) else {
            tracing::warn!("keyboard: invalid binding {k:?} = {c:?}, ignoring");
            continue;
        };
        map.insert(text, cmd);
    }
    map
}
