//! Navigation: a router, like the web.
//!
//! - the ROUTE stack: every navigation pushes a route — a page
//!   plus its page-local subview (`(Page, i32)`, subview 0 = the
//!   page itself). Back pops the stack and restores the exact
//!   route; page-internal navigation (settings detail pages,
//!   later library detail views) pushes ordinary routes — no
//!   page-specific logic in the router.
//! - the NP layer: now-playing and the queue, both always-mounted
//!   overlays above the routed pages. Its stack is [Np] or [Np,
//!   Queue]: opening the queue peeks from now-playing, back walks
//!   back to the peek, the chevron closes the whole layer.
//!
//! The transitions live in the pure `NavState` core (unit-tested
//! below); `Navigation` is the thin shell that syncs the slint UI
//! and resets the library session when the library page is left.

use crate::providers::SourceCommand;
use crate::{Mestro, Page};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// The NP layer stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// the now-playing overlay
    Np,
    /// the queue overlay (peeked from now-playing)
    Queue,
    /// the VU faceplate overlay (peeked from now-playing)
    Vu,
}

/// Pure navigation state — no UI, fully testable.
#[derive(Default, Debug)]
struct NavState {
    /// the route stack (below the current route): every push
    /// (page switch or subview) lands here, back pops it
    root: Vec<(Page, i32)>,
    page: Option<Page>,
    /// the current page-local subview (0 = the page itself);
    /// the page decides what the numbers mean — the router
    /// only stores and restores them
    sub: i32,
    /// the overlay layer: empty = closed, [Np] or [Np, Queue]
    layer: Vec<Layer>,
    /// how often the library session was ended by leaving the page
    resets: usize,
    /// an external source plays: there is no queue to show
    queue_unavailable: bool,
}

impl NavState {
    fn page(&self) -> Page {
        self.page.unwrap_or(Page::Hub)
    }

    /// the current route
    fn route(&self) -> (Page, i32) {
        (self.page(), self.sub)
    }

    /// push the current route and switch to the target page's
    /// root view. Returns true when the transition ended a
    /// library session. Re-entering the same page at its root is
    /// a no-op; re-entering it from a subview pops back to the
    /// root like a nav link — no duplicate stack entries.
    fn go(&mut self, target: Page) -> bool {
        let (prev, sub) = self.route();
        if prev == target && sub == 0 {
            tracing::debug!("nav: go {prev:?} — already there");
            return false;
        }
        tracing::debug!("nav: go {prev:?}/{sub} -> {target:?}");
        self.root.push((prev, sub));
        self.page = Some(target);
        self.sub = 0;
        self.left_library(prev, target)
    }

    /// page-internal navigation: push the current route, switch
    /// the subview. Never leaves the page — no library reset
    fn push_sub(&mut self, sub: i32) {
        let (page, prev) = self.route();
        tracing::debug!("nav: push_sub {page:?}/{prev} -> {sub}");
        self.root.push((page, prev));
        self.sub = sub;
    }

    /// back: the NP layer walks first (queue → now-playing →
    /// closed); with no layer, the root stack pops. Returns true
    /// when the transition ended a library session.
    fn back(&mut self) -> bool {
        // the NP top closes as a whole — a back from the overlay
        // leaves it (same as the chevron); Queue/Vu pop back to
        // the now-playing peek
        if self.layer.last() == Some(&Layer::Np) {
            self.layer.clear();
            tracing::debug!("nav: back closes the np overlay");
            return false;
        }
        if let Some(top) = self.layer.pop() {
            tracing::debug!("nav: layer back {top:?}");
            debug_assert!(
                matches!(top, Layer::Queue | Layer::Vu),
                "the Np top closes via close_np"
            );
            // queue/vu back → now-playing peek
            return false;
        }
        let prev = self.page();
        tracing::debug!(
            "nav: back {prev:?}/{} (stack depth {})",
            self.sub,
            self.root.len()
        );
        match self.root.pop() {
            Some((p, sub)) => {
                self.page = Some(p);
                self.sub = sub;
                self.left_library(prev, p)
            }
            None => {
                self.page = Some(Page::Hub);
                self.sub = 0;
                self.left_library(prev, Page::Hub)
            }
        }
    }

