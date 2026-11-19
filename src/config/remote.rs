//! The embedded web endpoint (`[remote]`): setup-wizard SPA and
//! the WS channel future remote editors ride (docs/remote.md).
//! LAN-only by design — auth is the pairing PIN shown on the
//! kiosk (restart-stable, persisted 0600), there is no TLS.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Remote {
    /// serve the wizard + WS channel at all
    pub enabled: bool,
    /// TCP port on all interfaces
    pub port: u16,
}

impl Default for Remote {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 8787,
        }
    }
}
