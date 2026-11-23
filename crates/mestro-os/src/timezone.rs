//! The timezone picker (feature `timezone`): zone validation against
//! the device's zoneinfo, the zone list, and applying a zone by
//! writing `TZ` into mestro's own env file (written whole — the init
//! script sources it before it starts mestro, and the user's
//! `/data/mestro.env` after it). Applying needs a restart of the
//! player — a live `tzset` would need `unsafe` and chrono's cached
//! local offset would still lag — so the caller restarts through
//! `restart.rs` when [`Applied::restart`] says so.

#![deny(clippy::indexing_slicing)]

use crate::paths::setting;
use std::path::Path;

/// the env file mestro owns and writes whole (`TZ=`, from the timezone
/// setting); the init script sources it before it starts mestro
pub const ENV_FILE: &str = setting(
    option_env!("MESTRO_ENV_FILE"),
    "/data/state/mestro/mestro.env",
);

/// the zoneinfo database a chosen zone must exist in
pub const ZONEINFO_DIR: &str = setting(option_env!("MESTRO_ZONEINFO_DIR"), "/usr/share/zoneinfo");

/// the system zone link, read to learn the running zone when `TZ` is
/// not set
pub const LOCALTIME: &str = setting(option_env!("MESTRO_LOCALTIME"), "/etc/localtime");

/// longest zone name accepted (the longest real one is 32)
const MAX_ZONE_LEN: usize = 64;
/// directory levels below the zoneinfo root
/// (`America/Argentina/Buenos_Aires` is 3)
const MAX_DEPTH: usize = 3;
/// what the env file starts with
const HEADER: &str =
    "# written by mestro from its timezone setting — do not edit\n# (yours: /data/mestro.env)\n";
/// entries of the zoneinfo root that are not zones
const NOT_ZONES: &[&str] = &["posix", "right", "posixrules", "localtime", "Factory"];

/// a zone name's shape: path segments of `[A-Za-z0-9_+-]`, no empty,
/// dot or `..` segments, no leading slash — rejects traversal before
/// the filesystem is touched
fn is_well_formed(zone: &str) -> bool {
    !zone.is_empty()
        && zone.len() <= MAX_ZONE_LEN
        && zone.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'))
        })
}

/// does the file start with the TZif magic
fn is_tzif(path: &Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && &magic == b"TZif"
}

/// the zone must exist as a TZif file under `zoneinfo_dir`: shape
/// first, then the resolved path must stay inside the directory
/// (a symlink out of it is no zone)
pub fn validate_zone(zoneinfo_dir: &Path, zone: &str) -> Result<(), String> {
    if !is_well_formed(zone) {
        return Err(format!("{zone:?} is not a timezone name"));
    }
    let root = std::fs::canonicalize(zoneinfo_dir)
        .map_err(|e| format!("no zoneinfo at {}: {e}", zoneinfo_dir.display()))?;
    let resolved =
        std::fs::canonicalize(root.join(zone)).map_err(|_| format!("unknown timezone {zone}"))?;
    if !resolved.starts_with(&root) || !is_tzif(&resolved) {
        return Err(format!("unknown timezone {zone}"));
    }
    Ok(())
}

/// every zone under `zoneinfo_dir`, sorted: TZif files only, the
/// `posix/` and `right/` mirrors and the non-zone entries skipped
pub fn list_zones(zoneinfo_dir: &Path) -> Vec<String> {
    let mut zones = Vec::new();
    collect(zoneinfo_dir, "", 0, &mut zones);
    zones.sort();
    zones
}

fn collect(dir: &Path, prefix: &str, depth: usize, zones: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if (depth == 0 && NOT_ZONES.contains(&name.as_str())) || !is_well_formed(&name) {
            continue;
        }
        let zone = format!("{prefix}{name}");
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect(&path, &format!("{zone}/"), depth + 1, zones);
            }
        } else if is_tzif(&path) {
            zones.push(zone);
        }
    }
}

/// the zone this process runs in: `TZ` when set (a leading `:` is
/// the POSIX "file name" marker), else the zone `localtime` links
/// to. None when neither names one
pub fn process_zone(tz_env: Option<&str>, localtime: &Path) -> Option<String> {
    if let Some(tz) = tz_env.map(|tz| tz.trim().trim_start_matches(':')) {
        if !tz.is_empty() {
            return Some(tz.to_string());
        }
    }
    let target = std::fs::read_link(localtime).ok()?;
    let target = target.to_string_lossy();
    let (_, zone) = target.rsplit_once("/zoneinfo/")?;
    is_well_formed(zone).then(|| zone.to_string())
}

