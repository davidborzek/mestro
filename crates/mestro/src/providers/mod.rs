//! Playback providers — the abstraction boundary between mestro's core
//! (state machine + UI) and a remote control protocol (Music Assistant,
//! later LMS/MPD). The source contract itself lives in
//! `mestro_player::source`, re-exported here under its historical path.

mod banner;
pub mod ma;
pub mod session;
pub mod startup;

pub use mestro_player::source::{
    Enqueue, ExternalSource, MediaEntry, PlaybackSource, QueueItem, RepeatMode, Retry, SourceCaps,
    SourceCommand, SourceEvent, TrackMeta,
};
