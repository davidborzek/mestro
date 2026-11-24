// unsafe has no place in the player domain — the audited sites
// live in mestro-os
#![forbid(unsafe_code)]

//! The player domain: the playback source contract ([`source`]),
//! the display state machine ([`state`]), the imperative shell
//! ([`shell`]), the cover seam ([`cover`]) and the artwork palette
//! ([`palette`]) — pure logic plus channels, no UI types. The
//! binary adds the now-playing view, the queue, the MA provider
//! and the HTTP cover fetcher.

pub mod cover;
pub mod palette;
pub mod shell;
pub mod source;
pub mod state;
