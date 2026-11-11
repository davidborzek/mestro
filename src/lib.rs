// what mestro does
pub mod config;
pub mod display;
pub mod library;
pub mod player;
pub mod settings;
pub mod standby;
pub mod ui;
pub mod vu;

// how it talks to the outside
pub mod providers;
pub mod remote;

slint::include_modules!();
