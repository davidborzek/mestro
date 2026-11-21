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

/// the env file squeezelite's init script sources; the audio output
/// picker rewrites its `OUTPUT=` line
pub const SQUEEZELITE_ENV: &str = setting(
    option_env!("MESTRO_SQUEEZELITE_ENV"),
    "/data/squeezelite.env",
);

/// the command that restarts squeezelite (split on whitespace, no
/// shell) so a changed `OUTPUT` takes effect
pub const SQUEEZELITE_RESTART: &str = setting(
    option_env!("MESTRO_SQUEEZELITE_RESTART"),
    "/etc/init.d/S60squeezelite restart",
);

/// the env file the init script sources before it starts mestro; the
/// timezone picker writes `TZ=` there
pub const ENV_FILE: &str = setting(option_env!("MESTRO_ENV_FILE"), "/data/mestro.env");

/// the zoneinfo database a chosen zone must exist in
pub const ZONEINFO_DIR: &str = setting(option_env!("MESTRO_ZONEINFO_DIR"), "/usr/share/zoneinfo");

/// the system zone link, read to learn the running zone when `TZ` is
/// not set
pub const LOCALTIME: &str = setting(option_env!("MESTRO_LOCALTIME"), "/etc/localtime");

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
            option_env!("MESTRO_SQUEEZELITE_ENV").unwrap_or("/data/squeezelite.env")
        );
        assert_eq!(
            SQUEEZELITE_RESTART,
            option_env!("MESTRO_SQUEEZELITE_RESTART")
                .unwrap_or("/etc/init.d/S60squeezelite restart")
        );
    }

    #[test]
    fn the_system_settings_follow_their_build_variables() {
        assert_eq!(
            ENV_FILE,
            option_env!("MESTRO_ENV_FILE").unwrap_or("/data/mestro.env")
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
