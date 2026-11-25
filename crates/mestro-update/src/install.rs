//! Writing a new image into the prepared slot, verified twice before
//! anything is booted: against the expected sha256, and against what
//! the store reads back behind the cache.

use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read, Write};

use crate::{sha256_hex, BootControl, UpdateTarget};

/// the update's failure modes; the messages are what the uploading
/// client and the SSH user see
#[derive(Debug)]
pub enum Error {
    /// the update would overwrite the running slot
    RunningSlot(String),
    /// this boot is an undecided trial; committing or abandoning
    /// it comes before any new install
    UndecidedTrial,
    /// the image did not hash to the expected sha256
    ShaMismatch { got: String, want: String },
    /// what the store read back is not what was written
    CorruptedWrite,
    /// the slot store could not be read or written
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RunningSlot(slot) => {
                write!(f, "refusing to overwrite the running slot {slot}")
            }
            Self::UndecidedTrial => write!(
                f,
                "this boot is an undecided trial (commit or abandon it first)"
            ),
            Self::ShaMismatch { got, want } => {
                write!(f, "sha256 mismatch: got {got}, want {want}")
            }
            Self::CorruptedWrite => write!(f, "corrupted write on the boot partition"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// a finished update: the slot that was written and the sha256 that
/// landed on it
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub slot: String,
    pub sha256: String,
}

/// stream the image into the prepared slot while hashing it, then
/// arm the trial boot of it. The write must survive two checks —
/// the expected sha256, and the store's own read-back, which sees
/// the medium, not a cache: a write that never reached the card is
/// caught, not booted. Every failure on the way — a stream that
/// ends in an error included — discards the slot, so a
/// half-written image never gets a trial
pub fn install(
    backend: &mut dyn BootControl,
    target: &mut UpdateTarget,
    image: &mut dyn Read,
    expected_sha256: Option<&str>,
) -> Result<Installed, Error> {
    let written = match stream_hashing(&mut target.sink, image) {
        Ok(written) => written,
        Err(e) => {
            backend.discard(target);
            return Err(e);
        }
    };
    if let Some(expected) = expected_sha256 {
        if written != expected {
            backend.discard(target);
            return Err(Error::ShaMismatch {
                got: written,
                want: expected.to_string(),
            });
        }
    }
    let on_store = match backend.read_back(target) {
        Ok(sha) => sha,
        Err(e) => {
            backend.discard(target);
            return Err(Error::Io(e));
        }
    };
    if on_store != written {
        backend.discard(target);
        return Err(Error::CorruptedWrite);
    }
    backend.arm_trial(target)?;
    Ok(Installed {
        slot: target.slot.clone(),
        sha256: written,
    })
}

/// stream into the store while hashing: the sha256 that was written
fn stream_hashing(store: &mut dyn Write, image: &mut dyn Read) -> Result<String, Error> {
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = image.read(&mut chunk)?;
        if n == 0 {
            return Ok(sha256_hex(&hasher.finalize()));
        }
        hasher.update(&chunk[..n]);
        store.write_all(&chunk[..n])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// a boot chain that keeps its "card" in memory: enough of a
    /// backend to test the update flow without any board
    struct FakeBoot {
        card: Arc<Mutex<Vec<u8>>>,
        armed: Vec<String>,
        discarded: Vec<String>,
        /// read back a corrupt copy of what was written
        corrupt_read_back: bool,
    }

    impl FakeBoot {
        fn new() -> Self {
            Self {
                card: Arc::new(Mutex::new(Vec::new())),
                armed: Vec::new(),
                discarded: Vec::new(),
                corrupt_read_back: false,
            }
        }
    }

    /// the store end of [`FakeBoot`]
    struct CardSink(Arc<Mutex<Vec<u8>>>);

    impl Write for CardSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl BootControl for FakeBoot {
        fn mount(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn release(&mut self) {}
        fn active(&self) -> io::Result<String> {
            Ok("slot-a".into())
        }
        fn pending(&self) -> io::Result<Option<String>> {
            Ok(None)
        }
        fn trial_boot(&self) -> bool {
            false
        }
        fn begin_update(&mut self) -> Result<UpdateTarget, Error> {
            Ok(UpdateTarget {
                slot: "slot-b".into(),
                running: "slot-a".into(),
                sink: Box::new(CardSink(self.card.clone())),
            })
        }
        fn read_back(&mut self, _target: &UpdateTarget) -> io::Result<String> {
            let mut hasher = Sha256::new();
            if self.corrupt_read_back {
                hasher.update(b"not what was written");
            } else {
                hasher.update(&*self.card.lock().unwrap());
            }
            Ok(sha256_hex(&hasher.finalize()))
        }
        fn discard(&mut self, target: &UpdateTarget) {
            self.discarded.push(target.slot.clone());
        }
        fn arm_trial(&mut self, target: &UpdateTarget) -> io::Result<()> {
            self.armed.push(target.slot.clone());
            Ok(())
        }
        fn commit(&mut self) -> Result<crate::Commit, Error> {
            Ok(crate::Commit::NoTrial)
        }
        fn abandon(&mut self) -> io::Result<Option<String>> {
            Ok(None)
        }
        fn reboot(&mut self, _into_trial: bool) -> io::Result<()> {
            Ok(())
        }
    }

    fn sha_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        sha256_hex(&hasher.finalize())
    }

    fn image() -> Vec<u8> {
        (0..100_000).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn install_streams_verifies_and_arms_the_trial() {
        let mut backend = FakeBoot::new();
        let mut target = backend.begin_update().unwrap();
        let image = image();
        let sha = sha_of(&image);
        let installed = install(&mut backend, &mut target, &mut &image[..], Some(&sha)).unwrap();
        assert_eq!(
            installed,
            Installed {
                slot: "slot-b".into(),
                sha256: sha
            }
        );
        assert_eq!(*backend.card.lock().unwrap(), image);
        assert_eq!(backend.armed, ["slot-b"]);
        assert!(backend.discarded.is_empty());
    }

    #[test]
    fn a_sha_mismatch_discards_the_slot_and_arms_nothing() {
        let mut backend = FakeBoot::new();
        let mut target = backend.begin_update().unwrap();
        let image = image();
        let err = install(&mut backend, &mut target, &mut &image[..], Some("0")).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("sha256 mismatch: got {}, want 0", sha_of(&image))
        );
        assert_eq!(backend.discarded, ["slot-b"]);
        assert!(backend.armed.is_empty(), "no trial of a bad image");
    }

    #[test]
    fn a_corrupted_read_back_discards_the_slot_and_arms_nothing() {
        let mut backend = FakeBoot {
            corrupt_read_back: true,
            ..FakeBoot::new()
        };
        let mut target = backend.begin_update().unwrap();
        let image = image();
        let err = install(&mut backend, &mut target, &mut &image[..], None).unwrap_err();
        assert_eq!(err.to_string(), "corrupted write on the boot partition");
        assert_eq!(backend.discarded, ["slot-b"]);
        assert!(backend.armed.is_empty(), "no trial of a bad write");
    }

    #[test]
    fn without_an_expected_sha_the_written_hash_is_reported() {
        let mut backend = FakeBoot::new();
        let mut target = backend.begin_update().unwrap();
        let image = image();
        let installed = install(&mut backend, &mut target, &mut &image[..], None).unwrap();
        assert_eq!(installed.sha256, sha_of(&image));
        assert_eq!(backend.armed, ["slot-b"]);
    }

    #[test]
    fn a_broken_stream_discards_the_slot_and_arms_nothing() {
        // a reader that dies mid-image (the web upload's socket
        // broke): what was written so far must not survive
        struct BrokenStream;
        impl Read for BrokenStream {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "upload ended early",
                ))
            }
        }
        let mut backend = FakeBoot::new();
        let mut target = backend.begin_update().unwrap();
        let err = install(&mut backend, &mut target, &mut BrokenStream, None).unwrap_err();
        assert!(err.to_string().contains("upload ended early"));
        assert_eq!(backend.discarded, ["slot-b"]);
        assert!(backend.armed.is_empty());
    }

    #[test]
    fn the_running_slot_is_named_in_the_refusal() {
        assert_eq!(
            Error::RunningSlot("Image.a".into()).to_string(),
            "refusing to overwrite the running slot Image.a"
        );
    }
}
