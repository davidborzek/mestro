mod app;
mod restart;

use app::clock::StandbyClock;
use app::dispatch::Dispatch;
use app::theme::{self, PaletteCell};
use clap::Parser;
use mestro::display::Display;
use mestro::library::Library;
use mestro::player::cover::{CoverSource, HttpCovers};
use mestro::player::shell::{Shared, Shell};
use mestro::player::state::{self, PlayerUpdate};
use mestro::providers::ma::MaProvider;
use mestro::providers::SourceCommand;
use mestro::settings::{Handle, NpState, VuState};
use mestro::{
    library::search::Search, player::nowplaying, player::queue, ui::navigation, ui::osd, vu, Mestro,
};
use mestro_ipc::command::Command;
use parking_lot::{Mutex, RwLock};
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about = "mestro - a touch player display for Music Assistant")]
struct Args {
    /// window size as WIDTHxHEIGHT logical pixels
    #[arg(long, value_parser = mestro::config::parse_size)]
    size: Option<(f32, f32)>,
    /// fullscreen window
    #[arg(long)]
    fullscreen: bool,
    /// 5in DSI panel profile: 573x344 logical pixels (about
    /// 10.9x6.5 cm at the panel's 117 lpi)
    #[arg(long, conflicts_with = "size")]
    kiosk: bool,
    /// configuration file (default: $MESTRO_CONFIG or XDG config path)
    #[arg(long, env = "MESTRO_CONFIG")]
    config: Option<std::path::PathBuf>,
    /// run the UI without a provider (dev)
    #[arg(long)]
    minimal: bool,
    /// inject mock track data (dev)
    #[arg(long)]
    mock: bool,
    /// DRM output (connector) for the linuxkms backend, e.g.
    /// DSI-1 or HDMI-A-1 — "list" prints the available outputs
    /// (maps onto SLINT_DRM_OUTPUT; a second process with its own
    /// --output drives the second screen)
    #[cfg(feature = "linuxkms")]
    #[arg(long, env = "MESTRO_OUTPUT")]
    output: Option<String>,
    #[command(subcommand)]
    sub: Option<Sub>,
}

#[derive(clap::Subcommand)]
enum Sub {
    /// control the running instance over the local ctl socket
    Ctl {
        #[command(subcommand)]
        cmd: CtlCmd,
    },
}

/// the ctl surface: pure control/navigation. Settings writes
/// stay with the settings UI and the web endpoint
#[derive(clap::Subcommand)]
enum CtlCmd {
    /// toggle play/pause
    PlayPause,
    /// next track
    NextTrack,
    /// previous track
    PrevTrack,
    /// volume up one step
    VolumeUp,
    /// volume down one step
    VolumeDown,
    /// one level back in the navigation stack
    Back,
    /// straight to the hub
    Home,
    /// open the now-playing layer
    NowPlaying,
    /// open the queue
    Queue,
    /// open the VU faceplate
    Vu,
    /// power the player off (standby)
    StandbyOn,
    /// wake the player
    StandbyOff,
    /// flip standby/awake
    StandbyToggle,
    /// next VU view in the cycle
    CycleForward,
    /// previous VU view in the cycle
    CycleBack,
}

impl From<CtlCmd> for mestro_ipc::command::Command {
    fn from(cmd: CtlCmd) -> Self {
        use mestro_ipc::command::Command;
        match cmd {
            CtlCmd::PlayPause => Command::PlayPause,
            CtlCmd::NextTrack => Command::NextTrack,
            CtlCmd::PrevTrack => Command::PrevTrack,
            CtlCmd::VolumeUp => Command::VolumeUp,
            CtlCmd::VolumeDown => Command::VolumeDown,
            CtlCmd::Back => Command::Back,
            CtlCmd::Home => Command::Home,
            CtlCmd::NowPlaying => Command::NowPlaying,
            CtlCmd::Queue => Command::Queue,
            CtlCmd::Vu => Command::Vu,
            CtlCmd::StandbyOn => Command::StandbyOn,
            CtlCmd::StandbyOff => Command::StandbyOff,
            CtlCmd::StandbyToggle => Command::StandbyToggle,
            CtlCmd::CycleForward => Command::CycleForward,
            CtlCmd::CycleBack => Command::CycleBack,
        }
    }
}

