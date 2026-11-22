//! the UI-bound half of the protocol: settings reads/writes and
//! the process-internal ops. Runs on the UI thread (through the
//! `web-op` callback via [`crate::remote::web::Bridge`]) — MA calls never
//! arrive here (they would block the UI). Writes map 1:1 onto
//! `control::Command`, the same vocabulary the touch UI and the
//! text protocol use.

use super::Dispatch;
use crate::remote::control::Command;
use crate::remote::web::protocol::InternalOp;
use crate::remote::web::PlayerCtx;
use crate::remote::web::{Envelope, PreviewRenderer, Request, Response, SettingsOp, WebInfo};
use crate::settings::Handle;
use crate::Mestro;

/// one round on the UI thread. `dispatch` is the shared command
/// path (it owns the UI resync after a write), `previews` renders
/// the settings thumbnails.
pub(crate) fn handle_ui(
    text: &str,
    ui: &Mestro,
    settings: &Handle,
    dispatch: &Dispatch,
    info: &WebInfo,
    player: &PlayerCtx,
    previews: &PreviewRenderer,
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
            InternalOp::Preview { kind, name } => preview_response(previews, &kind, &name),
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
                    action: SettingsOp::Schema,
                } => match settings.effective() {
                    Ok(effective) => {
                        match serde_json::to_value(crate::settings::schema(&effective)) {
                            Ok(schema) => Response::ok(id, schema),
                            Err(e) => Response::err(id, e.to_string()),
                        }
                    }
                    Err(e) => Response::err(id, e),
                },
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
                Request::Settings {
                    action: SettingsOp::FactoryReset { wifi },
                } => {
                    #[cfg(feature = "wifi-setup")]
                    {
                        match factory_reset(settings, dispatch, wifi) {
                            Ok(outcome) => {
                                ui.invoke_restart_app();
                                Response::ok(
                                    id,
                                    serde_json::json!({
                                        "restarting": true,
                                        // what the wifi part did — kept |
                                        // restored | removed
                                        "wifi": outcome.as_str(),
                                    }),
                                )
                            }
                            Err(e) => Response::err(id, e),
                        }
                    }
                    #[cfg(not(feature = "wifi-setup"))]
                    {
                        let _ = wifi;
                        Response::err(id, "factory reset not available")
                    }
                }
                Request::Ctl { cmd } => super::ctl::handle(
                    id,
                    &cmd,
                    dispatch,
                    ui.get_vu_view_count() as usize,
                    ui.get_external_source().as_str(),
                ),
                Request::Player { action } => super::player::handle(id, &action, player),
                Request::Timezone { action } => {
                    timezone_response(id, action, ui, settings, dispatch)
                }
                Request::Hello
                | Request::Ma { .. }
                | Request::Wifi { .. }
                | Request::Audio { .. } => Response::err(id, "not a ui-bound request"),
            }
        }
    };
    serde_json::to_string(&response)
        .unwrap_or_else(|e| format!(r#"{{"id":0,"ok":false,"error":"{e}"}}"#))
}

/// the timezone ops: the decision is `timezone::run`, this carries
/// it out on the UI thread — record the chosen zone as the
/// `system.timezone` setting, then restart (the restart flushes the
/// setting; the new `TZ` is exported just before the exec)
fn timezone_response(
    id: u64,
    action: crate::remote::web::TimezoneOp,
    ui: &Mestro,
    settings: &Handle,
    dispatch: &Dispatch,
) -> Response {
    let chosen = settings
        .effective()
        .map(|cfg| cfg.system.timezone)
        .unwrap_or_default();
    let tz_env = std::env::var("TZ").ok();
    let outcome = super::timezone::run(id, action, &chosen, tz_env.as_deref());
    if let Some(zone) = outcome.record {
        // restart.rs execs with the current environment and the init
        // script's mestro.env is not read again — the exec must carry
        // the new zone, or the restart comes back in the old one
        #[cfg(feature = "timezone")]
        if outcome.restart {
            crate::system::timezone::set_pending_zone(&zone);
        }
        let key = "system.timezone".to_string();
        let value = toml::Value::String(zone);
        if let Err(e) = settings.set(&key, value.clone()) {
            return Response::err(id, e);
        }
        dispatch(&Command::SetSetting { key, value });
    }
    if outcome.restart {
        ui.invoke_restart_app();
    }
    outcome.response
}

/// the effective config state for the wizard/remote editor:
/// redacted config, effective config (defaults folded in),
/// override keys, provider status, wizard URL
fn state_payload(ui: &Mestro, settings: &Handle, info: &WebInfo) -> serde_json::Value {
    // a broken base cannot fold defaults — the wizard keeps
    // working on the built-in ones (config::load does the same)
    let effective = settings.effective().unwrap_or_else(|e| {
        tracing::warn!("web: effective config: {e}");
        crate::config::Config::default()
    });
    crate::remote::web::protocol::state_data(
        &settings.merged(),
        &effective,
        &settings.override_keys(),
        settings
            .base_path()
            .map(|p| p.display().to_string())
            .as_deref(),
        ui.get_provider_status().as_ref(),
        &info.remote_url(),
    )
}
/// the preview op's reply: PNG bytes as hex
fn preview_response(previews: &PreviewRenderer, kind: &str, name: &str) -> Response {
    match previews(kind, name) {
        Some(png) => Response::ok(
            0,
            serde_json::json!({ "png": super::http::hex_encode(&png) }),
        ),
        None => Response::err(0, format!("unknown preview {kind}/{name}")),
    }
}

/// factory reset (pairing is enforced at the route): the state
/// files go (settings overlay, web-pin — every phone re-pairs),
/// the wifi credentials return to the template — or, without a
/// template, are removed (the returned outcome says which) — and
/// the fresh overlay keeps only the empty MA keys — first run
/// shows even when config.toml carries a connection. The restart
/// that follows persists the overlay and starts over
#[cfg(feature = "wifi-setup")]
fn factory_reset(
    settings: &Handle,
    dispatch: &Dispatch,
    wifi: bool,
) -> Result<crate::system::reset::WifiReset, String> {
    use crate::system::reset::ResetPaths;
    let outcome = crate::system::reset::factory_reset(
        &ResetPaths::device(crate::system::paths::WPA_CONF),
        wifi,
    )?;
    // clears the in-memory overlay too; the dispatch owns the
    // UI resync (the restart redraws everything anyway)
    dispatch(&Command::ResetAllSettings);
    for key in [
        "music-assistant.url",
        "music-assistant.token",
        "music-assistant.player",
    ] {
        settings
            .set(key, toml::Value::String(String::new()))
            .map_err(|e| format!("could not clear {key}: {e}"))?;
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn renderer(png: Option<Vec<u8>>) -> PreviewRenderer {
        Rc::new(move |_kind, _name| png.clone())
    }

    fn data_of(reply: &str) -> serde_json::Value {
        let value: serde_json::Value = serde_json::from_str(reply).unwrap();
        assert_eq!(value["ok"], true, "{reply}");
        value["data"].clone()
    }

    #[test]
    fn the_preview_op_returns_png_bytes_as_hex() {
        let reply = preview_response(
            &renderer(Some(vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a])),
            "vu",
            "round",
        );
        let data = data_of(&serde_json::to_string(&reply).unwrap());
        assert_eq!(data["png"], "89504e470d0a");
    }

    #[test]
    fn an_unknown_preview_is_an_error() {
        let reply = preview_response(&renderer(None), "clock", "missing");
        let text = serde_json::to_string(&reply).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["ok"], false);
        assert!(value["error"].as_str().unwrap().contains("missing"));
    }
}
