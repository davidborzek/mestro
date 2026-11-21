//! [diagnostics]: what the web app's "Download diagnostics" report
//! collects besides the config and the player state. Config-file
//! only; the defaults are the player image's paths, missing files and
//! commands are skipped — nothing here is required to exist.

use serde::{Deserialize, Serialize};

/// the most log lines a report may carry per file
pub const MAX_LOG_LINES: usize = 5000;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Diagnostics {
    /// log files whose last `log-lines` lines go into the report
    /// (the image logs mestro and the OTA scripts to tmpfs; the
    /// `.old` file is the previous run — what a crash leaves)
    pub logs: Vec<String>,
    /// lines kept per log file (1..=5000)
    pub log_lines: usize,
    /// small text files copied whole (kernel version, sound cards)
    pub files: Vec<String>,
    /// a command whose output joins the report (split on whitespace,
    /// no shell, 5 s budget); empty or missing program = skipped
    pub status_command: String,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            logs: vec![
                "/tmp/mestro.log".into(),
                "/tmp/mestro.log.old".into(),
                "/tmp/ota.log".into(),
            ],
            log_lines: 200,
            files: vec!["/proc/version".into(), "/proc/asound/cards".into()],
            status_command: "/usr/sbin/mestro-upgrade --status".into(),
        }
    }
}

impl Diagnostics {
    /// the line count the report really uses
    pub fn lines(&self) -> usize {
        self.log_lines.clamp(1, MAX_LOG_LINES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_image_paths_and_roundtrip() {
        let diagnostics = Diagnostics::default();
        assert_eq!(
            diagnostics.logs,
            ["/tmp/mestro.log", "/tmp/mestro.log.old", "/tmp/ota.log"]
        );
        assert_eq!(diagnostics.files, ["/proc/version", "/proc/asound/cards"]);
        assert_eq!(
            diagnostics.status_command,
            "/usr/sbin/mestro-upgrade --status"
        );
        let text = toml::to_string(&diagnostics).unwrap();
        assert!(text.contains("log-lines = 200"), "{text}");
        assert_eq!(toml::from_str::<Diagnostics>(&text).unwrap(), diagnostics);
        assert_eq!(toml::from_str::<Diagnostics>("").unwrap(), diagnostics);
    }

    #[test]
    fn the_line_count_is_clamped() {
        let lines = |n| {
            Diagnostics {
                log_lines: n,
                ..Diagnostics::default()
            }
            .lines()
        };
        assert_eq!(lines(0), 1);
        assert_eq!(lines(200), 200);
        assert_eq!(lines(1_000_000), MAX_LOG_LINES);
    }
}
