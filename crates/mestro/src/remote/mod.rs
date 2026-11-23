//! Control from outside the touchscreen: the keyboard map, the
//! local ctl socket, the embedded web endpoint and the pairing QR
//! code. The vocabulary and the wire shapes they speak are the
//! `mestro-ipc` crate.

pub mod ipc;
pub mod keyboard;
pub mod qr;
pub mod web;
