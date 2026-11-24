//! squeezelite's VISEXPORT shm reader: the decoded-output ring
//! squeezelite's `-v` (a `-DVISEXPORT` build) writes into /dev/shm.
//!
//! squeezelite writes into a `struct vis_t` ring (see its
//! `output_vis.c`): a 56-byte pthread rwlock, then
//! `buf_size`/`buf_index`/`running`/`rate`/`updated` and
//! `buffer[16384]` interleaved stereo s16 samples. The ring layout
//! is hard-wired; the byte offsets below are the C struct's, valid
//! for 64-bit Linux (glibc's `pthread_rwlock_t` is 56 bytes on both
//! x86_64 and aarch64).
//!
//! The reader takes no lock: a torn sample or two does not move an
//! RMS window, and the writer skips the export rather than block
//! when the lock is held.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

// struct vis_t offsets — see output_vis.c
const OFF_BUF_SIZE: u64 = 56;
const OFF_BUF_INDEX: u64 = 60;
const OFF_RUNNING: u64 = 64;
const OFF_RATE: u64 = 68;
const OFF_BUFFER: u64 = 80;
/// the ring is hard-wired to this many s16 samples
const VIS_BUF_SIZE: usize = 16384;

/// one poll of the ring: what the meter should do this tick
#[derive(Debug, PartialEq)]
pub enum Poll {
    /// no shm file (yet): the meter reads silence
    NoRing,
    /// the ring is there but the writer is paused: silence
    Silent,
    /// the ring went unreadable mid-read or its header is not a
    /// vis_t: keep the last reading, the reader retries next tick
    Retry,
    /// the decoded tail window: interleaved stereo s16 samples and
    /// the sample rate they were captured at
    Playing { rate: u32, samples: Vec<i16> },
}

/// the ring behind one squeezelite instance: keeps its shm file
/// across polls, re-finds it when squeezelite starts after us,
/// restarts with a new MAC-suffixed name, or runs on another
/// machine's /dev
pub struct VisRing {
    file: Option<(PathBuf, File)>,
    last_rate: Option<u32>,
}

impl Default for VisRing {
    fn default() -> Self {
        Self::new()
    }
}

impl VisRing {
    pub fn new() -> Self {
        Self {
            file: None,
            last_rate: None,
        }
    }

    /// one poll. Cheap; the meter calls it on its tick
    pub fn poll(&mut self, prefix: &str) -> Poll {
        // (re)acquire: squeezelite may start after us, restart with
        // a new MAC-suffixed name, or run on another machine's /dev
        if self.file.is_none() {
            if let Some((path, f)) = open_shm(prefix) {
                tracing::info!("vu: reading {}", path.display());
                self.file = Some((path, f));
            }
        }
        let Some((_, f)) = self.file.as_ref() else {
            return Poll::NoRing;
        };

        // header: buf_size, buf_index, running, rate
        let mut hdr = [0u8; 9];
        if f.read_exact_at(&mut hdr, OFF_BUF_SIZE).is_err() {
            tracing::warn!("vu: shm unreadable, retrying");
            self.file = None;
            return Poll::Retry;
        }
        #[expect(
            clippy::unwrap_used,
            reason = "a 4-byte slice of the fixed 9-byte header"
        )]
        let buf_size = u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize;
        #[expect(
            clippy::unwrap_used,
            reason = "a 4-byte slice of the fixed 9-byte header"
        )]
        let buf_index = u32::from_le_bytes(
            hdr[OFF_BUF_INDEX as usize - OFF_BUF_SIZE as usize..][..4]
                .try_into()
                .unwrap(),
        ) as usize;
        let running = hdr[OFF_RUNNING as usize - OFF_BUF_SIZE as usize] != 0;
        let mut rb = [0u8; 4];
        if f.read_exact_at(&mut rb, OFF_RATE).is_err() {
            self.file = None;
            return Poll::Retry;
        }
        let rate = u32::from_le_bytes(rb);
        if buf_size == 0 || buf_size > VIS_BUF_SIZE {
            return Poll::Retry; // not a vis_t we understand
        }
        if self.last_rate != Some(rate) {
            if self.last_rate.is_some() {
                tracing::debug!("vu: rate changed to {rate}");
            }
            self.last_rate = Some(rate);
        }
        if !running {
            return Poll::Silent;
        }

        // ~100ms tail window: the 4096-frame FFT needs the
        // integration time (a 50ms read flickers band to band)
        let frames = (rate as usize / 10)
            .clamp(440, VIS_BUF_SIZE / 2)
            .min(buf_size / 2);
        let count = 2 * frames; // interleaved stereo samples
        let idx = buf_index % buf_size;
        let mut samples = vec![0i16; count];
        let mut chunk = [0u8; 512];
        let mut filled = 0usize;
        while filled < count {
            // start can be near the ring edge: never read past the
            // buffer end — the ring wraps, the next pass reads on
            let start = (idx + buf_size - count + filled) % buf_size;
            let edge = (buf_size - start) * 2; // bytes until the wrap
            let take = chunk.len().min(edge).min((count - filled) * 2);
            if f.read_exact_at(&mut chunk[..take], OFF_BUFFER + start as u64 * 2)
                .is_err()
            {
                self.file = None;
                return Poll::Retry;
            }
            for b in chunk[..take].chunks_exact(2) {
                samples[filled] = i16::from_le_bytes([b[0], b[1]]);
                filled += 1;
            }
        }
        Poll::Playing { rate, samples }
    }
}

/// find the shm file: squeezelite names it `<prefix>` or
/// `<prefix>-<MAC>` (it appends its interface address)
fn open_shm(prefix: &str) -> Option<(PathBuf, File)> {
    let dir = std::fs::read_dir("/dev/shm").ok()?;
    let dash = format!("{prefix}-");
    let mut found: Vec<PathBuf> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name == prefix || name.starts_with(&dash)
        })
        .collect();
    found.sort();
    if found.len() > 1 {
        tracing::warn!("vu: multiple shm candidates, using the first");
    }
    let path = found.into_iter().next()?;
    File::open(&path).ok().map(|f| (path, f))
}
