//! Restarting the board for an update: the last code that runs on
//! it — no shutdown script comes after this.

use std::io;

/// clean reboot of the board: sync, best-effort unmount of the data
/// and boot partitions, then the raw reboot(2). busybox `reboot`
/// asks PID 1 to restart userspace, but the whole system lives in
/// an initramfs: busybox init would tear everything down and
/// re-exec nothing — a black limbo instead of a reboot.
/// `into_trial` passes the `0 tryboot` argument busybox cannot —
/// the firmware's one-shot flag: that boot reads tryboot.txt, any
/// boot after it is back on config.txt — the audited unsafe site of
/// this crate
pub(super) fn reboot(into_trial: bool) -> io::Result<()> {
    // SAFETY: libc calls with our own arguments only: sync(2) and
    // the best-effort umount(2)s take no references, and for the
    // raw reboot(2) the magic and command constants come from
    // libc, with a NUL-terminated C string we own as RESTART2's
    // argument — nothing else is referenced
    // (unsafe_code is denied crate-wide — this is the audited
    // reboot path)
    #[allow(unsafe_code)]
    let ret = unsafe {
        libc::sync();
        // the data and boot partitions go down whole — no
        // shutdown script runs after this
        let _ = libc::umount(c"/data".as_ptr() as *const libc::c_char);
        let _ = libc::umount(c"/boot".as_ptr() as *const libc::c_char);
        libc::sync();
        libc::syscall(
            libc::SYS_reboot,
            libc::LINUX_REBOOT_MAGIC1 as libc::c_long,
            libc::LINUX_REBOOT_MAGIC2 as libc::c_long,
            if into_trial {
                libc::LINUX_REBOOT_CMD_RESTART2 as libc::c_long
            } else {
                libc::LINUX_REBOOT_CMD_RESTART as libc::c_long
            },
            if into_trial {
                c"0 tryboot".as_ptr() as libc::c_long
            } else {
                0
            },
        )
    };
    if ret == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
