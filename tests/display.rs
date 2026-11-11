//! [display] backlight control — exercised against a fake
//! backlight directory via MESTRO_BACKLIGHT.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fake_backlight(max: u32, raw: u32) -> (TempDir, std::rc::Rc<mestro::display::Display>) {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = TempDir(std::env::temp_dir().join(format!("mestro-display-test-{n}")));
    let _ = fs::remove_dir_all(&dir.0);
    fs::create_dir_all(dir.join("bl")).unwrap();
    fs::write(dir.join("bl/max_brightness"), max.to_string()).unwrap();
    fs::write(dir.join("bl/brightness"), raw.to_string()).unwrap();
    std::env::set_var("MESTRO_BACKLIGHT", dir.join("bl"));
    (dir, std::rc::Rc::new(mestro::display::Display::probe()))
}

fn raw(dir: &TempDir) -> String {
    fs::read_to_string(dir.join("bl/brightness"))
        .unwrap()
        .trim()
        .to_string()
}

// one test touches MESTRO_BACKLIGHT (process-global env): parallel
// tests would race on it
#[test]
fn backlight_levels() {
    let (dir, display) = fake_backlight(512, 256);
    assert!(display.supported());
    // percent scales against max_brightness
    assert_eq!(display.brightness(), Some(50));
    // unset awake level falls back to the boot brightness
    assert_eq!(display.awake_brightness(), Some(50));

    display.configure(Some(100), 15);
    display.apply_now();
    assert_eq!(raw(&dir), "512");

    display.configure(Some(25), 15);
    display.apply_now();
    assert_eq!(raw(&dir), "128");
    assert_eq!(display.brightness(), Some(25));

    // standby dims to the standby level, 0 is allowed (dark)
    display.configure(Some(25), 0);
    assert_eq!(display.target(), Some(25));
    display.set_standby(true);
    assert_eq!(display.target(), Some(0));
}

#[test]
fn writes_are_spaced_by_the_interval() {
    use mestro::display::wait_before_write;
    use std::time::{Duration, Instant};

    let interval = Duration::from_millis(200);
    let now = Instant::now();
    // never written: write right away
    assert_eq!(wait_before_write(None, now, interval), None);
    // 50ms after a write: defer the remaining 150ms
    assert_eq!(
        wait_before_write(Some(now), now + Duration::from_millis(50), interval),
        Some(Duration::from_millis(150))
    );
    // interval passed: write right away
    assert_eq!(wait_before_write(Some(now), now + interval, interval), None);
}
