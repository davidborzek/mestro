// what mestro does
pub mod config;
pub mod display;
pub mod library;
pub mod player;
pub mod settings;
pub mod standby;
pub mod ui;
pub mod vu;

// how it talks to the outside
pub mod providers;
pub mod remote;

// device-level system logic (kiosk builds: wifi setup, factory
// reset — feature `wifi-setup`)
#[cfg(feature = "wifi-setup")]
pub mod system;

/// `0.1.0 (<git hash>)`; the plain version when git was unavailable
pub fn app_version() -> String {
    version_string(env!("CARGO_PKG_VERSION"), env!("MESTRO_GIT"))
}

fn version_string(version: &str, git: &str) -> String {
    if git.is_empty() {
        version.to_string()
    } else {
        format!("{version} ({git})")
    }
}

#[cfg(test)]
mod tests {
    use super::version_string;

    #[test]
    fn version_carries_the_git_hash_when_known() {
        assert_eq!(version_string("0.1.0", "abc1234"), "0.1.0 (abc1234)");
        assert_eq!(version_string("0.1.0", ""), "0.1.0");
    }
}

slint::include_modules!();