fn run_ui(
    ui: Mestro,
    restart: &Cell<bool>,
    wifi_stop: &Cell<Option<mestro::remote::web::WifiShutdown>>,
) {
    // a dead event loop leaves nothing to recover into — this is
    // the one place a UI failure turns into an exit code
    if let Err(e) = ui.run() {
        tracing::error!("ui: event loop failed: {e}");
        std::process::exit(1);
    }
    drop(ui);
    if restart.get() {
        // exec keeps the wifi children alive with their handles
        // lost — the manager must confirm they are stopped (the
        // graceful stop spares a working STA) before the process is
        // replaced. No confirmation in time: kill the tracked
        // children by handle (we know them) — the next start's
        // ensure_sta rebuilds the STA side
        if let Some(handle) = wifi_stop.take() {
            if !handle.wait_stopped(Duration::from_secs(2)) {
                tracing::warn!(
                    "restart: the wifi manager did not stop in time — killing the tracked children"
                );
                handle.stop_tracked();
            }
        }
        #[cfg(feature = "timezone")]
        let timezone = mestro_os::timezone::take_pending_zone();
        #[cfg(not(feature = "timezone"))]
        let timezone = None::<String>;
        restart::restart(timezone.as_deref());
    }
}

fn main() {
    // mestro=info by default; RUST_LOG overrides per module
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mestro=info")),
        )
        .init();

    let args = Args::parse();
    // `mestro ctl …`: one roundtrip to the running instance,
    // no UI (ipc/client.rs)
    if let Some(Sub::Ctl { cmd }) = args.sub {
        std::process::exit(mestro::remote::ipc::client(Command::from(cmd).as_str()));
    }
    // must land before the Slint platform initializes — the
    // linuxkms backend reads SLINT_DRM_OUTPUT exactly once
    #[cfg(feature = "linuxkms")]
    if let Some(output) = &args.output {
        std::env::set_var("SLINT_DRM_OUTPUT", output);
    }

    let (config, base_tree, overlay, base_path) =
        mestro::config::load_parts(args.config.as_deref()).unwrap_or_else(|e| {
            // a kiosk must come up — on the defaults, with the error
            // in the settings, never a black screen
            tracing::error!("config: {e} — starting on the defaults");
            (
                mestro::config::Config::default(),
                toml::Value::Table(Default::default()),
                Default::default(),
                None,
            )
        });
    // the now-playing lists the settings UI can edit (chips)
    let np_state = Rc::new(RefCell::new(NpState::from(&config)));
    // the settings handle: UI edits land here — base ⊕ overlay,
    // debounced persist (docs/configuration.md)
    let settings = Rc::new(Handle::new(overlay, base_tree, base_path));
    let restart = Rc::new(Cell::new(false));
    // the wifi manager's stop handle — set when wire_remote spawns
    // it, drained on the restart path (exec keeps children alive)
    let wifi_stop = Cell::new(None::<mestro::remote::web::WifiShutdown>);

    apply_scale(&config);
    let display = start_display(&config);
    let ui = Mestro::new().unwrap_or_else(|e| {
        eprintln!("cannot open the UI: {e}");
        std::process::exit(2);
    });
    wire_window(&ui, &args, &config, &settings, &restart, &display);
    let clock = StandbyClock::new(&ui, &config.clock);
    wire_static_ui(&ui, &config);

    let ui_weak = ui.as_weak();
    let nav = navigation::Navigation::new();
    app::debug::apply_startup_env(&ui, &nav);

    if args.minimal {
        run_ui(ui, &restart, &wifi_stop);
        return;
    }

    let shared: Shared = Arc::new(Mutex::new(None));
    let library = Library::new();
    let (update_tx, update_rx) = tokio::sync::mpsc::unbounded_channel::<PlayerUpdate>();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<SourceCommand>();

    // navigation owns the back stack and the now-playing overlay;
    // entering the library triggers its root fetch
    nav.wire(&ui, &cmd_tx, library.clone());
    // the search session: query state + debounce live in the module
    let search = Search::new();
    search.wire(
        &ui,
        &cmd_tx,
        &nav,
        &library,
        config.ui.auto_open_now_playing,
    );
    let shell = Shell::new(
        shared.clone(),
        update_tx,
        CoverSource {
            fetcher: Arc::new(HttpCovers),
            variant: config.theme.variant.raw(),
        },
    );
    nowplaying::transport::wire(&ui, &cmd_tx, &shell);
    queue::wire(&ui, &cmd_tx, &nav, &library);

    if args.mock {
        let _mock = app::debug::mock_player(&ui);
        app::display::sync(&ui, &settings, &config, &display);
        // the settings pages read these (the real startup sync
        // below is never reached in mock mode)
        mestro::settings::sync_theme_ui(&ui, &settings, &config);
        run_ui(ui, &restart, &wifi_stop);
        return;
    }

    // the cover palette — extracted + M3-tonalized in the cover
    // fetch, stored here, pushed to the UI/skins by the tick and
    // the update loop ([theme] palette = "cover")
    let palette: PaletteCell = Rc::new(RefCell::new(None));
    // the last quality values — the settings dispatch re-renders
    // the quality line from this cache, no player event needed
    let quality_vals = Rc::new(RefCell::new(nowplaying::QualityVals::default()));
    // [theme] palette = "cover": the update loop follows the
    // artwork — a Cell so the settings dispatch can flip it live
    let theme_cover = Rc::new(Cell::new(matches!(
        config.theme.palette,
        mestro::config::ThemePalette::Cover
    )));
    let standby = standby_hook(&ui, &nav, &clock, config.ui.auto_open_now_playing);
    // the provider connection as shared live state: settings edits
    // (kiosk, wizard, ws) rewrite it and rewire the live session —
    // no app restart needed
    let ma_cfg = Arc::new(RwLock::new(
        mestro::providers::ma::Config::from_music_assistant(&config.music_assistant),
    ));
    // the remote-control push pipeline: the hub is fed by the
    // update loop (UI thread) and the view watcher (web thread).
    // The view state arrives through the watch: the binary
    // publishes a snapshot whenever a view-relevant UI property
    // changes (the slint `changed` hooks fire view-changed), the
    // pipeline reacts — change-driven, no poll
    let events = mestro::remote::web::Events::new();
    let (views_tx, views_rx) = tokio::sync::watch::channel(mestro::remote::web::UiView::default());
    let push =
        mestro::remote::web::Push::new(events.clone(), &config.music_assistant.player, views_rx);
    spawn_update_loop(
        &ui,
        update_rx,
        UpdateDeps {
            palette: palette.clone(),
            theme_cover: theme_cover.clone(),
            np_state: np_state.clone(),
            quality_vals: quality_vals.clone(),
        },
        standby,
        &push,
    );

    // library wiring: all browse callbacks + the slint-thread
    // drain for replies, covers and late subject images
    library.wire(&ui, &cmd_tx, &nav, config.ui.auto_open_now_playing);
    library.spawn_drain(ui_weak.clone());
    // the VU overlay: view registry, skin activation, the meter
    // tick — all in the vu module
    let vui = vu::wire::Vui::new(&config, &nav, palette.clone());
    vui.wire(&ui, ui_weak.clone());
    let vu_state = Rc::new(RefCell::new(VuState::from(&config)));
    // the [osd] placement + show/hide wiring
    osd::wire(&ui);

    // the abstract control channel (mestro_ipc::command): keyboard, ctl
    // socket, web endpoint and the settings page all speak it
    let dispatch = Rc::new(Dispatch {
        ui: ui_weak.clone(),
        nav: nav.clone(),
        osd: config.osd.clone(),
        settings: settings.clone(),
        np_state: np_state.clone(),
        quality_vals,
        theme_cover,
        palette,
        display: display.clone(),
        clock: clock.clone(),
        vui: vui.clone(),
        vu_state: vu_state.clone(),
        ma_cfg: ma_cfg.clone(),
        cmd_tx: cmd_tx.clone(),
        events: events.clone(),
        views: views_tx.clone(),
    });
    let dispatch_fn: Rc<dyn Fn(&Command)> = {
        let dispatch = dispatch.clone();
        Rc::new(move |cmd: &Command| dispatch.run(cmd))
    };
    // the settings page wiring: typed editors, list arithmetic,
    // mirror sync — all in the settings module
    mestro::settings::wire(&ui, &settings, &np_state, &dispatch_fn);
    mestro::settings::wire_vu(&ui, &dispatch_fn, &vu_state, &ui_weak, &nav);
    // before wire_remote: it reads the display-supported prop
    // for the hello capabilities
    app::display::sync(&ui, &settings, &config, &display);
    let wifi_shutdown = wire_remote(
        &ui,
        &config,
        &settings,
        &dispatch_fn,
        &events,
        &push,
        &views_tx,
        &ma_cfg,
        &mestro::remote::web::PlayerCtx {
            shell: shell.clone(),
            cmd_tx: cmd_tx.clone(),
        },
        &vui,
        &clock,
    );
    wifi_stop.set(wifi_shutdown);
    wire_input(&ui, &config, &dispatch_fn);

    // overlay flags for the reset buttons — pure props, no impure
    // callbacks in bindings (slint forbids those)
    mestro::settings::sync_initial(&ui, &settings, &np_state);
    mestro::settings::sync_theme_ui(&ui, &settings, &config);
    mestro::settings::sync_connection(&ui, &config);
    mestro::settings::sync_vu(&ui, &settings, &config, &vui, &vu_state);
    mestro::settings::sync_overlay_state(&ui, &settings);
    announce_config_problems(&ui);

    start_ticker(&ui, &shell, &config.music_assistant.player, &clock);

    // provider session: retry loop + event dispatch live in the
    // providers module — the factory reads the shared connection
    // config per attempt, so settings edits take effect live
    mestro::providers::session::spawn(
        move || MaProvider::new(ma_cfg.read().clone()),
        cmd_rx,
        shell,
        library.feed_tx().clone(),
        ui_weak.clone(),
    );

    run_ui(ui, &restart, &wifi_stop);
}

