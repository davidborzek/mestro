//! [system]: the device settings the web app writes (feature
//! `timezone` applies the zone). Where the image keeps the files it
//! is applied through is a build setting, not config
//! (`crate::system::paths`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct System {
    /// the IANA zone the user chose, or the web app's browser zone
    /// adopted once; empty = never chosen, the first paired browser
    /// may still set it. Written by the `timezone` op, which also
    /// puts it into the mestro env file as `TZ`
    pub timezone: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zone_is_auto_by_default_and_roundtrips() {
        let system = System::default();
        assert_eq!(system.timezone, "", "empty means never chosen");
        let text = toml::to_string(&system).unwrap();
        assert_eq!(toml::from_str::<System>(&text).unwrap(), system);
        assert_eq!(toml::from_str::<System>("").unwrap(), system);
    }
}
