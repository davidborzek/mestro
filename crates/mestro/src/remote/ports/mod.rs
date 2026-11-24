//! The binary's implementation of the mestro-remote ports: the
//! slint [`UiPort`] implementation (`SlintUi`), the MA auth client
//! (the provider's REST calls), the `web-op` handler glue
//! (settings, ctl, player,
//! timezone — everything that needs `&Mestro` or the settings
//! handle on the UI thread) and the OTA overlay updater. The
//! server, auth and push pipeline live in the `mestro-remote`
//! crate behind the ports — this module is the only remote
//! code that touches slint or the provider.

use crate::providers::ma::auth as ma_auth;
use crate::settings::Handle;
use crate::Mestro;
use mestro_ipc::command::Command;
use mestro_ipc::web::{Envelope, InternalOp, Request, Response, SettingsOp};
use mestro_remote::server::ctl;
use mestro_remote::server::http;
use mestro_remote::server::player;
use mestro_remote::server::timezone;
use mestro_remote::server::Dispatch;
use mestro_remote::{
    AuthMode, MaAuth, PlayerCtx, PlayerSummary, ServerInfo, UiPort, UiUpdate, UiView, WebInfo,
};
use slint::ComponentHandle as _;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

mod state_data;

/// renders one settings-preview thumbnail (kind "vu" or
/// "clock", name = skin/style) as PNG bytes on the UI thread;
/// None when the name is unknown. Built in main.rs from the
/// compiled skin registries and threaded into [`wire`]
pub type PreviewRenderer = Rc<dyn Fn(&str, &str) -> Option<Vec<u8>>>;

/// the MA auth port against the provider's REST client (ureq):
/// the web endpoint's wizard calls land here, the web side never
/// links the provider
pub struct MaAuthClient;

impl MaAuth for MaAuthClient {
    fn probe(&self, url: &str) -> Result<ServerInfo, String> {
        ma_auth::probe(url)
            .map(|s| ServerInfo {
                version: s.version,
                schema: s.schema,
                auth: match s.auth {
                    ma_auth::AuthMode::Login => AuthMode::Login,
                    ma_auth::AuthMode::None => AuthMode::None,
                },
            })
            .map_err(|e| e.to_string())
    }

    fn login(
        &self,
        url: &str,
        username: &str,
        password: &str,
        device_name: &str,
    ) -> Result<String, String> {
        ma_auth::login(url, username, password, device_name).map_err(|e| e.to_string())
    }

    fn players(&self, url: &str, token: &str) -> Result<Vec<PlayerSummary>, String> {
        ma_auth::players(url, token)
            .map(|list| {
                list.into_iter()
                    .map(|p| PlayerSummary {
                        id: p.id,
                        name: p.name,
                    })
                    .collect()
            })
            .map_err(|e| e.to_string())
    }

    fn same_endpoint(&self, a: &str, b: &str) -> bool {
        ma_auth::same_endpoint(a, b)
    }
}

/// the slint side of the UI port: op round-trips through
/// `invoke_from_event_loop` (the handles are `!Sync`), progress
/// hints fire and forget
pub struct SlintUi {
    ui: slint::Weak<Mestro>,
}

impl SlintUi {
    pub fn new(ui: &Mestro) -> Self {
        Self { ui: ui.as_weak() }
    }
}

impl UiPort for SlintUi {
    fn op(&self, envelope: &str) -> Result<String, String> {
        let (tx, rx) = mpsc::channel();
        let ui = self.ui.clone();
        let envelope = envelope.to_string();
        let _ = slint::invoke_from_event_loop(move || {
            let out = ui
                .upgrade()
                .map(|u| u.invoke_web_op(envelope.as_str().into()).to_string());
            let _ = tx.send(out);
        });
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "ui thread timeout".to_string())?
            .ok_or_else(|| "ui unavailable".to_string())
    }

    fn notify(&self, update: UiUpdate) {
        let ui = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                match update {
                    UiUpdate::UpdateStatus { text, failure } => {
                        show_update_status(&ui, &text, failure)
                    }
                }
            }
        });
    }

    fn control(&self, cmd: String) {
        let ui = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                ui.invoke_control_command(cmd.as_str().into());
            }
        });
    }
}

/// one view snapshot for the push pipeline: the kiosk view flags,
/// the VU cycle, the backlight and the MA connection as the UI
/// props hold them (UI thread only). The binary sends it into the
/// view watch whenever a view-relevant property changes — the
/// slint `changed` hooks fire `view-changed`, `wire_remote` and
/// the settings dispatch are the Rust-side publishers
pub fn view_of(ui: &Mestro, ma_ws_url: &str) -> UiView {
    UiView {
        np_open: ui.get_np_open(),
        queue_open: ui.get_queue_open(),
        vu_open: ui.get_vu_open(),
        vu_index: ui.get_vu_view_index(),
        vu_count: ui.get_vu_view_count(),
        vu_skins: slint::Model::iter(&ui.get_vu_view_names())
            .map(|name| name.to_string())
            .collect(),
        standby: ui.get_standby_showing(),
        display_supported: ui.get_display_supported(),
        brightness: ui.get_display_brightness().round() as i32,
        connected: ui.get_provider_connected(),
        ma_ws_url: ma_ws_url.to_string(),
    }
}

