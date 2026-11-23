//! [audio]: the squeezelite settings (feature `audio-output` writes
//! them into the env file mestro owns). `output`, `name` and `server`
//! are device settings the web app edits; `extra` is config-file only.
//! Empty = derived by mestro (`crate::system::squeezelite`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Audio {
    /// the ALSA device squeezelite plays on (`hw:CARD=<id>`); empty =
    /// a USB DAC, else an I2S HAT
    pub output: String,
    /// the player's name in Music Assistant; empty = `mestro-XXXX`
    pub name: String,
    /// the Music Assistant host squeezelite connects to; empty = the
    /// host of `[music-assistant] url`
    pub server: String,
    /// further squeezelite flags, passed verbatim (the init script
    /// splits them on whitespace)
    pub extra: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_derived_by_default_and_roundtrips() {
        let audio = Audio::default();
        assert_eq!(audio.output, "");
        assert_eq!(audio.name, "");
        assert_eq!(audio.server, "");
        assert_eq!(audio.extra, "");
        let text = toml::to_string(&audio).unwrap();
        assert_eq!(toml::from_str::<Audio>(&text).unwrap(), audio);
        assert_eq!(toml::from_str::<Audio>("").unwrap(), audio);
    }

    #[test]
    fn the_keys_are_kebab_case_and_unknown_ones_are_refused() {
        let audio: Audio =
            toml::from_str("output = \"hw:CARD=S3\"\nname = \"HiFi\"\nextra = \"-r\"\n").unwrap();
        assert_eq!(audio.output, "hw:CARD=S3");
        assert_eq!(audio.name, "HiFi");
        assert_eq!(audio.extra, "-r");
        assert!(toml::from_str::<Audio>("volume = 3\n").is_err());
    }
}
