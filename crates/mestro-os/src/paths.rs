//! Where the player keeps its per-user state, and the shared
//! helper the image's build-time path constants are built with
//! (each constant lives in the module that uses it). They are
//! image choices, not user settings: fixed when mestro is built
//! (a `MESTRO_*` environment variable at build time;
//! CONTRIBUTING.md, "Building for another image") and defaulting
//! to the Buildroot image's values. Functions elsewhere take
//! their paths as parameters so tests run on temp dirs; only the
//! production call sites read these constants.

/// the build-time value, else the image default
pub const fn setting(
    from_build: Option<&'static str>,
    image_default: &'static str,
) -> &'static str {
    match from_build {
        Some(value) => value,
        None => image_default,
    }
}

/// $XDG_STATE_HOME/mestro, per the base dir spec ~/.local/state
pub fn state_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default();
            home.join(".local/state")
        })
        .join("mestro")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_the_build_value_else_the_image_default() {
        assert_eq!(setting(Some("/x"), "/default"), "/x");
        assert_eq!(setting(None, "/default"), "/default");
    }
}
