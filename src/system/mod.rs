//! Device-level system logic behind the `wifi-setup` feature: the
//! wifi setup lane (`wifi/` — wpa_supplicant ctrl client, AP mode
//! with the captive-portal policy, the join state machine) and the
//! factory reset. Everything runs on its own thread or the UI
//! thread, never on the web runtime; desktop builds compile without
//! it (the protocol reports the missing capabilities).

pub mod reset;
pub mod wifi;
