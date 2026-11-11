//! the ctl wire protocol: one line in, one line out. The
//! vocabulary is exactly `control::Command`'s wire form — this
//! module owns the framing and the answer shapes, nothing else.

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
    match crate::remote::control::Command::parse(line) {
        // settings writes stay out of the ctl surface — the
        // settings UI and the web `settings` op own them
        Some(cmd) if cmd.is_settings() => Err(err("not a ctl command")),
        Some(_) => Ok(OK),
        None => Err(err(format!("unknown command: {line}"))),
    }
}
