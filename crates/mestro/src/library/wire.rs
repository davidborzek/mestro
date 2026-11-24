//! The UI callbacks of the library page: row taps, the context menu
//! (composition + dispatch), back navigation and the subject play
//! button. Shared handles travel in the `Ctx`.

use super::drill::{
    album_drill, artist_drill, drill_level, drill_level_fresh, prefetch_cover, CmdTx, LibTx,
};
use super::state::LibUi;
use crate::providers::{Enqueue, SourceCommand};
use crate::ui::navigation::Navigation;
use crate::Page;
use crate::{LibEntry, MenuAction, Mestro, SheetAction};
use std::cell::RefCell;
use std::rc::Rc;

/// everything the callbacks need, cloned per handler
#[derive(Clone)]
pub(super) struct Ctx {
    pub ui_weak: slint::Weak<Mestro>,
    pub lib: Rc<RefCell<LibUi>>,
    pub cmd_tx: CmdTx,
    pub lib_tx: LibTx,
    pub nav: Navigation,
    /// [ui] auto_open_now_playing: reveal the overlay when a
    /// library tap starts playback
    pub auto_open_np: bool,
}

impl Ctx {
    /// playback started from the library: the overlay reveals
    /// itself when the setting says so
    fn maybe_open_np(&self, ui: &Mestro) {
        if self.auto_open_np {
            self.nav.open_np(ui);
        }
    }
}

pub(super) fn wire(ctx: &Ctx, ui: &Mestro) {
    wire_open(ctx, ui);
    wire_menu(ctx, ui);
    wire_menu_trigger(ctx, ui);
    wire_back(ctx, ui);
    wire_play_subject(ctx, ui);
}

/// row taps: folders walk the tree, artists/albums/playlists open
/// detail levels, tracks play — with their subject as context when
/// inside a detail level
fn wire_open(ctx: &Ctx, ui: &Mestro) {
    let ctx = ctx.clone();
    ui.on_open_lib_entry(move |entry| {
        let Some(ui) = ctx.ui_weak.upgrade() else {
            return;
        };
        let kind = entry.kind.to_string();
        let subject = |entry: &LibEntry| Some(entry.clone());
        let (cmd, drill) = match kind.as_str() {
            // folders (and genres with a browse path) walk the tree
            "folder" | "genre" if !entry.path.to_string().is_empty() => (
                SourceCommand::Browse(entry.path.to_string()),
                (entry.path.to_string(), entry.name.to_string(), None),
            ),
            "artist" => (
                SourceCommand::ArtistAlbums {
                    item_id: entry.item_id.to_string(),
                    provider: entry.provider.to_string(),
                },
                (
                    format!("artist-albums:{}:{}", entry.provider, entry.item_id),
                    entry.name.to_string(),
                    subject(&entry),
                ),
            ),
            "album" => (
                SourceCommand::AlbumTracks {
                    item_id: entry.item_id.to_string(),
                    provider: entry.provider.to_string(),
                },
                (
                    format!("album-tracks:{}:{}", entry.provider, entry.item_id),
                    entry.name.to_string(),
                    subject(&entry),
                ),
            ),
            "playlist" => (
                SourceCommand::PlaylistTracks {
                    item_id: entry.item_id.to_string(),
                    provider: entry.provider.to_string(),
                },
                (
                    format!("playlist-tracks:{}:{}", entry.provider, entry.item_id),
                    entry.name.to_string(),
                    subject(&entry),
                ),
            ),
            // playable leaves: no navigation. A track inside a
            // detail level plays its SUBJECT from the tapped
            // position (album/playlist context); anything else with
            // a URI (radio, podcast episode, ...) plays solo —
            // plain list tracks have no context
            _ => {
                let uri = entry.uri.to_string();
                if uri.is_empty() {
                    return;
                }
                if kind == "track" && ui.get_lib_mode() != "list" {
                    let media = ctx
                        .lib
                        .borrow()
                        .current_subject
                        .as_ref()
                        .map(|s| s.uri.to_string())
                        .unwrap_or_default();
                    if !media.is_empty() {
                        let _ = ctx.cmd_tx.send(SourceCommand::PlayMediaFrom {
                            media,
                            start_item: uri,
                        });
                        ctx.maybe_open_np(&ui);
                        return;
                    }
                }
                let _ = ctx.cmd_tx.send(SourceCommand::PlayMedia {
                    uri,
                    enqueue: Enqueue::Default,
                });
                ctx.maybe_open_np(&ui);
                return;
            }
        };
        let (key, title, subject) = drill;
        drill_level(
            &ctx.lib,
            &ui,
            &ctx.cmd_tx,
            &ctx.lib_tx,
            cmd,
            key,
            title,
            subject,
        );
    });
}

