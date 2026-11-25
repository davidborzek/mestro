//! The image's A/B system update, neutral over the board's boot
//! chain. The update itself — streaming an image into the inactive
//! slot, verifying it, reporting the state — lives here; everything
//! the boot chain does (where slots are stored, how a trial boot is
//! armed and committed, how the board restarts) sits behind the
//! [`BootControl`] trait, with the Raspberry Pi firmware as the one
//! backend (the `rpi` feature). [`detect`] picks the backend at
//! runtime: one binary, every board.
//!
//! The whole system is one kernel image (the rootfs is the initramfs
//! inside it), so the two slots are two kernel images and only one
//! of them boots: an update writes the other, boots it once on trial
//! and only a commit makes it permanent — anything else falls back
//! to the previous slot on its own.
//!
//! The CLI over this (and the boot partition mounting) is the
//! `mestro-update` binary (`cli` feature); `POST /ota` uses the same
//! library in process (mestro runs as root on the image) — web
//! upload and SSH update share one writer.

mod install;
mod lock;

#[cfg(feature = "rpi")]
pub mod rpi;

use std::fmt::Write as _;
use std::io;

pub use install::{install, Error, Installed};
pub use lock::UpdateLock;
#[cfg(feature = "rpi")]
pub use rpi::RpiFirmware;

/// what the boot chain does for an update: where the slots live and
/// how a boot picks one. One implementation per boot chain (the
/// Raspberry Pi firmware, `rpi` feature); the update code talks only
/// to this, so a different board is a backend, not a rewrite
pub trait BootControl {
    /// make the slot store reachable (the boot partition mounted);
    /// paired with [`release`](Self::release)
    fn mount(&mut self) -> Result<(), Error>;

    /// let go of the slot store, if this control took it
    fn release(&mut self);

    /// the slot this boot runs
    fn active(&self) -> io::Result<String>;

    /// the slot a pending trial would boot, if one is pending
    fn pending(&self) -> io::Result<Option<String>>;

    /// is this boot a trial (the boot chain's one-shot flag)
    fn trial_boot(&self) -> bool;

    /// prepare the update: pick the inactive slot, drop whatever is
    /// stale on it and open the store the image streams into.
    /// Refuses when the running slot would be overwritten
    fn begin_update(&mut self) -> Result<UpdateTarget, Error>;

    /// the sha256 of the image as the store reads it back — behind
    /// any cache, off the real medium
    fn read_back(&mut self, target: &UpdateTarget) -> io::Result<String>;

    /// remove a failed write, so a half-written slot is never booted
    fn discard(&mut self, target: &UpdateTarget);

    /// arm a one-shot trial boot of the just-written slot
    fn arm_trial(&mut self, target: &UpdateTarget) -> io::Result<()>;

    /// make the trial permanent; nothing to commit is not an error
    fn commit(&mut self) -> Result<Commit, Error>;

    /// drop a pending trial that never committed; the trial's slot
    /// is handed back for the log line
    fn abandon(&mut self) -> io::Result<Option<String>>;

    /// restart the board, into the pending trial or on the current
    /// slot
    fn reboot(&mut self, into_trial: bool) -> io::Result<()>;
}

/// where a new image is being written: the slot it becomes, the slot
/// that keeps running, and the store it streams into
pub struct UpdateTarget {
    /// the slot the image is written to
    pub slot: String,
    /// the slot that keeps running until the trial proves the new one
    pub running: String,
    /// the store the image streams into (a file, a partition …):
    /// what lands here is what the next boot runs
    pub sink: Box<dyn std::io::Write>,
}

/// what [`detect`] found on this board
pub enum Detection {
    /// the boot chain of this board
    Backend(Box<dyn BootControl + Send>),
    /// no backend is compiled in (the desktop build)
    NotCompiledIn,
    /// backends are compiled in, but none fits this board
    UnsupportedBoard,
}

/// the boot chain of this board, detected at runtime — one binary,
/// every board. The Raspberry Pi firmware announces itself in the
/// device tree (chosen/bootloader, where its tryboot flag lives too);
/// anything else is not supported
pub fn detect() -> Detection {
    #[cfg(feature = "rpi")]
    {
        match rpi::RpiFirmware::detect() {
            Some(backend) => Detection::Backend(Box::new(backend)),
            None => Detection::UnsupportedBoard,
        }
    }
    #[cfg(not(feature = "rpi"))]
    {
        Detection::NotCompiledIn
    }
}

/// the update status, exactly the lines the player image reports
/// (the diagnostics download and the S99ota init script parse them)
pub fn status(backend: &dyn BootControl) -> io::Result<String> {
    let mut out = format!("active:  {}\n", backend.active()?);
    if let Some(pending) = backend.pending()? {
        out.push_str(&format!("pending: {pending}\n"));
    }
    out.push_str(if backend.trial_boot() {
        "this boot: trial\n"
    } else {
        "this boot: normal\n"
    });
    Ok(out)
}

/// this boot is a trial that has not been decided yet: the
/// commit (S99ota) or the abandon still needs the pending
/// `tryboot.txt`, and an install would delete exactly that — the
/// caller refuses instead
pub fn undecided_trial(backend: &dyn BootControl) -> io::Result<bool> {
    Ok(backend.trial_boot() && backend.pending()?.is_some())
}

/// the outcome of a commit
#[derive(Debug, Clone, PartialEq)]
pub enum Commit {
    /// not a trial boot, or no pending trial: nothing was changed
    NoTrial,
    /// the trial slot became the active one
    Committed { kernel: String },
}

/// lowercase hex, the sha256sum format everything here compares
/// against
pub(crate) fn sha256_hex(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
