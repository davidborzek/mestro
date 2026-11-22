//! The now-playing pipeline: config wiring and player-state
//! application for the np overlay.
//!
//! main() stays the process skeleton (event loop, WS feeds,
//! timers); everything the np overlay reads flows through here:
//! [`wire`] translates [now-playing] config into UI properties
//! once at startup, [`apply`] pushes every player update
//! (metadata, quality line, transport state, cover) onto the UI
//! — queue rows route on to [`crate::player::queue::sync_model`].
//!
//! Channel rules hold: slint properties are only set on the UI
//! thread — apply runs on it.

use crate::player::state::{self, PlayerUpdate};

pub mod transport;
use crate::{Mestro, QualityItem, QueueEntry, RepeatMode};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

/// transport playing state, process-wide: the VU mock reads it to
/// move only while something actually plays
static PLAYING: AtomicBool = AtomicBool::new(false);

pub fn playing() -> &'static AtomicBool {
    &PLAYING
}

/// [now-playing] and [now-playing.fullbleed] config → UI
/// properties. Once at startup.
pub fn wire(ui: &Mestro, config: &crate::config::Config) {
    ui.set_np_layout(match config.now_playing.layout {
        crate::config::NpLayout::Side => 0,
        crate::config::NpLayout::Fullbleed => 1,
    });
    ui.set_np_inline_vu(match config.now_playing.inline_vu {
        crate::config::InlineVu::Off => 0,
        crate::config::InlineVu::Led => 1,
        crate::config::InlineVu::Thin => 2,
        crate::config::InlineVu::Matrix => 3,
    });
    // [now-playing] controls: which transport buttons the row
    // shows, in the configured display order
    let controls: Vec<slint::SharedString> = config
        .now_playing
        .controls
        .iter()
        .map(|c| {
            use crate::config::TransportControl::*;
            match c {
                Shuffle => "shuffle",
                Previous => "previous",
                PlayPause => "play-pause",
                Next => "next",
                Repeat => "repeat",
            }
            .into()
        })
        .collect();
    ui.set_np_controls(slint::ModelRc::new(slint::VecModel::from(controls)));
    let np_font = config.now_playing.font;
    ui.set_np_title_size(np_font.title_size);
    ui.set_np_title_weight(np_font.title_weight);
    ui.set_np_artist_size(np_font.artist_size);
    ui.set_np_artist_weight(np_font.artist_weight);
    ui.set_np_album_size(np_font.album_size);
    ui.set_np_album_weight(np_font.album_weight);
    ui.set_np_quality_size(np_font.quality_size);
    ui.set_np_quality_weight(np_font.quality_weight);
}

/// the quality-relevant player values, cached in main so a
/// settings reorder can re-render the line instantly — no
/// player update needed
#[derive(Debug, Clone, Default)]
pub struct QualityVals {
    pub codec: String,
    pub rate: String,
    pub depth: String,
    pub bitrate: String,
    pub channels: String,
    pub lossless: bool,
}

impl QualityVals {
    pub fn from_state(s: &crate::player::state::PlayerState) -> Self {
        Self {
            codec: s.codec.clone(),
            rate: s.rate.clone(),
            depth: s.depth.clone(),
            bitrate: s.bitrate.clone(),
            channels: s.channels.clone(),
            lossless: s.lossless,
        }
    }
}

/// render the ordered quality line: [now-playing] quality
/// decides parts AND positions — the codec is just a part, not
/// a hardcoded prefix
pub fn apply_quality(ui: &Mestro, v: &QualityVals, order: &[crate::config::QualityPart]) {
    use crate::config::QualityPart as Q;
    let mut items: Vec<QualityItem> = vec![];
    for p in order {
        let (text, primary) = match p {
            Q::Codec => (v.codec.as_str(), true),
            Q::Rate => (v.rate.as_str(), false),
            Q::Depth => (v.depth.as_str(), false),
            Q::Bitrate => (v.bitrate.as_str(), false),
            Q::Channels => (v.channels.as_str(), false),
        };
        if !text.is_empty() {
            items.push(QualityItem {
                text: text.into(),
                primary,
            });
        }
    }
    ui.set_np_quality_items(slint::ModelRc::new(slint::VecModel::from(items)));
    ui.set_lossless(v.lossless);
}