/// [window] scale: render everything larger on physically small
/// panels. Must happen before the slint backend initializes
/// (Mestro::new) — slint reads the env at backend setup
fn apply_scale(config: &mestro::config::Config) {
    if (config.window.scale - 1.0).abs() > f32::EPSILON {
        if !(0.5..=3.0).contains(&config.window.scale) {
            eprintln!(
                "invalid [window] scale {} — must be 0.5..=3.0",
                config.window.scale
            );
            std::process::exit(2);
        }
        std::env::set_var("SLINT_SCALE_FACTOR", config.window.scale.to_string());
    }
}

/// [display]: best-effort backlight (panel, laptop, none in a VM —
/// values are ignored then)
fn start_display(config: &mestro::config::Config) -> Rc<Display> {
    let display = Rc::new(Display::probe_with(config.display.backlight));
    display.configure(config.display.brightness, config.display.standby_brightness);
    if display.supported() {
        display.apply_now();
    } else if let Some(percent) = config.display.brightness {
        eprintln!("display: no backlight found — ignoring [display] brightness = {percent}");
    }
    display
}

/// window size, fullscreen, the standby backlight and the restart
fn wire_window(
    ui: &Mestro,
    args: &Args,
    config: &mestro::config::Config,
    settings: &Rc<Handle>,
    restart: &Rc<Cell<bool>>,
    display: &Rc<Display>,
) {
    // CLI over config: --kiosk is a preset, --size beats both,
    // --fullscreen is a sticky OR
    let (size_w, size_h) = if args.kiosk {
        (573.0, 344.0)
    } else {
        #[expect(
            clippy::expect_used,
            reason = "[window] size is validated during config load"
        )]
        args.size
            .unwrap_or(mestro::config::parse_size(&config.window.size).expect("validated in load"))
    };
    let fullscreen = args.fullscreen || config.window.fullscreen;
    ui.set_start_w(size_w);
    ui.set_start_h(size_h);
    ui.set_fullscreen(fullscreen);
    // compact hub rows on short windows (kiosk 344, custom --size);
    // fullscreen windows are large by definition — unless the UI is
    // scaled up, which shrinks the logical size
    let scale = config.window.scale.max(0.1);
    ui.set_hub_compact(!fullscreen && size_h / scale < 400.0);
    ui.set_volume(100);

    // standby clock shows/hides → dim/restore the backlight
    ui.on_standby_changed({
        let display = display.clone();
        move |standby| display.set_standby(standby)
    });
    // `changed` only fires on transitions: booting straight into
    // standby (player off) never flips it. Settle once the first
    // transport state had time to arrive
    Timer::single_shot(Duration::from_secs(3), {
        let display = display.clone();
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                display.set_standby(ui.get_standby_showing());
            }
        }
    });
    ui.on_restart_app({
        let settings = settings.clone();
        let restart = restart.clone();
        let weak = ui.as_weak();
        move || {
            if let Err(error) = settings.flush() {
                tracing::error!("restart: could not save settings: {error}");
                if let Some(ui) = weak.upgrade() {
                    ui.invoke_show_toast("Could not save settings".into());
                }
                return;
            }
            if let Err(error) = slint::quit_event_loop() {
                tracing::error!("restart: could not stop UI: {error}");
                return;
            }
            restart.set(true);
        }
    });
}

