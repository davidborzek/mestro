//! The abstract control channel: every input source speaks the
//! same command vocabulary. Today: the touch UI callbacks, the
//! keyboard, the web `ctl` op and the ctl socket; tomorrow: a USB
//! remote (evdev), an IR receiver — they all parse into
//! [`Command`] and go through the same dispatch in the binary.
//!
//! The wire format is plain kebab-case strings (`"cycle-forward"`,
//! `"back"`, `"play-pause"`, ...) so future transports can stay
//! dumb and testable from a terminal.

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
    /// open the VU faceplate on the cycle's skin at this index
    /// (`vu-select:<index>`, 0-based, as in the state frame's
    /// `view.vu_skins`)
    VuSelect(usize),
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
        if let Some(index) = s.strip_prefix("vu-select:") {
            if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            return index.parse().ok().map(Self::VuSelect);
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
            Self::VuSelect(_) => "vu-select",
            Self::StandbyOn => "standby-on",
            Self::StandbyOff => "standby-off",
            Self::StandbyToggle => "standby-toggle",
            Self::SetSetting { .. } => "setting",
            Self::ResetSetting { .. } => "reset-setting",
            Self::ResetAllSettings => "reset-all-settings",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vu_select_parses_the_index() {
        assert_eq!(Command::parse("vu-select:2"), Some(Command::VuSelect(2)));
        assert_eq!(Command::parse("vu-select:0"), Some(Command::VuSelect(0)));
        assert_eq!(Command::VuSelect(2).as_str(), "vu-select");
    }

    #[test]
    fn vu_select_rejects_malformed_indices() {
        for bad in [
            "vu-select:",
            "vu-select:x",
            "vu-select:-1",
            "vu-select:+1",
            "vu-select: 1",
            "vu-select:1.5",
            "vu-select:99999999999999999999",
            "vu-select",
        ] {
            assert_eq!(Command::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn vu_select_is_a_control_command() {
        assert!(!Command::VuSelect(1).is_settings());
    }
}
