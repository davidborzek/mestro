//! VU meter input: squeezelite's shared-memory visualizer
//! (`squeezelite -v`, needs a `-DVISEXPORT` build).
//!
//! squeezelite writes its decoded output into a `struct vis_t` ring
//! in /dev/shm (see its `output_vis.c`): a 56-byte pthread rwlock,
//! then `buf_size`/`buf_index`/`running`/`rate`/`updated` and
//! `buffer[16384]` interleaved stereo s16 samples. The ring layout
//! is hard-wired; the byte offsets below are the C struct's, valid
//! for 64-bit Linux (glibc's `pthread_rwlock_t` is 56 bytes on both
//! x86_64 and aarch64).
//!
//! The reader takes no lock: a torn sample or two does not move an
//! RMS window, and the writer skips the export rather than block
//! when the lock is held. `mock` synthesizes levels so the UI side
//! can be exercised without squeezelite.

use crate::config::VuMode;
use crate::locks;

/// The interpreter's Compiler defaults to the ExportedWindows
/// selection, which deprecates non-Window exports — but embedded
/// skins ARE non-Window components by design (they embed via
/// ComponentContainer, not as standalone windows). The old
/// interpreter default (LastExported) is what we want: generate
/// the last exported component regardless of Window inheritance.
/// Needs the `internal` feature (compiler_configuration +
/// InternalToken) — hence the i-slint-core/i-slint-compiler deps.
pub(crate) fn select_last_exported(compiler: &mut slint_interpreter::Compiler) {
    compiler
        .compiler_configuration(i_slint_core::InternalToken)
        .components_to_generate = i_slint_compiler::ComponentSelection::LastExported;
}

/// spectrum analyzer: FFT + log-spaced bands (see fft.rs)
mod fft;
/// manifest-driven VU skins (built-in and custom alike)
pub mod manifest;
/// runtime-loaded skin embedding (ComponentContainer)
pub mod preview;
pub mod skin;
pub mod wire;

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// reader→UI: stereo level 0..1 per channel (already ballistics'd)
pub type Shared = Arc<Mutex<[f32; 2]>>;

/// how many spectrum bands the analyzer pushes
pub const BANDS: usize = 32;
/// reader→UI: log-spaced spectrum bands 0..1 (ballistics'd)
/// plus their slow-falling peak caps
pub type SharedBands = Arc<Mutex<([f32; BANDS], [f32; BANDS])>>;

// struct vis_t offsets — see output_vis.c
const OFF_BUF_SIZE: u64 = 56;
const OFF_BUF_INDEX: u64 = 60;
const OFF_RUNNING: u64 = 64;
const OFF_RATE: u64 = 68;
const OFF_BUFFER: u64 = 80;
/// the ring is hard-wired to this many s16 samples
const VIS_BUF_SIZE: usize = 16384;
/// meter tick; 30fps reads like a physical meter, stays cheap
const TICK: Duration = Duration::from_millis(33);
/// VU ballistics: single-pole smoothing with ~300ms time
/// constant (the VU spec: the needle reaches its reading in
/// ~300ms) — applied symmetrically to rise and fall, which is
/// what makes the motion read smooth instead of steppy
const SMOOTH: f32 = 0.12;

/// start the reader for the configured mode; returns the shared
/// level the UI polls. `off` returns a dead slot with no thread.
/// The mock needs the transport's playing state — a process-wide
/// static, hence 'static. `reference_dbfs` is the 0 VU alignment.
pub fn spawn(
    mode: VuMode,
    prefix: &str,
    playing: &'static AtomicBool,
    reference_dbfs: f64,
) -> (Shared, SharedBands) {
    let shared: Shared = Arc::new(Mutex::new([0.0; 2]));
    let bands: SharedBands = Arc::new(Mutex::new(([0.0; BANDS], [0.0; BANDS])));
    match mode {
        VuMode::Off => {}
        VuMode::Shmem => {
            let shared = shared.clone();
            let bands = bands.clone();
            let prefix = prefix.to_string();
            std::thread::spawn(move || shmem_loop(shared, bands, &prefix, reference_dbfs));
        }
        VuMode::Mock => {
            let shared = shared.clone();
            let bands = bands.clone();
            std::thread::spawn(move || mock_loop(shared, bands, playing));
        }
    }
    (shared, bands)
}

/// classic VU ballistics: 300ms single-pole smoothing
struct Ballistics {
    level: [f32; 2],
}

impl Ballistics {
    fn new() -> Self {
        Self { level: [0.0; 2] }
    }

    fn push(&mut self, target: [f32; 2]) {
        for (level, t) in self.level.iter_mut().zip(target) {
            let t = t.clamp(0.0, 1.0);
            *level += (t - *level) * SMOOTH;
        }
    }
}

