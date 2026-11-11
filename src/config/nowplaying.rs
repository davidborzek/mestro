//! [now-playing]: the now-playing overlay layout

use serde::{Deserialize, Serialize};

impl TransportControl {
    // the settings vocabulary (see QualityPart::ALL)
    pub const ALL: [TransportControl; 5] = [
        TransportControl::Shuffle,
        TransportControl::Previous,
        TransportControl::PlayPause,
        TransportControl::Next,
        TransportControl::Repeat,
    ];

    pub fn wire_name(&self) -> &'static str {
        match self {
            TransportControl::Shuffle => "shuffle",
            TransportControl::Previous => "previous",
            TransportControl::PlayPause => "play-pause",
            TransportControl::Next => "next",
            TransportControl::Repeat => "repeat",
        }
    }
}
/// display order. The play/pause button keeps its accent styling
/// wherever it sits in the list
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportControl {
    /// the queue shuffle toggle
    Shuffle,
    /// previous track
    Previous,
    /// play/pause/loading — the round accent button
    PlayPause,
    /// next track
    Next,
    /// repeat: off → all → one per tap
    Repeat,
}

/// the now-playing layout
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NpLayout {
    /// square cover left, metadata column right (default)
    #[default]
    Side,
    /// sharp cover edge to edge, metadata cluster over a bottom
    /// scrim
    Fullbleed,
}

/// the inline VU style under the track metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InlineVu {
    /// none
    Off,
    /// segmented LED strips (default)
    #[default]
    Led,
    /// two thin minimal bars
    Thin,
    /// the spectrum as a dim bar matrix behind the whole view
    /// (the eversolo background visualizer)
    Matrix,
}

/// which stream quality parts to show, in display order —
/// codec is the badge, the rest assemble into the detail line
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityPart {
    /// the codec badge (FLAC, MP3, …)
    Codec,
    /// 44.1 kHz
    Rate,
    /// 16 bit
    Depth,
    /// kbps — only when the stream reports one (lossless usually
    /// does not)
    Bitrate,
    /// 2 ch
    Channels,
}

fn default_quality() -> Vec<QualityPart> {
    vec![QualityPart::Codec, QualityPart::Rate, QualityPart::Depth]
}

impl QualityPart {
    // the settings vocabulary: wire name + canonical order —
    // the settings UI, the text protocol and a future web client
    // all speak these
    pub const ALL: [QualityPart; 5] = [
        QualityPart::Codec,
        QualityPart::Rate,
        QualityPart::Depth,
        QualityPart::Bitrate,
        QualityPart::Channels,
    ];

    pub fn wire_name(&self) -> &'static str {
        match self {
            QualityPart::Codec => "codec",
            QualityPart::Rate => "rate",
            QualityPart::Depth => "depth",
            QualityPart::Bitrate => "bitrate",
            QualityPart::Channels => "channels",
        }
    }
}

/// [now-playing.font]: per-line sizes (px) and weights — defaults
/// are the built-in look
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct NpFont {
    #[serde(default = "title_size")]
    pub title_size: f32,
    #[serde(default = "bold")]
    pub title_weight: i32,
    #[serde(default = "artist_size")]
    pub artist_size: f32,
    #[serde(default = "regular")]
    pub artist_weight: i32,
    #[serde(default = "album_size")]
    pub album_size: f32,
    #[serde(default = "regular")]
    pub album_weight: i32,
    #[serde(default = "quality_size")]
    pub quality_size: f32,
    #[serde(default = "regular")]
    pub quality_weight: i32,
}

impl Default for NpFont {
    fn default() -> Self {
        Self {
            title_size: title_size(),
            title_weight: bold(),
            artist_size: artist_size(),
            artist_weight: regular(),
            album_size: album_size(),
            album_weight: regular(),
            quality_size: quality_size(),
            quality_weight: regular(),
        }
    }
}

fn title_size() -> f32 {
    24.0
}

fn artist_size() -> f32 {
    18.0
}

fn album_size() -> f32 {
    14.0
}

fn quality_size() -> f32 {
    12.0
}

fn bold() -> i32 {
    700
}

fn regular() -> i32 {
    400
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct NowPlaying {
    /// [now-playing] layout (see NpLayout)
    #[serde(default)]
    pub layout: NpLayout,
    /// which inline VU to show under the metadata (see InlineVu)
    pub inline_vu: InlineVu,
    /// which quality parts to show, in display order; empty =
    /// none
    #[serde(default = "default_quality")]
    pub quality: Vec<QualityPart>,
    /// which transport buttons the row shows, in display order;
    /// an empty list hides the row entirely
    #[serde(default = "default_controls")]
    pub controls: Vec<TransportControl>,
    /// the metadata stack's per-line fonts
    pub font: NpFont,
}

fn default_controls() -> Vec<TransportControl> {
    use TransportControl::*;
    vec![Shuffle, Previous, PlayPause, Next, Repeat]
}

impl Default for NowPlaying {
    fn default() -> Self {
        Self {
            layout: NpLayout::default(),
            inline_vu: InlineVu::default(),
            quality: default_quality(),
            controls: default_controls(),
            font: NpFont::default(),
        }
    }
}

impl NowPlaying {
    pub fn validate(&mut self) {
        // duplicates would render the same part twice
        let before = self.quality.len();
        let mut seen = std::collections::HashSet::new();
        self.quality.retain(|p| seen.insert(*p));
        if self.quality.len() != before {
            tracing::warn!("now-playing quality has duplicates, keeping first occurrences");
        }
        let f = &mut self.font;
        for (name, size) in [
            ("title-size", &mut f.title_size),
            ("artist-size", &mut f.artist_size),
            ("album-size", &mut f.album_size),
            ("quality-size", &mut f.quality_size),
        ] {
            if !(8.0..=48.0).contains(size) {
                tracing::warn!(
                    "now-playing font {name} {} out of 8..48, using the default",
                    size
                );
                *size = match name {
                    "title-size" => title_size(),
                    "artist-size" => artist_size(),
                    "album-size" => album_size(),
                    _ => quality_size(),
                };
            }
        }
        for (name, weight) in [
            ("title-weight", &mut f.title_weight),
            ("artist-weight", &mut f.artist_weight),
            ("album-weight", &mut f.album_weight),
            ("quality-weight", &mut f.quality_weight),
        ] {
            if !(100..=900).contains(weight) {
                tracing::warn!(
                    "now-playing font {name} {} out of 100..900, using 400",
                    weight
                );
                *weight = regular();
            }
        }
    }
}
