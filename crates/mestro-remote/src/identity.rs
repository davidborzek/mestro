//! Who this player is on the network: a label. The stable id
//! (the MAC-derived `device_id`, `own_player_id` and the name
//! `device_suffix`) lives in `mestro_os::identity`; this is
//! the player-facing name built from the config and the kernel's
//! hostname.

pub use mestro_os::identity::{device_id, device_suffix, own_player_id};

/// the player's label: the configured Music Assistant player name,
/// else the hostname, else "mestro"
pub(crate) fn label(configured_player: &str, hostname: &str) -> String {
    [configured_player, hostname]
        .into_iter()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .unwrap_or("mestro")
        .to_string()
}

/// the kernel's hostname ("" when unreadable)
pub(crate) fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_prefers_the_player_then_the_hostname() {
        assert_eq!(label("Wohnzimmer", "pi"), "Wohnzimmer");
        assert_eq!(label("  ", "pi"), "pi");
        assert_eq!(label("", ""), "mestro");
    }
}
