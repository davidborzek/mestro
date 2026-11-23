#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! Broken settings must not reset the device: an unknown key in the
//! settings overlay (an older binary meeting a newer overlay, the
//! reset after the October reboot) or an unreadable config file
//! keep everything else and get reported.

use std::fs;
use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mestro-config-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// one test: XDG_STATE_HOME is process-global
#[test]
fn broken_settings_keep_the_rest_and_are_reported() {
    // 1. the overlay of a newer build, read by a build that has no
    //    [display]: the connection survives, the key is reported
    let state = temp_dir("state");
    fs::create_dir_all(state.join("mestro")).unwrap();
    fs::write(
        state.join("mestro/settings.toml"),
        "[display]\nfuture-key = 1\n\n[music-assistant]\nurl = \"http://ma:8095\"\nplayer = \"Living Room\"\n",
    )
    .unwrap();
    std::env::set_var("XDG_STATE_HOME", &state);

    let config = mestro::config::load(Some(std::path::Path::new("/dev/null"))).unwrap();
    assert_eq!(
        config.music_assistant.url, "http://ma:8095",
        "the connection survives"
    );
    assert_eq!(config.music_assistant.player, "Living Room");
    let problems = mestro::config::problems();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].key, "display.future-key");

    // 2. a config file that is not even TOML: defaults + a report,
    //    no exit (the kiosk would show a black screen)
    let dir = temp_dir("broken");
    let file = dir.join("config.toml");
    fs::write(&file, "[theme\npalette = \n").unwrap();
    fs::remove_file(state.join("mestro/settings.toml")).unwrap();
    let config = mestro::config::load(Some(&file)).expect("a broken file must not be fatal");
    assert_eq!(config.music_assistant.url, "", "defaults");
    let problems = mestro::config::problems();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].key, "config file");

    // 3. a clean load clears the earlier report
    fs::write(&file, "[theme]\npalette = \"amber\"\n").unwrap();
    mestro::config::load(Some(&file)).unwrap();
    assert!(mestro::config::problems().is_empty());

    // 4. a config directory merges every *.toml — a config.toml
    //    next to window.toml must not hide it
    fs::write(dir.join("config.toml"), "[wifi]\ncountry = \"DE\"\n").unwrap();
    fs::write(dir.join("window.toml"), "[window]\nscale = 1.5\n").unwrap();
    let config = mestro::config::load(Some(&dir)).unwrap();
    assert_eq!(config.window.scale, 1.5, "window.toml beside config.toml");
    assert_eq!(config.wifi.country, "DE", "config.toml still read");

    fs::remove_dir_all(&state).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
