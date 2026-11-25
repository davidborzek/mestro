//! Running the image's external commands (mount, sync, umount,
//! pidof, ip, date, dmesg) under a deadline: spawn, poll for the
//! exit, kill when the budget is up. Nothing ever waits on a child
//! that ignores the kill — a process stuck in the kernel (a hung
//! sync in D state) cannot be reaped and must not be waited for,
//! or it would hang the caller. The zombie it leaves is inherited
//! and reaped by PID 1 instead.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// how often a bounded command is polled for its exit
const WAIT_POLL: Duration = Duration::from_millis(50);

/// run an external command with a deadline: spawn it, poll for
/// its exit, and kill it when its budget is up
pub fn run_bounded(
    program: impl AsRef<OsStr>,
    args: impl FnOnce(&mut Command) -> &mut Command,
    timeout: Duration,
) -> io::Result<ExitStatus> {
    let mut command = Command::new(program.as_ref());
    let _ = args(command.stdin(Stdio::null()));
    let mut child = command.spawn()?;
    wait_bounded(&mut child, timeout, program.as_ref().to_string_lossy())
}

/// the same deadline, with the child's stdout captured — for the
/// commands whose answer is read (pidof, ip, date, dmesg). The
/// pipe is drained on a reader thread, so an output longer than
/// the pipe cannot wedge the child, and the deadline still holds:
/// when it ends the child is killed and the reader is abandoned
/// (it ends at this process's exit)
pub fn run_bounded_output(
    program: impl AsRef<OsStr>,
    args: impl FnOnce(&mut Command) -> &mut Command,
    timeout: Duration,
) -> io::Result<(ExitStatus, Vec<u8>)> {
    let mut command = Command::new(program.as_ref());
    let _ = args(command.stdin(Stdio::null()));
    command.stdout(Stdio::piped());
    let mut child = command.spawn()?;
    let pipe = child.stdout.take();
    let (sent, received) = mpsc::channel();
    // a reader thread that cannot start is an error, not a panic:
    // kill the child (nobody would drain its pipe) and hand the
    // caller its own failure
    let reader = match std::thread::Builder::new().spawn(move || {
        let mut out = Vec::new();
        if let Some(mut pipe) = pipe {
            let mut chunk = [0u8; 8 * 1024];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => out.extend_from_slice(&chunk[..n]),
                }
            }
        }
        let _ = sent.send(out);
    }) {
        Ok(reader) => reader,
        Err(e) => {
            let _ = child.kill();
            return Err(e);
        }
    };
    let name = program.as_ref().to_string_lossy();
    let status = wait_bounded(&mut child, timeout, name)?;
    // the child is gone, its pipe's write end went with it: EOF
    // closes the reader and the answer is already on the channel
    let _ = reader.join();
    let output = received.recv().unwrap_or_default();
    Ok((status, output))
}

/// poll a spawned child for its exit until `timeout` is up, then
/// kill it (best effort — a kill that does not take is not waited
/// on) and hand the caller its own timeout error
fn wait_bounded(
    child: &mut Child,
    timeout: Duration,
    name: std::borrow::Cow<'_, str>,
) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{name} did not finish in time"),
                ));
            }
            None => std::thread::sleep(WAIT_POLL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fast_command_is_waited_out_and_captured() {
        let (status, out) =
            run_bounded_output("sh", |command| command.args(["-c", "printf mestro"]), PROBE)
                .unwrap();
        assert!(status.success());
        assert_eq!(out, b"mestro");
    }

    #[test]
    fn an_output_longer_than_the_pipe_is_captured_whole() {
        // the pipe itself holds far less: only the reader thread
        // keeps the child writing
        let (status, out) = run_bounded_output(
            "sh",
            |command| command.args(["-c"]).arg("yes abc | head -c 200000"),
            PROBE,
        )
        .unwrap();
        assert!(status.success());
        assert_eq!(out.len(), 200000);
    }

    #[test]
    fn a_command_that_ignores_its_deadline_is_killed() {
        let started = Instant::now();
        let err = run_bounded_output(
            "sh",
            |command| command.args(["-c", "sleep 3600"]),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the bound holds"
        );
    }

    /// a generous budget for the fixture commands of these tests
    const PROBE: Duration = Duration::from_secs(10);
}
