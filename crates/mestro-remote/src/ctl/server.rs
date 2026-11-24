//! the ctl socket listener: one thread for the life of the
//! process, one line per connection. Validation happens here
//! (`mestro_ipc::ctl::answer_for`), the work happens on the UI
//! thread through the [`UiPort`] — the same crossing the
//! keyboard and the web endpoint use.

use crate::UiPort;
use mestro_ipc::ctl;
use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

/// start the listener; a bind failure logs and leaves the rest
/// of the app running (ctl is a convenience, never load-bearing)
pub fn serve(ui: Arc<dyn UiPort>) {
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
                Ok(stream) => handle(stream, &ui),
                Err(e) => tracing::debug!("ipc socket: accept: {e}"),
            }
        }
    });
}

fn handle(stream: std::os::unix::net::UnixStream, ui: &Arc<dyn UiPort>) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    for line in std::io::BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let answer = match ctl::answer_for(&line) {
            // validated: the UI thread runs parse + dispatch
            Ok(ok) => {
                ui.control(line.trim().to_string());
                ok.to_string()
            }
            Err(e) => e,
        };
        if writeln!(out, "{answer}").is_err() {
            return;
        }
    }
}
