//! The factory reset of the device state: delete the state files
//! so the next start is a first run — the settings overlay, the
//! pairing PIN (a new one is drawn on restart, every phone
//! re-pairs). Never touches /data/config (the user's config.toml) or the
//! boot partition.

use std::path::{Path, PathBuf};

/// the files a reset touches — configurable so tests run in a
/// temp dir
pub struct ResetPaths {
    pub settings: PathBuf,
    pub web_pin: PathBuf,
}

impl ResetPaths {
    /// the device locations: the state dir is /data/state/mestro on
    /// the kiosk (XDG_STATE_HOME)
    pub fn device() -> Self {
        let state = crate::paths::state_dir();
        Self {
            settings: state.join("settings.toml"),
            web_pin: state.join("web-pin"),
        }
    }
}

/// remove one file; missing is success (nothing to reset)
fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
    }
}

/// run the state reset: the settings overlay and the pairing PIN
/// go
pub fn factory_reset(paths: &ResetPaths) -> Result<(), String> {
    remove(&paths.settings)?;
    remove(&paths.web_pin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> ResetPaths {
        ResetPaths {
            settings: dir.join("settings.toml"),
            web_pin: dir.join("web-pin"),
        }
    }

    #[test]
    fn state_files_go_missing_ones_are_fine() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.settings, "[theme]\n").unwrap();
        std::fs::write(&p.web_pin, "ABCD2345").unwrap();
        factory_reset(&p).unwrap();
        assert!(!p.settings.exists());
        assert!(!p.web_pin.exists());
    }
}
