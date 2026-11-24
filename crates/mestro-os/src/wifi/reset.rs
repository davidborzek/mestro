//! The factory reset's wifi half: the credentials return to the
//! image's template ([`paths::WPA_TEMPLATE`]) so the next boot
//! comes up as the setup hotspot — or, without a template on this
//! image, go entirely. Part of the `setup` lane: only the binary's
//! `wifi-setup` build reaches for it.

use std::path::{Path, PathBuf};

use super::paths;

/// the files the wifi reset touches — configurable so tests run
/// in a temp dir
pub struct WifiResetPaths {
    pub wpa_conf: PathBuf,
    pub wpa_template: PathBuf,
}

impl WifiResetPaths {
    /// the image locations: the persistent STA conf
    /// ([`paths::WPA_CONF`]) and the image's seeded default
    /// ([`paths::WPA_TEMPLATE`])
    pub fn device() -> Self {
        Self {
            wpa_conf: PathBuf::from(paths::WPA_CONF),
            wpa_template: PathBuf::from(paths::WPA_TEMPLATE),
        }
    }
}

/// what the wifi part of the reset did — the op reports it, the
/// web shows it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiReset {
    /// wifi=false: the credentials stay (the caller picks this
    /// without calling [`reset_credentials`])
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

/// remove one file; missing is success (nothing to reset)
fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
    }
}

/// reset the wifi credentials: the `.bak` and `.good` sidecars
/// always go, then the template is restored — or, without one,
/// the conf is removed
pub fn reset_credentials(paths: &WifiResetPaths) -> Result<WifiReset, String> {
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

    fn paths(dir: &Path) -> WifiResetPaths {
        WifiResetPaths {
            wpa_conf: dir.join("wpa_supplicant.conf"),
            wpa_template: dir.join("template.conf"),
        }
    }

    #[test]
    fn wifi_reset_restores_the_template_and_drops_the_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.wpa_template, "network={\n\tssid=\"YourNetwork\"\n}\n").unwrap();
        std::fs::write(&p.wpa_conf, "network={\n\tssid=\"Real\"\n}\n").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.bak"), "old").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.good"), "last good").unwrap();
        assert_eq!(reset_credentials(&p).unwrap(), WifiReset::Restored);
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
        let p = paths(dir.path());
        std::fs::write(&p.wpa_conf, "network={\n\tssid=\"Real\"\n}\n").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.bak"), "old").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.good"), "last good").unwrap();
        assert_eq!(reset_credentials(&p).unwrap(), WifiReset::Removed);
        assert!(!p.wpa_conf.exists());
        assert!(!p.wpa_conf.with_extension("conf.bak").exists());
        assert!(!p.wpa_conf.with_extension("conf.good").exists());
        assert_eq!(WifiReset::Removed.as_str(), "removed");
        assert_eq!(WifiReset::Restored.as_str(), "restored");
        assert_eq!(WifiReset::Kept.as_str(), "kept");
    }
}
