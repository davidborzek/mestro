//! Characterization scenarios for what the display shows: MA wire
//! frames in → published `PlayerState` snapshots and cover requests
//! out. They pin today's behavior from the outside — the state
//! machine's guards, the external-source hand-over, the cover rules —
//! so a refactor of `player/state.rs` can be judged without touching
//! them. A `// CHARACTERIZED:` comment marks behavior that looks
//! wrong but is asserted as it is.
//!
//! This lives inside `providers::ma` because the wire parser and the
//! external tracker are private to it; nothing in production code was
//! widened for it.

mod external_sources;
mod harness;
mod queue;
