//! [window]

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Window {
    /// logical window size as WIDTHxHEIGHT
    pub size: String,
    pub fullscreen: bool,
    /// UI scale factor: everything renders larger — for panels
    /// that physically show the resolution smaller (a 5" 800x480
    /// next to a 7" one). Passed to slint as SLINT_SCALE_FACTOR
    /// before the backend initializes; the window reflows into
    /// physical/factor logical pixels.
    pub scale: f32,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            size: "800x480".into(),
            fullscreen: false,
            scale: 1.0,
        }
    }
}

/// WIDTHxHEIGHT parser, shared by --size and the config file
pub fn parse_size(s: &str) -> Result<(f32, f32), String> {
    let (w, h) = s
        .split_once('x')
        .ok_or_else(|| format!("expected WIDTHxHEIGHT, got {s:?}"))?;
    let w: f32 = w.parse().map_err(|e| format!("width: {e}"))?;
    let h: f32 = h.parse().map_err(|e| format!("height: {e}"))?;
    if w <= 0.0 || h <= 0.0 {
        return Err("width and height must be positive".into());
    }
    Ok((w, h))
}
