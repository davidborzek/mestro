//! Conversions between provider entries and the slint UI structs,
//! including the folder glyph mapping.

use crate::providers::MediaEntry;
use crate::LibEntry;

/// slint struct conversion for a provider entry.
pub fn lib_entry(e: &MediaEntry) -> LibEntry {
    LibEntry {
        name: e.name.clone().into(),
        kind: e.kind.clone().into(),
        path: e.path.clone().into(),
        item_id: e.item_id.clone().into(),
        provider: e.provider.clone().into(),
        uri: e.uri.clone().into(),
        duration: if e.duration_sec > 0.0 {
            crate::player::state::mmss((e.duration_sec * 1000.0) as u64).into()
        } else {
            "".into()
        },
        artist: e.artist.clone().into(),
        year: e.year.clone().into(),
        image_url: e.image_url.clone().into(),
        artist_id: e.artist_id.clone().into(),
        artist_uri: e.artist_uri.clone().into(),
        album_id: e.album_id.clone().into(),
        album_name: e.album_name.clone().into(),
        album_uri: e.album_uri.clone().into(),
        // folder icons: the root level (provider rows) maps
        // recognized provider domains; category folders inside a
        // provider map by their path suffix. Everything else keeps
        // the kind-derived icon (Icon::none)
        icon: folder_icon(e),
    }
}

fn folder_icon(e: &MediaEntry) -> crate::Icon {
    use crate::Icon;
    if e.kind != "folder" {
        return Icon::None;
    }
    if !e.provider.contains("--") {
        // the root provider rows
        return match e.provider.as_str() {
            "builtin" => Icon::MusicNote,
            "ambient_sounds" => Icon::Waves,
            "plex" => Icon::PlayArrow, // the Plex mark
            "tunein" => Icon::Radio,
            _ => Icon::None,
        };
    }
    // category folders inside a provider
    match e.path.rsplit("://").next().unwrap_or("") {
        "artists" => Icon::Artist,
        "albums" => Icon::Album,
        "tracks" => Icon::MusicNote,
        "playlists" => Icon::QueueMusic,
        "radio" => Icon::Radio,
        "recommendations" => Icon::Recommend,
        _ => Icon::None,
    }
}

/// raw cover pixels → slint image (slint::Image is not Send, so
/// this happens on the UI thread)
pub fn to_image(data: &crate::player::cover::CoverData) -> slint::Image {
    slint::Image::from_rgba8(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &data.rgba,
            data.width,
            data.height,
        ),
    )
}
