//! Everything that talks to the player device's OS or hardware —
//! one crate, a module per capability, the fakes beside the real
//! code. Nothing here knows about Music Assistant, the UI or the
//! web protocol; that separation is the crate's whole point (see
//! docs/architecture.md).
//!
//! unsafe is denied crate-wide and allowed only at the two audited
//! sites — the `kill(2)` wrapper below and the Pi firmware mailbox
//! ioctl in [`display`] — and `tests/unsafe_pin.rs` pins exactly
//! those two locations, so a third one fails CI.

pub mod atomic_file;
pub mod audio;
pub mod display;
pub mod identity;
pub mod paths;
#[cfg(feature = "wifi-setup")]
pub mod reset;
#[cfg(feature = "timezone")]
pub mod timezone;
pub mod wifi;

/// the LAN address a phone can reach (UDP connect trick — no
/// packets are sent, it only picks the outbound interface).
/// 127.0.0.1 while no interface has a route yet
pub fn lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("8.8.8.8:80")?;
            Ok(socket.local_addr()?.ip().to_string())
        })
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// send `signal` to `pid` (SIGTERM/SIGKILL …, `libc`'s constants) —
/// the one audited unsafe site of this crate
#[cfg(unix)]
pub fn signal(pid: i32, signal: i32) -> std::io::Result<()> {
    // SAFETY: a plain kill(2) on a pid we own; no invariants to
    // uphold beyond libc's own signature
    // (unsafe_code is denied crate-wide — this is the audited
    // signal-send path)
    #[allow(unsafe_code)]
    let ret = unsafe { libc::kill(pid, signal) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// no signals off unix
#[cfg(not(unix))]
pub fn signal(_pid: i32, _signal: i32) -> std::io::Result<()> {
    Ok(())
}
