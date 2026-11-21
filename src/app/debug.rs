//! Developer entry points: land on a page, a settings subview or
//! the now-playing overlay at startup, and the `--mock` player.
//! Screenshotting a touch-only UI has no other way in.

use mestro::ui::navigation::Navigation;
use mestro::{player::state, Mestro, Page};
use slint::{ComponentHandle, Timer, TimerMode};
use std::time::Duration;

/// MESTRO_PAGE, MESTRO_SUBVIEW, MESTRO_NP
pub fn apply_startup_env(ui: &Mestro, nav: &Navigation) {
    if let Ok(p) = std::env::var("MESTRO_PAGE") {
        ui.set_page(match p.as_str() {
            "library" => Page::Library,
            "lyrics" => Page::Lyrics,
            "search" => Page::Search,
            "settings" => Page::Settings,
            _ => Page::Hub,
        });
    }
    // MESTRO_SUBVIEW=10 lands on the standby clock gallery
    if let Some(sub) = std::env::var("MESTRO_SUBVIEW")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        // through the navigation state: a raw set_page is reset by
        // the first nav sync
        nav.go(ui, Page::Settings);
        nav.push_sub(ui, sub);
    }
    // the nav layer must be pushed, a raw set_np_open would break
    // back()
    if std::env::var("MESTRO_NP").is_ok() {
        nav.open_np(ui);
    }
}

/// `--mock`: a fake playing track with a moving progress bar, one
/// clock snapshot for the standby faces (MESTRO_STANDBY=1 starts
/// powered off). The returned timer must stay alive.
pub fn mock_player(ui: &Mestro) -> Timer {
    ui.set_powered(std::env::var("MESTRO_STANDBY").is_err());
    super::clock::show(ui, &super::clock::tick_now());
    ui.set_track_title("Mock Track".into());
    ui.set_track_artist("Mestro Dev".into());
    ui.set_track_album("Skeleton".into());
    ui.set_playing(true);
    ui.set_volume(50);

    let timer = Timer::default();
    let weak = ui.as_weak();
    let mut pos: u32 = 0;
    timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
        pos = (pos + 500) % 210_000;
        if let Some(ui) = weak.upgrade() {
            ui.set_pos_label(state::mmss(pos as u64).into());
            ui.set_dur_label(state::mmss(210_000).into());
            ui.set_progress(pos as f32 / 210_000.0);
        }
    });
    timer
}