fn shmem_loop(shared: Shared, bands: SharedBands, prefix: &str, reference_dbfs: f64) {
    let mut file: Option<(PathBuf, File)> = None;
    let mut ballistic = Ballistics::new();
    let mut analyzer = fft::Analyzer::new();
    let mut last_rate: Option<u32> = None;
    loop {
        std::thread::sleep(TICK);
        // (re)acquire: squeezelite may start after us, restart with
        // a new MAC-suffixed name, or run on another machine's /dev
        if file.is_none() {
            if let Some((path, f)) = open_shm(prefix) {
                tracing::info!("vu: reading {}", path.display());
                file = Some((path, f));
            }
        }
        let Some((_, f)) = file.as_ref() else {
            ballistic.push([0.0; 2]);
            *locks::lock(&shared) = ballistic.level;
            *locks::lock(&bands) = ([0.0; BANDS], [0.0; BANDS]);
            continue;
        };

        // header: buf_size, buf_index, running, rate
        let mut hdr = [0u8; 9];
        if f.read_exact_at(&mut hdr, OFF_BUF_SIZE).is_err() {
            tracing::warn!("vu: shm unreadable, retrying");
            file = None;
            continue;
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
            file = None;
            continue;
        }
        let rate = u32::from_le_bytes(rb);
        if buf_size == 0 || buf_size > VIS_BUF_SIZE {
            continue; // not a vis_t we understand
        }
        if last_rate != Some(rate) {
            if last_rate.is_some() {
                tracing::debug!("vu: rate changed to {rate}");
            }
            last_rate = Some(rate);
        }
        if !running {
            ballistic.push([0.0; 2]);
            *locks::lock(&shared) = ballistic.level;
            *locks::lock(&bands) = ([0.0; BANDS], [0.0; BANDS]);
            continue;
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
        let mut ok = true;
        while filled < count {
            // start can be near the ring edge: never read past the
            // buffer end — the ring wraps, the next pass reads on
            let start = (idx + buf_size - count + filled) % buf_size;
            let edge = (buf_size - start) * 2; // bytes until the wrap
            let take = chunk.len().min(edge).min((count - filled) * 2);
            if f.read_exact_at(&mut chunk[..take], OFF_BUFFER + start as u64 * 2)
                .is_err()
            {
                ok = false;
                break;
            }
            for b in chunk[..take].chunks_exact(2) {
                samples[filled] = i16::from_le_bytes([b[0], b[1]]);
                filled += 1;
            }
        }
        if !ok {
            file = None;
            continue;
        }

        // RMS per channel → dBFS → VU (0 VU = the configured
        // reference level, default -14 dBFS: modern loud masters
        // center the needle around 0) → level 0..1 across
        // -20..+3 VU — the classic VU scale
        let mut level = [0.0f32; 2];
        for ch in 0..2 {
            let sum: f64 = samples[ch..]
                .iter()
                .step_by(2)
                .map(|&s| (s as f64) * (s as f64))
                .sum();
            let rms = (sum / frames as f64).sqrt();
            let db = 20.0 * (rms / 32768.0).log10();
            let vu = db - reference_dbfs;
            level[ch] = ((vu + 20.0) / 23.0) as f32;
        }
        ballistic.push(level);
        *locks::lock(&shared) = ballistic.level;
        let (bands_now, peaks_now) = analyzer.analyze(&samples, rate);
        *locks::lock(&bands) = (bands_now, peaks_now);
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

/// synthesized levels while `playing` — UI work without squeezelite
fn mock_loop(shared: Shared, bands: SharedBands, playing: &'static AtomicBool) {
    let mut ballistic = Ballistics::new();
    let start = std::time::Instant::now();
    loop {
        std::thread::sleep(TICK);
        let t = start.elapsed().as_secs_f32();
        let mut target = [0.0f32; 2];
        if playing.load(Ordering::Relaxed) {
            // slow music-ish bounce with a fast shimmer on top
            let base = 0.42 + 0.18 * (t * 0.7).sin();
            for (ch, w) in [(0usize, 0.0f32), (1usize, 0.6f32)] {
                target[ch] =
                    (base + 0.16 * (t * (2.3 + w * 0.2) + w).sin() + 0.06 * (t * (9.1 + w)).sin())
                        .clamp(0.05, 0.95);
            }
        }
        ballistic.push(target);
        *locks::lock(&shared) = ballistic.level;
        // synthesized spectrum: per-band noise bounce around a
        // tilted hill, so UI work reads like music
        let mut bands_now = [0.0f32; BANDS];
        if playing.load(Ordering::Relaxed) {
            for (b, band) in bands_now.iter_mut().enumerate() {
                let hill = (1.0 - b as f32 / BANDS as f32) * 0.75 + 0.1;
                let wob = 0.5 + 0.5 * (t * (1.7 + b as f32 * 0.31) + b as f32 * 1.3).sin();
                let shimmer = 0.5 + 0.5 * (t * (11.0 + b as f32)).sin();
                *band = (hill * (0.55 + 0.45 * wob) * (0.8 + 0.2 * shimmer)).clamp(0.03, 1.0);
            }
        }
        let (mut prev_bands, mut prev_peaks) = *locks::lock(&bands);
        for (p, n) in prev_bands.iter_mut().zip(bands_now) {
            // same ballistics as the real analyzer (fft.rs)
            *p = if n > *p {
                *p + (n - *p) * 0.35
            } else {
                n.max(*p - 0.008)
            };
        }
        for (cap, band) in prev_peaks.iter_mut().zip(prev_bands.iter()) {
            *cap = (*band).max(*cap - 0.004);
        }
        *locks::lock(&bands) = (prev_bands, prev_peaks);
    }
}
