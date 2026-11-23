//! Who this player is on the network: a stable id (a hardware
//! MAC) and a label. Both ride `hello` and the mDNS announcement,
//! so a client can tell players apart and recognize one after an
//! address change.

use sha2::{Digest, Sha256};
use std::path::Path;

/// the persisted fallback id lives here, in the state dir
const ID_FILE: &str = "device-id";

/// the lowercase-hex id without colons, 12 chars. Reads the
/// netdev tree: the first physical interface (one with a `device`
/// entry — no bridges, veth or tunnels) by name with a real MAC.
/// sysfs keeps the address while a link is down, so the id does
/// not change with cable or radio state; the hotspot lives on the
/// physical wifi interface and never changes its MAC. Without any
/// such interface (container, minimal VM) a random id is generated
/// once and persisted in the state dir (`device-id`): stable across
/// restarts, and two installs never share one. When the state dir is
/// not writable the id derives from the hostname instead: stable
/// across restarts as well
pub(crate) fn device_id(net_class: &Path, state_dir: &Path, hostname: &str) -> String {
    physical_mac_id(net_class).unwrap_or_else(|| persisted_id(&state_dir.join(ID_FILE), hostname))
}

/// the device's own Music Assistant player id: squeezelite registers
/// with the MAC of its interface, in MA's lowercase colon form. Only a
/// real MAC counts, never the random or hostname fallback of
/// [`device_id`] — that would match nothing and pretend otherwise
pub(crate) fn own_player_id(net_class: &Path) -> Option<String> {
    let id = physical_mac_id(net_class)?;
    let pairs: Vec<&str> = (0..id.len()).step_by(2).map(|i| &id[i..i + 2]).collect();
    Some(pairs.join(":"))
}

/// the first physical interface (by name) with a real MAC, as 12 hex chars
fn physical_mac_id(net_class: &Path) -> Option<String> {
    let mut names: Vec<_> = std::fs::read_dir(net_class)
        .map(|dir| dir.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    names.sort();
    names
        .iter()
        .filter(|name| net_class.join(name).join("device").exists())
        .find_map(|name| mac_id(&net_class.join(name).join("address")))
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

/// the stored id when it is valid, else a fresh random one that is
/// stored for the next start. When storing fails the id would change
/// with every restart (and break the Home Assistant identity), so the
/// stable hostname-derived id stands in and the failure is warned once
fn persisted_id(path: &Path, hostname: &str) -> String {
    if let Ok(stored) = std::fs::read_to_string(path) {
        let id = stored.trim().to_string();
        if is_id(&id) {
            return id;
        }
    }
    let id = random_id();
    match crate::atomic_file::write_atomic(path, &id) {
        Ok(()) => id,
        Err(e) => {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                tracing::warn!(
                    "web: cannot persist the player id ({e}), using the hostname-derived one"
                );
            });
            hostname_id(hostname)
        }
    }
}