/// install the UI-bound handler on the `web-op` callback. It runs
/// on the UI thread (via [`SlintUi`]); `dispatch` is the shared
/// command path (the same one touch UI and text protocol use),
/// `player` carries the live state the `player` op's volume
/// bookkeeping needs.
pub fn wire(
    ui: &Mestro,
    settings: &Rc<Handle>,
    dispatch: &Dispatch,
    info: &WebInfo,
    player: &PlayerCtx,
    previews: &PreviewRenderer,
) {
    let ui_weak = ui.as_weak();
    let settings = settings.clone();
    let dispatch = dispatch.clone();
    let info = info.clone();
    let player = player.clone();
    let previews = previews.clone();
    ui.on_web_op(move |envelope| -> slint::SharedString {
        let Some(ui) = ui_weak.upgrade() else {
            return r#"{"id":0,"ok":false,"error":"ui unavailable"}"#.into();
        };
        handle_ui(
            envelope.as_str(),
            &ui,
            &settings,
            &dispatch,
            &info,
            &player,
            &previews,
        )
        .into()
    });
}

/// the device overlay: a failure text clears itself, unless a
/// newer status replaced it meanwhile
fn show_update_status(ui: &Mestro, text: &str, failure: bool) {
    const FAILURE_VISIBLE: Duration = Duration::from_secs(8);
    let text = slint::SharedString::from(text);
    ui.set_update_status(text.clone());
    if failure {
        let ui = ui.as_weak();
        slint::Timer::single_shot(FAILURE_VISIBLE, move || {
            if let Some(ui) = ui.upgrade().filter(|ui| ui.get_update_status() == text) {
                ui.set_update_status("".into());
            }
        });
    }
}

/// one round on the UI thread: the UI-bound half of the protocol
/// (settings reads/writes, ctl, the player volume, the timezone
/// picker) plus the process-internal ops (the preview renderer,
/// the stored MA token). `dispatch` is the shared command path
/// (it owns the UI resync after a write), `previews` renders the
/// settings thumbnails.
pub fn handle_ui(
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
                Request::Ctl { cmd } => {
                    let caps = ui.get_transport_caps();
                    ctl::handle(
                        id,
                        &cmd,
                        dispatch,
                        ui.get_vu_view_count() as usize,
                        ui.get_external_source().as_str(),
                        &ctl::TransportCaps {
                            play_pause: caps.play_pause,
                            next_previous: caps.next_previous,
                            shuffle: caps.shuffle,
                            repeat: caps.repeat,
                        },
                    )
                }
                Request::Player { action } => player::handle(id, &action, player),
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
    action: mestro_ipc::web::TimezoneOp,
    ui: &Mestro,
    settings: &Handle,
    dispatch: &Dispatch,
) -> Response {
    let chosen = settings
        .effective()
        .map(|cfg| cfg.system.timezone)
        .unwrap_or_default();
    let tz_env = std::env::var("TZ").ok();
    let outcome = timezone::run(id, action, &chosen, tz_env.as_deref());
    if let Some(zone) = outcome.record {
        // restart.rs execs with the current environment and the init
        // script's mestro.env is not read again — the exec must carry
        // the new zone, or the restart comes back in the old one
        #[cfg(feature = "timezone")]
        if outcome.restart {
            mestro_os::timezone::set_pending_zone(&zone);
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
    state_data::state_data(
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
        Some(png) => Response::ok(0, serde_json::json!({ "png": http::hex_encode(&png) })),
        None => Response::err(0, format!("unknown preview {kind}/{name}")),
    }
}

/// factory reset (pairing is enforced at the route): the state
/// files go (settings overlay, web-pin — every phone re-pairs;
/// the state half is `system::reset`, right below), then the wifi
/// credentials return to the template — or, without a template,
/// are removed (the returned outcome says which; the wifi half
/// lives in `mestro_os::wifi::reset`) — and the fresh overlay keeps only
/// the empty MA keys — first run shows even when config.toml
/// carries a connection. The restart that follows persists the
/// overlay and starts over
#[cfg(feature = "wifi-setup")]
fn factory_reset(
    settings: &Handle,
    dispatch: &Dispatch,
    wifi: bool,
) -> Result<mestro_os::wifi::reset::WifiReset, String> {
    // the state half first — on its failure the wifi half never
    // runs (today's behavior)
    mestro_os::reset::factory_reset(&mestro_os::reset::ResetPaths::device())?;
    let outcome = wifi_reset(&mestro_os::wifi::reset::WifiResetPaths::device(), wifi)?;
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

/// the wifi half of the reset: the op's `wifi` flag decides —
/// `false` leaves the credentials (and their sidecars) alone
#[cfg(feature = "wifi-setup")]
fn wifi_reset(
    paths: &mestro_os::wifi::reset::WifiResetPaths,
    wifi: bool,
) -> Result<mestro_os::wifi::reset::WifiReset, String> {
    if wifi {
        mestro_os::wifi::reset::reset_credentials(paths)
    } else {
        Ok(mestro_os::wifi::reset::WifiReset::Kept)
    }
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

    #[cfg(feature = "wifi-setup")]
    #[test]
    fn no_wifi_reset_keeps_the_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let p = mestro_os::wifi::reset::WifiResetPaths {
            wpa_conf: dir.path().join("wpa_supplicant.conf"),
            wpa_template: dir.path().join("template.conf"),
        };
        std::fs::write(&p.wpa_conf, "network={\n\tssid=\"Real\"\n}\n").unwrap();
        std::fs::write(p.wpa_conf.with_extension("conf.good"), "last good").unwrap();
        assert_eq!(
            wifi_reset(&p, false).unwrap(),
            mestro_os::wifi::reset::WifiReset::Kept
        );
        assert!(p.wpa_conf.exists());
        // the sidecars belong to the credentials: without the
        // wifi reset they stay
        assert!(p.wpa_conf.with_extension("conf.good").exists());
    }
}