/// config values the UI reads once: backdrop, rail, keyboard
/// layout, version, now-playing layout and the hub entries
fn wire_static_ui(ui: &Mestro, config: &mestro::config::Config) {
    ui.set_backdrop_enabled(config.background.blurred_cover);
    // [ui] rail: dark bezel or translucent glass (the backdrop runs
    // under the rail and shows through)
    ui.set_rail_style(theme::rail_style(config.ui.rail));
    // [ui] osk-layout: the OskLayouts registry key the search
    // keyboard renders; unknown names warn here and the slint
    // registry falls back to qwertz-de
    if !mestro::config::OSK_LAYOUTS.contains(&config.ui.osk_layout.as_str()) {
        tracing::warn!(
            "unknown [ui] osk-layout '{}', falling back to qwertz-de",
            config.ui.osk_layout
        );
    }
    ui.set_osk_layout(config.ui.osk_layout.as_str().into());
    ui.set_app_version(mestro::app_version().into());
    nowplaying::wire(ui, config);
    ui.set_hub_layout(match config.hub.layout {
        mestro::config::HubLayout::Cards => 0,
        mestro::config::HubLayout::List => 1,
    });
    ui.set_hub_entries(slint::ModelRc::new(slint::VecModel::from(
        mestro::ui::hub_entries(config),
    )));
}

