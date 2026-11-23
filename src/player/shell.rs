//! The imperative shell around the player state machine: the shared
//! slot the threads meet in, the channel the UI listens on, and the
//! carrying out of what `state::reduce` asks for (publish, cover
//! fetch whose result comes back as `Event::CoverLanded`).

use crate::locks;
use crate::player::cover::{cap_size, make_blur, CoverData, CoverSource};
use crate::player::state::{reduce, CoverRequest, Effects, Event, Live, PlayerState, PlayerUpdate};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub type Shared = Arc<Mutex<Option<Live>>>;

pub type UpdateTx = tokio::sync::mpsc::UnboundedSender<PlayerUpdate>;

/// What the ticker renders after a beat: position, duration, loading.
pub type Beat = (u64, u64, bool);

#[derive(Clone)]
pub struct Shell {
    shared: Shared,
    update_tx: UpdateTx,
    covers: Arc<CoverSource>,
}

impl Shell {
    pub fn new(shared: Shared, update_tx: UpdateTx, covers: CoverSource) -> Self {
        Self {
            shared,
            update_tx,
            covers: Arc::new(covers),
        }
    }

    /// the slot the state lives in, for readers (the transport asks what
    /// the source can do)
    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// run one event at `now`: reduce it under the lock, then carry out
    /// the effects. The state does not exist before the first transport,
    /// options or queue event; any other event finds nothing to change.
    pub fn dispatch(&self, event: Event, now: Instant) {
        let effects = {
            let mut slot = locks::lock(&self.shared);
            match slot.as_mut() {
                Some(live) => reduce(live, event, now),
                None => match Live::bootstrap(event, now) {
                    Some((live, effects)) => {
                        *slot = Some(live);
                        effects
                    }
                    None => return,
                },
            }
        };
        self.execute(effects);
    }

    /// like `dispatch`, for an event that depends on the state it meets:
    /// `decide` sees the state under the same lock the reduce runs in, so
    /// nothing can change in between. No state yet, no event.
    pub fn dispatch_with(&self, now: Instant, decide: impl FnOnce(&PlayerState) -> Option<Event>) {
        let effects = {
            let mut slot = locks::lock(&self.shared);
            let Some(live) = slot.as_mut() else { return };
            let Some(event) = decide(&live.state) else {
                return;
            };
            reduce(live, event, now)
        };
        self.execute(effects);
    }

    /// the ticker beat: the loading timeout and the position
    /// interpolation advance, the render values come back
    pub fn tick(&self, now: Instant) -> Option<Beat> {
        let (effects, beat) = {
            let mut slot = locks::lock(&self.shared);
            let live = slot.as_mut()?;
            let effects = reduce(live, Event::Tick, now);
            let state = &live.state;
            (
                effects,
                (state.position_ms, state.duration_ms, state.loading),
            )
        };
        self.execute(effects);
        Some(beat)
    }

    fn execute(&self, effects: Effects) {
        if let Some(publish) = effects.publish {
            let _ = self.update_tx.send(PlayerUpdate {
                state: publish.state,
                cover: None,
                bg: None,
                palette: None,
                toast: publish.toast,
                osd_volume: publish.osd_volume,
            });
        }
        if let Some(request) = effects.fetch_cover {
            self.fetch_cover(request);
        }
    }

    fn fetch_cover(&self, request: CoverRequest) {
        let CoverRequest { url, external } = request;
        let shell = self.clone();
        let landing_url = url.clone();
        self.covers.fetcher.fetch(
            url,
            Box::new(move |mut cover| {
                let url = landing_url;
                if let Some(c) = &mut cover {
                    cap_size(c, 900);
                }
                tracing::debug!(
                    "cover fetch {:?} -> {}",
                    url,
                    cover
                        .as_ref()
                        .map(|c| format!("{}x{}", c.width, c.height))
                        .unwrap_or_else(|| "FAIL".into())
                );
                let bg = make_blur(&cover);
                shell.land_cover(url, external, cover, bg);
            }),
        );
    }

    /// The state is read at LANDING time, not spawn time: a slow cover
    /// fetch must not resurrect a stale loading flag in the UI (and a
    /// result that lands after the source mode flipped belongs to the
    /// other mode: the reducer drops it).
    fn land_cover(
        &self,
        url: String,
        external: bool,
        cover: Option<CoverData>,
        bg: Option<CoverData>,
    ) {
        let landed = Event::CoverLanded {
            url,
            external,
            found: cover.is_some(),
        };
        let effects = {
            let mut slot = locks::lock(&self.shared);
            // a landing has no clock of its own: the reducer ignores the time
            slot.as_mut()
                .map(|live| reduce(live, landed, Instant::now()))
        };
        let Some(publish) = effects.and_then(|effects| effects.publish) else {
            return;
        };
        let palette = cover
            .as_ref()
            .and_then(|c| crate::ui::palette::from_cover(c, self.covers.variant.clone()));
        if let Some(p) = palette {
            let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
            tracing::info!(
                "palette: accent {} + tertiary {} + danger {}",
                hex(p.accent),
                hex(p.tertiary),
                hex(p.danger)
            );
        }
        let _ = self.update_tx.send(PlayerUpdate {
            state: publish.state,
            cover,
            bg,
            palette,
            toast: None,
            osd_volume: None,
        });
    }
}