    /// jump straight to the hub: the whole history AND the
    /// overlay layers are discarded. Returns true when a
    /// library session ended.
    fn go_home(&mut self) -> bool {
        let prev = self.page();
        tracing::debug!("nav: home {prev:?} (root depth {})", self.root.len());
        self.layer.clear();
        self.root.clear();
        self.page = Some(Page::Hub);
        self.sub = 0;
        self.left_library(prev, Page::Hub)
    }

    /// open the overlay from the capsule / standby wake
    fn open_np(&mut self) {
        if self.layer.is_empty() {
            self.layer.push(Layer::Np);
        }
    }

    /// the chevron: close the whole layer — the root page below
    /// was never left
    fn close_np(&mut self) {
        self.layer.clear();
    }

    /// the queue button inside now-playing: push the queue onto
    /// the layer stack
    fn np_open_queue(&mut self) {
        if self.queue_unavailable {
            return;
        }
        if self.layer.last() == Some(&Layer::Np) {
            self.layer.push(Layer::Queue);
        }
    }

    /// the VU button inside now-playing: push the faceplate onto
    /// the layer stack
    fn np_open_vu(&mut self) {
        if self.layer.last() == Some(&Layer::Np) {
            self.layer.push(Layer::Vu);
        }
    }

    /// the ctl/remote "show X" semantics (vs. the UI buttons'
    /// push semantics): whatever overlay is open gets replaced —
    /// the commands must work from anywhere
    fn show_np(&mut self) {
        self.layer.clear();
        self.layer.push(Layer::Np);
    }
    fn show_queue(&mut self) {
        if self.queue_unavailable {
            return;
        }
        self.show_np();
        self.layer.push(Layer::Queue);
    }

    /// an external source takes the queue away (or gives it back):
    /// an open queue view falls back to now playing
    fn set_queue_unavailable(&mut self, unavailable: bool) {
        self.queue_unavailable = unavailable;
        if unavailable {
            self.layer.retain(|layer| *layer != Layer::Queue);
        }
    }
    fn show_vu(&mut self) {
        self.show_np();
        self.layer.push(Layer::Vu);
    }

    /// leaving the layer via a go-to (queue row → library): the
    /// layer closes, the root stack takes the navigation
    fn leave_layer(&mut self) {
        self.layer.clear();
    }

    /// leaving the layer via a go-to (queue row → library): the
    /// layer closes, the root stack takes the navigation. Landing
    /// on the page we're already on starts a FRESH session
    /// instead of pushing a pointless same-page entry — back then
    /// returns to whatever led to that page.
    fn goto_from_queue(&mut self, target: Page) -> bool {
        self.leave_layer();
        if self.page() == target {
            tracing::debug!("nav: queue goto, same page {target:?} — fresh session");
            self.resets += 1;
            true
        } else {
            self.go(target)
        }
    }

    /// leaving the library page ends its browse session; overlay
    /// peeks don't count — the page below never changed
    /// leaving the library page ends its browse session; overlay
    /// peeks don't count — the page below never changed.
    /// Returns true when the session ended.
    fn left_library(&mut self, prev: Page, next: Page) -> bool {
        if prev == Page::Library && next != Page::Library {
            tracing::debug!("nav: left library {prev:?} -> {next:?} — session ends");
            self.resets += 1;
            true
        } else {
            false
        }
    }
}

pub type CmdTx = tokio::sync::mpsc::UnboundedSender<SourceCommand>;

/// The navigation state, wired into the UI once at startup.
#[derive(Clone)]
pub struct Navigation {
    state: Rc<RefCell<NavState>>,
    /// reset hook: leaving the library drops its browse session
    library: Rc<RefCell<Option<crate::library::Library>>>,
}