/// the standby side of every player update: a new cover feeds the
/// cover background, and leaving standby picks the view — a tap on
/// the clock returns to the page the user left, a remote start (MA
/// web UI, Home Assistant) follows the music into now playing when
/// [ui] auto_open_now_playing is on
fn standby_hook(
    ui: &Mestro,
    nav: &navigation::Navigation,
    clock: &Rc<StandbyClock>,
    auto_open: bool,
) -> impl Fn(&Mestro, &PlayerUpdate) {
    let wake = Rc::new(mestro::standby::StandbyWake::new(auto_open));
    ui.on_standby_power_on({
        let wake = wake.clone();
        let weak = ui.as_weak();
        move || {
            wake.local_wake();
            if let Some(ui) = weak.upgrade() {
                ui.invoke_set_power(true);
            }
        }
    });
    let nav = nav.clone();
    let clock = clock.clone();
    move |ui, update| {
        if update.cover.is_some() {
            clock.cover_changed();
        }
        // an external source has no queue: the view closes, the button dies
        nav.set_queue_unavailable(ui, update.state.external.is_some());
        wake.observe(ui, &nav);
    }
}

/// what the update loop renders from/into — one bundle so the
/// loop signature stays at one glance
struct UpdateDeps {
    palette: PaletteCell,
    theme_cover: Rc<Cell<bool>>,
    np_state: Rc<RefCell<NpState>>,
    quality_vals: Rc<RefCell<nowplaying::QualityVals>>,
}

