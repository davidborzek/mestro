//! The backlight's settings mirror and live apply.

use mestro::config::Config;
use mestro::display::Display;
use mestro::settings::Handle;
use mestro::Mestro;

/// push the [display] values and override flags to the settings page
pub fn sync(ui: &Mestro, handle: &Handle, effective: &Config, display: &Display) {
    ui.set_display_supported(display.supported());
    // no configured value: show what the system booted with
    ui.set_display_brightness(display.awake_brightness().map(f32::from).unwrap_or(100.0));
    ui.set_display_brightness_overridden(handle.is_overridden("display.brightness"));
    ui.set_display_standby_brightness(effective.display.standby_brightness as f32);
    ui.set_display_standby_brightness_overridden(
        handle.is_overridden("display.standby-brightness"),
    );
}

/// take new [display] values and write the current target
pub fn apply(display: &std::rc::Rc<Display>, effective: &Config) {
    display.configure(
        effective.display.brightness,
        effective.display.standby_brightness,
    );
    display.apply();
}
