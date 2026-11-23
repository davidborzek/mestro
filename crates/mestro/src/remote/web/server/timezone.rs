//! The timezone ops (paired only; they ride the UI thread because a
//! chosen zone is a device setting and applying it restarts the
//! player). The decision logic is a pure function so the auth-free
//! rules — adopt once, never over a choice, validate first — are
//! tested without a UI: [`run`] says what to answer, which zone to
//! record and whether to restart; `settings::handle_ui` carries it
//! out.

use crate::remote::web::{Response, TimezoneOp};

/// what an op decided
#[derive(Debug)]
pub(super) struct Outcome {
    pub response: Response,
    /// the zone to record as `system.timezone` (`None` = leave it)
    pub record: Option<String>,
    /// the player must restart to run in the recorded zone
    pub restart: bool,
}

impl Outcome {
    fn reply(response: Response) -> Self {
        Self {
            response,
            record: None,
            restart: false,
        }
    }
}

/// the capability hello reports: built in and a zoneinfo database
pub(super) fn capable() -> bool {
    #[cfg(feature = "timezone")]
    {
        crate::system::timezone::available(std::path::Path::new(crate::system::paths::ZONEINFO_DIR))
    }
    #[cfg(not(feature = "timezone"))]
    {
        false
    }
}

/// the files one timezone op works on
#[cfg(feature = "timezone")]
struct Paths<'a> {
    zoneinfo_dir: &'a std::path::Path,
    env_file: &'a std::path::Path,
    localtime: &'a std::path::Path,
}

/// `chosen` is the `system.timezone` setting, `tz_env` the process's
/// `TZ`
#[cfg(feature = "timezone")]
pub(super) fn run(id: u64, action: TimezoneOp, chosen: &str, tz_env: Option<&str>) -> Outcome {
    use crate::system::paths;
    use std::path::Path;
    run_in(
        &Paths {
            zoneinfo_dir: Path::new(paths::ZONEINFO_DIR),
            env_file: Path::new(paths::ENV_FILE),
            localtime: Path::new(paths::LOCALTIME),
        },
        id,
        action,
        chosen,
        tz_env,
    )
}

#[cfg(feature = "timezone")]
fn run_in(
    paths: &Paths,
    id: u64,
    action: TimezoneOp,
    chosen: &str,
    tz_env: Option<&str>,
) -> Outcome {
    use crate::system::timezone as tz;
    use serde_json::json;
    if !tz::available(paths.zoneinfo_dir) {
        return Outcome::reply(Response::err(id, "timezone not available"));
    }
    let process_zone = tz::process_zone(tz_env, paths.localtime);
    let (source, zone) = match action {
        TimezoneOp::Zones => {
            return Outcome::reply(Response::ok(
                id,
                json!({
                    "zones": tz::list_zones(paths.zoneinfo_dir),
                    "chosen": chosen,
                    "current": process_zone,
                }),
            ));
        }
        TimezoneOp::Set { zone } => (tz::Source::User, zone),
        TimezoneOp::Adopt { zone } => (tz::Source::Browser, zone),
    };
    match tz::apply(
        paths.zoneinfo_dir,
        paths.env_file,
        chosen,
        process_zone.as_deref(),
        source,
        &zone,
    ) {
        Ok(applied) => Outcome {
            response: Response::ok(
                id,
                json!({
                    "zone": applied.zone,
                    "taken": applied.taken,
                    "restarting": applied.restart,
                }),
            ),
            record: applied.taken.then(|| applied.zone.clone()),
            restart: applied.restart,
        },
        Err(e) => Outcome::reply(Response::err(id, e)),
    }
}

#[cfg(not(feature = "timezone"))]
pub(super) fn run(id: u64, _action: TimezoneOp, _chosen: &str, _tz_env: Option<&str>) -> Outcome {
    Outcome::reply(Response::err(id, "timezone not available"))
}

#[cfg(test)]
#[cfg(feature = "timezone")]
mod tests {
    use super::*;
    use serde_json::json;

    /// the files of a fake device
    struct Device {
        zoneinfo_dir: std::path::PathBuf,
        env_file: std::path::PathBuf,
        localtime: std::path::PathBuf,
    }

    fn run(
        id: u64,
        action: TimezoneOp,
        device: &Device,
        chosen: &str,
        tz_env: Option<&str>,
    ) -> Outcome {
        run_in(
            &Paths {
                zoneinfo_dir: &device.zoneinfo_dir,
                env_file: &device.env_file,
                localtime: &device.localtime,
            },
            id,
            action,
            chosen,
            tz_env,
        )
    }

