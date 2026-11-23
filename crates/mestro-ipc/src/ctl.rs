//! the ctl wire protocol: one line in, one line out. The
//! vocabulary is exactly [`Command`]'s wire form — this module
//! owns the framing and the answer shapes, nothing else.

use crate::command::Command;

/// the one-line success answer
pub const OK: &str = "ok";

/// the one-line failure answer (`err <reason>`)
pub fn err(reason: impl std::fmt::Display) -> String {
    format!("err {reason}")
}

/// the answer for one wire line — validated against the command
/// vocabulary before it ever reaches the UI thread
pub fn answer_for(line: &str) -> Result<&'static str, String> {
    let line = line.trim();
    if line.is_empty() {
        return Err(err("empty command"));
    }
    match Command::parse(line) {
        // settings writes stay out of the ctl surface — the
        // settings UI and the web `settings` op own them
        Some(cmd) if cmd.is_settings() => Err(err("not a ctl command")),
        Some(_) => Ok(OK),
        None => Err(err(format!("unknown command: {line}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_command_is_acknowledged() {
        assert_eq!(answer_for(" play-pause\n"), Ok(OK));
        assert_eq!(answer_for("vu-select:2"), Ok(OK));
    }

    #[test]
    fn an_empty_line_is_an_error() {
        assert_eq!(answer_for("  "), Err("err empty command".to_string()));
    }

    #[test]
    fn settings_writes_are_not_ctl_commands() {
        for line in [
            "setting vu.skin round",
            "reset-setting vu.skin",
            "reset-all-settings",
        ] {
            assert_eq!(answer_for(line), Err("err not a ctl command".to_string()));
        }
    }

    #[test]
    fn an_unknown_command_is_named_in_the_error() {
        assert_eq!(
            answer_for("explode"),
            Err("err unknown command: explode".to_string())
        );
    }
}