impl Navigation {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(NavState::default())),
            library: Rc::new(RefCell::new(None)),
        }
    }

    /// push the current page and switch to the target
    pub fn go(&self, ui: &Mestro, target: Page) {
        ui.set_sheet_close_token(ui.get_sheet_close_token() + 1);
        let fire = {
            let mut s = self.state.borrow_mut();
            s.go(target)
        };
        self.sync(ui, fire);
    }

    /// page-internal navigation: the subview joins the route
    /// stack — back returns to the caller, like the web
    pub fn push_sub(&self, ui: &Mestro, sub: i32) {
        ui.set_sheet_close_token(ui.get_sheet_close_token() + 1);
        self.state.borrow_mut().push_sub(sub);
        // a pushed subview opens at the top (back restores instead)
        ui.set_subview_pushes(ui.get_subview_pushes() + 1);
        self.sync(ui, false);
    }

    /// back: the page modal slot (sheet-open, see mestro.slint)
    /// sits above the route stack — back dismisses it first,
    /// then pops the route
    pub fn back(&self, ui: &Mestro) {
        if ui.get_vu_open() && ui.get_vu_overview_open() {
            ui.set_vu_overview_open(false);
            return;
        }
        let sheet = ui.get_sheet_open();
        tracing::debug!("nav: back (sheet-open={sheet})");
        if sheet {
            ui.set_sheet_close_token(ui.get_sheet_close_token() + 1);
            return;
        }
        let fire = {
            let mut s = self.state.borrow_mut();
            s.back()
        };
        self.sync(ui, fire);
    }

    /// the home button: jump to the hub, drop the history
    pub fn home(&self, ui: &Mestro) {
        ui.set_sheet_close_token(ui.get_sheet_close_token() + 1);
        let fire = {
            let mut s = self.state.borrow_mut();
            s.go_home()
        };
        self.sync(ui, fire);
    }

    /// open the overlay (capsule tap, standby wake)
    pub fn open_np(&self, ui: &Mestro) {
        self.state.borrow_mut().open_np();
        self.sync(ui, false);
    }

    /// the chevron: close the whole overlay layer
    pub fn close_np(&self, ui: &Mestro) {
        self.state.borrow_mut().close_np();
        self.sync(ui, false);
    }

    /// open the queue button inside now-playing
    pub fn np_open_queue(&self, ui: &Mestro) {
        self.state.borrow_mut().np_open_queue();
        self.sync(ui, false);
    }

    /// the VU button inside now-playing
    pub fn np_open_vu(&self, ui: &Mestro) {
        self.state.borrow_mut().np_open_vu();
        self.sync(ui, false);
    }

    /// ctl/remote: show the now-playing layer — any overlay on
    /// top of it drops off
    pub fn show_np(&self, ui: &Mestro) {
        self.state.borrow_mut().show_np();
        self.sync(ui, false);
    }

    /// follow the player's source: while an external source plays the
    /// queue view is closed (back on now playing) and cannot be opened
    pub fn set_queue_unavailable(&self, ui: &Mestro, unavailable: bool) {
        let changed = {
            let mut s = self.state.borrow_mut();
            let changed = s.queue_unavailable != unavailable;
            s.set_queue_unavailable(unavailable);
            changed
        };
        if changed {
            self.sync(ui, false);
        }
    }

    /// ctl/remote: show the queue over the np peek
    pub fn show_queue(&self, ui: &Mestro) {
        self.state.borrow_mut().show_queue();
        self.sync(ui, false);
    }

    /// ctl/remote: show the VU faceplate over the np peek
    pub fn show_vu(&self, ui: &Mestro) {
        self.state.borrow_mut().show_vu();
        self.sync(ui, false);
    }

    /// a go-to from the queue overlay (queue row → library): the
    /// layer closes and the root stack takes over
    pub fn goto_from_queue(&self, ui: &Mestro, target: Page) {
        let fire = {
            let mut s = self.state.borrow_mut();
            s.goto_from_queue(target)
        };
        self.sync(ui, fire);
    }

    /// push the UI from the state; end the library session when
    /// the transition demands it
    fn sync(&self, ui: &Mestro, fire_reset: bool) {
        let s = self.state.borrow();
        ui.set_page(s.page());
        ui.set_subview(s.sub);
        ui.set_np_open(s.layer.last() == Some(&Layer::Np));
        ui.set_queue_open(s.layer.last() == Some(&Layer::Queue));
        let vu_open = s.layer.last() == Some(&Layer::Vu);
        let vu_changed = ui.get_vu_open() != vu_open;
        ui.set_vu_open(vu_open);
        drop(s);
        if vu_changed {
            ui.invoke_vu_sync();
        }
        if fire_reset {
            if let Some(lib) = self.library.borrow().as_ref() {
                lib.reset(ui);
            }
        }
    }

    /// all navigation UI callbacks. Entering the library triggers
    /// its root fetch (ensure_root), so the library handle rides
    /// along.
    pub fn wire(&self, ui: &Mestro, cmd_tx: &CmdTx, library: crate::library::Library) {
        *self.library.borrow_mut() = Some(library.clone());
        let ui_weak = ui.as_weak();
        let nav = self.clone();
        let cmd_tx = cmd_tx.clone();
        ui.on_nav_go(move |target| {
            let Some(ui) = ui_weak.upgrade() else { return };
            // first entry into the library: fetch the provider
            // list (idempotent — sees existing state)
            if target == Page::Library {
                library.ensure_root(&ui, &cmd_tx);
            }
            nav.go(&ui, target);
        });

        let ui_weak = ui.as_weak();
        let nav = self.clone();
        ui.on_nav_back(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            nav.back(&ui);
        });

        // page-internal navigation (settings detail pages, later
        // library detail): pushes a route — back returns here
        let ui_weak = ui.as_weak();
        let nav = self.clone();
        ui.on_push_subview(move |sub| {
            if let Some(ui) = ui_weak.upgrade() {
                nav.push_sub(&ui, sub);
            }
        });

        let ui_weak = ui.as_weak();
        let nav = self.clone();
        ui.on_nav_home(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            nav.home(&ui);
        });

        let ui_weak = ui.as_weak();
        let nav = self.clone();
        ui.on_open_np(move || {
            if let Some(ui) = ui_weak.upgrade() {
                nav.open_np(&ui);
            }
        });

        let ui_weak = ui.as_weak();
        let nav = self.clone();
        ui.on_close_np(move || {
            if let Some(ui) = ui_weak.upgrade() {
                nav.close_np(&ui);
            }
        });

        let ui_weak = ui.as_weak();
        let nav = self.clone();
        ui.on_np_open_queue(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            nav.np_open_queue(&ui);
        });

        // the VU button inside now-playing: the faceplate joins
        // the NP layer like the queue — back returns to the peek
        let ui_weak = ui.as_weak();
        let nav = self.clone();
        ui.on_np_open_vu(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            nav.np_open_vu(&ui);
        });
    }
}

