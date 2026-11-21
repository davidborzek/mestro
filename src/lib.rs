// what mestro does
pub(crate) mod atomic_file;
pub mod config;
pub mod display;
pub mod library;
pub mod locks;
pub mod player;
pub mod settings;
pub mod standby;
pub mod ui;
pub mod vu;

// how it talks to the outside
pub mod providers;
pub mod remote;

// device-level system logic (kiosk builds: wifi setup + factory
// reset — feature `wifi-setup`; the audio output and timezone
// pickers — features `audio-output`, `timezone`; each part is gated
// inside, the image paths in `system::paths` are always there)
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

// the slint-generated UI module, isolated so the restriction lints
// from [lints] in Cargo.toml don't fire on generated code: the
// generator's panics are its internal invariants, not code we
// maintain by hand (see docs in CONTRIBUTING.md)
#[expect(
    clippy::unwrap_used,
    clippy::todo,
    reason = "slint-generated code; panics are generator invariants"
)]
mod slint_modules {
    slint::include_modules!();
}
pub use slint_modules::*;
