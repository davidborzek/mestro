//! What is playing: the player state machine, the now-playing view,
//! the queue and the cover artwork (fetch, blurred backdrop).
//! The state machine, shell and cover seam live in
//! `mestro_player`, re-exported here under their historical paths.

pub mod cover;
pub mod nowplaying;
pub mod queue;
pub use mestro_player::shell;
pub use mestro_player::state;
