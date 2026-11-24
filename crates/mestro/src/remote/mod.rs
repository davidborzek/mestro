//! Control from outside the touchscreen: the keyboard map, the
//! web endpoint's UI-side glue and the pairing QR code. The
//! vocabulary and the wire shapes they speak are the
//! `mestro-ipc` crate; the remote surface itself (the web
//! endpoint, the ctl socket, the push pipeline) is the
//! `mestro-remote` crate.

pub mod keyboard;
/// the binary's implementation of the mestro-remote ports: the
/// slint `UiPort` (`SlintUi`), the MA auth client and the
/// UI-thread op glue
pub mod ports;
pub mod qr;
