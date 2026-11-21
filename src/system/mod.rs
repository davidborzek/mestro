//! Device-level system logic, one feature per lane:
//! - `wifi-setup`: the wifi setup lane (`wifi/` — wpa_supplicant
//!   ctrl client, AP mode with the captive-portal policy, the join
//!   state machine) and the factory reset
//! - `audio-output` / `timezone`: the image's user-editable env
//!   files (`envfile` — one key rewritten, everything else kept)
//!
//! Wifi runs on its own thread or the UI thread, never on the web
//! runtime; desktop builds compile without any of it (the protocol
//! reports the missing capabilities).

#[cfg(feature = "audio-output")]
pub mod audio;
#[cfg(any(feature = "audio-output", feature = "timezone"))]
pub mod envfile;
#[cfg(feature = "wifi-setup")]
pub mod reset;
#[cfg(feature = "wifi-setup")]
pub mod wifi;
