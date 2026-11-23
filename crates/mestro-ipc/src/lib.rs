// unsafe has no place in the wire protocol
#![forbid(unsafe_code)]

//! The player's wire protocol, shared by the kiosk, the web app and
//! external tools: the control vocabulary ([`command`]), the web
//! endpoint's JSON requests and responses ([`web`]) and the ctl
//! socket's line framing ([`ctl`]). Pure data — no sockets, no UI,
//! no config, no sibling crate; the transports live in the binary.

pub mod command;
pub mod ctl;
pub mod web;
