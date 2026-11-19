//! [display]: screen backlight settings. Actual control is
//! best-effort — see src/display.rs; if no backlight device
//! exists, the values are silently ignored.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Display {
    /// backlight brightness in percent (1..=100), applied at
    /// startup; absent = leave whatever the system booted with
    pub brightness: Option<u8>,
    /// backlight in percent while the standby clock shows
    /// (0..=100; 0 = dark, 100 = no dimming)
    pub standby_brightness: u8,
    /// which backend sets it: "auto" (detect), "firmware" (Pi
    /// mailbox), "sysfs", "none". Auto picks the firmware when it
    /// drives the panel's touch (it owns the display i2c bus then)
    pub backlight: crate::display::backlight::BacklightChoice,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            brightness: None,
            standby_brightness: 15,
            backlight: crate::display::backlight::BacklightChoice::Auto,
        }
    }
}

impl Display {
    /// clamp to the documented ranges — awake never goes fully
    /// dark (a black kiosk looks dead), standby may
    pub fn validate(&mut self) {
        self.brightness = self.brightness.map(|b| b.clamp(1, 100));
        self.standby_brightness = self.standby_brightness.min(100);
    }
}
