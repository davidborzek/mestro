//! [background]

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
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