/// push a player update onto the UI: transport state, metadata,
/// the quality line, the queue rows and the covers. Runs on the
/// UI thread (slint properties must not cross threads).
pub fn apply(
    ui: &Mestro,
    update: &PlayerUpdate,
    palette_out: &Rc<std::cell::RefCell<Option<crate::ui::palette::ArtworkPalette>>>,
    np_quality: &[crate::config::QualityPart],
) {
    let s = &update.state;
    PLAYING.store(s.playing, Ordering::Relaxed);
    ui.set_playing(s.playing);
    ui.set_loading(s.loading);
    ui.set_np_shuffle(s.shuffle);
    ui.set_np_repeat(match s.repeat {
        crate::providers::RepeatMode::One => RepeatMode::One,
        crate::providers::RepeatMode::All => RepeatMode::All,
        crate::providers::RepeatMode::Off => RepeatMode::Off,
    });
    ui.set_track_title(s.title.clone().into());
    ui.set_track_artist(s.artist.clone().into());
    ui.set_track_album(s.album.clone().into());
    ui.set_volume(s.volume);
    // the quality values, cached for instant re-render when
    // the settings reorder the line (no update event needed)
    let vals = QualityVals::from_state(s);
    apply_quality(ui, &vals, np_quality);
    // the joined line for the VU faceplate (codec renders as its
    // own text there, not in the line)
    let mut parts: Vec<&str> = vec![];
    for p in np_quality {
        match p {
            crate::config::QualityPart::Codec => {}
            crate::config::QualityPart::Rate => {
                if !s.rate.is_empty() {
                    parts.push(&s.rate);
                }
            }
            crate::config::QualityPart::Depth => {
                if !s.depth.is_empty() {
                    parts.push(&s.depth);
                }
            }
            crate::config::QualityPart::Bitrate => {
                if !s.bitrate.is_empty() {
                    parts.push(&s.bitrate);
                }
            }
            crate::config::QualityPart::Channels => {
                if !s.channels.is_empty() {
                    parts.push(&s.channels);
                }
            }
        }
    }
    ui.set_quality(parts.join(" · ").into());
    ui.set_codec(s.codec.clone().into());
    ui.set_powered(s.powered);
    ui.set_has_media(s.has_media);
    ui.set_cover_missing(s.cover_missing);
    ui.set_external_source(s.external.clone().unwrap_or_default().into());
    ui.set_duration_seconds((s.duration_ms / 1000).min(i32::MAX as u64) as i32);
    ui.set_can_seek(
        s.external.is_none() && s.has_media && s.powered && !s.loading && s.duration_ms >= 1000,
    );
    ui.set_seek_track_key(transport::seek_track_key(s).into());
    // queue view: the playing track plus upcoming entries — the
    // played history below the current sort_index is dropped.
    // A fresh model per queue event re-instantiated ~100 rows of
    // text layouts (hidden under the overlay included) — update
    // in place, rebuild only when the row count changes
    let entries: Vec<QueueEntry> = s
        .queue
        .iter()
        .filter(|q| s.queue_current == usize::MAX || q.sort_index >= s.queue_current)
        .map(|q| QueueEntry {
            queue_item_id: q.queue_item_id.clone().into(),
            title: q.title.clone().into(),
            artist: q.artist.clone().into(),
            duration: state::mmss((q.duration_sec * 1000.0) as u64).into(),
            current: q.sort_index == s.queue_current,
            provider: q.provider.clone().into(),
            artist_id: q.artist_id.clone().into(),
            artist_uri: q.artist_uri.clone().into(),
            album_id: q.album_id.clone().into(),
            album_name: q.album_name.clone().into(),
            album_uri: q.album_uri.clone().into(),
            image_url: q.image_url.clone().into(),
        })
        .collect::<Vec<_>>();
    crate::player::queue::sync_model(ui, entries);
    if let Some(cover) = &update.cover {
        let tc = std::time::Instant::now();
        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &cover.rgba,
            cover.width,
            cover.height,
        );
        ui.set_cover(slint::Image::from_rgba8(buf));
        if tc.elapsed().as_millis() > 10 {
            tracing::info!("cover upload took {:?}", tc.elapsed());
        }
    }
    if let Some(bg) = &update.bg {
        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &bg.rgba, bg.width, bg.height,
        );
        ui.set_bg_cover(slint::Image::from_rgba8(buf));
    }
    if update.cover.is_some() {
        // the palette rides cover updates only — gray covers
        // reset it to None (the caller falls back to amber)
        *palette_out.borrow_mut() = update.palette;
    }
}
