//! UI-thread control dispatch. Settings writes use their own op.

use super::Dispatch;
use crate::remote::control::Command;
use crate::remote::web::Response;

/// one ctl round on the UI thread. `vu_views` is the size of the VU
/// cycle: a `vu-select` outside it is an error reply, never a dispatch.
/// `external` names the external source playing ("" = the MA queue):
/// the queue view does not exist then
pub(crate) fn handle(
    id: u64,
    cmd: &str,
    dispatch: &Dispatch,
    vu_views: usize,
    external: &str,
) -> Response {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Response::err(id, "empty command");
    }
    match Command::parse(cmd) {
        Some(command) if command.is_settings() => Response::err(id, "not a control command"),
        Some(Command::Queue) if !external.is_empty() => {
            Response::err(id, format!("queue is not available while {external} plays"))
        }
        Some(Command::VuSelect(index)) if index >= vu_views => {
            Response::err(id, format!("vu-select: no skin at index {index}"))
        }
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

    const VU_VIEWS: usize = 3;

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
            "vu-select:0",
            "vu-select:2",
            "standby-on",
            "standby-off",
            "standby-toggle",
        ] {
            let response = handle(7, cmd, &dispatch, VU_VIEWS, "");
            assert!(response.ok);
            assert_eq!(response.id, 7);
            assert_eq!(seen.borrow_mut().pop(), Command::parse(cmd));
            assert!(seen.borrow().is_empty());
        }
    }

    #[test]
    fn the_queue_is_refused_while_an_external_source_plays() {
        let dispatch: Dispatch = Rc::new(|_| panic!("unexpected dispatch"));
        let response = handle(4, "queue", &dispatch, VU_VIEWS, "AirPlay");
        assert!(!response.ok);
        assert_eq!(response.id, 4);
        assert_eq!(
            response.error.as_deref(),
            Some("queue is not available while AirPlay plays")
        );
    }

    #[test]
    fn everything_else_still_dispatches_while_an_external_source_plays() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let dispatch: Dispatch = Rc::new(move |cmd| sink.borrow_mut().push(cmd.clone()));
        for cmd in ["now-playing", "vu", "play-pause", "standby-toggle"] {
            assert!(handle(1, cmd, &dispatch, VU_VIEWS, "AirPlay").ok, "{cmd}");
        }
        assert_eq!(seen.borrow().len(), 4);
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
            "vu-select:3",
            "vu-select:99999999999999999999",
            "vu-select:-1",
            "vu-select:",
            "vu-select:x",
        ] {
            let response = handle(9, cmd, &dispatch, VU_VIEWS, "");
            assert!(!response.ok, "{cmd}");
            assert_eq!(response.id, 9);
        }
    }
}
