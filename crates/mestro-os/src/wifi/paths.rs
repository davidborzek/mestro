//! The image's wpa_supplicant paths — build settings of the image
//! (CONTRIBUTING.md, "Building for another image"). Owned by this
//! crate and always compiled: the web endpoint names `WPA_CONF` in
//! every build (the diagnostics redaction) and the setup lane's
//! factory reset (`wifi::reset`) pairs it with `WPA_TEMPLATE`,
//! while `WPA_CTRL_DIR` feeds the setup lane's plan.

use crate::paths::setting;

/// wpa_supplicant's control socket directory
pub const WPA_CTRL_DIR: &str = setting(option_env!("MESTRO_WPA_CTRL_DIR"), "/run/wpa_supplicant");

/// the persistent STA conf; must match what the boot scripts start
/// wpa_supplicant with
pub const WPA_CONF: &str = setting(option_env!("MESTRO_WPA_CONF"), "/data/wpa_supplicant.conf");

/// the image's seeded default conf — the factory reset restores it,
/// so the next boot comes up as the setup hotspot
pub const WPA_TEMPLATE: &str = setting(
    option_env!("MESTRO_WPA_TEMPLATE"),
    "/etc/mestro-defaults/wpa_supplicant.conf",
);

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            WPA_TEMPLATE,
            option_env!("MESTRO_WPA_TEMPLATE")
                .unwrap_or("/etc/mestro-defaults/wpa_supplicant.conf")
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
