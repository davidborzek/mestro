//! Spectrum analyzer: rustfft over a Hann-windowed mono tail
//! of the shmem samples, mapped onto log-spaced bands with
//! analyzer ballistics. The Analyzer owns the smoothing state —
//! callers hand it fresh samples, it hands back (bands, peaks).

use super::BANDS;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// analyzer window: 4096 samples ≈ 11Hz bins at 44.1kHz —
/// twice the integration time of 2048, which halves the
/// frame-to-frame variance (the flicker)
pub(super) const FFT: usize = 4096;

pub(super) struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<Complex<f32>>,
    /// band levels 0..1, smoothed across ticks — the state must
    /// live here: handing the analyzer a zeroed array every tick
    /// would restart the ballistics from zero each frame
    smooth: [f32; BANDS],
    /// peak caps, held ~1s then gliding down
    peaks: [f32; BANDS],
}

impl Analyzer {
    pub fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Self {
            fft,
            scratch,
            buf: vec![Complex::default(); FFT],
            smooth: [0.0; BANDS],
            peaks: [0.0; BANDS],
        }
    }

    /// compute the smoothed band levels and peak caps from the
    /// newest interleaved-stereo samples
    pub fn analyze(&mut self, samples: &[i16], rate: u32) -> ([f32; BANDS], [f32; BANDS]) {
        // mono mix of the tail window, Hann-weighted, zero-padded
        let frames = samples.len() / 2;
        let take = frames.min(FFT);
        self.buf.fill(Complex::default());
        for i in 0..take {
            let s = &samples[(frames - take + i) * 2..];
            let mono = (s[0] as f32 + s[1] as f32) * 0.5;
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / take as f32).cos();
            self.buf[i] = Complex::new(mono * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);

        // log-spaced band edges: 50Hz .. min(10kHz, Nyquist), like
        // cava; no pink tilt — real music reads mid-dominant
        // already (bass-loudest reads as a left slab against cava)
        let fmax = (10_000.0f64).min(rate as f64 / 2.0);
        for b in 0..BANDS {
            let f0 = 50.0 * (fmax / 50.0).powf(b as f64 / BANDS as f64);
            let f1 = 50.0 * (fmax / 50.0).powf((b + 1) as f64 / BANDS as f64);
            let lo = ((f0 * FFT as f64 / rate as f64).ceil() as usize).max(1);
            let hi = ((f1 * FFT as f64 / rate as f64).ceil() as usize)
                .max(lo + 1)
                .min(FFT / 2);
            // per-band peak, not average: averaging dilutes wide
            // high bands into darkness
            let mut mag = 0.0f64;
            for k in lo..hi {
                mag = mag.max(self.buf[k].norm() as f64);
            }
            // relative to a full-scale sine; the scale below is
            // compressed so music reaches the red zone — "loud",
            // like on real LED analyzers
            let norm = mag / (FFT as f64 / 4.0) / 32768.0;
            // (no pink tilt: cava-aligned, mids dominate)
            let db = 20.0 * (norm + 1e-9).log10();
            let target = ((db + 72.0) / 66.0).clamp(0.0, 1.0) as f32;
            // asymmetric ballistics: smoothed climb, slow fall —
            // instant attack flickers frame to frame
            self.smooth[b] = if target > self.smooth[b] {
                self.smooth[b] + (target - self.smooth[b]) * 0.35
            } else {
                target.max(self.smooth[b] - 0.048)
            };
            self.peaks[b] = self.smooth[b].max(self.peaks[b] - 0.004);
        }
        (self.smooth, self.peaks)
    }
}