/// the context menu: Rust composes the applicable actions for the
/// held entry, the generic ActionSheet renders them
fn wire_menu(_ctx: &Ctx, ui: &Mestro) {
    ui.on_lib_menu(|entry| slint::ModelRc::new(slint::VecModel::from(menu_actions(&entry))));
}

fn menu_actions(entry: &LibEntry) -> Vec<SheetAction> {
    let mut actions = Vec::new();
    // subjects (artist/album/playlist rows) play as a whole:
    // MA resolves the URI into their track list
    let playable_subject =
        matches!(entry.kind.as_str(), "artist" | "album" | "playlist") && !entry.uri.is_empty();
    if playable_subject {
        actions.push(SheetAction {
            id: MenuAction::PlayNow,
            label: "Play now".into(),
            icon: crate::Icon::PlayArrow,
        });
    }
    if matches!(entry.kind.as_str(), "album" | "playlist") && !entry.uri.is_empty() {
        actions.push(SheetAction {
            id: MenuAction::PlayShuffled,
            label: "Shuffle play".into(),
            icon: crate::Icon::Shuffle,
        });
    }
    actions.extend([
        SheetAction {
            id: MenuAction::PlayNext,
            label: "Play next".into(),
            icon: crate::Icon::ArrowInsert,
        },
        SheetAction {
            id: MenuAction::AddToQueue,
            label: "Add to queue".into(),
            icon: crate::Icon::PlaylistAdd,
        },
    ]);
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
    // track rows only — an album row IS its own album
    if !entry.album_id.is_empty() && entry.kind == "track" {
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

/// dispatch the tapped action on the held entry
fn wire_menu_trigger(ctx: &Ctx, ui: &Mestro) {
    let ctx = ctx.clone();
    ui.on_lib_menu_trigger(move |action, entry| {
        match action {
            MenuAction::PlayNow | MenuAction::PlayShuffled => {
                let uri = entry.uri.to_string();
                if uri.is_empty()
                    || (action == MenuAction::PlayShuffled
                        && !matches!(entry.kind.as_str(), "album" | "playlist"))
                {
                    return;
                }
                let command = if action == MenuAction::PlayShuffled {
                    SourceCommand::PlayMediaShuffled { uri }
                } else {
                    SourceCommand::PlayMedia {
                        uri,
                        enqueue: Enqueue::Default,
                    }
                };
                let _ = ctx.cmd_tx.send(command);
                let Some(ui) = ctx.ui_weak.upgrade() else {
                    return;
                };
                ctx.maybe_open_np(&ui);
            }
            MenuAction::PlayNext | MenuAction::AddToQueue => {
                let uri = entry.uri.to_string();
                if uri.is_empty() {
                    return;
                }
                let enqueue = if action == MenuAction::PlayNext {
                    Enqueue::Next
                } else {
                    Enqueue::Add
                };
                let _ = ctx.cmd_tx.send(SourceCommand::PlayMedia { uri, enqueue });
            }
            MenuAction::GotoArtist => {
                let Some(ui) = ctx.ui_weak.upgrade() else {
                    return;
                };
                if entry.artist_id.is_empty() {
                    return;
                }
                let (cmd, key, title, subject) = artist_drill(&entry);
                // from another page (a search result menu) the
                // drill lands on the library FRESH — like the
                // tap flow, back falls onto the page we came
                // from. On the library itself the current level
                // stays on the stack
                if ui.get_page() != Page::Library {
                    ctx.nav.go(&ui, Page::Library);
                    drill_level_fresh(
                        &ctx.lib,
                        &ui,
                        &ctx.cmd_tx,
                        &ctx.lib_tx,
                        cmd,
                        key,
                        title,
                        Some(subject),
                    );
                } else {
                    drill_level(
                        &ctx.lib,
                        &ui,
                        &ctx.cmd_tx,
                        &ctx.lib_tx,
                        cmd,
                        key,
                        title,
                        Some(subject),
                    );
                }
            }
            MenuAction::GotoAlbum => {
                let Some(ui) = ctx.ui_weak.upgrade() else {
                    return;
                };
                if entry.album_id.is_empty() {
                    return;
                }
                let (cmd, key, title, subject) = album_drill(&entry);
                if ui.get_page() != Page::Library {
                    ctx.nav.go(&ui, Page::Library);
                    drill_level_fresh(
                        &ctx.lib,
                        &ui,
                        &ctx.cmd_tx,
                        &ctx.lib_tx,
                        cmd,
                        key,
                        title,
                        Some(subject),
                    );
                } else {
                    drill_level(
                        &ctx.lib,
                        &ui,
                        &ctx.cmd_tx,
                        &ctx.lib_tx,
                        cmd,
                        key,
                        title,
                        Some(subject),
                    );
                }
            }
            // queue-only action, never composed by the library
            MenuAction::Remove => {}
        }
    });
}

/// the capsule back: pops a browse level, exits the page at the
/// root level (restores mode/subject/scroll/cover)
fn wire_back(ctx: &Ctx, ui: &Mestro) {
    let ctx = ctx.clone();
    ui.on_lib_back(move || {
        let Some(ui) = ctx.ui_weak.upgrade() else {
            return;
        };
        // the context-menu sheet sits above the browse levels —
        // back dismisses it first
        if ui.get_lib_menu_open() {
            ui.set_lib_menu_open(false);
            return;
        }
        let mut l = ctx.lib.borrow_mut();
        // pop a browse level; at the root level the same arrow
        // exits the page (the nav stack holds where we came from)
        let Some(level) = l.stack.pop() else {
            drop(l);
            // the level stack is empty: the same arrow leaves the
            // library page — the nav stack holds where we came from
            ctx.nav.back(&ui);
            return;
        };
        // any in-flight fetch for the abandoned level is stale
        l.pending = None;
        l.current = level.entries;
        l.current_subject = level.subject.clone();
        l.current_key = level.key.clone();
        ui.set_lib_model(slint::ModelRc::new(slint::VecModel::from(
            l.current.clone(),
        )));
        // the model swap collapsed the Flickable to the top —
        // put the level's scroll offset back
        ui.set_lib_scroll(level.scroll_y);
        ui.set_lib_title(level.title.into());
        ui.set_lib_loading(false);
        ui.set_lib_mode(level.mode.into());
        ui.set_lib_meta(level.meta.into());
        match &level.subject {
            Some(s) => {
                ui.set_lib_subject(s.clone());
                let url = s.image_url.to_string();
                if let Some(data) = l.covers.get(&url) {
                    ui.set_lib_cover(super::convert::to_image(data));
                } else {
                    ui.set_lib_cover(slint::Image::default());
                    drop(l);
                    prefetch_cover(&ctx.lib, &ctx.lib_tx, &url);
                }
            }
            None => {
                ui.set_lib_subject(LibEntry::default());
                ui.set_lib_cover(slint::Image::default());
            }
        }
    });
}

/// play button in the detail header: the subject's URI plays the
/// whole album/playlist
fn wire_play_subject(ctx: &Ctx, ui: &Mestro) {
    let ctx = ctx.clone();
    ui.on_play_lib_subject(move || {
        let Some(ui) = ctx.ui_weak.upgrade() else {
            return;
        };
        let uri = ctx
            .lib
            .borrow()
            .current_subject
            .as_ref()
            .map(|s| s.uri.to_string());
        if let Some(uri) = uri.filter(|uri| !uri.is_empty()) {
            let _ = ctx.cmd_tx.send(SourceCommand::PlayMedia {
                uri,
                enqueue: Enqueue::Default,
            });
            ctx.maybe_open_np(&ui);
        }
    });
}

#[cfg(test)]
mod menu_tests {
    use super::*;
    #[test]
    fn albums_and_playlists_offer_shuffle_and_enqueue_actions() {
        for kind in ["album", "playlist"] {
            let entry = LibEntry {
                kind: kind.into(),
                uri: "library://subject/1".into(),
                ..Default::default()
            };
            let actions = menu_actions(&entry);
            for id in [
                MenuAction::PlayNow,
                MenuAction::PlayShuffled,
                MenuAction::PlayNext,
                MenuAction::AddToQueue,
            ] {
                assert!(actions.iter().any(|action| action.id == id));
            }
        }
    }
    #[test]
    fn albums_offer_go_to_artist_when_the_server_reports_one() {
        let entry = LibEntry {
            kind: "album".into(),
            uri: "library://album/1".into(),
            artist_id: "artist-1".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        let actions = menu_actions(&entry);
        let goto = actions
            .iter()
            .find(|a| a.id == MenuAction::GotoArtist)
            .expect("go to artist");
        assert_eq!(goto.label, "Go to artist · Artist");
    }
    #[test]
    fn artists_tracks_and_missing_uris_do_not_offer_shuffle() {
        for (kind, uri) in [
            ("artist", "library://artist/1"),
            ("track", "library://track/1"),
            ("album", ""),
            ("playlist", ""),
        ] {
            let entry = LibEntry {
                kind: kind.into(),
                uri: uri.into(),
                ..Default::default()
            };
            assert!(!menu_actions(&entry)
                .iter()
                .any(|action| action.id == MenuAction::PlayShuffled));
        }
    }
}