/// a stable id from the hostname: 12 hex chars of its SHA-256
fn hostname_id(hostname: &str) -> String {
    let digest = Sha256::digest(hostname.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

fn is_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 12 random lowercase-hex chars
fn random_id() -> String {
    let mut bytes = [0u8; 6];
    // NB: read_exact, never fs::read — /dev/urandom is endless
    let random_ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .is_ok();
    if !random_ok {
        tracing::warn!("web: player id falls back to time entropy");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(nanos.to_le_bytes());
        hasher.update(std::process::id().to_le_bytes());
        bytes.copy_from_slice(&hasher.finalize()[..6]);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// the 4 uppercase hex digits that make a player's name unique
/// (`mestro-XXXX`: the hotspot SSID and squeezelite's `NAME`): the
/// last two bytes of the wifi interface's MAC (`wifi_mac`, empty when
/// there is none), else of the player id. Empty when neither has 4
/// hex digits
pub fn device_suffix(wifi_mac: &str, device_id: &str) -> String {
    hex_tail(wifi_mac)
        .or_else(|| hex_tail(device_id))
        .unwrap_or_default()
}

fn hex_tail(raw: &str) -> Option<String> {
    let hex: String = raw.trim().chars().filter(|c| *c != ':').collect();
    let tail = hex.get(hex.len().checked_sub(4)?..)?;
    tail.bytes()
        .all(|b| b.is_ascii_hexdigit())
        .then(|| tail.to_ascii_uppercase())
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

    const HOST: &str = "wohnzimmer";

    /// a fresh state dir that outlives the test (the OS cleans up)
    fn state() -> std::path::PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

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
        assert_eq!(device_id(root.path(), &state(), HOST), "d83add773455");
    }

    #[test]
    fn the_own_player_id_is_the_physical_mac_in_colon_form() {
        let root = tree(&[
            ("br0", "02:00:00:00:00:01", false),
            ("eth0", "D8:3A:DD:BC:6B:EC", true),
        ]);
        assert_eq!(
            own_player_id(root.path()).as_deref(),
            Some("d8:3a:dd:bc:6b:ec")
        );
    }

    #[test]
    fn without_a_physical_interface_there_is_no_own_player_id() {
        let root = tree(&[("lo", "00:00:00:00:00:00", false)]);
        assert_eq!(own_player_id(root.path()), None);
        assert_eq!(own_player_id(Path::new("/nonexistent")), None);
    }

    #[test]
    fn unusable_addresses_are_skipped() {
        let root = tree(&[
            ("eth0", "00:00:00:00:00:00", true),
            ("eth1", "garbage", true),
            ("wlan0", "a8:72:4d:0c:41:71", true),
        ]);
        assert_eq!(device_id(root.path(), &state(), HOST), "a8724d0c4171");
    }

    #[test]
    fn a_physical_mac_wins_over_a_stored_id() {
        let root = tree(&[("wlan0", "a8:72:4d:0c:41:71", true)]);
        let state = state();
        std::fs::write(state.join(ID_FILE), "0123456789ab").unwrap();
        assert_eq!(device_id(root.path(), &state, HOST), "a8724d0c4171");
    }

    #[test]
    fn without_a_physical_interface_a_persisted_random_id_is_reused() {
        let root = tree(&[("lo", "00:00:00:00:00:00", false)]);
        let state = state();
        let id = device_id(root.path(), &state, HOST);
        assert!(is_id(&id), "{id}");
        assert_eq!(std::fs::read_to_string(state.join(ID_FILE)).unwrap(), id);
        assert_eq!(device_id(root.path(), &state, HOST), id);
        // a missing tree behaves the same
        assert_eq!(device_id(Path::new("/nonexistent"), &state, HOST), id);
    }

    #[test]
    fn an_unwritable_state_dir_falls_back_to_the_hostname_id_every_time() {
        let root = tree(&[]);
        // a regular file where the state dir should be: nothing can
        // be created below it
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let state = blocker.path().join("state");
        let first = device_id(root.path(), &state, HOST);
        let second = device_id(root.path(), &state, HOST);
        assert!(is_id(&first), "{first}");
        assert_eq!(first, second, "a failed persist must not mint a new id");
        assert_eq!(first, hostname_id(HOST));
        assert_ne!(first, hostname_id("kueche"));
    }

    #[test]
    fn persisting_the_id_leaves_no_tmp_files() {
        let root = tree(&[]);
        let state = state().join("nested");
        let id = device_id(root.path(), &state, HOST);
        assert_eq!(std::fs::read_to_string(state.join(ID_FILE)).unwrap(), id);
        let names: Vec<_> = std::fs::read_dir(&state)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(ID_FILE)]);
    }

    #[test]
    fn two_fresh_state_dirs_get_different_ids() {
        let root = tree(&[]);
        let first = device_id(root.path(), &state(), HOST);
        let second = device_id(root.path(), &state(), HOST);
        assert!(is_id(&first) && is_id(&second));
        assert_ne!(first, second);
    }

    #[test]
    fn a_corrupt_stored_id_is_replaced() {
        let root = tree(&[]);
        let state = state();
        std::fs::write(state.join(ID_FILE), "not-an-id").unwrap();
        let id = device_id(root.path(), &state, HOST);
        assert!(is_id(&id), "{id}");
        assert_eq!(std::fs::read_to_string(state.join(ID_FILE)).unwrap(), id);
    }

    #[test]
    fn the_suffix_is_the_wifi_macs_last_two_bytes_uppercase() {
        assert_eq!(device_suffix("d8:3a:dd:77:34:56", "a1b2c3d46bec"), "3456");
        assert_eq!(device_suffix("a8:72:4d:0c:41:ab", "a1b2c3d46bec"), "41AB");
    }

    #[test]
    fn without_a_wifi_mac_the_suffix_is_the_device_ids() {
        for mac in ["", "garbage", "zz:zz:zz:zz:zz:zz"] {
            assert_eq!(device_suffix(mac, "a1b2c3d46bec"), "6BEC", "{mac:?}");
        }
        assert_eq!(device_suffix("", "  a1b2c3d46bec\n"), "6BEC");
    }

    #[test]
    fn a_short_mac_still_gives_four_digits_and_no_source_gives_none() {
        assert_eq!(device_suffix("aa:bb", ""), "AABB");
        assert_eq!(device_suffix("", ""), "");
        assert_eq!(device_suffix("", "wohnzimmer"), "");
    }

    #[test]
    fn the_label_prefers_the_player_then_the_hostname() {
        assert_eq!(label("Wohnzimmer", "pi"), "Wohnzimmer");
        assert_eq!(label("  ", "pi"), "pi");
        assert_eq!(label("", ""), "mestro");
    }
}