    /// a device: zoneinfo with two zones, a localtime link to Berlin,
    /// an env file with another key in it
    fn device() -> (tempfile::TempDir, Device) {
        let dir = tempfile::tempdir().unwrap();
        for zone in ["Europe/Berlin", "Europe/London", "Asia/Tokyo"] {
            let path = dir.path().join("zoneinfo").join(zone);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"TZif2").unwrap();
        }
        let berlin = dir.path().join("zoneinfo/Europe/Berlin");
        std::os::unix::fs::symlink(&berlin, dir.path().join("localtime")).unwrap();
        std::fs::write(dir.path().join("mestro.env"), "RUST_LOG=debug\n").unwrap();
        let system = Device {
            env_file: dir.path().join("mestro.env"),
            zoneinfo_dir: dir.path().join("zoneinfo"),
            localtime: dir.path().join("localtime"),
        };
        (dir, system)
    }

    fn set(zone: &str) -> TimezoneOp {
        TimezoneOp::Set { zone: zone.into() }
    }
    fn adopt(zone: &str) -> TimezoneOp {
        TimezoneOp::Adopt { zone: zone.into() }
    }
    fn env_text(system: &Device) -> String {
        std::fs::read_to_string(&system.env_file).unwrap()
    }

    #[test]
    fn zones_lists_the_database_with_the_choice_and_the_running_zone() {
        let (_dir, system) = device();
        let outcome = run(1, TimezoneOp::Zones, &system, "Europe/London", None);
        let data = outcome.response.data.unwrap();
        assert_eq!(
            data["zones"],
            json!(["Asia/Tokyo", "Europe/Berlin", "Europe/London"])
        );
        assert_eq!(data["chosen"], "Europe/London");
        assert_eq!(data["current"], "Europe/Berlin", "from the localtime link");
        assert!(outcome.record.is_none() && !outcome.restart);
        // TZ in the environment beats the link
        let outcome = run(2, TimezoneOp::Zones, &system, "", Some("Asia/Tokyo"));
        assert_eq!(outcome.response.data.unwrap()["current"], "Asia/Tokyo");
    }

    #[test]
    fn a_choice_is_recorded_written_and_restarts_the_player() {
        let (_dir, system) = device();
        let outcome = run(3, set("Europe/London"), &system, "", None);
        assert!(outcome.response.ok, "{:?}", outcome.response.error);
        assert_eq!(outcome.record.as_deref(), Some("Europe/London"));
        assert!(outcome.restart, "Berlin → London changes the running zone");
        assert_eq!(
            outcome.response.data.unwrap(),
            json!({"zone": "Europe/London", "taken": true, "restarting": true})
        );
        assert!(env_text(&system).ends_with("TZ=Europe/London\n"));
        assert!(
            !env_text(&system).contains("RUST_LOG"),
            "the file is mestro's, whole"
        );
    }

    #[test]
    fn the_first_browser_zone_is_adopted_then_ignored_for_ever() {
        let (_dir, system) = device();
        let first = run(4, adopt("Europe/London"), &system, "", None);
        assert_eq!(first.record.as_deref(), Some("Europe/London"));
        assert!(first.restart);
        let written = env_text(&system);
        // later: a phone in Tokyo (travel) — nothing changes
        let later = run(
            5,
            adopt("Asia/Tokyo"),
            &system,
            "Europe/London",
            Some("Europe/London"),
        );
        assert!(later.response.ok);
        assert_eq!(
            later.response.data.unwrap(),
            json!({"zone": "Asia/Tokyo", "taken": false, "restarting": false})
        );
        assert!(later.record.is_none() && !later.restart);
        assert_eq!(env_text(&system), written, "the file is untouched");
        // an explicit choice still works afterwards
        let explicit = run(
            6,
            set("Asia/Tokyo"),
            &system,
            "Europe/London",
            Some("Europe/London"),
        );
        assert_eq!(explicit.record.as_deref(), Some("Asia/Tokyo"));
    }

    #[test]
    fn invalid_zones_answer_an_error_and_touch_nothing() {
        let (_dir, system) = device();
        for zone in [
            "../../etc/passwd",
            "Nowhere/Land",
            "",
            "Europe/Berlin\nTZ=x",
        ] {
            for action in [set(zone), adopt(zone)] {
                let outcome = run(7, action, &system, "", None);
                assert!(!outcome.response.ok, "{zone:?}");
                assert!(outcome.record.is_none() && !outcome.restart, "{zone:?}");
            }
        }
        assert_eq!(env_text(&system), "RUST_LOG=debug\n");
    }

    #[test]
    fn no_zoneinfo_means_not_available() {
        let (_dir, mut system) = device();
        system.zoneinfo_dir = "/nonexistent/zoneinfo".into();
        let outcome = run(8, TimezoneOp::Zones, &system, "", None);
        assert_eq!(
            outcome.response.error.as_deref(),
            Some("timezone not available")
        );
    }
}
