//! Where the image keeps its logs — build settings of the image
//! (CONTRIBUTING.md, "Building for another image"), matching what
//! its init scripts write:
//!
//! - `S50mestro` logs mestro to `LOG_DIR/mestro.log` and moves the
//!   previous run to `mestro.log.old` (what a crash leaves)
//! - `S04trial`/`S99ota` and the update commands (`mestro-update`,
//!   reachable as `mestro-upgrade`) log to `LOG_DIR/ota.log`
//! - a trial boot snapshots its logs to [`trial_boot_dir`] on the
//!   data partition, so a rolled-back upgrade can be diagnosed
//!
//! The diagnostics download reads [`diagnostic_logs`] and runs
//! [`STATUS_COMMAND`]; it knows no path itself.

use std::path::{Path, PathBuf};

use crate::paths::setting;

/// the directory the image's init scripts log to (RAM on the image)
pub const LOG_DIR: &str = setting(option_env!("MESTRO_LOG_DIR"), "/tmp");

/// the command whose output describes the update state (split on
/// whitespace, no shell; empty = none)
pub const STATUS_COMMAND: &str = setting(
    option_env!("MESTRO_STATUS_COMMAND"),
    "/usr/sbin/mestro-update status",
);

/// the trial boot's log snapshots, under the state dir (the image
/// sets XDG_STATE_HOME=/data/state: /data/state/mestro/trial-boot)
pub fn trial_boot_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("trial-boot")
}

/// every log a diagnostics report tails, in report order: the live
/// logs, then what the last trial boot left behind
pub fn diagnostic_logs(log_dir: &Path, state_dir: &Path) -> Vec<PathBuf> {
    let trial = trial_boot_dir(state_dir);
    vec![
        log_dir.join("mestro.log"),
        log_dir.join("mestro.log.old"),
        log_dir.join("ota.log"),
        trial.join("summary.txt"),
        trial.join("ota.log"),
        trial.join("mestro.log"),
    ]
}

/// [`diagnostic_logs`] for the running image
pub fn image_diagnostic_logs() -> Vec<PathBuf> {
    diagnostic_logs(Path::new(LOG_DIR), &crate::paths::state_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_settings_follow_their_build_variables() {
        assert_eq!(LOG_DIR, option_env!("MESTRO_LOG_DIR").unwrap_or("/tmp"));
        assert_eq!(
            STATUS_COMMAND,
            option_env!("MESTRO_STATUS_COMMAND").unwrap_or("/usr/sbin/mestro-update status")
        );
    }

    #[test]
    fn the_image_layout_matches_the_init_scripts() {
        let logs = diagnostic_logs(Path::new("/tmp"), Path::new("/data/state/mestro"));
        let logs: Vec<String> = logs.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(
            logs,
            [
                "/tmp/mestro.log",
                "/tmp/mestro.log.old",
                "/tmp/ota.log",
                "/data/state/mestro/trial-boot/summary.txt",
                "/data/state/mestro/trial-boot/ota.log",
                "/data/state/mestro/trial-boot/mestro.log",
            ]
        );
    }

    #[test]
    fn the_trial_logs_follow_the_state_dir() {
        let logs = diagnostic_logs(Path::new("/var/log"), Path::new("/srv/state/mestro"));
        assert!(logs.contains(&PathBuf::from("/srv/state/mestro/trial-boot/summary.txt")));
        assert!(logs.contains(&PathBuf::from("/var/log/ota.log")));
    }

    #[test]
    fn build_rs_reruns_on_every_variable_read_here() {
        let build_rs = include_str!("../build.rs");
        let source = include_str!("logs.rs");
        let vars: Vec<&str> = source
            .split("option_env!(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|var| var.starts_with("MESTRO_"))
            .collect();
        assert!(!vars.is_empty());
        for var in vars {
            assert!(
                build_rs.contains(&format!("\"{var}\"")),
                "build.rs must emit rerun-if-env-changed for {var}"
            );
        }
    }
}
