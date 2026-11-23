//! The queue overlay: row context menu and the queue model.
//!
//! The queue is its own view (ui/queue.slint) on top of the
//! now-playing surface; this module wires its callbacks — the
//! row menu (remove + go-tos built from the item's
//! artist/album references) and the transport-style queue
//! commands — and owns the persistent queue model.
//!
//! Model rules (the perf lessons live here): a fresh model per
//! queue event re-instantiated ~100 rows of text layouts
//! (hidden under the overlay included) — [`sync_model`]
//! updates in place and only rebuilds when the row count
//! changes, and only while the overlay is open; the rows are
//! cached otherwise and [`rebuild_cache`] materializes the
//! model on open mid-session.

use crate::library::Library;
use crate::providers::SourceCommand;
use crate::ui::navigation::{CmdTx, Navigation};
use crate::{LibEntry, MenuAction, Mestro, Page, QueueEntry, SheetAction};
use slint::ComponentHandle;

/// all queue overlay callbacks. The menu trigger needs the
/// library for the artist/album drill and navigation for the
/// page handover (back from the drill lands on the queue).
pub fn wire(ui: &Mestro, cmd_tx: &CmdTx, nav: &Navigation, library: &Library) {
    // queue row context menu: remove (not on the playing row)
    // plus go-tos built from the item's artist/album references
    ui.on_queue_menu(move |entry| {
        let mut actions = Vec::new();
        if !entry.current {
            actions.push(SheetAction {
                id: MenuAction::Remove,
                label: "Remove from queue".into(),
                icon: crate::Icon::Delete, // trash can
            });
        }
        actions.extend(goto_actions(&entry));
        slint::ModelRc::new(slint::VecModel::from(actions))
    });
    ui.on_queue_menu_trigger({
        let ui_weak = ui.as_weak();
        let library = library.clone();
        let cmd_tx = cmd_tx.clone();
        let nav = nav.clone();
        move |action, entry| {
            match action {
                MenuAction::Remove => {
                    let _ = cmd_tx.send(SourceCommand::RemoveQueueItem(
                        entry.queue_item_id.to_string(),
                    ));
                }
                // library-only action, never composed by the queue
                MenuAction::PlayNow | MenuAction::PlayShuffled => {}
                MenuAction::GotoArtist | MenuAction::GotoAlbum => {
                    drill(&ui_weak, &library, &cmd_tx, &nav, &entry, action);
                }
                // library-only actions, never composed here
                MenuAction::PlayNext | MenuAction::AddToQueue => {}
            }
        }
    });
    // the now-playing meta sheet (tap on title/artist/album):
    // go-tos composed from the playing item's references
    ui.on_np_meta_menu(|| {
        let actions = current_entry()
            .map(|e| goto_actions(&e))
            .unwrap_or_default();
        slint::ModelRc::new(slint::VecModel::from(actions))
    });
    ui.on_np_meta_trigger({
        let ui_weak = ui.as_weak();
        let library = library.clone();
        let cmd_tx = cmd_tx.clone();
        let nav = nav.clone();
        move |action| match action {
            MenuAction::GotoArtist | MenuAction::GotoAlbum => {
                if let Some(entry) = current_entry() {
                    drill(&ui_weak, &library, &cmd_tx, &nav, &entry, action);
                }
            }
            _ => {}
        }
    });
    ui.on_play_queue_item({
        let cmd_tx = cmd_tx.clone();
        move |item_id| {
            let _ = cmd_tx.send(SourceCommand::PlayQueueItem(item_id.to_string()));
        }
    });
    ui.on_clear_queue({
        let cmd_tx = cmd_tx.clone();
        move || {
            let _ = cmd_tx.send(SourceCommand::ClearQueue);
        }
    });
    ui.on_remove_queue_item({
        let cmd_tx = cmd_tx.clone();
        move |item_id| {
            let _ = cmd_tx.send(SourceCommand::RemoveQueueItem(item_id.to_string()));
        }
    });
}