/// PlayerUpdates flow from the state machine into the slint event
/// loop: now playing, the cover backdrop, toasts, the
/// cover-following accent and the remote-control state frames
fn spawn_update_loop(
    ui: &Mestro,
    mut update_rx: tokio::sync::mpsc::UnboundedReceiver<PlayerUpdate>,
    deps: UpdateDeps,
    standby: impl Fn(&Mestro, &PlayerUpdate) + 'static,
    push: &mestro::remote::web::Push,
) {
    let ui_weak = ui.as_weak();
    let UpdateDeps {
        palette,
        theme_cover,
        np_state,
        quality_vals,
    } = deps;
    let push = push.clone();
    #[expect(
        clippy::expect_used,
        reason = "setup runs on the slint event loop thread"
    )]
    let spawned = slint::spawn_local(async move {
        while let Some(update) = update_rx.recv().await {
            let Some(ui) = ui_weak.upgrade() else {
                continue;
            };
            *quality_vals.borrow_mut() = nowplaying::QualityVals::from_state(&update.state);
            nowplaying::apply(&ui, &update, &palette, &np_state.borrow().quality);
            standby(&ui, &update);
            // toasts + remote volume ride the update channel; [osd]
            // gates them per context
            osd::show_gated(&ui, &update);
            // [theme] palette = "cover": the whole UI accent follows
            // the artwork via the M3 tonal scheme
            if theme_cover.get() {
                let (accent, _, danger) = theme::cover_accent(&palette);
                ui.invoke_set_accent(accent, danger);
            }
            // the remote-control state frame: coalesced inside the
            // hub, position-only updates never push
            push.update(&update.state);
            if update.cover.is_some() {
                push.cover(
                    update.cover.as_ref(),
                    update.bg.as_ref(),
                    update.palette.as_ref(),
                );
            }
        }
    })
    .expect("spawn on the slint event loop");
    // detach: the task outlives this setup call
    std::mem::forget(spawned);
}