impl Default for Navigation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> NavState {
        NavState::default()
    }

    /// hub → library → back lands on the hub, root stack drained
    #[test]
    fn go_and_back_restores_page() {
        let mut s = state();
        s.go(Page::Library);
        assert_eq!(s.page(), Page::Library);
        s.back();
        assert_eq!(s.page(), Page::Hub);
        assert!(s.root.is_empty());
    }

    /// back on an empty stack falls back to the hub
    #[test]
    fn back_on_empty_stack_lands_on_hub() {
        let mut s = state();
        s.back();
        assert_eq!(s.page(), Page::Hub);
    }

    /// the overlay peeks over anything without touching the root
    #[test]
    fn np_peek_never_touches_the_root_stack() {
        let mut s = state();
        s.go(Page::Library);
        s.open_np();
        assert_eq!(s.layer, vec![Layer::Np]);
        s.close_np();
        assert!(s.layer.is_empty());
        assert_eq!(s.page(), Page::Library);
        assert_eq!(s.root.len(), 1);
        s.back();
        assert_eq!(s.page(), Page::Hub);
    }

    /// queue from now-playing: back returns to the peek, the
    /// chevron closes the whole layer
    #[test]
    fn queue_from_np_walks_the_layer() {
        let mut s = state();
        s.open_np();
        s.np_open_queue();
        assert_eq!(s.layer, vec![Layer::Np, Layer::Queue]);
        s.back();
        assert_eq!(s.layer, vec![Layer::Np], "queue back → now-playing");
        // the Np top has no back — its chevron closes the layer
        s.close_np();
        assert!(s.layer.is_empty());
    }

    /// the home button discards the whole history: one tap from
    /// any depth lands on the hub; leaving a library fires the
    /// session reset; back on the hub stays put
    #[test]
    fn home_drops_the_history_and_lands_on_hub() {
        let mut s = state();
        s.go(Page::Library); // root=[hub]
                             // switching away from the library ends ITS session here
        assert!(s.go(Page::Settings), "library → settings reports the reset");
        assert_eq!(s.root.len(), 2);
        assert!(!s.go_home(), "the library session already ended");
        assert_eq!(s.page(), Page::Hub);
        assert!(s.root.is_empty());
        // back on the emptied stack stays on the hub
        assert!(!s.back());
        assert_eq!(s.page(), Page::Hub);
    }

    /// home pressed ON the library page itself: the history drops
    /// and the browse session ends — the next visit starts fresh
    #[test]
    fn home_from_the_library_resets() {
        let mut s = state();
        s.go(Page::Library); // root=[hub], page=library
        assert!(s.go_home(), "home out of the library reports the reset");
        assert_eq!(s.page(), Page::Hub);
        assert!(s.root.is_empty());
        assert_eq!(s.resets, 1);
    }

    /// home while already on the hub changes nothing
    #[test]
    fn home_on_hub_is_a_noop() {
        let mut s = state();
        assert!(!s.go_home());
        assert_eq!(s.page(), Page::Hub);
    }

    /// re-entering the page you're on must not pile up history
    #[test]
    fn go_to_the_same_page_is_a_noop() {
        let mut s = state();
        s.go(Page::Settings);
        assert_eq!(s.root.len(), 1);
        assert!(!s.go(Page::Settings));
        assert_eq!(s.root.len(), 1, "no duplicate entry");
        assert_eq!(s.page(), Page::Settings);
        s.back();
        assert_eq!(s.page(), Page::Hub, "one back still exits");
    }

    /// subviews push ordinary routes: back walks them in reverse
    /// (index → now-playing → buttons → back to now-playing →
    /// back to index → back to the previous page)
    #[test]
    fn subviews_walk_the_route_stack() {
        let mut s = state();
        s.go(Page::Settings); // route stack: [(hub, 0)]
        s.push_sub(1); // now-playing: [(hub, 0), (settings, 0)]
        s.push_sub(4); // buttons: [..., (settings, 1)]
        assert_eq!(s.route(), (Page::Settings, 4));
        s.back();
        assert_eq!(s.route(), (Page::Settings, 1), "buttons → now-playing");
        s.back();
        assert_eq!(s.route(), (Page::Settings, 0), "now-playing → index");
        s.back();
        assert_eq!(s.page(), Page::Hub, "index → the previous page");
    }

    /// a page switch from a subview pushes the exact route —
    /// back returns to it (like a web back)
    #[test]
    fn back_restores_the_exact_subview() {
        let mut s = state();
        s.go(Page::Settings);
        s.push_sub(5); // quality
        s.go(Page::Hub); // the rail links push routes
        assert_eq!(s.route(), (Page::Hub, 0));
        s.back();
        assert_eq!(s.route(), (Page::Settings, 5), "back → the quality page");
    }

    /// tapping the rail link of the page you're on pops to its
    /// root — a subview is not a place you're stuck on
    #[test]
    fn go_to_the_same_page_from_a_subview_pops_to_root() {
        let mut s = state();
        s.go(Page::Settings);
        s.push_sub(4); // buttons
        s.go(Page::Settings); // the rail link
        assert_eq!(s.route(), (Page::Settings, 0));
        s.back();
        assert_eq!(s.route(), (Page::Settings, 4), "back → buttons again");
    }

    /// the VU faceplate walks the layer like the queue: peek over
    /// now-playing, back returns to the peek, the chevron closes
    /// everything
    #[test]
    fn vu_from_np_walks_the_layer() {
        let mut s = state();
        s.open_np();
        s.np_open_vu();
        assert_eq!(s.layer, vec![Layer::Np, Layer::Vu]);
        s.back();
        assert_eq!(s.layer, vec![Layer::Np]);
        s.np_open_vu();
        s.close_np();
        assert!(s.layer.is_empty());
        assert_eq!(s.page(), Page::Hub, "the root stack was never touched");
    }

    /// the queue go-to chain: the layer closes, the root stack
    /// takes the navigation — landing back on the root page is
    /// fine
    #[test]
    fn queue_goto_leaves_the_layer() {
        let mut s = state();
        s.open_np();
        s.np_open_queue(); // np over hub, queue layer
        s.goto_from_queue(Page::Library);
        assert!(s.layer.is_empty());
        assert_eq!(s.page(), Page::Library);
        assert_eq!(s.resets, 0, "the root page was not the library");
        s.back();
        assert_eq!(s.page(), Page::Hub);
    }

    /// go-to while the root page is ALREADY the library: no
    /// same-page stack entry — a fresh session, back returns to
    /// the page that led to the library in one press
    #[test]
    fn queue_goto_from_library_page_starts_fresh() {
        let mut s = state();
        s.go(Page::Library); // root=[hub], page=library
        s.open_np(); // peek over the library
        s.np_open_queue();
        s.goto_from_queue(Page::Library);
        assert!(s.layer.is_empty());
        assert_eq!(s.root.len(), 1, "no same-page entry pushed");
        assert_eq!(s.resets, 1, "the library session restarted");
        // one back: out of the library to its predecessor
        s.back();
        assert_eq!(s.page(), Page::Hub);
    }

    /// leaving the library ends the browse session — a queue
    /// go-to would otherwise strand the next library visit inside
    /// the detail with no way back to its parents
    #[test]
    fn leaving_the_library_resets_the_session() {
        let mut s = state();
        s.go(Page::Library);
        assert_eq!(s.resets, 0);
        // the transition REPORTS the session end — the shell fires
        // the reset from this, not from a counter diff
        assert!(s.back(), "back out of the library must report the reset");
        assert_eq!(s.resets, 1);
    }

    /// an external source takes the queue away: an open queue view
    /// falls back to the now-playing peek, and none opens until the
    /// queue is back
    #[test]
    fn queue_view_closes_while_a_source_is_external() {
        let mut s = state();
        s.open_np();
        s.np_open_queue();
        assert_eq!(s.layer, vec![Layer::Np, Layer::Queue]);
        s.set_queue_unavailable(true);
        assert_eq!(s.layer, vec![Layer::Np], "queue → now playing");

        s.np_open_queue();
        assert_eq!(s.layer, vec![Layer::Np], "the queue button is dead");
        s.show_queue();
        assert_eq!(s.layer, vec![Layer::Np], "ctl/web 'queue' is dead too");

        s.set_queue_unavailable(false);
        s.np_open_queue();
        assert_eq!(s.layer, vec![Layer::Np, Layer::Queue]);
    }

    /// the queue went away while the user was on another overlay
    #[test]
    fn the_other_overlays_survive_the_queue_going_away() {
        let mut s = state();
        s.open_np();
        s.np_open_vu();
        s.set_queue_unavailable(true);
        assert_eq!(s.layer, vec![Layer::Np, Layer::Vu]);
        s.go(Page::Library);
        assert_eq!(s.page(), Page::Library);
    }

    /// np peeks over the library don't reset anything
    #[test]
    fn np_peeks_over_library_do_not_reset() {
        let mut s = state();
        s.go(Page::Library);
        s.open_np();
        s.np_open_queue();
        s.back(); // back to the peek
        assert_eq!(s.resets, 0);
        assert_eq!(s.page(), Page::Library);
    }

    /// switching library → another root page ends the session
    #[test]
    fn library_to_other_page_resets() {
        let mut s = state();
        s.go(Page::Library);
        assert!(s.go(Page::Hub), "leaving the library reports the reset");
        assert_eq!(s.resets, 1);
    }
}
