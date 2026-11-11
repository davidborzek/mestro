//! the ctl socket listener: one thread for the life of the
//! process, one line per connection. Validation happens here
//! (`proto::answer_for`), the work happens on the UI thread
//! through the `control-command` callback — the same crossing
//! the keyboard and the web endpoint use.

use super::proto;
use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;

/// start the listener; a bind failure logs and leaves the rest
/// of the app running (ctl is a convenience, never load-bearing)
pub fn serve(ui_weak: slint::Weak<crate::Mestro>) {
    let path = super::socket_path();
    // a stale socket from a killed run must not block startup
    let _ = std::fs::remove_file(&path);
    let listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("ipc socket: bind {}: {e}", path.display());
            return;
        }
    };
    // the state dir is not guaranteed private — the socket is
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle(stream, &ui_weak),
                Err(e) => tracing::debug!("ipc socket: accept: {e}"),
            }
        }
    });
}

fn handle(stream: std::os::unix::net::UnixStream, ui_weak: &slint::Weak<crate::Mestro>) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    for line in std::io::BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let answer = match proto::answer_for(&line) {
            // validated: the UI thread runs parse + dispatch
            Ok(ok) => {
                let line = line.trim().to_string();
                let ui_weak = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.invoke_control_command(line.as_str().into());
                    }
                });
                ok.to_string()
            }
            Err(e) => e,
        };
        if writeln!(out, "{answer}").is_err() {
            return;
        }
    }
}