/// who asks for the zone
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// the web app's browser zone, sent on connect: taken only while
    /// nobody ever chose a zone
    Browser,
    /// the user picked it
    User,
}

/// what applying a zone did
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub zone: String,
    /// the zone was taken: `TZ` written, the setting is to be
    /// recorded. False = a browser zone that was ignored because a
    /// zone was chosen before
    pub taken: bool,
    /// the running process is in another zone — it must restart to
    /// follow
    pub restart: bool,
}

/// apply `zone`: a browser zone is ignored once a zone was chosen
/// (`chosen` = the setting is not empty) — a phone in another
/// timezone must never flip the player later. Otherwise the zone is
/// validated and `TZ` written into mestro's env file (whole, only when
/// it changes); nothing is written
/// for a rejected zone. The caller records the setting when `taken`
/// and restarts when `restart`
pub fn apply(
    zoneinfo_dir: &Path,
    env_file: &Path,
    chosen: &str,
    process_zone: Option<&str>,
    source: Source,
    zone: &str,
) -> Result<Applied, String> {
    if source == Source::Browser && !chosen.is_empty() {
        return Ok(Applied {
            zone: zone.to_string(),
            taken: false,
            restart: false,
        });
    }
    validate_zone(zoneinfo_dir, zone)?;
    // a validated zone is made of `[A-Za-z0-9_+-/]`: a plain shell word
    let text = format!("{HEADER}TZ={zone}\n");
    if std::fs::read_to_string(env_file).ok().as_deref() != Some(&text) {
        crate::atomic_file::write_atomic(env_file, &text)?;
    }
    Ok(Applied {
        zone: zone.to_string(),
        taken: true,
        restart: process_zone != Some(zone),
    })
}

/// the zone the next restart must run in: recorded by the timezone
/// op on the UI thread, handed to the exec by `restart.rs` — the
/// environment of this process is never touched
static PENDING_ZONE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// ask the next restart to run in `zone`
pub fn set_pending_zone(zone: &str) {
    *crate::locks::lock(&PENDING_ZONE) = Some(zone.to_string());
}

