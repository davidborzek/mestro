//! The keyboard as a control source: key names to the `KeyEvent.text`
//! slint delivers, the default key map and the compiled map the
//! `[keyboard]` section overrides. The commands themselves are
//! `mestro_ipc::command::Command`.

use mestro_ipc::command::Command;
use std::collections::BTreeMap;

/// a key NAME (config-speak: "left", "escape", "f1", or a
/// single character) to the KeyEvent.text encoding slint
/// delivers — the special-key codepoints come from slint's
/// own key table (internal/common/key_codes.rs)
pub fn key_text(name: &str) -> Option<String> {
    Some(
        match name.to_ascii_lowercase().as_str() {
            "backspace" => "\u{0008}",
            "tab" => "\u{0009}",
            "enter" | "return" => "\u{000a}",
            "escape" => "\u{001b}",
            "delete" => "\u{007f}",
            "space" => "\u{0020}",
            "up" => "\u{F700}",
            "down" => "\u{F701}",
            "left" => "\u{F702}",
            "right" => "\u{F703}",
            "f1" => "\u{F704}",
            "f2" => "\u{F705}",
            "f3" => "\u{F706}",
            "f4" => "\u{F707}",
            "f5" => "\u{F708}",
            "f6" => "\u{F709}",
            "f7" => "\u{F70A}",
            "f8" => "\u{F70B}",
            "f9" => "\u{F70C}",
            "f10" => "\u{F70D}",
            "f11" => "\u{F70E}",
            "f12" => "\u{F70F}",
            "home" => "\u{F729}",
            "end" => "\u{F72B}",
            "pageup" => "\u{F72C}",
            "pagedown" => "\u{F72D}",
            // printable keys: the name is the text
            c if c.chars().count() == 1 => c,
            _ => return None,
        }
        .into(),
    )
}

/// the default key map ([keyboard] overrides entries).
/// Optimized for USB HID remotes, which register as plain
/// keyboards (arrows / OK=Enter / Back=Escape / Home=Home),
/// with keyboard fallbacks borrowed from the conventions
/// people already know: VLC (space play, n/p tracks) and
/// Kodi (+/- volume, PageUp/PageDown skip)
pub fn default_keymap() -> &'static [(&'static str, &'static str)] {
    &[
        ("right", "cycle-forward"),
        ("down", "cycle-forward"),
        ("left", "cycle-back"),
        ("up", "cycle-back"),
        ("escape", "back"),
        ("backspace", "back"),
        ("home", "home"),
        ("h", "home"),
        ("space", "play-pause"),
        ("enter", "play-pause"),
        ("n", "next-track"),
        ("pagedown", "next-track"),
        ("p", "prev-track"),
        ("pageup", "prev-track"),
        ("f12", "volume-up"),
        ("+", "volume-up"),
        ("f11", "volume-down"),
        ("-", "volume-down"),
    ]
}

/// the compiled map: KeyEvent.text -> Command. Config entries
/// override the defaults; invalid names/commands warn and drop
pub fn keymap(overrides: &BTreeMap<String, String>) -> BTreeMap<String, Command> {
    let mut map: BTreeMap<String, Command> = default_keymap()
        .iter()
        .filter_map(|(k, c)| Some((key_text(k)?, Command::parse(c)?)))
        .collect();
    for (k, c) in overrides {
        let (Some(text), Some(cmd)) = (key_text(k), Command::parse(c)) else {
            tracing::warn!("keyboard: invalid binding {k:?} = {c:?}, ignoring");
            continue;
        };
        map.insert(text, cmd);
    }
    map
}
