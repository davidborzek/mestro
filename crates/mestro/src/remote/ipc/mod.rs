//! the local IPC (ctl socket): `mestro ctl <command>` talks to
//! the running instance. A deliberately dumb transport in the
//! `mestro_ipc::command` sense — the vocabulary is the pure
//! control/navigation commands ("play-pause", "volume-up",
//! "back", "home", …), the answer is one line: `ok` or
//! `err <reason>`. Settings writes are NOT part of this surface:
//! the settings UI and the web endpoint own them.
//!
//! The socket lives in the user-private state dir (0600). The
//! listener thread only validates the line; the UI thread does
//! the work through the `control-command` callback, the same
//! crossing the web endpoint and the keyboard use.

pub mod client;
pub mod server;

pub use client::client;
pub use server::serve;

/// where the running instance listens: $XDG_STATE_HOME/mestro/ctl.sock,
/// or $MESTRO_CTL_SOCK when set — the kiosk keeps its persistent
/// state (settings.toml) on a FAT data partition, which cannot host
/// unix sockets
pub fn socket_path() -> std::path::PathBuf {
    std::env::var_os("MESTRO_CTL_SOCK")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| mestro_os::paths::state_dir().join("ctl.sock"))
}
