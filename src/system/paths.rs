//! Where the player image keeps its files and how it runs its
//! commands. These are image choices, not user settings: they are
//! fixed when mestro is built (a `MESTRO_*` environment variable at
//! build time; CONTRIBUTING.md, "Building for another image") and
//! default to the Buildroot image's values. Functions elsewhere take
//! their paths as parameters so tests run on temp dirs; only the
//! production call sites read these constants.

/// the build-time value, else the image default
const fn setting(from_build: Option<&'static str>, image_default: &'static str) -> &'static str {
    match from_build {
        Some(value) => value,
        None => image_default,
    }
}

/// the env file mestro owns and writes whole (`OUTPUT`, `NAME`, `SERVER`,
/// `EXTRA`); the only file squeezelite's init script sources
pub const SQUEEZELITE_ENV: &str = setting(
    option_env!("MESTRO_SQUEEZELITE_ENV"),
    "/data/state/mestro/squeezelite.env",
);

/// the command that restarts squeezelite (split on whitespace, no
/// shell) so a changed `OUTPUT` takes effect
pub const SQUEEZELITE_RESTART: &str = setting(
    option_env!("MESTRO_SQUEEZELITE_RESTART"),
    "/etc/init.d/S60squeezelite restart",
);

/// the env file mestro owns and writes whole (`TZ=`, from the timezone
/// setting); the init script sources it before it starts mestro
pub const ENV_FILE: &str = setting(
    option_env!("MESTRO_ENV_FILE"),
    "/data/state/mestro/mestro.env",
);

/// the zoneinfo database a chosen zone must exist in
pub const ZONEINFO_DIR: &str = setting(option_env!("MESTRO_ZONEINFO_DIR"), "/usr/share/zoneinfo");

/// the system zone link, read to learn the running zone when `TZ` is
/// not set
pub const LOCALTIME: &str = setting(option_env!("MESTRO_LOCALTIME"), "/etc/localtime");

/// the log files a diagnostics report tails, colon-separated (the image
/// logs mestro and the OTA scripts to tmpfs; the `.old` file is the
/// previous run — what a crash leaves)
pub const DIAG_LOGS: &str = setting(
    option_env!("MESTRO_DIAG_LOGS"),
    "/tmp/mestro.log:/tmp/mestro.log.old:/tmp/ota.log",
);

/// the command whose output joins a diagnostics report (split on
/// whitespace, no shell); empty = none
pub const DIAG_STATUS: &str = setting(
    option_env!("MESTRO_DIAG_STATUS"),
    "/usr/sbin/mestro-upgrade --status",
);

/// wpa_supplicant's control socket directory
pub const WPA_CTRL_DIR: &str = setting(option_env!("MESTRO_WPA_CTRL_DIR"), "/run/wpa_supplicant");

/// the persistent STA conf; must match what the boot scripts start
/// wpa_supplicant with
pub const WPA_CONF: &str = setting(option_env!("MESTRO_WPA_CONF"), "/data/wpa_supplicant.conf");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_the_build_value_else_the_image_default() {
        assert_eq!(setting(Some("/x"), "/default"), "/x");
        assert_eq!(setting(None, "/default"), "/default");
    }

    #[test]
    fn the_audio_settings_follow_their_build_variables() {
        assert_eq!(
            SQUEEZELITE_ENV,
            option_env!("MESTRO_SQUEEZELITE_ENV").unwrap_or("/data/state/mestro/squeezelite.env")
        );
        assert_eq!(
            SQUEEZELITE_RESTART,
            option_env!("MESTRO_SQUEEZELITE_RESTART")
                .unwrap_or("/etc/init.d/S60squeezelite restart")
        );
    }

    #[test]
    fn the_wifi_settings_follow_their_build_variables() {
        assert_eq!(
            WPA_CTRL_DIR,
            option_env!("MESTRO_WPA_CTRL_DIR").unwrap_or("/run/wpa_supplicant")
        );
        assert_eq!(
            WPA_CONF,
            option_env!("MESTRO_WPA_CONF").unwrap_or("/data/wpa_supplicant.conf")
        );
    }

    #[test]
    fn the_system_settings_follow_their_build_variables() {
        assert_eq!(
            ENV_FILE,
            option_env!("MESTRO_ENV_FILE").unwrap_or("/data/state/mestro/mestro.env")
        );
        assert_eq!(
            ZONEINFO_DIR,
            option_env!("MESTRO_ZONEINFO_DIR").unwrap_or("/usr/share/zoneinfo")
        );
        assert_eq!(
            LOCALTIME,
            option_env!("MESTRO_LOCALTIME").unwrap_or("/etc/localtime")
        );
    }

    #[test]
    fn the_diagnostics_settings_follow_their_build_variables() {
        assert_eq!(
            DIAG_LOGS,
            option_env!("MESTRO_DIAG_LOGS")
                .unwrap_or("/tmp/mestro.log:/tmp/mestro.log.old:/tmp/ota.log")
        );
        assert_eq!(
            DIAG_STATUS,
            option_env!("MESTRO_DIAG_STATUS").unwrap_or("/usr/sbin/mestro-upgrade --status")
        );
    }

    #[test]
    fn build_rs_reruns_on_every_variable_read_here() {
        let build_rs = include_str!("../../build.rs");
        let source = include_str!("paths.rs");
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
