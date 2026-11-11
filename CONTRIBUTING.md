# Contributing

## Layout

```text
src/
  main.rs, app/    the binary: wiring, command dispatch, ticker
  config/          TOML config, the settings overlay merge, lenient
                   parsing (a broken key is dropped and reported)
  settings/        the settings page: overlay handle, UI sync
  player/          what plays: state machine, now playing, queue,
                   cover art
  library/         browse, drill-down, search
  standby/         clock faces, custom skins, image backdrop
  vu/              VU faceplates and the spectrum
  display/         backlight backends (Pi firmware, sysfs)
  ui/              navigation stack, OSD, accent palette
  providers/       PlaybackSource trait + the Music Assistant client
  remote/          control vocabulary, ctl socket, web endpoint, QR
ui/                the slint views (mestro.slint is the root)
web/               the phone setup wizard (Svelte)
image/             the Buildroot kiosk image (see image/README.md)
tools/             dev tools: wscap (MA protocol capture), vu-feed
```

## Dev setup

```sh
nix develop -c bash -c "cargo build"   # or plain cargo if your system
                                      # provides the build deps
MESTRO_TOKEN=eyJ… ./target/debug/mestro --kiosk
```

`--kiosk` opens a window at the panel's logical size — use it for
every manual test run so layout work happens at the real aspect
ratio.

## Slint is pinned to a git rev

`Cargo.toml` pins slint to a commit of its main branch for the
linuxkms fixes mestro relies on. Consequences:

- build variants: base = player (linuxkms only,
  `--no-default-features`), the `desktop` feature adds winit

## Music Assistant protocol notes

Useful when touching `src/providers/ma.rs` or talking to MA's
WebSocket API elsewhere:

- the auth frame needs a `message_id`, or the session silently
  never authenticates
- push events only start flowing after a state change; idle systems
  are silent
- the event stream is incoherent during transitions: `queue_updated`
  replays the previous `current_item` for ~1.5s after a skip and
  replays the old clock on `queue_time`; natural advances surface
  seconds late. The state machine (`src/state.rs`) guards all of
  this — providers only emit resolved events, guards never live in
  a provider
- queue lists must be fetched via `player_queues/items`;
  `queue_updated` carries only an item *count*, and a cleared queue
  arrives *without* `current_item`
- queue entries have a rich `media_item` (name, artists[], album,
  image.proxy_id); covers resolve through
  `{http_base}/imageproxy/{proxy_id}?size=512&fmt=jpg`

```sh
# replay a scripted session against your server and dump every
# frame with sanitized tokens — the tool this knowledge came from
# (tools/wscap.rs)
MESTRO_TOKEN=eyJ… cargo run --example wscap -- 16
```

## UI work

- icons: Material Symbols codepoints differ from classic Material
  Icons — verify against the codepoints file in the
  material-design-icons repo, not from memory (`e16b` is a sofa,
  not a broom)
- layout rules that bit us: struct literals need a trailing `;`,
  integer repetition is `for name[index] in N`, if-statements take
  no colon, elements sizing across component boundaries cause
  binding loops — size at the instantiation site
- colors live in the `Theme` global (`ui/common.slint`), including
  `danger`; palette changes should not introduce hardcodes

## Logging

`tracing` with `EnvFilter`: default `mestro=info`, override with
`RUST_LOG=mestro=debug`. Showing/track changes at info, cover fetches
and position corrections at debug, config problems at warn/error.
Keep stderr quiet in the normal path.

## Web wizard (`web/`)

Svelte 5 + Vite single-file SPA, embedded in the binary.

- **dev loop**: `bun install && bun run dev` in `web/`, then open
  `http://localhost:5173/#pair=<PIN>` — Vite serves with HMR and
  proxies `/ws` to `ws://localhost:8787` (a running mestro; the PIN
  is in the kiosk's connection page, the startup log or
  `~/.local/state/mestro/web-pin`). No rebuild needed, the wizard
  is always live.
- **release**: `bun run build` → `web/dist/index.html` (generated,
  gitignored). `build.rs` embeds `$MESTRO_WEB_DIST` or that file.
- **checks**: `bun run check` (svelte-check), plus `cargo test`.
