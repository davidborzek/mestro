//! squeezelite: the VISEXPORT shm reader ([`visexport`], always
//! built) and, behind the `audio-output` feature, the ALSA output
//! discovery and picker (`outputs`) plus the env-file writer
//! (`squeezelite`) — fed an [`AudioSettings`] value, never the
//! binary's config types. `outputs` is the renderer-neutral device
//! half (a future in-process player reuses it); `squeezelite` and
//! `visexport` are the squeezelite renderer's own.

/// the `audio.*` settings as data: what the env file is composed
/// from. The config file's `[audio]` section stays in the binary;
/// this is the value that crosses the module edge
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioSettings {
    /// the ALSA device squeezelite plays on (`hw:CARD=<id>`); empty =
    /// a USB DAC, else an I2S HAT
    pub output: String,
    /// the player's name in Music Assistant; empty = `mestro-XXXX`
    pub name: String,
    /// the Music Assistant host squeezelite connects to; empty = the
    /// host of the Music Assistant url
    pub server: String,
    /// further squeezelite flags, passed verbatim
    pub extra: String,
}

#[cfg(feature = "audio-output")]
pub mod outputs;
#[cfg(feature = "audio-output")]
pub mod squeezelite;
pub mod visexport;
