//! wpa_supplicant.conf writing. The STA conf is the persistent one
//! (`[wifi] conf`, PC-editable, survives re-flashes); the AP conf
//! is ephemeral under /run. Writes are atomic (tmp in the same
//! dir, fsync, rename) and the STA write keeps the previous file
//! as `.bak` — the join-failure path restores it.

#![deny(clippy::indexing_slicing)]

use super::pbkdf2;
use crate::atomic_file::{sync_dir, write_atomic, write_atomic_with};
use std::path::Path;

/// the socket and config-reload lines every mestro-written conf
/// carries, plus the configured regdom — the ctrl dir is the
/// configured one; the boot path (S39wifi) also passes `-C`, so
/// old files gain the socket either way. An empty country adds no
/// line (the kernel's regdom stays)
fn header(ctrl_dir: &str, country: &str) -> String {
    let country = if country.is_empty() {
        String::new()
    } else {
        format!("country={country}\n")
    };
    format!("ctrl_interface={ctrl_dir}\nupdate_config=1\n{country}")
}

/// one SSID's value: quoted with escaped `"` and `\` when every byte
/// is printable ASCII, raw bytes as hex otherwise (the two forms
/// wpa_supplicant's parser takes)
fn ssid_value(ssid: &str) -> String {
    let bytes = ssid.as_bytes();
    if bytes.iter().all(|b| (0x20..=0x7e).contains(b)) {
        let escaped = ssid.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        hex
    }
}

/// the STA conf for one network. `psk` hashed via PBKDF2-SHA1 (the
/// passphrase never lands in the file), `None` = open network
pub fn sta_conf(
    ssid: &str,
    psk: Option<&str>,
    ctrl_dir: &str,
    country: &str,
) -> Result<String, String> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("ssid must be 1 to 32 bytes".into());
    }
    let key = match psk {
        Some(psk) => format!("\tpsk={}\n", pbkdf2::hex(&pbkdf2::wpa_psk(psk, ssid)?)),
        None => "\tkey_mgmt=NONE\n".to_string(),
    };
    Ok(format!(
        "# written by mestro — the hotspot setup\n{}\nnetwork={{\n\tssid={}\n{key}\tscan_ssid=1\n}}\n",
        header(ctrl_dir, country),
        ssid_value(ssid)
    ))
}

/// the AP conf: an open network on channel 6 (2437 MHz), the name
/// from the wireless interface's MAC
pub fn ap_conf(ap_ssid: &str, ctrl_dir: &str, country: &str) -> String {
    format!(
        "# mestro setup hotspot — ephemeral, regenerated on every boot\n{}\nnetwork={{\n\tmode=2\n\tfrequency=2437\n\tkey_mgmt=NONE\n\tssid={}\n}}\n",
        header(ctrl_dir, country),
        ssid_value(ap_ssid)
    )
}

/// does this conf text contain a network worth booting into STA
/// mode? The seeded template (ssid "YourNetwork") does not count
pub fn usable_sta(text: &str) -> bool {
    let mut in_network = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("network={") {
            in_network = true;
        } else if in_network && line.starts_with("ssid=") {
            let ssid = line.trim_start_matches("ssid=").trim_matches('"');
            if !ssid.is_empty() && ssid != "YourNetwork" {
                return true;
            }
        } else if line == "}" {
            in_network = false;
        }
    }
    false
}

/// copy atomically — the same crash discipline as
/// [`write_atomic`]: tmp + fsync + rename, never an in-place
/// overwrite. NB a vfat rename is NOT crash-atomic: a power cut
/// can still leave both or neither file — [`recover_sta`] mends
/// that window at boot
fn copy_atomic(src: &Path, dst: &Path) -> Result<(), String> {
    copy_atomic_with(src, dst, &sync_dir)
}

