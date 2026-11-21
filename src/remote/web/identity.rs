//! Who this player is on the network: a stable id (a hardware
//! MAC) and a label. Both ride `hello` and the mDNS announcement,
//! so a client can tell players apart and recognize one after an
//! address change.

use sha2::{Digest, Sha256};
use std::path::Path;

/// the lowercase-hex id without colons, 12 chars. Reads the
/// netdev tree: the first physical interface (one with a `device`
/// entry — no bridges, veth or tunnels) by name with a real MAC.
/// sysfs keeps the address while a link is down, so the id does
/// not change with cable or radio state; the hotspot lives on the
/// physical wifi interface and never changes its MAC. Without any
/// such interface (container, minimal VM) it derives from the
/// hostname instead — still stable across restarts
pub(crate) fn device_id(net_class: &Path, hostname: &str) -> String {
    let mut names: Vec<_> = std::fs::read_dir(net_class)
        .map(|dir| dir.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    names.sort();
    names
        .iter()
        .filter(|name| net_class.join(name).join("device").exists())
        .find_map(|name| mac_id(&net_class.join(name).join("address")))
        .unwrap_or_else(|| hostname_id(hostname))
}

/// one `address` file as an id: 6 hex pairs, not all zero
fn mac_id(address_file: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(address_file).ok()?;
    let id = raw.trim().replace(':', "").to_ascii_lowercase();
    let valid = id.len() == 12
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && id.bytes().any(|b| b != b'0');
    valid.then_some(id)
}

fn hostname_id(hostname: &str) -> String {
    Sha256::digest(hostname.as_bytes())[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

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

    /// a fake /sys/class/net: (name, MAC, physical)
    fn tree(entries: &[(&str, &str, bool)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (name, mac, physical) in entries {
            let dir = root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("address"), format!("{mac}\n")).unwrap();
            if *physical {
                std::fs::create_dir_all(dir.join("device")).unwrap();
            }
        }
        root
    }

    #[test]
    fn the_first_physical_interface_names_the_player() {
        let root = tree(&[
            ("br0", "02:00:00:00:00:01", false),
            ("eth0", "D8:3A:DD:77:34:55", true),
            ("lo", "00:00:00:00:00:00", false),
            ("wlan0", "D8:3A:DD:77:34:56", true),
        ]);
        assert_eq!(device_id(root.path(), "host"), "d83add773455");
    }

    #[test]
    fn unusable_addresses_are_skipped() {
        let root = tree(&[
            ("eth0", "00:00:00:00:00:00", true),
            ("eth1", "garbage", true),
            ("wlan0", "a8:72:4d:0c:41:71", true),
        ]);
        assert_eq!(device_id(root.path(), "host"), "a8724d0c4171");
    }

    #[test]
    fn without_a_physical_interface_the_hostname_decides() {
        let root = tree(&[("lo", "00:00:00:00:00:00", false)]);
        let id = device_id(root.path(), "wohnzimmer");
        assert_eq!(id.len(), 12);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(id, device_id(root.path(), "wohnzimmer"));
        assert_ne!(id, device_id(root.path(), "kueche"));
        // a missing tree behaves the same
        assert_eq!(device_id(Path::new("/nonexistent"), "wohnzimmer"), id);
    }

    #[test]
    fn the_label_prefers_the_player_then_the_hostname() {
        assert_eq!(label("Wohnzimmer", "pi"), "Wohnzimmer");
        assert_eq!(label("  ", "pi"), "pi");
        assert_eq!(label("", ""), "mestro");
    }
}
