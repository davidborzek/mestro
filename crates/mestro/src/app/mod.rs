//! The running app's wiring: what `main` assembles, split by
//! concern. Binary-side (it owns the UI handles and the provider
//! channels); the library keeps the reusable pieces.

pub mod clock;
pub mod debug;
pub mod dispatch;
pub mod display;
pub mod theme;