fn copy_atomic_with(
    src: &Path,
    dst: &Path,
    dir_sync: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let text = std::fs::read(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    write_atomic_with(dst, &String::from_utf8_lossy(&text), dir_sync)
}

/// write the STA conf, keeping the current file as `.bak` (it
/// stays until the new conf is confirmed — [`confirm_sta`] on a
/// successful join, [`restore_sta`] on a failed one — and it
/// STAYS when a sync fails: the caller treats that as a failed
/// write and the boot mend can still find it)
pub fn write_sta(sta: &Path, bak: &Path, text: &str) -> Result<(), String> {
    write_sta_with(sta, bak, text, &sync_dir)
}

fn write_sta_with(
    sta: &Path,
    bak: &Path,
    text: &str,
    dir_sync: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    if sta.is_file() {
        copy_atomic_with(sta, bak, dir_sync)
            .map_err(|e| format!("cannot back up the wifi conf: {e}"))?;
    } else {
        // a join from a conf-less boot: the failure path has nothing
        // to restore to — make that explicit
        let _ = std::fs::remove_file(bak);
    }
    write_atomic_with(sta, text, dir_sync)
}

/// the new conf worked — its predecessor can go
pub fn confirm_sta(bak: &Path) {
    let _ = std::fs::remove_file(bak);
}

/// keep a copy of a conf that brought the player online (atomic,
/// like every write here); unchanged copies are left alone so a
/// steady boot does not rewrite the FAT every time
pub fn remember_good(sta: &Path, good: &Path) -> Result<(), String> {
    let current = std::fs::read(sta).map_err(|e| format!("read {}: {e}", sta.display()))?;
    if std::fs::read(good).is_ok_and(|kept| kept == current) {
        return Ok(());
    }
    copy_atomic(sta, good)
}

/// put the last good conf back in place (the hotspot failed and
/// there is nothing else to fall back to); false when none exists
pub fn restore_good(sta: &Path, good: &Path) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(good) else {
        return Ok(false);
    };
    if !usable_sta(&text) {
        return Ok(false);
    }
    write_atomic(sta, &text)?;
    Ok(true)
}

/// undo the last [`write_sta`]: `.bak` back in place (never an
/// in-place copy), or the file gone when there was no previous
/// conf. A failed sync keeps the `.bak` — the next attempt (or
/// the boot mend) still has it
pub fn restore_sta(sta: &Path, bak: &Path) -> Result<(), String> {
    restore_sta_with(sta, bak, &sync_dir)
}

fn restore_sta_with(
    sta: &Path,
    bak: &Path,
    dir_sync: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    if bak.is_file() {
        copy_atomic_with(bak, sta, dir_sync)
            .map_err(|e| format!("cannot restore the wifi conf: {e}"))?;
        let _ = std::fs::remove_file(bak);
        Ok(())
    } else {
        match std::fs::remove_file(sta) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove the wifi conf: {e}")),
        }
    }
}