/// the embedded web endpoint (setup wizard + remote channel, LAN,
/// pairing PIN in the URL hash — docs/remote.md) and the local
/// ctl socket. `info` first, `wire`, then `serve` — a request
/// between serve and wire would hit an unset `web-op` handler
#[allow(clippy::too_many_arguments)]
fn wire_remote(
    ui: &Mestro,
    config: &mestro::config::Config,
    settings: &Rc<Handle>,
    dispatch: &Rc<dyn Fn(&Command)>,
    events: &mestro::remote::web::Events,
    push: &mestro::remote::web::Push,
    views: &tokio::sync::watch::Sender<mestro::remote::web::UiView>,
    ma_cfg: &Arc<RwLock<mestro::providers::ma::Config>>,
    player: &mestro::remote::web::PlayerCtx,
    vui: &vu::wire::Vui,
    clock: &Rc<app::clock::StandbyClock>,
) -> Option<mestro::remote::web::WifiShutdown> {
    // display-supported is up (app::display::sync ran first);
    // the trust boundary needs the hotspot's subnet (`[wifi]
    // ap-address`) — the peer check derives from it
    let web_settings = mestro::remote::web::WebSettings {
        enabled: config.remote.enabled,
        port: config.remote.port,
    };
    let (web_info, web_pin) = mestro::remote::web::info(
        &web_settings,
        &config.music_assistant.player,
        &mestro::app_version(),
        ui.get_display_supported(),
        config.wifi.ap_net(),
    );
    // the settings-preview thumbnails: rendered on this (UI)
    // thread from the compiled registries, on demand through the
    // bridge; the effective config keeps them in sync with edits
    let previews: mestro::remote::web_ui::PreviewRenderer = {
        let vui = vui.clone();
        let clock = clock.clone();
        let settings = settings.clone();
        Rc::new(move |kind: &str, name: &str| match kind {
            "vu" => {
                let Ok(effective) = settings.effective() else {
                    return None;
                };
                vui.preview_png(name, &effective)
            }
            "clock" => clock.preview_png(name),
            _ => None,
        })
    };
    mestro::remote::web_ui::wire(ui, settings, dispatch, &web_info, player, &previews);
    #[cfg(all(feature = "audio-output", feature = "wifi-setup"))]
    let name_suffix = mestro_os::wifi::name_suffix(
        &mestro_os::wifi::backend::Roots::default(),
        &config.wifi.interface,
        &web_info.id,
    );
    #[cfg(all(feature = "audio-output", not(feature = "wifi-setup")))]
    let name_suffix = mestro::remote::web::device_suffix("", &web_info.id);
    // squeezelite's env file (the audio settings, else derived values),
    // written on a thread of its own (the wait for a slow DAC and the
    // restart take seconds)
    #[cfg(feature = "audio-output")]
    mestro_os::audio::squeezelite::start(
        &name_suffix,
        &config.audio.settings(),
        &config.music_assistant.url,
    );
    // the wifi manager thread (feature `wifi-setup`): AP-mode flag,
    // the hotspot panel through the view sink and the ops channel.
    // Without the feature every wifi op answers "unavailable"
    #[cfg(feature = "wifi-setup")]
    let (wifi, wifi_stop) = {
        let shutdown = mestro::remote::web::WifiShutdown::default();
        // the hotspot panel: the manager publishes pure data, the
        // binary renders it — the slint properties and the QR image
        // stay on the UI thread
        let ui_weak = ui.as_weak();
        let view: mestro_os::wifi::HotspotSink = std::sync::Arc::new(move |panel| {
            let weak = ui_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                ui.set_hotspot_active(panel.active);
                ui.set_hotspot_reason(match panel.reason {
                    mestro_os::wifi::ApReason::Setup => mestro::HotspotReason::Setup,
                    mestro_os::wifi::ApReason::Fallback => mestro::HotspotReason::Fallback,
                });
                ui.set_hotspot_ssid(panel.ssid.clone().into());
                ui.set_hotspot_saved_ssid(panel.saved_ssid.clone().into());
                ui.set_hotspot_portal(panel.portal_url.clone().into());
                if panel.active {
                    ui.set_hotspot_qr(mestro::remote::qr::image(
                        &format!("WIFI:T:nopass;S:{};;", panel.ssid),
                        260,
                    ));
                }
            });
        });
        // the hotspot's URLs are the web endpoint's concern — the
        // manager never formats one: the RFC 8910 portal URI DHCP
        // option 114 advertises and the setup address the overlay
        // names, both with the web port spelled out unless it is 80
        let port = mestro::remote::web::url_port_suffix(config.remote.port);
        let portal = mestro_os::wifi::Portal {
            uri: format!("http://{}{port}/setup/wifi", config.wifi.ap_net().ip),
            address: format!("mestro.setup{port}"),
        };
        let channel = mestro_os::wifi::spawn(
            web_info.ap_mode.clone(),
            web_info.mdns_gate.clone(),
            view,
            config.wifi.clone(),
            web_info.id.clone(),
            portal,
            shutdown.clone(),
        );
        (channel, Some(shutdown))
    };
    #[cfg(not(feature = "wifi-setup"))]
    let (wifi, wifi_stop) = (mestro::remote::web::WifiChannel::unavailable(), None);
    // the view publisher: the slint `changed` hooks on the
    // view-relevant properties fire view-changed, every one
    // samples the props into the watch the push pipeline reads
    {
        let views = views.clone();
        let ma_cfg = ma_cfg.clone();
        let weak = ui.as_weak();
        ui.on_view_changed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let _ = views.send_replace(mestro::remote::web_ui::view_of(&ui, &ma_cfg.read().ws_url));
        });
    }
    // the initial snapshot: the first frame and the view watcher
    // both start from the current props
    let _ = views.send_replace(mestro::remote::web_ui::view_of(ui, &ma_cfg.read().ws_url));
    // the snapshot exists before the first socket can connect:
    // every paired WS starts with a state frame
    push.publish_snapshot();
    mestro::remote::web::serve(
        &web_settings,
        web_info.clone(),
        std::sync::Arc::new(mestro::remote::web_ui::SlintUiPort::new(ui)),
        std::sync::Arc::new(mestro::remote::web_ui::MaAuthClient),
        web_pin,
        events.clone(),
        push.clone(),
        wifi,
    );
    // the local control socket: `mestro ctl <command>`
    mestro::remote::ipc::serve(ui.as_weak());
    // the connection page shows the wizard URL (incl. pairing PIN)
    // and the first-run overlay its QR code. The address follows
    // DHCP: the kiosk starts before the network is up, and leases
    // change
    let show_url = {
        let weak = ui.as_weak();
        let shown = RefCell::new(String::new());
        move || {
            let url = web_info.remote_url();
            let Some(ui) = weak.upgrade() else { return };
            if url.is_empty() || *shown.borrow() == url {
                return;
            }
            tracing::info!("web: wizard url: {url}");
            ui.set_settings_remote_url(url.clone().into());
            ui.set_first_run_qr(mestro::remote::qr::image(&url, 260));
            *shown.borrow_mut() = url;
        }
    };
    show_url();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(5), show_url);
    std::mem::forget(timer);
    wifi_stop
}

