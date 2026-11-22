//! Device-level system logic, one feature per lane:
//! - `wifi-setup`: the wifi setup lane (`wifi/` — wpa_supplicant
//!   ctrl client, AP mode with the captive-portal policy, the join
//!   state machine) and the factory reset
//! - `audio-output` / `timezone`: the env files mestro owns and writes
//!   whole — squeezelite's (`squeezelite`, from the `audio.*`
//!   settings) and mestro's own (`TZ`, from `system.timezone`)
//!
//! Wifi runs on its own thread or the UI thread, never on the web
//! runtime; desktop builds compile without any of it (the protocol
//! reports the missing capabilities).

#[cfg(feature = "audio-output")]
pub mod audio;
pub mod paths;
#[cfg(feature = "wifi-setup")]
pub mod reset;
#[cfg(feature = "audio-output")]
pub mod squeezelite;
#[cfg(feature = "timezone")]
pub mod timezone;
#[cfg(feature = "wifi-setup")]
pub mod wifi;
