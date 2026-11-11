//! Restart the current executable with its original arguments and environment.
//! The UI and pending settings must be shut down/flushed by the caller first.

use std::io;
use std::process::Command;

pub fn restart() -> ! {
    let result = start_replacement();
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            tracing::error!("restart: {error}");
            std::process::exit(1);
        }
    }
}

fn command() -> io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command.args(std::env::args_os().skip(1));
    Ok(command)
}

#[cfg(unix)]
fn start_replacement() -> io::Result<()> {
    use std::os::unix::process::CommandExt as _;

    Err(command()?.exec())
}

#[cfg(not(unix))]
fn start_replacement() -> io::Result<()> {
    command()?.spawn()?;
    Ok(())
}
