//! The search session: query state, keystroke debounce, result
//! routing and row-art prefetch.
//!
//! The UI owns nothing but mirrors: the on-screen keyboard
//! (ui/osk.slint) drives the query through the root callbacks,
//! this module owns the string and the single-shot debounce
//! (typing bursts fire one search, not one per key), and results
//! land in the search model as plain library rows — tracks play
//! on tap, artists/albums hand over to the library drill with a
//! fresh stack, so back from the drill lands on the search page
//! with its results intact.

use crate::library::Library;
use crate::providers::{Enqueue, MediaEntry, SourceCommand};
use crate::ui::navigation::{CmdTx, Navigation};
use crate::{library, LibEntry, Mestro, Page};
use slint::ComponentHandle;
use std::rc::Rc;

/// keystroke quiet period before a search fires
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);
/// queries below this char count never fire
const MIN_CHARS: usize = 2;

pub struct Search {
    debounce: Rc<slint::Timer>,
}

impl Default for Search {
    fn default() -> Self {
        Self::new()
    }
}

impl Search {
    pub fn new() -> Self {
        Self {
            debounce: Rc::new(slint::Timer::default()),
        }
    }

    /// all UI callbacks of the search page. The QUERY IS THE
    /// PROPERTY (two-way bound to the TextInput): the OSK and
    /// the physical keyboard both mutate it, this module only
    /// debounces and dispatches. `on_search_open` needs
    /// navigation + the library for the artist/album handover
    pub fn wire(
        &self,
        ui: &Mestro,
        cmd_tx: &CmdTx,
        nav: &Navigation,
        library: &Library,
        auto_open_np: bool,
    ) {
        let arm = {
            let ui_weak = ui.as_weak();
            let cmd_tx = cmd_tx.clone();
            let debounce = self.debounce.clone();
            move || {
                let ui_weak = ui_weak.clone();
                let cmd_tx = cmd_tx.clone();
                debounce.start(slint::TimerMode::SingleShot, DEBOUNCE, move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    let q = ui.get_search_query().to_string();
                    // below the minimum the query can never fire —
                    // stale loading states (from a longer query
                    // that fired before backspaces) must clear
                    if q.chars().count() < MIN_CHARS {
                        ui.set_search_loading(false);
                        ui.set_search_model(slint::ModelRc::default());
                        return;
                    }
                    ui.set_search_loading(true);
                    let _ = cmd_tx.send(SourceCommand::Search(q));
                });
            }
        };
        let arm = Rc::new(arm);

        // the OSK: every key dispatches a synthetic key event
        // into the focused field — the virtual-keyboard pattern.
        // The TextInput handles chars, backspace ("backspace"),
        // submit ("return") natively; its edited() arms the
        // debounce like a physical keystroke would
        ui.on_osk_key({
            let ui_weak = ui.as_weak();
            move |k| {
                // NEVER dispatch from inside the pointer handler:
                // the synthetic key events re-enter the event loop
                // mid-press and wreck the mouse grab state — the
                // NEXT press lands on a stale grabber and dies.
                // Queue through the event loop instead: the press
                // finishes cleanly, then the key dispatches
                let ui_weak = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    // the specials travel as their control chars —
                    // key NAMES would land as literal text (the
                    // slint Key enum: Backspace = U+0008, Return =
                    // U+000A)
                    let text: slint::SharedString = match k.as_str() {
                        "backspace" => slint::platform::Key::Backspace.into(),
                        "return" => slint::platform::Key::Return.into(),
                        _ => k.clone(),
                    };
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                            text: text.clone(),
                        });
                    ui.window()
                        .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
                });
            }
        });

        // physical keyboard (and the synthetic events above):
        // the TextInput already synced the property (two-way) —
        // only the debounce arms
        ui.on_search_edited({
            let ui_weak = ui.as_weak();
            let arm = arm.clone();
            move |q| {
                // mirror the byte count — focus placement puts
                // the caret at the end (cursor offsets are bytes)
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_search_query_len(q.len() as i32);
                }
                arm();
            }
        });

        ui.on_search_fire({
            let ui_weak = ui.as_weak();
            let cmd_tx = cmd_tx.clone();
            move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                let q = ui.get_search_query().to_string();
                if q.chars().count() < MIN_CHARS {
                    ui.set_search_loading(false);
                    ui.set_search_model(slint::ModelRc::default());
                    return;
                }
                ui.set_search_loading(true);
                let _ = cmd_tx.send(SourceCommand::Search(q));
            }
        });

        ui.on_search_clear({
            let ui_weak = ui.as_weak();
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_search_query_len(0);
                    ui.set_search_query("".into());
                    ui.set_search_model(slint::ModelRc::default());
                    ui.set_search_loading(false);
                }
            }
        });

        // result taps: tracks play now (peek the now-playing like
        // the library does), artists/albums hand over to the
        // library drill. A search result carries its own id in
        // item_id — the drill helpers read the goto fields
        // (artist_id/album_id), so remap onto a subject entry
        // like the queue go-to does
        ui.on_search_open({
            let ui_weak = ui.as_weak();
            let cmd_tx = cmd_tx.clone();
            let nav = nav.clone();
            let library = library.clone();
            move |entry| {
                let Some(ui) = ui_weak.upgrade() else { return };
                let kind = entry.kind.to_string();
                match kind.as_str() {
                    "track" => {
                        let uri = entry.uri.to_string();
                        if uri.is_empty() {
                            return;
                        }
                        let _ = cmd_tx.send(SourceCommand::PlayMedia {
                            uri,
                            enqueue: Enqueue::Default,
                        });
                        if auto_open_np {
                            nav.open_np(&ui);
                        }
                    }
                    "artist" => {
                        let subject = LibEntry {
                            artist: entry.name.clone(),
                            artist_id: entry.item_id.clone(),
                            artist_uri: entry.uri.clone(),
                            provider: entry.provider.clone(),
                            image_url: entry.image_url.clone(),
                            ..LibEntry::default()
                        };
                        nav.go(&ui, Page::Library);
                        library.goto_artist(&ui, &cmd_tx, &subject);
                    }
                    "album" => {
                        let subject = LibEntry {
                            album_name: entry.name.clone(),
                            album_id: entry.item_id.clone(),
                            artist: entry.artist.clone(),
                            provider: entry.provider.clone(),
                            image_url: entry.image_url.clone(),
                            ..LibEntry::default()
                        };
                        nav.go(&ui, Page::Library);
                        library.goto_album(&ui, &cmd_tx, &subject);
                    }
                    _ => {}
                }
            }
        });
    }

    /// apply a search reply: rows as plain glyph rows — row art
    /// lives in the library drill headers, not in flat result
    /// lists (30 covers per keystroke round is not worth it).
    /// Free-standing — the provider thread holds no session
    /// state; answers keyed by a query that is no longer current
    /// drop silently
    pub fn apply_results(ui_weak: &slint::Weak<Mestro>, key: &str, entries: Vec<MediaEntry>) {
        let weak = ui_weak.clone();
        let key = key.to_string();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_search_query() != key {
                return;
            }
            let rows: Vec<LibEntry> = entries.iter().map(library::convert::lib_entry).collect();
            ui.set_search_loading(false);
            ui.set_search_model(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    }
}
