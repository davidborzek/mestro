//! [system]: the timezone (a device setting the web app writes) and
//! the files it is applied through (feature `timezone`). The paths
//! are config-file only; the defaults are the image's, a desktop or
//! another board overrides them — nothing is hard-wired in code.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct System {
    /// the IANA zone the user chose, or the web app's browser zone
    /// adopted once; empty = never chosen, the first paired browser
    /// may still set it. Written by the `timezone` op, which also
    /// puts it into `env-file` as `TZ`
    pub timezone: String,
    /// the env file the init script sources before it starts mestro
    pub env_file: String,
    /// the zoneinfo database a zone must exist in
    pub zoneinfo_dir: String,
    /// the system's zone link, read to learn the running zone when
    /// `TZ` is not set
    pub localtime: String,
}

impl Default for System {
    fn default() -> Self {
        Self {
            timezone: String::new(),
            env_file: "/data/mestro.env".into(),
            zoneinfo_dir: "/usr/share/zoneinfo".into(),
            localtime: "/etc/localtime".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_image_paths_and_the_zone_is_auto() {
        let system = System::default();
        assert_eq!(system.timezone, "", "empty means never chosen");
        assert_eq!(system.env_file, "/data/mestro.env");
        assert_eq!(system.zoneinfo_dir, "/usr/share/zoneinfo");
        assert_eq!(system.localtime, "/etc/localtime");
        let text = toml::to_string(&system).unwrap();
        assert!(text.contains("zoneinfo-dir = "), "{text}");
        assert_eq!(toml::from_str::<System>(&text).unwrap(), system);
        assert_eq!(toml::from_str::<System>("").unwrap(), system);
    }
}