/// the text protocol (ctl socket, web) and the keyboard with its
/// configurable key map ([keyboard] overrides the defaults in
/// remote/keyboard.rs) both land in the dispatch
fn wire_input(ui: &Mestro, config: &mestro::config::Config, dispatch: &Rc<dyn Fn(&Command)>) {
    ui.on_control_command({
        let dispatch = dispatch.clone();
        move |cmd| {
            if let Some(cmd) = Command::parse(&cmd) {
                dispatch(&cmd);
            }
        }
    });
    let keymap = mestro::remote::keyboard::keymap(&config.keyboard);
    ui.on_key_event({
        let dispatch = dispatch.clone();
        move |text| {
            if let Some(cmd) = keymap.get(text.as_str()) {
                dispatch(cmd);
            }
        }
    });
}

/// config errors: a short toast at startup, the details sit in the
/// settings banner (or the config-error screen)
fn announce_config_problems(ui: &Mestro) {
    let count = mestro::config::problems().len();
    if count > 0 {
        ui.invoke_show_toast(
            if count == 1 {
                "Config: 1 setting ignored — see Settings".to_string()
            } else {
                format!("Config: {count} settings ignored — see Settings")
            }
            .into(),
        );
    }
}

/// the 500ms ticker: standby clock, hub greeting, the progress bar
/// and the loading dots. Mutation runs through the state machine,
/// rendering happens here
fn start_ticker(ui: &Mestro, shell: &Shell, player: &str, clock: &Rc<StandbyClock>) {
    let shell = shell.clone();
    let ui_weak = ui.as_weak();
    let hub_player = player.to_string();
    let clock = clock.clone();
    let timer = Timer::default();
    let mut tick: u32 = 0;
    timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
        if let Some(ui) = ui_weak.upgrade() {
            clock.tick(&ui);
            // hub greeting: time-of-day word + date · player — two
            // strings, change-tracking keeps it cheap
            let (greeting, sub) = mestro::standby::hub_lines(
                chrono::Local::now().naive_local(),
                mestro::standby::BUILD_YEAR,
                &hub_player,
            );
            ui.set_hub_greeting(greeting.into());
            ui.set_hub_sub(sub.into());
        }
        let Some((pos, dur, loading)) = shell.tick(std::time::Instant::now()) else {
            return;
        };
        if let Some(ui) = ui_weak.upgrade() {
            // re-sync loading every tick: a stale state snapshot in a
            // late PlayerUpdate (async cover fetch) must not stick.
            // The library header dots share the same opacity cycling
            let lib_loading = ui.get_lib_loading();
            ui.set_loading(loading);
            if loading || lib_loading {
                ui.set_loading_opacity([0.35, 0.65, 1.0][(tick % 3) as usize]);
            }
            if dur > 0 {
                ui.set_pos_label(state::mmss(pos).into());
                ui.set_dur_label(state::mmss(dur).into());
                ui.set_progress(pos as f32 / dur as f32);
            } else {
                // cleared queue / idle: the bar must not keep the
                // last track's leftovers
                ui.set_pos_label("".into());
                ui.set_dur_label("".into());
                ui.set_progress(0.0);
            }
        }
        tick += 1;
    });
    std::mem::forget(timer);
}
