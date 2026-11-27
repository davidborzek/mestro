//! VU meter input: squeezelite's shared-memory visualizer
//! (`squeezelite -v`, needs a `-DVISEXPORT` build — the ring reader
//! lives in `mestro_os::audio::visexport`).
//!
//! This module owns the meter: the ballistics, the RMS→VU mapping
//! and the spectrum analyzer (FFT + bands). `mock` synthesizes
//! levels so the UI side can be exercised without squeezelite.

use crate::config::VuMode;
use mestro_os::audio::visexport::Poll;

/// The Slint edition of every runtime-compiled skin and face;
/// build.rs compiles the app's own UI with the same one.
const SLINT_EDITION: (u32, u32) = (1, 19);

/// The interpreter Compiler for every runtime-compiled skin, face
/// and the embedded svgskin engine. A fresh Compiler defaults to
/// the ExportedWindows selection, which deprecates non-Window
/// exports — but embedded skins ARE non-Window components by design
/// (they embed via ComponentContainer, not as standalone windows).
/// The old interpreter default (LastExported) is what we want:
/// generate the last exported component regardless of Window
/// inheritance. Needs the `internal` feature (compiler_configuration
/// + InternalToken) — hence the i-slint-core/i-slint-compiler deps.
pub(crate) fn skin_compiler() -> slint_interpreter::Compiler {
    let mut compiler = slint_interpreter::Compiler::new();
    compiler
        .compiler_configuration(i_slint_core::InternalToken)
        .components_to_generate = i_slint_compiler::ComponentSelection::LastExported;
    compiler.set_edition(SLINT_EDITION);
    compiler
}

/// spectrum analyzer: FFT + log-spaced bands (see fft.rs)
mod fft;
/// manifest-driven VU skins (built-in and custom alike)
pub mod manifest;
/// runtime-loaded skin embedding (ComponentContainer)
pub mod preview;
/// skin art rasterized with resvg, the glow lamp painted on it
mod raster;
pub mod skin;
pub mod wire;

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// reader→UI: stereo level 0..1 per channel (already ballistics'd)
pub type Shared = Arc<Mutex<[f32; 2]>>;

/// how many spectrum bands the analyzer pushes
pub const BANDS: usize = 32;
/// reader→UI: log-spaced spectrum bands 0..1 (ballistics'd)
/// plus their slow-falling peak caps
pub type SharedBands = Arc<Mutex<([f32; BANDS], [f32; BANDS])>>;

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
    let mut ring = mestro_os::audio::visexport::VisRing::new();
    let mut ballistic = Ballistics::new();
    let mut analyzer = fft::Analyzer::new();
    loop {
        std::thread::sleep(TICK);
        // the ring's per-tick outcome: silence zeroes the meter, a
        // read error keeps the last reading for one tick
        let (rate, samples) = match ring.poll(prefix) {
            Poll::Playing { rate, samples } => (rate, samples),
            Poll::NoRing | Poll::Silent => {
                ballistic.push([0.0; 2]);
                *shared.lock() = ballistic.level;
                *bands.lock() = ([0.0; BANDS], [0.0; BANDS]);
                continue;
            }
            Poll::Retry => continue,
        };

        // RMS per channel → dBFS → VU (0 VU = the configured
        // reference level, default -14 dBFS: modern loud masters
        // center the needle around 0) → level 0..1 across
        // -20..+3 VU — the classic VU scale
        let frames = samples.len() / 2;
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
        *shared.lock() = ballistic.level;
        let (bands_now, peaks_now) = analyzer.analyze(&samples, rate);
        *bands.lock() = (bands_now, peaks_now);
    }
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
        *shared.lock() = ballistic.level;
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
        let (mut prev_bands, mut prev_peaks) = *bands.lock();
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
        *bands.lock() = (prev_bands, prev_peaks);
    }
}
