//! Backlight control, best-effort: whatever the platform offers
//! (see [`mestro_os::display`] for the backends and their detection).
//! No backlight → all methods are no-ops, the app runs on (kiosk
//! on a panel, desktop on a laptop, VM without any).
//!
//! Two targets: the awake brightness ([display] brightness, or
//! whatever the system booted with) and the standby brightness
//! the panel dims to while the standby clock shows.
//!
//! `MESTRO_BACKLIGHT` points at a backlight directory
//! (containing `brightness` + `max_brightness`) to override
//! discovery — for tests and exotic setups.

pub use mestro_os::display::{Backlight, BacklightChoice};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Minimum spacing between backlight writes. On the Pi panel the
/// backlight µC shares its i2c bus with the firmware's touch
/// polling; a slider drag writing per pointer move hung the
/// VideoCore firmware.
pub const WRITE_INTERVAL: Duration = Duration::from_millis(200);

pub struct Display {
    backend: Backlight,
    /// brightness found at probe — the awake target when
    /// [display] brightness is unset
    boot: Option<u8>,
    awake: Cell<Option<u8>>,
    standby_level: Cell<u8>,
    in_standby: Cell<bool>,
    last_write: Cell<Option<Instant>>,
    last_value: Cell<Option<u8>>,
    pending: Cell<Option<u8>>,
    flush: slint::Timer,
}

impl Display {
    /// Probe for a backlight device; never fails.
    pub fn probe() -> Self {
        Self::probe_with(BacklightChoice::Auto)
    }

    /// probe honoring `[display] backlight`
    pub fn probe_with(choice: BacklightChoice) -> Self {
        let backend = match std::env::var("MESTRO_BACKLIGHT") {
            // explicit sysfs directory (tests, exotic setups)
            Ok(dir) => {
                let dir = PathBuf::from(dir);
                let max = mestro_os::display::read_u32(&dir.join("max_brightness"))
                    .unwrap_or(1)
                    .max(1);
                Backlight::Sysfs { dir, max }
            }
            Err(_) => mestro_os::display::detect(choice, &mestro_os::display::Roots::default()),
        };
        tracing::info!("display: backlight via {}", backend.name());
        Self::with_backend(backend)
    }

    pub fn with_backend(backend: Backlight) -> Self {
        let boot = backend.read();
        Self {
            backend,
            boot,
            awake: Cell::new(None),
            standby_level: Cell::new(100),
            in_standby: Cell::new(false),
            last_write: Cell::new(None),
            last_value: Cell::new(None),
            pending: Cell::new(None),
            flush: slint::Timer::default(),
        }
    }

    pub fn supported(&self) -> bool {
        self.backend.supported()
    }

    /// which backend drives the backlight ("pi-firmware", "sysfs",
    /// "none")
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// current brightness in percent: read back where the backend
    /// can, else the last level written
    pub fn brightness(&self) -> Option<u8> {
        self.backend.read().or(self.last_value.get())
    }

    /// the awake target: configured, else the boot brightness
    pub fn awake_brightness(&self) -> Option<u8> {
        self.awake.get().or(self.boot)
    }

    /// take the [display] values; the next apply/request uses them
    pub fn configure(&self, awake: Option<u8>, standby: u8) {
        self.awake.set(awake);
        self.standby_level.set(standby.min(100));
    }

    /// the level the panel should show right now (None = leave it)
    pub fn target(&self) -> Option<u8> {
        if self.in_standby.get() {
            Some(self.standby_level.get())
        } else {
            self.awake.get()
        }
    }

    /// startup: write the target directly (no throttle needed)
    pub fn apply_now(&self) {
        if let Some(level) = self.target() {
            self.write(level);
        }
    }

    /// live edits and standby transitions: throttled write of
    /// the current target. Leaving standby without a configured
    /// awake level restores the boot brightness
    pub fn apply(self: &Rc<Self>) {
        let level = self.target().or_else(|| {
            (!self.in_standby.get() && self.last_value.get().is_some())
                .then_some(self.boot)
                .flatten()
        });
        if let Some(level) = level {
            self.request(level);
        }
    }

    /// enter/leave standby (the standby clock shows/hides)
    pub fn set_standby(self: &Rc<Self>, standby: bool) {
        if self.in_standby.replace(standby) != standby {
            self.apply();
        }
    }

    /// write a level in percent (0..=100) right away; no-op when
    /// unsupported, log-only when the write fails
    fn write(&self, percent: u8) {
        if !self.backend.supported() {
            return;
        }
        let percent = percent.min(100);
        // every write is logged: firmware hangs on the Pi were
        // traced to backlight writes — the log ties a hang to one
        match self.backend.write(percent) {
            Ok(()) => tracing::info!("display: backlight {percent}% via {}", self.backend.name()),
            Err(err) => tracing::warn!(
                "display: backlight write failed ({}): {err}",
                self.backend.name()
            ),
        }
        self.last_write.set(Some(Instant::now()));
        self.last_value.set(Some(percent));
    }

    /// Rate-limited write: at most one per [`WRITE_INTERVAL`], the
    /// last requested level always lands (trailing flush). Must
    /// run on the UI thread.
    fn request(self: &Rc<Self>, percent: u8) {
        if !self.supported() {
            return;
        }
        match wait_before_write(self.last_write.get(), Instant::now(), WRITE_INTERVAL) {
            None => {
                self.flush.stop();
                self.pending.set(None);
                if self.last_value.get() != Some(percent) {
                    self.write(percent);
                }
            }
            Some(wait) => {
                self.pending.set(Some(percent));
                if !self.flush.running() {
                    let display = Rc::downgrade(self);
                    self.flush
                        .start(slint::TimerMode::SingleShot, wait, move || {
                            let Some(display) = display.upgrade() else {
                                return;
                            };
                            if let Some(value) = display.pending.take() {
                                if display.last_value.get() != Some(value) {
                                    display.write(value);
                                }
                            }
                        });
                }
            }
        }
    }
}

/// None = write now; Some(d) = defer by d (the interval since the
/// last write has not passed yet)
pub fn wait_before_write(
    last: Option<Instant>,
    now: Instant,
    interval: Duration,
) -> Option<Duration> {
    let elapsed = now.saturating_duration_since(last?);
    (elapsed < interval).then(|| interval - elapsed)
}