/// the go-to actions built from an item's artist/album
/// references — shared by the queue row menu and the
/// now-playing meta sheet
fn goto_actions(entry: &QueueEntry) -> Vec<SheetAction> {
    let mut actions = Vec::new();
    if !entry.artist_id.is_empty() {
        let label = if entry.artist.is_empty() {
            "Go to artist".to_string()
        } else {
            format!("Go to artist · {}", entry.artist)
        };
        actions.push(SheetAction {
            id: MenuAction::GotoArtist,
            label: label.into(),
            icon: crate::Icon::Artist,
        });
    }
    if !entry.album_id.is_empty() {
        let label = if entry.album_name.is_empty() {
            "Go to album".to_string()
        } else {
            format!("Go to album · {}", entry.album_name)
        };
        actions.push(SheetAction {
            id: MenuAction::GotoAlbum,
            label: label.into(),
            icon: crate::Icon::Album,
        });
    }
    actions
}

/// the currently playing queue item — identity source for the
/// now-playing meta sheet
pub fn current_entry() -> Option<QueueEntry> {
    QUEUE_MODEL.with(|qm| {
        qm.borrow()
            .as_ref()
            .and_then(|(_, rows)| rows.iter().find(|r| r.current).cloned())
    })
}

/// the library handover for a go-to (queue rows and the
/// now-playing meta sheet): navigation FIRST, drill second — the
/// same-page case ends the old library session (reset), and the
/// reset must not wipe the freshly armed pending request of the
/// drill
fn drill(
    ui_weak: &slint::Weak<Mestro>,
    library: &Library,
    cmd_tx: &CmdTx,
    nav: &Navigation,
    entry: &QueueEntry,
    action: MenuAction,
) {
    let Some(ui) = ui_weak.upgrade() else { return };
    nav.goto_from_queue(&ui, Page::Library);
    let lib_entry = LibEntry {
        artist: entry.artist.clone(),
        artist_id: entry.artist_id.clone(),
        artist_uri: entry.artist_uri.clone(),
        provider: entry.provider.clone(),
        album_id: entry.album_id.clone(),
        album_name: entry.album_name.clone(),
        album_uri: entry.album_uri.clone(),
        // the track's artwork IS the album cover
        image_url: entry.image_url.clone(),
        ..LibEntry::default()
    };
    if action == MenuAction::GotoArtist {
        library.goto_artist(&ui, cmd_tx, &lib_entry);
    } else {
        library.goto_album(&ui, cmd_tx, &lib_entry);
    }
}

/// push the current queue rows (playing track first, history
/// dropped — the caller filters) onto the UI. Syncs the model
/// only while the overlay is open; caches the rows otherwise
/// (the tick rebuilds from the cache on open).
pub fn sync_model(ui: &Mestro, entries: Vec<QueueEntry>) {
    use slint::Model as _;
    QUEUE_MODEL.with(|qm| {
        let mut qm = qm.borrow_mut();
        if !ui.get_queue_open() {
            if let Some((_, last)) = qm.as_mut() {
                *last = entries;
            } else {
                *qm = Some((slint::ModelRc::default(), entries));
            }
            return;
        }
        let rebuild = match qm.as_ref() {
            Some((model, _)) => model.row_count() != entries.len(),
            None => true,
        };
        if rebuild {
            let model = slint::ModelRc::new(slint::VecModel::from(entries.clone()));
            ui.set_queue_model(model.clone());
            *qm = Some((model, entries));
        } else if let Some((model, last)) = qm.as_mut() {
            for (i, e) in entries.iter().enumerate() {
                if &last[i] != e {
                    model.set_row_data(i, e.clone());
                }
            }
            *last = entries;
        }
    });
}

/// queue opened mid-session: rebuild the model from the row
/// cache
pub fn rebuild_cache(ui: &Mestro) {
    QUEUE_MODEL.with(|qm| {
        let mut qm = qm.borrow_mut();
        if let Some((model, entries)) = qm.as_mut() {
            let fresh = slint::ModelRc::new(slint::VecModel::from(entries.clone()));
            ui.set_queue_model(fresh.clone());
            *model = fresh;
        }
    });
}

// the persistent queue model + last pushed rows (see sync_model:
// a fresh model per queue event re-instantiates every row)
thread_local! {
    static QUEUE_MODEL: std::cell::RefCell<
        Option<(slint::ModelRc<QueueEntry>, Vec<QueueEntry>)>,
    > = const { std::cell::RefCell::new(None) };
}
