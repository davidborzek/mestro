//! Factory reset: delete the device-state files so the next start is
//! a first run — the settings overlay, the pairing PIN (a new one is
//! drawn on restart, every phone re-pairs). With `wifi` the
//! wpa_supplicant.conf returns to the template, so the next boot
//! comes up as the setup hotspot. Never touches /data/config (the
//! user's config.toml) or the boot partition.

use std::path::{Path, PathBuf};

/// the files a reset touches — configurable so tests run in a
/// temp dir
pub struct ResetPaths {
    pub settings: PathBuf,
    pub web_pin: PathBuf,
    /// the wifi conf ([wifi] conf on the device) + its .bak and
    /// .good sidecars
    pub wpa_conf: PathBuf,
    pub wpa_template: PathBuf,
}

impl ResetPaths {
    /// the device locations: the state dir is /data/state/mestro on
    /// the kiosk (XDG_STATE_HOME); the wifi conf is the configured
    /// one, the template is the image's seeded default
    pub fn device(wifi_conf: &str) -> Self {
        let state = crate::settings::state_dir();
        Self {
            settings: state.join("settings.toml"),
            web_pin: state.join("web-pin"),
            wpa_conf: PathBuf::from(wifi_conf),
            wpa_template: PathBuf::from("/etc/mestro-defaults/wpa_supplicant.conf"),
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

/// what the wifi part of the reset did — the op reports it, the
/// web shows it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiReset {
    /// wifi=false: the credentials stay
    Kept,
    /// restored from the image's template — the next boot is the
    /// first-run hotspot
    Restored,
    /// no template on this image: the credentials (and their
    /// backup) were removed instead — still a fresh start, not a
    /// silent keep of the old network
    Removed,
}

impl WifiReset {
    /// the wire name of the outcome
    pub fn as_str(&self) -> &'static str {
        match self {
            WifiReset::Kept => "kept",
            WifiReset::Restored => "restored",
            WifiReset::Removed => "removed",
        }
    }
}

/// run the reset: state files go, optionally the wifi credentials
/// return to the template — or, without a template, are removed
pub fn factory_reset(paths: &ResetPaths, wifi: bool) -> Result<WifiReset, String> {
    remove(&paths.settings)?;
    remove(&paths.web_pin)?;
    if !wifi {
        return Ok(WifiReset::Kept);
    }
    remove(&paths.wpa_conf.with_extension("conf.bak"))?;
    // the last-good copy must go with the credentials — a
    // surviving one would restore the old network at the next
    // hotspot failure
    remove(&paths.wpa_conf.with_extension("conf.good"))?;
    if paths.wpa_template.is_file() {
        std::fs::copy(&paths.wpa_template, &paths.wpa_conf)
            .map_err(|e| format!("cannot restore the wifi template: {e}"))?;
        Ok(WifiReset::Restored)
    } else {
        remove(&paths.wpa_conf)?;
        Ok(WifiReset::Removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> ResetPaths {
        ResetPaths {
            settings: dir.join("settings.toml"),
            web_pin: dir.join("web-pin"),
            wpa_conf: dir.join("wpa_supplicant.conf"),
            wpa_template: "/etc/mestro-defaults/wpa_supplicant.conf".into(),
        }
    }

    fn local_paths(dir: &Path) -> ResetPaths {
        let mut p = paths(dir);
        p.wpa_template = dir.join("template.conf");
        p
    }

    #[test]
    fn state_files_go_missing_ones_are_fine() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.settings, "[theme]\n").unwrap();
        std::fs::write(&p.web_pin, "ABCD2345").unwrap();
        assert_eq!(factory_reset(&p, false).unwrap(), WifiReset::Kept);
        assert!(!p.settings.exists());
        assert!(!p.web_pin.exists());
    }

    #[test]
    fn wifi_reset_restores_the_template_and_drops_the_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let p = local_paths(dir.path());
        std::fs::write(&p.wpa_template, "network={\n\tssid=\"YourNetwork\"\n}\n").unwrap();
        std::fs::write(&p.wpa_conf, "network={\n\tssid=\"Real\"\n}\n").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.bak"), "old").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.good"), "last good").unwrap();
        assert_eq!(factory_reset(&p, true).unwrap(), WifiReset::Restored);
        assert_eq!(
            std::fs::read_to_string(&p.wpa_conf).unwrap(),
            std::fs::read_to_string(&p.wpa_template).unwrap()
        );
        assert!(!p.wpa_conf.with_extension("conf.bak").exists());
        // the last-good copy must not outlive the reset — it
        // would resurrect the old network at the next hotspot
        // failure
        assert!(!p.wpa_conf.with_extension("conf.good").exists());
    }

    #[test]
    fn a_missing_template_means_removal_not_a_silent_keep() {
        // an image without the seeded template must not report a
        // reset while the old credentials survive — they go, and
        // the outcome says so
        let dir = tempfile::tempdir().unwrap();
        let p = local_paths(dir.path());
        std::fs::write(&p.wpa_conf, "network={\n\tssid=\"Real\"\n}\n").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.bak"), "old").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.good"), "last good").unwrap();
        assert_eq!(factory_reset(&p, true).unwrap(), WifiReset::Removed);
        assert!(!p.wpa_conf.exists());
        assert!(!p.wpa_conf.with_extension("conf.bak").exists());
        assert!(!p.wpa_conf.with_extension("conf.good").exists());
        assert_eq!(WifiReset::Removed.as_str(), "removed");
        assert_eq!(WifiReset::Restored.as_str(), "restored");
        assert_eq!(WifiReset::Kept.as_str(), "kept");
    }

    #[test]
    fn no_wifi_reset_keeps_the_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let p = local_paths(dir.path());
        std::fs::write(&p.wpa_conf, "network={\n\tssid=\"Real\"\n}\n").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.good"), "last good").unwrap();
        assert_eq!(factory_reset(&p, false).unwrap(), WifiReset::Kept);
        assert!(p.wpa_conf.exists());
        // the sidecars belong to the credentials: without the
        // wifi reset they stay
        assert!(p.wpa_conf.with_extension("conf.good").exists());
    }
}
