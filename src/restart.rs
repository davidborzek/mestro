//! Restart the current executable with its original arguments and environment.
//! The UI and pending settings must be shut down/flushed by the caller first.

use std::io;
use std::process::Command;

/// `timezone`: the zone the replacement must run in — handed over
/// through the new process's environment, never by changing this
/// process's (other threads may be reading it)
pub fn restart(timezone: Option<&str>) -> ! {
    let result = start_replacement(timezone);
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            tracing::error!("restart: {error}");
            std::process::exit(1);
        }
    }
}

fn command(timezone: Option<&str>) -> io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command.args(std::env::args_os().skip(1));
    if let Some(zone) = timezone {
        command.env("TZ", zone);
    }
    Ok(command)
}

#[cfg(unix)]
fn start_replacement(timezone: Option<&str>) -> io::Result<()> {
    use std::os::unix::process::CommandExt as _;

    Err(command(timezone)?.exec())
}

#[cfg(not(unix))]
fn start_replacement(timezone: Option<&str>) -> io::Result<()> {
    command(timezone)?.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn tz_override(command: &Command) -> Option<Option<&OsStr>> {
        command
            .get_envs()
            .find(|(key, _)| *key == "TZ")
            .map(|(_, value)| value)
    }

    #[test]
    fn the_replacement_carries_the_new_zone_in_its_environment() {
        let command = command(Some("Europe/London")).unwrap();
        assert_eq!(
            tz_override(&command),
            Some(Some(OsStr::new("Europe/London")))
        );
    }

    #[test]
    fn without_a_zone_the_environment_is_inherited_untouched() {
        let command = command(None).unwrap();
        assert_eq!(tz_override(&command), None);
    }
}
