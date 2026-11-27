//! The settings UI wiring: the typed editors behind the settings
//! page. Owns the chip/order list arithmetic and the sync push
//! (slint props) — main only calls [`wire`].
//!
//! The wire vocabulary (names, canonical order) lives with the
//! config types (`TransportControl::wire_name`, `QualityPart::ALL`)
//! so the settings page, the text protocol and a future web
//! client all speak the same names.

use super::Handle;
use crate::config::{self, QualityPart, TransportControl};
use crate::ui::navigation::Navigation;
use crate::{player::nowplaying, Mestro};
use mestro_ipc::command::Command;
use slint::Model as _;
use std::cell::RefCell;
use std::rc::Rc;

/// the settings-editable now-playing lists, shared between the
/// update loop (quality parts feed apply()) and the editors here
/// — a settings change rewrites this from the effective config
#[derive(Debug)]
pub struct NpState {
    pub quality: Vec<QualityPart>,
    pub controls: Vec<TransportControl>,
}

impl NpState {
    pub fn from(config: &config::Config) -> Self {
        Self {
            quality: config.now_playing.quality.clone(),
            controls: config.now_playing.controls.clone(),
        }
    }
}

/// one dispatch call — the shared path with the text protocol
type Dispatch = Rc<dyn Fn(&Command)>;

fn set_list(dispatch: &Dispatch, key: &str, names: Vec<&str>) {
    dispatch(&Command::SetSetting {
        key: key.into(),
        value: toml::Value::Array(names.into_iter().map(toml::Value::from).collect()),
    });
}

/// toggle one item of an ordered list (in ↔ out, canonical order
/// for re-added items)
fn toggled<T: Copy + PartialEq>(
    list: &[T],
    all: &[T],
    name: &str,
    wire: impl Fn(&T) -> &'static str,
) -> Vec<T> {
    let has = |t: &T| list.contains(t);
    all.iter()
        .filter(|t| if wire(t) == name { !has(t) } else { has(t) })
        .copied()
        .collect()
}

/// move one item by ±1 within the list (the order editors)
fn moved<T: Copy>(
    list: &[T],
    name: &str,
    dir: i32,
    wire: impl Fn(&T) -> &'static str,
) -> Option<Vec<T>> {
    let pos = list.iter().position(|t| wire(t) == name)?;
    let target = pos as i32 + dir;
    if target < 0 || target >= list.len() as i32 {
        return None;
    }
    let mut out = list.to_vec();
    out.swap(pos, target as usize);
    Some(out)
}

/// re-push the mirror state: override flags (the reset buttons
/// show only on keys the overlay holds) + the editor models
fn sync(ui: &Mestro, handle: &Handle, np: &NpState) {
    let is = |k: &str| handle.is_overridden(k);
    ui.set_np_title_size_overridden(is("now-playing.font.title-size"));
    ui.set_np_artist_size_overridden(is("now-playing.font.artist-size"));
    ui.set_np_album_size_overridden(is("now-playing.font.album-size"));
    ui.set_np_quality_size_overridden(is("now-playing.font.quality-size"));
    ui.set_np_title_weight_overridden(is("now-playing.font.title-weight"));
    ui.set_np_artist_weight_overridden(is("now-playing.font.artist-weight"));
    ui.set_np_album_weight_overridden(is("now-playing.font.album-weight"));
    ui.set_np_quality_weight_overridden(is("now-playing.font.quality-weight"));
    ui.set_np_inline_vu_overridden(is("now-playing.inline-vu"));
    ui.set_np_layout_overridden(is("now-playing.layout"));
    ui.set_np_controls_overridden(is("now-playing.controls"));
    ui.set_np_quality_overridden(is("now-playing.quality"));

    let ss_model = |v: &[&str]| {
        let ss: Vec<slint::SharedString> = v.iter().map(|s| (*s).into()).collect();
        slint::ModelRc::new(slint::VecModel::from(ss))
    };
    let controls: Vec<&str> = np.controls.iter().map(|t| t.wire_name()).collect();
    let inactive_c: Vec<&str> = TransportControl::ALL
        .iter()
        .map(|t| t.wire_name())
        .filter(|n| !controls.contains(n))
        .collect();
    let quality: Vec<&str> = np.quality.iter().map(|q| q.wire_name()).collect();
    let inactive_q: Vec<&str> = QualityPart::ALL
        .iter()
        .map(|q| q.wire_name())
        .filter(|n| !quality.contains(n))
        .collect();
    ui.set_ctrl_order(ss_model(&controls));
    ui.set_ctrl_inactive(ss_model(&inactive_c));
    ui.set_quality_order(ss_model(&quality));
    ui.set_quality_inactive(ss_model(&inactive_q));
}

