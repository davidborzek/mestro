//! [background]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Background {
    /// blurred cover art as the now-playing background
    pub blurred_cover: bool,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            blurred_cover: true,
        }
    }
}
