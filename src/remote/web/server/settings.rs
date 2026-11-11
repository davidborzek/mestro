//! the UI-bound half of the protocol: settings reads/writes and
//! the process-internal ops. Runs on the UI thread (through the
//! `web-op` callback via [`crate::remote::web::Bridge`]) — MA calls never
//! arrive here (they would block the UI). Writes map 1:1 onto
//! `control::Command`, the same vocabulary the touch UI and the
//! text protocol use.

use super::Dispatch;
use crate::remote::control::Command;
use crate::remote::web::protocol::InternalOp;
use crate::remote::web::{Envelope, Request, Response, SettingsOp, WebInfo};
use crate::settings::Handle;
use crate::Mestro;

/// one round on the UI thread. `dispatch` is the shared command
/// path (it owns the UI resync after a write).
pub(crate) fn handle_ui(
    text: &str,
    ui: &Mestro,
    settings: &Handle,
    dispatch: &Dispatch,
    info: &WebInfo,
) -> String {
    // internal bridge ops (process-internal, never wire-visible:
    // the socket side is the only producer, `dispatch` rejects
    // everything outside the public Request vocabulary)
    if let Ok(op) = serde_json::from_str::<InternalOp>(text) {
        let response = match op {
            InternalOp::MaToken => {
                let merged = settings.merged();
                let get = |key: &str| -> String {
                    merged
                        .as_table()
                        .and_then(|t| t.get("music-assistant"))
                        .and_then(|m| m.as_table())
                        .and_then(|m| m.get(key))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                };
                // the caller decides what an empty token means
                // (schema < 28 servers need none) — always report
                // both fields together
                Response::ok(
                    0,
                    serde_json::json!({ "token": get("token"), "url": get("url") }),
                )
            }
        };
        return serde_json::to_string(&response)
            .unwrap_or_else(|e| format!(r#"{{"id":0,"ok":false,"error":"{e}"}}"#));
    }
    let response = match serde_json::from_str::<Envelope>(text) {
        Err(e) => Response::err(0, format!("bad envelope: {e}")),
        Ok(env) => {
            let id = env.id;
            match env.request {
                Request::Settings {
                    action: SettingsOp::State,
                } => Response::ok(id, state_payload(ui, settings, info)),
                Request::Settings {
                    action: SettingsOp::Set { key, value },
                } => match settings.set(&key, value.clone()) {
                    // the write also rides the canonical command
                    // path — it owns the UI resync (the second
                    // apply writes the same value again)
                    Ok(_) => {
                        dispatch(&Command::SetSetting { key, value });
                        Response::ok(id, state_payload(ui, settings, info))
                    }
                    Err(e) => Response::err(id, e),
                },
                Request::Settings {
                    action: SettingsOp::Reset { key },
                } => match settings.reset(&key) {
                    Ok(_) => {
                        dispatch(&Command::ResetSetting { key: key.clone() });
                        Response::ok(id, state_payload(ui, settings, info))
                    }
                    Err(e) => Response::err(id, e),
                },
                Request::Settings {
                    action: SettingsOp::ResetAll,
                } => match settings.reset_all() {
                    Ok(_) => {
                        dispatch(&Command::ResetAllSettings);
                        Response::ok(id, state_payload(ui, settings, info))
                    }
                    Err(e) => Response::err(id, e),
                },
                Request::Settings {
                    action: SettingsOp::Restart,
                } => {
                    // the root callback: flush + quit + restart —
                    // the wizard reconnects to the fresh process
                    ui.invoke_restart_app();
                    Response::ok(id, serde_json::json!({ "restarting": true }))
                }
                Request::Ctl { cmd } => super::ctl::handle(id, &cmd, dispatch),
                Request::Hello | Request::Ma { .. } => Response::err(id, "not a ui-bound request"),
            }
        }
    };
    serde_json::to_string(&response)
        .unwrap_or_else(|e| format!(r#"{{"id":0,"ok":false,"error":"{e}"}}"#))
}

/// the effective config state for the wizard/remote editor:
/// redacted config, override keys, provider status, wizard URL
fn state_payload(ui: &Mestro, settings: &Handle, info: &WebInfo) -> serde_json::Value {
    crate::remote::web::protocol::state_data(
        &settings.merged(),
        &settings.override_keys(),
        settings
            .base_path()
            .map(|p| p.display().to_string())
            .as_deref(),
        ui.get_provider_status().as_ref(),
        &info.remote_url(),
    )
}