/// push the theme + ui family: effective values (config names)
/// and override flags. Called at startup and after every
/// theme.*/ui.* edit
pub fn sync_theme_ui(ui: &Mestro, handle: &Handle, effective: &config::Config) {
    let is = |k: &str| handle.is_overridden(k);
    ui.set_theme_palette(
        match effective.theme.palette {
            config::ThemePalette::Amber => "amber",
            config::ThemePalette::Cover => "cover",
        }
        .into(),
    );
    ui.set_theme_variant(crate::ui::palette::variant_as_str(effective.theme.variant.raw()).into());
    ui.set_ui_rail(
        match effective.ui.rail {
            config::RailStyle::Dark => "dark",
            config::RailStyle::Glass => "glass",
        }
        .into(),
    );
    ui.set_ui_auto_open(effective.ui.auto_open_now_playing);
    ui.set_ui_osk(effective.ui.osk_layout.as_str().into());
    ui.set_bg_blurred(effective.background.blurred_cover);
    // clock
    // the gallery previews come from main (it owns the compiled
    // skins), see activate_clock
    ui.set_clock_style(effective.clock.style.as_str().into());
    let bg = &effective.clock.background;
    ui.set_bg_source(bg.source.wire_name().into());
    ui.set_bg_dim((bg.dim * 100.0).round());
    ui.set_bg_blur(bg.blur);
    ui.set_bg_interval(bg.interval as f32);
    ui.set_bg_source_overridden(is("clock.background.source"));
    ui.set_bg_dim_overridden(is("clock.background.dim"));
    ui.set_bg_blur_overridden(is("clock.background.blur"));
    ui.set_bg_interval_overridden(is("clock.background.interval"));
    // hub (home screen): layout + entry order (a permutation).
    // `hub-look` is the settings display — the renderer reads the
    // 0/1 `hub-layout` int below
    ui.set_hub_look(
        match effective.hub.layout {
            config::HubLayout::Cards => "cards",
            config::HubLayout::List => "list",
        }
        .into(),
    );
    ui.set_hub_order(slint::ModelRc::new(slint::VecModel::from(
        effective
            .hub
            .order
            .iter()
            .map(|e| slint::SharedString::from(e.wire_name()))
            .collect::<Vec<_>>(),
    )));
    // osd (the notification layer)
    ui.set_osd_style(
        match effective.osd.style {
            config::OsdStyle::Translucent => "translucent",
            config::OsdStyle::Solid => "solid",
        }
        .into(),
    );
    ui.set_osd_position(anchor_name(effective.osd.position).into());
    ui.set_osd_volume_position(anchor_name(effective.osd.volume_position).into());
    ui.set_osd_toast_duration(effective.osd.toast_duration as f32);
    ui.set_osd_volume_duration(effective.osd.volume_duration as f32);
    ui.set_osd_gates(gates("osd", &effective.osd, None, &is));
    ui.set_osd_np_gates(gates(
        "osd.now-playing",
        &effective.osd,
        Some(&effective.osd.now_playing),
        &is,
    ));
    ui.set_osd_vu_gates(gates(
        "osd.vu",
        &effective.osd,
        Some(&effective.osd.vu),
        &is,
    ));
    // the render props (clock/hub/osd visuals read these — live
    // edits land here, not just at startup)
    ui.set_clock_analog(effective.clock.is_analog());
    ui.set_hub_layout(match effective.hub.layout {
        config::HubLayout::Cards => 0,
        config::HubLayout::List => 1,
    });
    ui.set_toast_anchor(crate::ui::osd::anchor(effective.osd.position));
    ui.set_osd_anchor(crate::ui::osd::anchor(effective.osd.volume_position));
    ui.set_osd_solid(matches!(effective.osd.style, config::OsdStyle::Solid));
    ui.set_theme_palette_overridden(is("theme.palette"));
    ui.set_theme_variant_overridden(is("theme.variant"));
    ui.set_ui_rail_overridden(is("ui.rail"));
    ui.set_ui_auto_open_overridden(is("ui.auto_open_now_playing"));
    ui.set_ui_osk_overridden(is("ui.osk_layout"));
    ui.set_bg_blurred_overridden(is("background.blurred_cover"));
    ui.set_clock_style_overridden(is("clock.style"));
    ui.set_hub_look_overridden(is("hub.layout"));
    ui.set_hub_order_overridden(is("hub.order"));
    ui.set_osd_style_overridden(is("osd.style"));
    ui.set_osd_position_overridden(is("osd.position"));
    ui.set_osd_volume_position_overridden(is("osd.volume-position"));
    ui.set_osd_toast_duration_overridden(is("osd.toast-duration"));
    ui.set_osd_volume_duration_overridden(is("osd.volume-duration"));
}

