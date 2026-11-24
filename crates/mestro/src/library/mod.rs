//! The library browse feature: the level stack, request keys,
//! detail subjects and cover prefetch — the client-side state
//! machine on the UI thread. The provider half (websocket requests
//! and replies) lives in `providers/`; this module consumes the
//! keyed `SourceEvent::Browse` / `SubjectImage` events it emits.
//!
//! - `state`: the level stack and the messages crossing threads
//! - `drill`: opening levels, cover prefetch, go-to subjects
//! - `wire`: the UI callbacks
//! - `drain`: applying replies/ covers/ images on the slint thread
//! - `convert`: provider entries ⇄ slint structs (+ glyphs)

pub mod convert;
mod drain;
mod drill;
pub mod search;
mod state;
mod wire;

use crate::ui::navigation::Navigation;
use crate::LibEntry;
use drill::CmdTx;
pub use drill::LibTx;
use slint::ComponentHandle;
use state::{LibMsg, LibUi};
use std::cell::RefCell;
use std::rc::Rc;

/// The library feature, wired into the UI once at startup.
pub struct Library {
    state: Rc<RefCell<LibUi>>,
    lib_tx: LibTx,
    /// taken by spawn_drain — the receiver lives on the slint thread
    lib_rx: RefCell<Option<tokio::sync::mpsc::UnboundedReceiver<LibMsg>>>,
}

impl Clone for Library {
    fn clone(&self) -> Self {
        // clones share state and sender; the receiver exists once
        Self {
            state: self.state.clone(),
            lib_tx: self.lib_tx.clone(),
            lib_rx: RefCell::new(None),
        }
    }
}

impl Library {
    pub fn new() -> Self {
        let (lib_tx, lib_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            state: Rc::new(RefCell::new(LibUi::default())),
            lib_tx,
            lib_rx: RefCell::new(Some(lib_rx)),
        }
    }

    /// Leaving the library page: drop the browse session —
    /// level stack, current list, subjects. The next entry
    /// fetches the provider root again (a go-to detail from the
    /// queue would otherwise strand the next visit inside the
    /// album with no way back to its parents). Keeps the cover
    /// cache.
    pub fn reset(&self, ui: &crate::Mestro) {
        tracing::debug!("library: reset");
        let mut l = self.state.borrow_mut();
        l.stack.clear();
        l.current.clear();
        l.pending = None;
        l.current_subject = None;
        l.current_key.clear();
        drop(l);
        ui.set_lib_model(slint::ModelRc::new(slint::VecModel::from(
            Vec::<LibEntry>::new(),
        )));
        ui.set_lib_mode("list".into());
        ui.set_lib_subject(LibEntry::default());
        ui.set_lib_title("".into());
        ui.set_lib_meta("".into());
        ui.set_lib_loading(false);
        ui.set_lib_scroll(0.0);
        ui.set_lib_cover(slint::Image::default());
        ui.set_lib_menu_open(false);
    }

    /// first entry into the library page: fetch the provider list
    pub fn ensure_root(&self, ui: &crate::Mestro, cmd_tx: &CmdTx) {
        let mut l = self.state.borrow_mut();
        if l.current.is_empty() && l.pending.is_none() {
            l.pending = Some(("root".into(), "Library".into(), None));
            drop(l);
            ui.set_lib_loading(true);
            let _ = cmd_tx.send(crate::providers::SourceCommand::Browse("root".into()));
        }
    }

    /// all library UI callbacks
    pub fn wire(&self, ui: &crate::Mestro, cmd_tx: &CmdTx, nav: &Navigation, auto_open_np: bool) {
        let ctx = wire::Ctx {
            ui_weak: ui.as_weak(),
            lib: self.state.clone(),
            cmd_tx: cmd_tx.clone(),
            lib_tx: self.lib_tx.clone(),
            nav: nav.clone(),
            auto_open_np,
        };
        wire::wire(&ctx, ui);
    }

    /// go-to-artist for a foreign entry (queue rows): opens the
    /// artist detail level with a fresh stack — the first back
    /// leaves the library back to the caller
    pub fn goto_artist(&self, ui: &crate::Mestro, cmd_tx: &CmdTx, entry: &LibEntry) {
        let (cmd, key, title, subject) = drill::artist_drill(entry);
        drill::drill_level_fresh(
            &self.state,
            ui,
            cmd_tx,
            &self.lib_tx,
            cmd,
            key,
            title,
            Some(subject),
        );
    }

    /// go-to-album for a foreign entry (queue rows)
    pub fn goto_album(&self, ui: &crate::Mestro, cmd_tx: &CmdTx, entry: &LibEntry) {
        let (cmd, key, title, subject) = drill::album_drill(entry);
        drill::drill_level_fresh(
            &self.state,
            ui,
            cmd_tx,
            &self.lib_tx,
            cmd,
            key,
            title,
            Some(subject),
        );
    }

    /// drain the lib channel on the slint thread
    pub fn spawn_drain(&self, ui_weak: slint::Weak<crate::Mestro>) {
        let Some(lib_rx) = self.lib_rx.borrow_mut().take() else {
            panic!("library drain already spawned")
        };
        drain::spawn(ui_weak, self.state.clone(), self.lib_tx.clone(), lib_rx);
    }

    /// the provider loop sends browse replies through this channel
    pub fn feed_tx(&self) -> &LibTx {
        &self.lib_tx
    }
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

/// Route a provider event into the lib channel (called from the
/// tokio core loop).
pub fn route(lib_tx: &LibTx, ev: crate::providers::SourceEvent) {
    match ev {
        crate::providers::SourceEvent::Browse { key, entries } => {
            let _ = lib_tx.send(LibMsg::Update { key, entries });
        }
        crate::providers::SourceEvent::SubjectImage { key, image_url } => {
            let _ = lib_tx.send(LibMsg::SubjectImage {
                key,
                url: image_url,
            });
        }
        _ => {}
    }
}
