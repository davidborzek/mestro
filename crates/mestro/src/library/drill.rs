//! Opening browse levels: the shared push-and-fetch flow, cover
//! prefetch and the detail subjects for context-menu go-tos.

use super::state::{LibMsg, LibUi};
use crate::providers::SourceCommand;
use crate::{LibEntry, Mestro};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) type CmdTx = tokio::sync::mpsc::UnboundedSender<SourceCommand>;
pub type LibTx = tokio::sync::mpsc::UnboundedSender<LibMsg>;

/// Open a new browse level: push the current one onto the stack,
/// mark loading, apply the title immediately and fire the fetch.
/// Shared by row taps and the context-menu go-tos.
#[allow(clippy::too_many_arguments)]
pub(super) fn drill_level(
    lib: &Rc<RefCell<LibUi>>,
    ui: &Mestro,
    cmd_tx: &CmdTx,
    lib_tx: &LibTx,
    cmd: SourceCommand,
    key: String,
    title: String,
    subject: Option<LibEntry>,
) {
    drill_level_opt(lib, ui, cmd_tx, lib_tx, cmd, key, title, subject, true)
}

/// Entering from outside the library (queue rows): the level
/// stack starts fresh — the first back leaves the library page
/// back to where the goto started instead of restoring a level
/// the user never saw.
#[allow(clippy::too_many_arguments)]
pub(super) fn drill_level_fresh(
    lib: &Rc<RefCell<LibUi>>,
    ui: &Mestro,
    cmd_tx: &CmdTx,
    lib_tx: &LibTx,
    cmd: SourceCommand,
    key: String,
    title: String,
    subject: Option<LibEntry>,
) {
    drill_level_opt(lib, ui, cmd_tx, lib_tx, cmd, key, title, subject, false)
}

#[allow(clippy::too_many_arguments)]
fn drill_level_opt(
    lib: &Rc<RefCell<LibUi>>,
    ui: &Mestro,
    cmd_tx: &CmdTx,
    lib_tx: &LibTx,
    cmd: SourceCommand,
    key: String,
    title: String,
    subject: Option<LibEntry>,
    push_current: bool,
) {
    if let Some(s) = &subject {
        prefetch_cover(lib, lib_tx, s.image_url.as_ref());
    }
    tracing::debug!("library: drill {key} push_current={push_current}");
    let mut l = lib.borrow_mut();
    if push_current {
        let snapshot = l.snapshot(ui);
        l.stack.push(snapshot);
    } else {
        l.stack.clear();
    }
    l.pending = Some((key, title.clone(), subject));
    drop(l);
    ui.set_lib_loading(true);
    ui.set_lib_title(title.into());
    let _ = cmd_tx.send(cmd);
}

/// Fetch a detail subject's cover off-thread; the result flows back
/// through the lib channel. Cached urls skip the fetch. A plain
/// thread, NOT spawn_blocking: this runs from slint UI callbacks,
/// outside the tokio runtime.
pub(super) fn prefetch_cover(lib: &Rc<RefCell<LibUi>>, lib_tx: &LibTx, url: &str) {
    if url.is_empty() || lib.borrow().covers.contains_key(url) {
        return;
    }
    let tx = lib_tx.clone();
    let url = url.to_string();
    std::thread::spawn(move || {
        if let Some(c) = crate::player::cover::load_cover(&url) {
            let _ = tx.send(LibMsg::Cover { url, data: c });
        }
    });
}

/// go-to-artist: (command, reply key, level title, subject) built
/// from a track/album row's embedded artist reference
pub(super) fn artist_drill(entry: &LibEntry) -> (SourceCommand, String, String, LibEntry) {
    let artist_id = entry.artist_id.to_string();
    let subject = LibEntry {
        name: entry.artist.clone(),
        kind: "artist".into(),
        item_id: artist_id.clone().into(),
        provider: entry.provider.clone(),
        uri: entry.artist_uri.clone(),
        // the real image arrives via the chained artists/get
        ..LibEntry::default()
    };
    (
        SourceCommand::ArtistAlbums {
            item_id: artist_id,
            provider: entry.provider.to_string(),
        },
        format!("artist-albums:{}:{}", entry.provider, entry.artist_id),
        entry.artist.to_string(),
        subject,
    )
}

/// go-to-album: the track's artwork IS the album cover
pub(super) fn album_drill(entry: &LibEntry) -> (SourceCommand, String, String, LibEntry) {
    let album_id = entry.album_id.to_string();
    let subject = LibEntry {
        name: entry.album_name.clone(),
        kind: "album".into(),
        item_id: album_id.clone().into(),
        provider: entry.provider.clone(),
        uri: entry.album_uri.clone(),
        image_url: entry.image_url.clone(),
        ..LibEntry::default()
    };
    (
        SourceCommand::AlbumTracks {
            item_id: album_id,
            provider: entry.provider.to_string(),
        },
        format!("album-tracks:{}:{}", entry.provider, entry.album_id),
        entry.album_name.to_string(),
        subject,
    )
}