fn anchor_name(a: config::OsdAnchor) -> &'static str {
    match a {
        config::OsdAnchor::Top => "top",
        config::OsdAnchor::Bottom => "bottom",
        config::OsdAnchor::Left => "left",
        config::OsdAnchor::Right => "right",
        config::OsdAnchor::Center => "center",
    }
}

/// one gate group: the base flags supply the value, a context
/// override wins where set (`Option<bool>` = inherit)
fn gates(
    prefix: &str,
    base: &config::Osd,
    ovr: Option<&config::OsdFlags>,
    is: &dyn Fn(&str) -> bool,
) -> crate::OsdGates {
    let g =
        |name: &str, b: bool, o: Option<bool>| (o.unwrap_or(b), is(&format!("{prefix}.{name}")));
    let (transport, transport_overridden) =
        g("transport", base.transport, ovr.and_then(|o| o.transport));
    let (track, track_overridden) = g("track", base.track, ovr.and_then(|o| o.track));
    let (queue, queue_overridden) = g("queue", base.queue, ovr.and_then(|o| o.queue));
    let (volume, volume_overridden) = g("volume", base.volume, ovr.and_then(|o| o.volume));
    crate::OsdGates {
        transport,
        transport_overridden,
        track,
        track_overridden,
        queue,
        queue_overridden,
        volume,
        volume_overridden,
    }
}

/// push the connection family (music-assistant.*): effective
/// values, byte lengths (the caret parks at the text end) and
/// override flags. Restart-class: the provider session reads
/// them at the next start
/// The first-run overlay latches from boot (or an in-session
/// connection reset) until the next restart picks the new values
/// up — url/token are restart-class, so "detected" still shows the
/// overlay with the apply button.
static BOOTED_UNCONFIGURED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn sync_connection(ui: &Mestro, effective: &config::Config) {
    let ma = &effective.music_assistant;
    ui.set_conn_url(ma.url.as_str().into());
    ui.set_startup_server(server_host(&ma.url).into());
    ui.set_conn_player(ma.player.as_str().into());
    // never the token itself — only whether one exists
    ui.set_conn_token_set(!ma.token.is_empty());
    // unconfigured = no server at all. A missing token is fine
    // (schema < 28 servers need none), so it does not count.
    let unconfigured = ma.url.is_empty();
    if unconfigured {
        BOOTED_UNCONFIGURED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let latched = BOOTED_UNCONFIGURED.load(std::sync::atomic::Ordering::Relaxed);
    ui.set_first_run(latched);
    // the wizard writes more than the url (token and/or player)
    // once it is done — that is the "apply & restart" point
    let wizard_done = !ma.url.is_empty() && (!ma.token.is_empty() || !ma.player.is_empty());
    ui.set_first_run_detected(latched && wizard_done);
}

/// `ws://host:8095/ws` → `host`: the boot screen names the server,
/// not its endpoint
fn server_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or_default();
    match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    }
}

