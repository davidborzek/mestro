//! The process-wide update lock: one updater at a time, across the
//! CLI, the web OTA and S99ota — different processes on the same
//! boot store, so only the kernel can arbitrate between them. A
//! non-blocking lock on `/run/mestro-update.lock` (tmpfs: it does
//! not survive a reboot, and a stale one from a crashed process is
//! just an empty file nobody holds), held for the whole write
//! operation (install, commit, abandon); reads (the status report)
//! run without it. Nobody waits for a running update — the holder
//! finishes and the next one tries again.

use std::fs::{File, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// the image's lock file
const LOCK_FILE: &str = "/run/mestro-update.lock";

/// the held update lock; released when dropped (the file's fd
/// closes)
pub struct UpdateLock {
    _file: File,
}

impl UpdateLock {
    /// the image's lock, at its fixed path — every write operation
    /// of the CLI takes this. Held by someone else (the web OTA,
    /// S99ota, another CLI run): the caller refuses, it never waits
    pub fn try_acquire() -> io::Result<Self> {
        Self::try_acquire_at(LOCK_FILE)
    }

    /// the same lock at an explicit path — the test seam
    pub fn try_acquire_at(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::create(path.as_ref())?;
        file.try_lock().map_err(|e| match e {
            TryLockError::Error(e) => e,
            TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
        })?;
        Ok(Self { _file: file })
    }

    /// wait for the image's lock, up to `bound`: a short holder (a
    /// commit, an abandon) gets out of the way; an upload that
    /// would hold it for minutes is not worth the wait —
    /// `Duration::ZERO` never waits
    pub fn wait_acquire(bound: Duration) -> io::Result<Self> {
        Self::wait_acquire_at(LOCK_FILE, bound)
    }

    /// the same wait at an explicit path — the test seam
    pub fn wait_acquire_at(path: impl AsRef<Path>, bound: Duration) -> io::Result<Self> {
        let deadline = Instant::now() + bound;
        loop {
            match Self::try_acquire_at(path.as_ref()) {
                Ok(lock) => return Ok(lock),
                Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
                Err(_) if Instant::now() >= deadline => {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock))
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_released_lock_can_be_taken_again() {
        // the image's /run path is root's on the device; a temp
        // path stands in here
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("mestro-update.lock");
        {
            let _held = UpdateLock::try_acquire_at(&lock).unwrap();
            assert!(lock.exists(), "the lock file is there");
            // nobody waits: a second taker is told the update runs
            match UpdateLock::try_acquire_at(&lock) {
                Ok(_) => panic!("the lock is held"),
                Err(e) => assert_eq!(e.kind(), io::ErrorKind::WouldBlock),
            }
        }
        // dropping the guard releases the lock: the next updater
        // gets it
        let _again = UpdateLock::try_acquire_at(&lock).unwrap();
    }

    #[test]
    fn the_image_lock_path_is_fixed() {
        assert_eq!(LOCK_FILE, "/run/mestro-update.lock");
    }

    #[test]
    fn a_holder_that_leaves_within_the_bound_is_waited_out() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("mestro-update.lock");
        // another updater holds the lock for a moment, then goes —
        // like S99ota's commit arriving right after an upload
        let holder = {
            let lock = lock.clone();
            std::thread::spawn(move || {
                let _held = UpdateLock::try_acquire_at(&lock).unwrap();
                std::thread::sleep(Duration::from_millis(200));
            })
        };
        assert!(
            UpdateLock::wait_acquire_at(&lock, Duration::from_secs(5)).is_ok(),
            "waited out"
        );
        holder.join().unwrap();
    }

    #[test]
    fn the_bound_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("mestro-update.lock");
        {
            let _held = UpdateLock::try_acquire_at(&lock).unwrap();
            let start = Instant::now();
            assert_eq!(
                match UpdateLock::wait_acquire_at(&lock, Duration::from_millis(150)) {
                    Err(e) => e.kind(),
                    Ok(_) => panic!("the lock is held"),
                },
                io::ErrorKind::WouldBlock
            );
            assert!(
                start.elapsed() >= Duration::from_millis(150),
                "the bound was waited out"
            );
        }
        // dropped: free again
        assert!(UpdateLock::try_acquire_at(&lock).is_ok());
    }
}