/// the boot-time crash mend: a vfat rename is not atomic, a power
/// cut between the `.bak` write and the conf rename can leave the
/// conf missing or empty while its predecessor sits in `.bak` —
/// restore it. A conf that exists and is non-empty wins
pub fn recover_sta(sta: &Path, bak: &Path) {
    let conf_lost = std::fs::read_to_string(sta)
        .map(|text| text.trim().is_empty())
        .unwrap_or(true);
    if conf_lost && bak.is_file() {
        tracing::warn!("wifi: {} missing — restoring the backup", sta.display());
        if let Err(e) = copy_atomic(bak, sta) {
            tracing::warn!("wifi: restore: {e}");
        } else {
            let _ = std::fs::remove_file(bak);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_no_tmp_left(dir: &Path) {
        let left: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "tmp left behind: {left:?}");
    }

    fn paths(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        (
            dir.join("wpa_supplicant.conf"),
            dir.join("wpa_supplicant.conf.bak"),
        )
    }

    #[test]
    fn the_sta_conf_is_golden() {
        // the psk is the IEEE vector for ThisIsAPassword/ThisIsASSID
        let conf = sta_conf(
            "ThisIsASSID",
            Some("ThisIsAPassword"),
            "/run/wpa_supplicant",
            "",
        )
        .unwrap();
        assert_eq!(
            conf,
            "# written by mestro — the hotspot setup\n\
             ctrl_interface=/run/wpa_supplicant\n\
             update_config=1\n\
             \nnetwork={\n\
             \tssid=\"ThisIsASSID\"\n\
             \tpsk=0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af\n\
             \tscan_ssid=1\n\
             }\n"
        );
        // open networks take key_mgmt=NONE
        let conf = sta_conf("Coffee", None, "/run/wpa_supplicant", "").unwrap();
        assert!(conf.contains("key_mgmt=NONE"));
        assert!(!conf.contains("psk="));
    }

    #[test]
    fn the_ap_conf_is_golden() {
        assert_eq!(
            ap_conf("mestro-3456", "/run/wpa_supplicant", ""),
            "# mestro setup hotspot — ephemeral, regenerated on every boot\n\
             ctrl_interface=/run/wpa_supplicant\n\
             update_config=1\n\
             \nnetwork={\n\
             \tmode=2\n\
             \tfrequency=2437\n\
             \tkey_mgmt=NONE\n\
             \tssid=\"mestro-3456\"\n\
             }\n"
        );
    }

    #[test]
    fn ssids_quote_escape_and_hex() {
        assert!(sta_conf("G\u{e4}stehaus", None, "/run/wpa_supplicant", "")
            .unwrap()
            .contains("ssid=47c3a473746568617573"));
        let conf = sta_conf("she said \"hi\"", None, "/run/wpa_supplicant", "").unwrap();
        assert!(conf.contains("ssid=\"she said \\\"hi\\\"\""), "{conf}");
        let conf = sta_conf("back\\slash", None, "/run/wpa_supplicant", "").unwrap();
        assert!(conf.contains("ssid=\"back\\\\slash\""), "{conf}");
    }

    #[test]
    fn bad_inputs_are_refused() {
        assert!(sta_conf("", None, "/run/wpa_supplicant", "").is_err());
        assert!(sta_conf(&"x".repeat(33), None, "/run/wpa_supplicant", "").is_err());
        assert!(sta_conf("Home", Some("short"), "/run/wpa_supplicant", "").is_err());
    }

    #[test]
    fn template_and_missing_confs_are_not_usable() {
        let template = std::fs::read_to_string("/etc/mestro-defaults/wpa_supplicant.conf")
            .unwrap_or_else(|_| {
                "network={\n\tssid=\"YourNetwork\"\n\tpsk=\"YourPassword\"\n}\n".to_string()
            });
        assert!(!usable_sta(&template));
        assert!(!usable_sta(""));
        let conf = sta_conf("Home", Some("password12"), "/run/wpa_supplicant", "").unwrap();
        assert!(usable_sta(&conf));
        // an open network is usable too
        let conf = sta_conf("Coffee", None, "/run/wpa_supplicant", "").unwrap();
        assert!(usable_sta(&conf));
    }

    #[test]
    fn the_ctrl_dir_is_configurable() {
        let conf = sta_conf("Home", Some("password12"), "/var/run/custom", "").unwrap();
        assert!(conf.contains("ctrl_interface=/var/run/custom"), "{conf}");
        assert!(ap_conf("mestro-0000", "/var/run/custom", "")
            .contains("ctrl_interface=/var/run/custom"));
    }

    #[test]
    fn a_configured_country_lands_in_every_conf() {
        // the regdom rides in the header — STA and AP conf alike
        let conf = sta_conf("Home", Some("password12"), "/run/wpa_supplicant", "DE").unwrap();
        assert!(conf.contains("country=DE\n"), "{conf}");
        assert!(ap_conf("mestro-3456", "/run/wpa_supplicant", "DE").contains("country=DE\n"));
        // an empty country adds no line — the kernel's regdom stays
        assert!(
            !sta_conf("Home", Some("password12"), "/run/wpa_supplicant", "")
                .unwrap()
                .contains("country=")
        );
        assert!(!ap_conf("mestro-3456", "/run/wpa_supplicant", "").contains("country="));
    }

    #[test]
    fn sta_writes_keep_a_bak_and_restore_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let (sta, bak) = paths(dir.path());
        let first = sta_conf("Home", Some("password12"), "/run/wpa_supplicant", "").unwrap();
        write_sta(&sta, &bak, &first).unwrap();
        assert!(!bak.exists(), "the first write has no predecessor");
        assert_eq!(std::fs::read_to_string(&sta).unwrap(), first);
        assert_no_tmp_left(dir.path());

        // the second write backs the first up — atomically, and the
        // bak STAYS until the new conf proved itself
        let second = sta_conf("Other", Some("password12"), "/run/wpa_supplicant", "").unwrap();
        write_sta(&sta, &bak, &second).unwrap();
        assert_eq!(std::fs::read_to_string(&sta).unwrap(), second);
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), first);
        assert_no_tmp_left(dir.path());

        // a confirmed join drops the predecessor
        confirm_sta(&bak);
        assert!(!bak.exists());

        // …a failed one restores it and drops the bak again
        let third = sta_conf("Third", Some("password12"), "/run/wpa_supplicant", "").unwrap();
        write_sta(&sta, &bak, &third).unwrap();
        restore_sta(&sta, &bak).unwrap();
        assert_eq!(std::fs::read_to_string(&sta).unwrap(), second);
        assert!(!bak.exists());
    }

    #[test]
    fn restore_without_a_bak_removes_the_conf() {
        let dir = tempfile::tempdir().unwrap();
        let (sta, bak) = paths(dir.path());
        std::fs::write(&sta, "written by a failed join").unwrap();
        restore_sta(&sta, &bak).unwrap();
        assert!(!sta.exists());
        // and is idempotent
        restore_sta(&sta, &bak).unwrap();
    }

    #[test]
    fn the_boot_mend_restores_a_conf_lost_to_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let (sta, bak) = paths(dir.path());
        let first = sta_conf("Home", Some("password12"), "/run/wpa_supplicant", "").unwrap();
        write_sta(&sta, &bak, &first).unwrap();
        let second = sta_conf("Other", Some("password12"), "/run/wpa_supplicant", "").unwrap();
        write_sta(&sta, &bak, &second).unwrap();

        // crash window A: the conf never made it — the bak mends it
        std::fs::remove_file(&sta).unwrap();
        recover_sta(&sta, &bak);
        assert_eq!(std::fs::read_to_string(&sta).unwrap(), first);
        assert!(!bak.exists());

        // crash window B: the conf is a truncated empty shell — the
        // bak mends it too
        write_sta(&sta, &bak, &second).unwrap();
        std::fs::write(&sta, "").unwrap();
        recover_sta(&sta, &bak);
        assert_eq!(std::fs::read_to_string(&sta).unwrap(), first);

        // an intact conf is never touched
        write_sta(&sta, &bak, &second).unwrap();
        recover_sta(&sta, &bak);
        assert_eq!(std::fs::read_to_string(&sta).unwrap(), second);
        assert!(bak.exists(), "the bak of a working conf stays");
        // and a missing bak is not an error
        std::fs::remove_file(&bak).unwrap();
        std::fs::remove_file(&sta).unwrap();
        recover_sta(&sta, &bak);
    }

    /// a failing directory sync (EIO — the rename may never have
    /// reached the disk); EINVAL/ENOTSUP stand in for vfat
    fn io_error(code: i32) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(code))
    }

    #[test]
    fn a_real_sync_failure_fails_the_write_vfat_refusals_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let (sta, bak) = paths(dir.path());
        let first = sta_conf("Home", Some("password12"), "/run/wpa_supplicant", "").unwrap();
        write_sta(&sta, &bak, &first).unwrap();
        let second = sta_conf("Other", Some("password12"), "/run/wpa_supplicant", "").unwrap();

        // EIO is a real failure — and the `.bak` of the previous
        // conf SURVIVES it (the boot mend can still find it)
        let error = write_sta_with(&sta, &bak, &second, &|_| io_error(libc::EIO)).unwrap_err();
        assert!(error.contains("cannot sync"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            first,
            "the bak stays"
        );
        // EINVAL/ENOTSUP are vfat's "no directory sync": tolerated
        for tolerated in [libc::EINVAL, libc::ENOTSUP] {
            write_sta_with(&sta, &bak, &second, &|_| io_error(tolerated))
                .unwrap_or_else(|e| panic!("vfat refusal must not fail: {e}"));
        }
        // a failing restore keeps the bak as well — the next
        // attempt (or the boot mend) still has it
        std::fs::write(&bak, "previous").unwrap();
        let error = restore_sta_with(&sta, &bak, &|_| io_error(libc::EIO)).unwrap_err();
        assert!(error.contains("cannot sync"), "{error}");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "previous");
    }
}