/// the settings-layer banner state: what config file the edits
/// override, and whether anything deviates from it
pub fn sync_overlay_state(ui: &Mestro, handle: &Handle) {
    ui.set_config_problems(slint::ModelRc::new(slint::VecModel::from(
        crate::config::problems()
            .into_iter()
            .map(|p| crate::ConfigProblem {
                key: p.key.into(),
                reason: p.reason.into(),
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_settings_base_config(
        handle
            .base_path()
            .map(short_path)
            .unwrap_or_default()
            .into(),
    );
    ui.set_settings_overrides_active(handle.has_overrides());
}

/// the file name and its folder ("mestro/config.toml"): enough to
/// recognize the file, short enough for the banner. Short paths
/// show as they are
fn short_path(path: &std::path::Path) -> String {
    let normal: Vec<_> = path
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect();
    if normal.len() <= 2 {
        return path.display().to_string();
    }
    format!(
        "…/{}",
        normal[normal.len() - 2..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    )
}

#[cfg(test)]
mod short_path_tests {
    use super::short_path;
    use std::path::Path;

    #[test]
    fn keeps_folder_and_file() {
        assert_eq!(
            short_path(Path::new("/data/config/mestro/config.toml")),
            "…/mestro/config.toml"
        );
        assert_eq!(short_path(Path::new("/dev/null")), "/dev/null");
        assert_eq!(short_path(Path::new("config.toml")), "config.toml");
    }
}

/// wire every settings-page callback. `dispatch` is the shared
/// command path (text protocol + keyboard ride it too)
pub fn wire(
    ui: &Mestro,
    handle: &Rc<Handle>,
    np_state: &Rc<RefCell<NpState>>,
    dispatch: &Dispatch,
) {
    // numeric stepper + text picker entries: route through the
    // same SetSetting command as everything else
    {
        let dispatch = dispatch.clone();
        ui.on_set_setting_num(move |key, v| {
            let value = if v.fract() == 0.0 {
                toml::Value::Integer(v as i64)
            } else {
                toml::Value::Float(v as f64)
            };
            dispatch(&Command::SetSetting {
                key: key.to_string(),
                value,
            });
        });
    }
    {
        let dispatch = dispatch.clone();
        ui.on_set_setting_bool(move |key, v| {
            dispatch(&Command::SetSetting {
                key: key.to_string(),
                value: toml::Value::Boolean(v),
            });
        });
    }
    {
        let dispatch = dispatch.clone();
        ui.on_set_setting_text(move |key, v| {
            dispatch(&Command::SetSetting {
                key: key.to_string(),
                value: toml::Value::String(v.to_string()),
            });
        });
    }
    {
        let dispatch = dispatch.clone();
        ui.on_reset_setting_value(move |key| {
            dispatch(&Command::ResetSetting {
                key: key.to_string(),
            });
        });
    }
    {
        let dispatch = dispatch.clone();
        ui.on_reset_all_settings(move || {
            dispatch(&Command::ResetAllSettings);
        });
    }
    // the connection page's recovery button: one tap clears the
    // whole music-assistant family (url, token, player)
    {
        let dispatch = dispatch.clone();
        ui.on_reset_connection(move || {
            for key in [
                "music-assistant.url",
                "music-assistant.token",
                "music-assistant.player",
            ] {
                // WRITES empty overrides instead of resetting —
                // reset restores the config.toml values and could
                // never reach "unconfigured" (first-run wizard)
                dispatch(&Command::SetSetting {
                    key: key.to_string(),
                    value: toml::Value::String(String::new()),
                });
            }
        });
    }

    // the chip/order editors: Rust owns the list arithmetic,
    // slint stays dumb (names in, names out)
    {
        let dispatch = dispatch.clone();
        let np_state = np_state.clone();
        ui.on_toggle_control(move |name| {
            let cur = np_state.borrow().controls.clone();
            let list = toggled(&cur, &TransportControl::ALL, name.as_str(), |t| {
                t.wire_name()
            });
            let names: Vec<&str> = list.iter().map(|t| t.wire_name()).collect();
            set_list(&dispatch, "now-playing.controls", names);
        });
    }
    {
        let dispatch = dispatch.clone();
        let np_state = np_state.clone();
        ui.on_toggle_quality_part(move |name| {
            let cur = np_state.borrow().quality.clone();
            let list = toggled(&cur, &QualityPart::ALL, name.as_str(), |q| q.wire_name());
            let names: Vec<&str> = list.iter().map(|q| q.wire_name()).collect();
            set_list(&dispatch, "now-playing.quality", names);
        });
    }
    {
        let dispatch = dispatch.clone();
        let np_state = np_state.clone();
        ui.on_move_control(move |name, dir| {
            let cur = np_state.borrow().controls.clone();
            if let Some(list) = moved(&cur, name.as_str(), dir, |t| t.wire_name()) {
                let names: Vec<&str> = list.iter().map(|t| t.wire_name()).collect();
                set_list(&dispatch, "now-playing.controls", names);
            }
        });
    }
    {
        let dispatch = dispatch.clone();
        let np_state = np_state.clone();
        ui.on_move_quality_part(move |name, dir| {
            let cur = np_state.borrow().quality.clone();
            if let Some(list) = moved(&cur, name.as_str(), dir, |q| q.wire_name()) {
                let names: Vec<&str> = list.iter().map(|q| q.wire_name()).collect();
                set_list(&dispatch, "now-playing.quality", names);
            }
        });
    }
    {
        let dispatch = dispatch.clone();
        let handle = handle.clone();
        // the hub order: read the effective list, move one entry
        // (missing entries append — validate() never drops one)
        ui.on_move_hub(move |name, dir| {
            let merged = handle.merged();
            let mut cur: Vec<config::HubEntry> = merged
                .as_table()
                .and_then(|t| t.get("hub"))
                .and_then(|h| h.as_table())
                .and_then(|h| h.get("order"))
                .and_then(|o| o.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| {
                            config::HubEntry::ALL
                                .iter()
                                .copied()
                                .find(|e| e.wire_name() == s)
                        })
                        .collect()
                })
                .unwrap_or_default();
            for e in config::HubEntry::ALL {
                if !cur.contains(&e) {
                    cur.push(e);
                }
            }
            if let Some(list) = moved(&cur, name.as_str(), dir, |e| e.wire_name()) {
                let names: Vec<&str> = list.iter().map(|e| e.wire_name()).collect();
                set_list(&dispatch, "hub.order", names);
            }
        });
    }
}

/// apply a merged effective config: the now-playing family
/// re-wires live, the shared list state updates, the mirror sync
/// re-pushes — called from the dispatch (set + reset) and at
/// startup
pub fn apply_effective(
    ui: &Mestro,
    handle: &Handle,
    np_state: &Rc<RefCell<NpState>>,
    effective: &config::Config,
) {
    nowplaying::wire(ui, effective);
    *np_state.borrow_mut() = NpState::from(effective);
    sync(ui, handle, &np_state.borrow());
}

/// the startup push (before any edit)
pub fn sync_initial(ui: &Mestro, handle: &Handle, np_state: &Rc<RefCell<NpState>>) {
    sync(ui, handle, &np_state.borrow());
}

// ── VU settings ──

/// the settings-editable VU cycle order, shared between the
/// sync push and the structural editors here
#[derive(Debug, Clone)]
pub struct VuState {
    pub cycle: Vec<String>,
    pub info: std::collections::HashMap<String, Vec<String>>,
    pub info_overridden: std::collections::HashMap<String, bool>,
    pub options: std::collections::HashMap<String, VuOptionState>,
}

#[derive(Debug, Clone, Default)]
pub struct VuOptionState {
    glow: f32,
    glow_overridden: bool,
    meters: bool,
    meters_overridden: bool,
    gradient: bool,
    gradient_overridden: bool,
}

impl VuState {
    pub fn from(config: &config::Config) -> Self {
        let fields = |items: &[config::MetadataField]| {
            items
                .iter()
                .map(|f| format!("{f:?}").to_lowercase())
                .collect()
        };
        let info = [
            ("night", fields(&config.vu.night.info)),
            ("cream", fields(&config.vu.cream.info)),
            ("round", fields(&config.vu.round.info)),
            ("spectrum", fields(&config.vu.spectrum.info)),
            ("bars", fields(&config.vu.bars.info)),
        ]
        .into_iter()
        .map(|(name, fields)| (name.to_owned(), fields))
        .collect();
        Self {
            cycle: config.vu.cycle.clone(),
            info,
            info_overridden: Default::default(),
            options: Default::default(),
        }
    }
}

fn set_vu_info(ui: &Mestro, fields: &[String]) {
    let all = ["title", "artist", "codec", "quality", "time"];
    let inactive: Vec<String> = all
        .iter()
        .filter(|f| !fields.iter().any(|v| v == **f))
        .map(|f| f.to_string())
        .collect();
    let model = |items: &[String]| {
        slint::ModelRc::new(slint::VecModel::from(
            items
                .iter()
                .map(|s| s.as_str().into())
                .collect::<Vec<slint::SharedString>>(),
        ))
    };
    ui.set_vu_info_order(model(fields));
    ui.set_vu_info_inactive(model(&inactive));
}

/// which skins expose editable options in the VU settings
fn vu_has_options(name: &str) -> bool {
    matches!(
        name,
        "night" | "cream" | "round" | "wall-light" | "spectrum" | "bars"
    )
}

/// push the VU list model and per-skin option state. The list
/// is the cycle order followed by inactive skins; previews come
/// from the VU module's render-once cache.
pub fn sync_vu(
    ui: &Mestro,
    handle: &Handle,
    effective: &config::Config,
    vui: &crate::vu::wire::Vui,
    vu_state: &Rc<RefCell<VuState>>,
) {
    let is = |k: &str| handle.is_overridden(k);
    // the per-skin options (glow, info, meters/gradient) reach
    // the live faceplate: glow lands on the attached instance
    // now, the rest rides the next tick's push
    vui.apply_options(effective);
    let edit = ui.get_vu_edit_name().to_string();
    if !edit.is_empty() {
        if let Some(image) = vui.refresh_settings_preview(&edit, effective) {
            ui.set_vu_edit_preview(image);
        }
    }
    let previews = vui.previews();
    // update shared state for the structural editors
    let mut state = VuState::from(effective);
    for name in state.info.keys() {
        state
            .info_overridden
            .insert(name.clone(), is(&format!("vu.{name}.info")));
    }
    for (name, glow, meters, gradient) in [
        ("night", 0.0, false, false),
        ("cream", effective.vu.cream.glow, false, false),
        ("round", effective.vu.round.glow, false, false),
        ("wall-light", effective.vu.wall_light.glow, false, false),
        (
            "spectrum",
            0.0,
            effective.vu.spectrum.meters,
            effective.vu.spectrum.gradient,
        ),
        (
            "bars",
            0.0,
            effective.vu.bars.meters,
            effective.vu.bars.gradient,
        ),
    ] {
        state.options.insert(
            name.into(),
            VuOptionState {
                glow: glow as f32,
                glow_overridden: is(&format!("vu.{name}.glow")),
                meters,
                meters_overridden: is(&format!("vu.{name}.meters")),
                gradient,
                gradient_overridden: is(&format!("vu.{name}.gradient")),
            },
        );
    }
    *vu_state.borrow_mut() = state;
    // the list: cycle order first, then inactive skins
    let cycle = &effective.vu.cycle;
    let mut list: Vec<crate::VuListEntry> = Vec::new();
    for name in cycle {
        if let Some(p) = previews.iter().find(|p| p.name.as_str() == name.as_str()) {
            list.push(crate::VuListEntry {
                name: p.name.clone(),
                active: true,
                has_options: vu_has_options(name),
                preview: p.image.clone(),
            });
        }
    }
    for p in previews {
        if !cycle.iter().any(|c| c.as_str() == p.name.as_str()) {
            list.push(crate::VuListEntry {
                name: p.name.clone(),
                active: false,
                has_options: vu_has_options(p.name.as_str()),
                preview: p.image.clone(),
            });
        }
    }
    ui.set_vu_list(slint::ModelRc::new(slint::VecModel::from(list)));

    // per-skin option state for the level-2 page
    if !edit.is_empty() {
        let glow_key = format!("vu.{edit}.glow");
        let info_key = format!("vu.{edit}.info");
        let meters_key = format!("vu.{edit}.meters");
        let grad_key = format!("vu.{edit}.gradient");
        let glow = match edit.as_str() {
            "cream" => effective.vu.cream.glow,
            "round" => effective.vu.round.glow,
            "wall-light" => effective.vu.wall_light.glow,
            _ => 0.0,
        };
        ui.set_vu_glow_value(glow as f32);
        ui.set_vu_glow_overridden(is(&glow_key));

        if let Some(info) = vu_state.borrow().info.get(&edit) {
            set_vu_info(ui, info);
        } else {
            set_vu_info(ui, &[]);
        }
        ui.set_vu_info_overridden(is(&info_key));

        let (meters, grad) = match edit.as_str() {
            "spectrum" => (effective.vu.spectrum.meters, effective.vu.spectrum.gradient),
            "bars" => (effective.vu.bars.meters, effective.vu.bars.gradient),
            _ => (false, false),
        };
        ui.set_vu_meters_value(meters);
        ui.set_vu_meters_overridden(is(&meters_key));
        ui.set_vu_gradient_value(grad);
        ui.set_vu_gradient_overridden(is(&grad_key));
    }
}

/// wire the VU settings callbacks. Structural edits (cycle
/// reorder/membership) and info-line chips go through the
/// shared dispatch; per-skin scalars ride the generic path.
pub fn wire_vu(
    ui: &Mestro,
    dispatch: &Dispatch,
    vu_state: &Rc<RefCell<VuState>>,
    ui_weak: &slint::Weak<Mestro>,
    nav: &Navigation,
) {
    // cycle reorder
    {
        let dispatch = dispatch.clone();
        let vu_state = vu_state.clone();
        ui.on_vu_move(move |name, dir| {
            let mut cycle = vu_state.borrow().cycle.clone();
            if let Some(pos) = cycle.iter().position(|c| c.as_str() == name.as_str()) {
                let target = pos as i32 + dir;
                if target >= 0 && target < cycle.len() as i32 {
                    cycle.swap(pos, target as usize);
                    let names: Vec<toml::Value> = cycle
                        .iter()
                        .map(|s| toml::Value::String(s.clone()))
                        .collect();
                    dispatch(&Command::SetSetting {
                        key: "vu.cycle".into(),
                        value: toml::Value::Array(names),
                    });
                }
            }
        });
    }
    // cycle membership toggle
    {
        let dispatch = dispatch.clone();
        let vu_state = vu_state.clone();
        ui.on_vu_toggle_cycle(move |name| {
            let mut cycle = vu_state.borrow().cycle.clone();
            if let Some(pos) = cycle.iter().position(|c| c.as_str() == name.as_str()) {
                cycle.remove(pos);
            } else {
                cycle.push(name.to_string());
            }
            let names: Vec<toml::Value> = cycle
                .iter()
                .map(|s| toml::Value::String(s.clone()))
                .collect();
            dispatch(&Command::SetSetting {
                key: "vu.cycle".into(),
                value: toml::Value::Array(names),
            });
        });
    }
    // open options (level 2)
    {
        let ui_weak = ui_weak.clone();
        let nav = nav.clone();
        let vu_state = vu_state.clone();
        ui.on_vu_open_options(move |name| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_vu_edit_name(name.clone());
                let list = ui.get_vu_list();
                if let Some(entry) = (0..list.row_count())
                    .filter_map(|i| list.row_data(i))
                    .find(|entry| entry.name == name)
                {
                    ui.set_vu_edit_preview(entry.preview);
                }
                let state = vu_state.borrow();
                set_vu_info(
                    &ui,
                    state
                        .info
                        .get(name.as_str())
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                );
                ui.set_vu_info_overridden(
                    *state.info_overridden.get(name.as_str()).unwrap_or(&false),
                );
                let options = state
                    .options
                    .get(name.as_str())
                    .cloned()
                    .unwrap_or_default();
                ui.set_vu_glow_value(options.glow);
                ui.set_vu_glow_overridden(options.glow_overridden);
                ui.set_vu_meters_value(options.meters);
                ui.set_vu_meters_overridden(options.meters_overridden);
                ui.set_vu_gradient_value(options.gradient);
                ui.set_vu_gradient_overridden(options.gradient_overridden);
                drop(state);
                nav.push_sub(&ui, 9);
            }
        });
    }
    // info-line membership toggle
    {
        let dispatch = dispatch.clone();
        let ui_weak = ui_weak.clone();
        ui.on_vu_toggle_info(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let edit = ui.get_vu_edit_name().to_string();
            let key = format!("vu.{edit}.info");
            // read current from the UI model
            let cur: Vec<String> = {
                let m = ui.get_vu_info_order();
                (0..m.row_count())
                    .map(|i| m.row_data(i).unwrap_or_default().to_string())
                    .collect()
            };
            let mut updated = cur;
            if let Some(pos) = updated.iter().position(|field| field == name.as_str()) {
                updated.remove(pos);
            } else {
                updated.push(name.to_string());
            }
            let values: Vec<toml::Value> = updated
                .iter()
                .map(|s| toml::Value::String(s.clone()))
                .collect();
            dispatch(&Command::SetSetting {
                key,
                value: toml::Value::Array(values),
            });
        });
    }
    // info-line reorder
    {
        let dispatch = dispatch.clone();
        let ui_weak = ui_weak.clone();
        ui.on_vu_move_info(move |name, dir| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let edit = ui.get_vu_edit_name().to_string();
            let key = format!("vu.{edit}.info");
            let mut cur: Vec<String> = {
                let m = ui.get_vu_info_order();
                (0..m.row_count())
                    .map(|i| m.row_data(i).unwrap_or_default().to_string())
                    .collect()
            };
            if let Some(pos) = cur.iter().position(|c| c.as_str() == name.as_str()) {
                let target = pos as i32 + dir;
                if target >= 0 && target < cur.len() as i32 {
                    cur.swap(pos, target as usize);
                    let values: Vec<toml::Value> =
                        cur.iter().map(|s| toml::Value::String(s.clone())).collect();
                    dispatch(&Command::SetSetting {
                        key,
                        value: toml::Value::Array(values),
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod server_host_tests {
    use super::server_host;

    #[test]
    fn names_the_host_only() {
        assert_eq!(server_host("ws://ma.example.com:8095/ws"), "ma.example.com");
        assert_eq!(server_host("ws://192.168.1.50:8095/ws"), "192.168.1.50");
        assert_eq!(server_host("wss://ma.example.com/ws"), "ma.example.com");
        assert_eq!(server_host(""), "");
    }
}
