//! UI-thread control dispatch. Settings writes use their own op.

use super::Dispatch;
use crate::remote::control::Command;
use crate::remote::web::Response;

/// one ctl round on the UI thread
pub(crate) fn handle(id: u64, cmd: &str, dispatch: &Dispatch) -> Response {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Response::err(id, "empty command");
    }
    match Command::parse(cmd) {
        Some(command) if command.is_settings() => Response::err(id, "not a control command"),
        Some(command) => {
            dispatch(&command);
            Response::ok(id, serde_json::json!({ "cmd": cmd }))
        }
        None => Response::err(id, "unknown command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn controls_dispatch_once_and_preserve_id() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let dispatch: Dispatch = Rc::new(move |cmd| sink.borrow_mut().push(cmd.clone()));
        for cmd in [
            "play-pause",
            "home",
            "now-playing",
            "queue",
            "vu",
            "standby-on",
            "standby-off",
            "standby-toggle",
        ] {
            let response = handle(7, cmd, &dispatch);
            assert!(response.ok);
            assert_eq!(response.id, 7);
            assert_eq!(seen.borrow_mut().pop(), Command::parse(cmd));
            assert!(seen.borrow().is_empty());
        }
    }

    #[test]
    fn rejected_commands_never_dispatch() {
        let dispatch: Dispatch = Rc::new(|_| panic!("unexpected dispatch"));
        for cmd in [
            "",
            "unknown",
            "setting ui.rail glass",
            "reset-setting ui.rail",
            "reset-all-settings",
        ] {
            let response = handle(9, cmd, &dispatch);
            assert!(!response.ok, "{cmd}");
            assert_eq!(response.id, 9);
        }
    }
}
