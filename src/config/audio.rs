//! [audio]: where the player image keeps the squeezelite env file
//! the web app edits (feature `audio-output`). Config file only; the
//! defaults are the image's paths, a desktop or another board
//! overrides them — nothing is hard-wired in code.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Audio {
    /// the env file squeezelite's init script sources; the picker
    /// rewrites its `OUTPUT=` line. A missing file turns the
    /// capability off
    pub env_file: String,
    /// the command that restarts squeezelite (split on whitespace,
    /// no shell) so a changed `OUTPUT` takes effect
    pub restart_command: String,
    /// the ALSA proc tree the playback devices are listed from
    pub asound_dir: String,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            env_file: "/data/squeezelite.env".into(),
            restart_command: "/etc/init.d/S60squeezelite restart".into(),
            asound_dir: "/proc/asound".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_image_paths_and_roundtrip() {
        let audio = Audio::default();
        assert_eq!(audio.env_file, "/data/squeezelite.env");
        assert_eq!(audio.restart_command, "/etc/init.d/S60squeezelite restart");
        assert_eq!(audio.asound_dir, "/proc/asound");
        let text = toml::to_string(&audio).unwrap();
        assert!(text.contains("restart-command = "), "{text}");
        assert_eq!(toml::from_str::<Audio>(&text).unwrap(), audio);
        assert_eq!(toml::from_str::<Audio>("").unwrap(), audio);
    }
}
