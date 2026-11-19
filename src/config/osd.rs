//! [osd]: the notification layer

use serde::{Deserialize, Serialize};

/// where a tile anchors on screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OsdAnchor {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
    Center,
}

/// tile background style
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OsdStyle {
    #[default]
    Translucent,
    Solid,
}

/// one notification kind switch: None = inherit the base [osd]
/// flag — context overrides only name what differs
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OsdFlags {
    /// "Playing" / "Paused" on play/pause transitions
    pub transport: Option<bool>,
    /// "Title — Artist" on track changes
    pub track: Option<bool>,
    /// "Title — Artist (+n)" on queue additions
    pub queue: Option<bool>,
    /// the volume OSD on control-channel volume changes
    pub volume: Option<bool>,
}

/// [osd]: the notification layer. Base flags gate each kind;
/// [osd.now-playing] and [osd.vu] override them per context
/// (only entries that differ from the base).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Osd {
    #[serde(default = "yes")]
    pub transport: bool,
    #[serde(default = "yes")]
    pub track: bool,
    #[serde(default = "yes")]
    pub queue: bool,
    #[serde(default = "yes")]
    pub volume: bool,
    /// toast anchor
    #[serde(default)]
    pub position: OsdAnchor,
    /// volume OSD anchor
    #[serde(default)]
    pub volume_position: OsdAnchor,
    /// tile style
    #[serde(default)]
    pub style: OsdStyle,
    /// how long a toast stays (ms)
    #[serde(default = "toast_duration")]
    pub toast_duration: u64,
    /// how long the volume OSD stays (ms)
    #[serde(default = "volume_duration")]
    pub volume_duration: u64,
    /// overrides while the now-playing overlay is open
    #[serde(default)]
    pub now_playing: OsdFlags,
    /// overrides while the VU faceplate is open
    #[serde(default)]
    pub vu: OsdFlags,
}

fn yes() -> bool {
    true
}

fn toast_duration() -> u64 {
    2600
}

fn volume_duration() -> u64 {
    1800
}

impl Default for Osd {
    // the same values as the serde defaults — a config without
    // an [osd] section must not fall back to "everything off"
    // (the derived Default would: bool -> false)
    fn default() -> Self {
        Self {
            transport: true,
            track: true,
            queue: true,
            volume: true,
            position: OsdAnchor::default(),
            volume_position: OsdAnchor::default(),
            style: OsdStyle::default(),
            toast_duration: 2600,
            volume_duration: 1800,
            now_playing: OsdFlags::default(),
            vu: OsdFlags::default(),
        }
    }
}

impl Osd {
    pub fn validate(&mut self) {
        if self.toast_duration == 0 {
            tracing::warn!("osd toast-duration must be > 0, using the default");
            self.toast_duration = 2600;
        }
        if self.volume_duration == 0 {
            tracing::warn!("osd volume-duration must be > 0, using the default");
            self.volume_duration = 1800;
        }
    }
}
