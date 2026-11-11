//! Library browse state: the level stack, the pending request and
//! the message enum crossing onto the slint thread.

use crate::providers::MediaEntry;
use crate::{LibEntry, Mestro};
use std::collections::HashMap;

/// One pushed level of the library browse tree (the current list is
/// `LibUi::current`; going back pops these). Detail levels
/// (artist/album/playlist) carry their subject.
pub(super) struct LibLevel {
    pub title: String,
    pub entries: Vec<LibEntry>,
    pub subject: Option<LibEntry>,
    pub meta: String,
    pub mode: String,
    /// list scroll offset at leave time — restored on the way back
    pub scroll_y: f32,
    /// the level's request key — late SubjectImage events match it
    pub key: String,
}

/// Library browse state, owned by the UI thread: the level stack
/// (slint arrays are read-only in the language), the pending request
/// as (reply key, level title, subject) — slow providers answer out
/// of band, stale replies drop via the key mismatch — and a small
/// cover cache so revisits don't refetch.
#[derive(Default)]
pub(super) struct LibUi {
    pub stack: Vec<LibLevel>,
    pub current: Vec<LibEntry>,
    pub pending: Option<(String, String, Option<LibEntry>)>,
    pub current_subject: Option<LibEntry>,
    /// request key of the currently shown level — late image events
    /// match against it
    pub current_key: String,
    pub covers: HashMap<String, crate::player::cover::CoverData>,
}

impl LibUi {
    /// snapshot the currently shown level (what going back will
    /// restore) — title/meta/mode live in the UI, the rest here
    pub fn snapshot(&self, ui: &Mestro) -> LibLevel {
        LibLevel {
            title: ui.get_lib_title().to_string(),
            entries: self.current.clone(),
            subject: self.current_subject.clone(),
            meta: ui.get_lib_meta().to_string(),
            mode: ui.get_lib_mode().to_string(),
            scroll_y: ui.get_lib_scroll(),
            key: self.current_key.clone(),
        }
    }

    /// meta line for a detail level's header: album gets
    /// "Artist · Year" when known, otherwise everything counts its
    /// entries
    pub fn meta_for(subject: &LibEntry, entries: &[LibEntry]) -> String {
        let artist = subject.artist.to_string();
        let year = subject.year.to_string();
        if !artist.is_empty() {
            if !year.is_empty() {
                return format!("{artist} · {year}");
            }
            return artist;
        }
        let noun = if subject.kind == "artist" {
            "albums"
        } else {
            "tracks"
        };
        format!("{} {noun}", entries.len())
    }
}

/// Messages onto the slint thread: provider replies, cover pixels
/// and late subject images. The key matches the request it answers.
pub enum LibMsg {
    Update {
        key: String,
        entries: Vec<MediaEntry>,
    },
    /// raw pixels — slint::Image is not Send; the UI thread builds it
    Cover {
        url: String,
        data: crate::player::cover::CoverData,
    },
    SubjectImage {
        key: String,
        url: String,
    },
}

/// how a detail level renders
pub(super) fn mode_of(subject: &LibEntry) -> String {
    let kind = subject.kind.to_string();
    if kind == "artist" || kind == "album" || kind == "playlist" {
        kind
    } else {
        "list".to_string()
    }
}