/// the zone the restart in progress must carry (once)
pub fn take_pending_zone() -> Option<String> {
    crate::locks::lock(&PENDING_ZONE).take()
}
/// the capability: a zoneinfo database to validate against
pub fn available(zoneinfo_dir: &Path) -> bool {
    zoneinfo_dir.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timezone_settings_follow_their_build_variables() {
        assert_eq!(
            ENV_FILE,
            option_env!("MESTRO_ENV_FILE").unwrap_or("/data/state/mestro/mestro.env")
        );
        assert_eq!(
            ZONEINFO_DIR,
            option_env!("MESTRO_ZONEINFO_DIR").unwrap_or("/usr/share/zoneinfo")
        );
        assert_eq!(
            LOCALTIME,
            option_env!("MESTRO_LOCALTIME").unwrap_or("/etc/localtime")
        );
    }

    #[test]
    fn build_rs_reruns_on_every_variable_read_here() {
        let build_rs = include_str!("../build.rs");
        let source = include_str!("timezone.rs");
        let vars: Vec<&str> = source
            .split("option_env!(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|var| var.starts_with("MESTRO_"))
            .collect();
        assert!(!vars.is_empty());
        for var in vars {
            assert!(
                build_rs.contains(&format!("\"{var}\"")),
                "build.rs must emit rerun-if-env-changed for {var}"
            );
        }
    }

    /// a fake zoneinfo: real zones are TZif files (the magic is all
    /// the code looks at), plus the entries a real tree also holds
    fn zoneinfo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for zone in [
            "Europe/Berlin",
            "Europe/London",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+5",
            "UTC",
            "posix/Europe/Berlin",
            "right/Europe/Berlin",
            "posixrules",
        ] {
            let path = dir.path().join(zone);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"TZif2\0\0\0").unwrap();
        }
        std::fs::write(dir.path().join("zone.tab"), "# not a zone\n").unwrap();
        std::fs::write(dir.path().join("tzdata.zi"), "# not a zone\n").unwrap();
        std::fs::write(dir.path().join("Europe/notes"), "plain text").unwrap();
        dir
    }

    #[test]
    fn existing_zones_validate() {
        let dir = zoneinfo();
        for zone in [
            "Europe/Berlin",
            "UTC",
            "Etc/GMT+5",
            "America/Argentina/Buenos_Aires",
        ] {
            assert_eq!(validate_zone(dir.path(), zone), Ok(()), "{zone}");
        }
    }

    #[test]
    fn path_traversal_and_odd_names_are_rejected() {
        let dir = zoneinfo();
        // a secret outside the database that a traversal would reach
        let outside = dir.path().parent().unwrap().join("tz-secret");
        std::fs::write(&outside, b"TZif-but-outside").unwrap();
        for zone in [
            "",
            "../tz-secret",
            "Europe/../../tz-secret",
            "Europe/../Europe/Berlin",
            "/etc/passwd",
            "/Europe/Berlin",
            "Europe//Berlin",
            "Europe/Berlin/",
            "./UTC",
            ".hidden",
            "Europe\\Berlin",
            "Europe/Berlin\0",
            "Europe/Berlin\nTZ=x",
            "Europe/Berlin ",
            "Eur ope",
            "Europe",
            "Europe/notes",
            "zone.tab",
            "Nowhere/Land",
            &"A".repeat(MAX_ZONE_LEN + 1),
        ] {
            assert!(
                validate_zone(dir.path(), zone).is_err(),
                "{zone:?} must be rejected"
            );
        }
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn a_symlink_out_of_the_database_is_no_zone() {
        let dir = zoneinfo();
        let outside = dir.path().parent().unwrap().join("tz-linked");
        std::fs::write(&outside, b"TZif2").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("Escape")).unwrap();
        assert!(validate_zone(dir.path(), "Escape").is_err());
        // a symlink inside the database is fine (zoneinfo aliases)
        std::os::unix::fs::symlink(dir.path().join("Europe/Berlin"), dir.path().join("Alias"))
            .unwrap();
        assert_eq!(validate_zone(dir.path(), "Alias"), Ok(()));
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn the_pending_zone_is_handed_over_once() {
        assert_eq!(take_pending_zone(), None);
        set_pending_zone("Europe/London");
        set_pending_zone("Asia/Tokyo");
        assert_eq!(take_pending_zone().as_deref(), Some("Asia/Tokyo"));
        assert_eq!(take_pending_zone(), None);
    }

    #[test]
    fn a_missing_database_rejects_everything() {
        assert!(validate_zone(Path::new("/nonexistent/zoneinfo"), "UTC").is_err());
    }

    #[test]
    fn the_zone_list_holds_tzif_files_only() {
        let dir = zoneinfo();
        assert_eq!(
            list_zones(dir.path()),
            [
                "America/Argentina/Buenos_Aires",
                "Etc/GMT+5",
                "Europe/Berlin",
                "Europe/London",
                "UTC",
            ],
            "no posix/right mirrors, posixrules, tables or text files"
        );
        assert!(list_zones(Path::new("/nonexistent")).is_empty());
    }

    #[test]
    fn the_process_zone_is_tz_else_the_localtime_link() {
        let dir = zoneinfo();
        let link = dir.path().join("localtime");
        std::os::unix::fs::symlink("/usr/share/zoneinfo/Europe/Berlin", &link).unwrap();
        assert_eq!(process_zone(None, &link).as_deref(), Some("Europe/Berlin"));
        assert_eq!(
            process_zone(Some(""), &link).as_deref(),
            Some("Europe/Berlin")
        );
        assert_eq!(
            process_zone(Some("Europe/London"), &link).as_deref(),
            Some("Europe/London")
        );
        assert_eq!(
            process_zone(Some(":Europe/London"), &link).as_deref(),
            Some("Europe/London")
        );
        assert_eq!(process_zone(None, &dir.path().join("missing")), None);
        // a link that is not into a zoneinfo tree names no zone
        let odd = dir.path().join("odd");
        std::os::unix::fs::symlink("/etc/hostname", &odd).unwrap();
        assert_eq!(process_zone(None, &odd), None);
    }

    fn env_text(env: &Path) -> String {
        std::fs::read_to_string(env).unwrap()
    }

    #[test]
    fn the_user_choice_writes_tz_as_the_whole_file_and_asks_for_a_restart() {
        let dir = zoneinfo();
        let env = dir.path().join("state/mestro/mestro.env");
        let applied = apply(
            dir.path(),
            &env,
            "",
            Some("Europe/Berlin"),
            Source::User,
            "Europe/London",
        )
        .unwrap();
        assert_eq!(
            applied,
            Applied {
                zone: "Europe/London".into(),
                taken: true,
                restart: true
            }
        );
        assert_eq!(env_text(&env), format!("{HEADER}TZ=Europe/London\n"));
        // a second choice replaces the file whole, whatever it held
        std::fs::write(&env, "TZ=Europe/London\nRUST_LOG=trace\n").unwrap();
        apply(
            dir.path(),
            &env,
            "Europe/London",
            Some("Europe/London"),
            Source::User,
            "UTC",
        )
        .unwrap();
        assert_eq!(env_text(&env), format!("{HEADER}TZ=UTC\n"));
    }

    #[test]
    fn the_shell_reads_the_zone_back_from_the_file() {
        let dir = zoneinfo();
        let env = dir.path().join("mestro.env");
        for zone in [
            "Europe/Berlin",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+5",
        ] {
            apply(dir.path(), &env, "", None, Source::User, zone).unwrap();
            let out = std::process::Command::new("sh")
                .args(["-c", ". \"$0\"; printf %s \"$TZ\""])
                .arg(&env)
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), zone);
        }
    }

    #[test]
    fn the_same_zone_again_does_not_rewrite_the_file() {
        let dir = zoneinfo();
        let env = dir.path().join("mestro.env");
        apply(dir.path(), &env, "", None, Source::User, "UTC").unwrap();
        let before = std::fs::metadata(&env).unwrap().modified().unwrap();
        apply(dir.path(), &env, "UTC", None, Source::User, "UTC").unwrap();
        assert_eq!(std::fs::metadata(&env).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn the_zone_the_process_already_runs_in_needs_no_restart() {
        let dir = zoneinfo();
        let env = dir.path().join("mestro.env");
        let applied = apply(
            dir.path(),
            &env,
            "",
            Some("Europe/Berlin"),
            Source::User,
            "Europe/Berlin",
        )
        .unwrap();
        assert!(applied.taken && !applied.restart);
        // an unknown process zone restarts
        let applied = apply(dir.path(), &env, "", None, Source::User, "Europe/Berlin").unwrap();
        assert!(applied.restart);
    }

    #[test]
    fn a_browser_zone_is_adopted_once_and_never_over_a_choice() {
        let dir = zoneinfo();
        let env = dir.path().join("mestro.env");
        // never chosen: taken
        let first = apply(
            dir.path(),
            &env,
            "",
            Some("Europe/Berlin"),
            Source::Browser,
            "Europe/London",
        )
        .unwrap();
        assert!(first.taken && first.restart);
        assert_eq!(env_text(&env), format!("{HEADER}TZ=Europe/London\n"));
        // chosen (by the user or by that first adoption): a traveller's
        // phone in another zone changes nothing, and writes nothing
        let before = env_text(&env);
        let second = apply(
            dir.path(),
            &env,
            "Europe/London",
            Some("Europe/London"),
            Source::Browser,
            "Asia/Tokyo",
        )
        .unwrap();
        assert!(
            !second.taken && !second.restart,
            "a zone the device does not even know is ignored quietly"
        );
        let third = apply(
            dir.path(),
            &env,
            "Europe/London",
            Some("Europe/London"),
            Source::Browser,
            "UTC",
        )
        .unwrap();
        assert!(!third.taken && !third.restart);
        assert_eq!(env_text(&env), before);
    }

    #[test]
    fn a_rejected_zone_writes_nothing() {
        let dir = zoneinfo();
        let env = dir.path().join("mestro.env");
        std::fs::write(&env, "RUST_LOG=debug\n").unwrap();
        for source in [Source::User, Source::Browser] {
            for zone in [
                "../../etc/passwd",
                "Nowhere/Land",
                "",
                "Europe/Berlin\nEXTRA=1",
            ] {
                assert!(
                    apply(dir.path(), &env, "", None, source, zone).is_err(),
                    "{zone:?}"
                );
            }
        }
        assert_eq!(std::fs::read_to_string(&env).unwrap(), "RUST_LOG=debug\n");
    }

    #[test]
    fn the_capability_needs_a_zoneinfo_database() {
        let dir = zoneinfo();
        assert!(available(dir.path()));
        assert!(!available(&dir.path().join("missing")));
    }
}
