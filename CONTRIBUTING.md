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
  system/          wifi hotspot + first-boot setup, factory reset
                   (feature `wifi-setup`); the env-file editor, audio
                   output and timezone pickers (features
                   `audio-output`, `timezone`)
  restart.rs       self-restart after a settings change or update
ui/                the slint views (mestro.slint is the root)
web/               the companion web app (Svelte PWA)
image/             the Buildroot player image (see image/README.md)
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

## Building for another image

The player image's paths and commands are not user settings: they
are fixed at build time. Set the variable in the environment of
`cargo build` (e.g. `MESTRO_SQUEEZELITE_ENV=/mnt/squeezelite.env
cargo build`); unset, the Buildroot image's value applies.
`src/system/paths.rs` reads them with `option_env!` and `build.rs`
reruns when one changes, so a cached build never keeps an old value.
Functions take their paths as parameters (tests use temp dirs);
only the production call sites read the constants.

| variable | default | what |
|---|---|---|
| `MESTRO_SQUEEZELITE_ENV` | `/data/squeezelite.env` | the env file the audio output picker rewrites (`OUTPUT=`); the picker shows only while it exists |
| `MESTRO_SQUEEZELITE_RESTART` | `/etc/init.d/S60squeezelite restart` | the command that restarts squeezelite (split on whitespace, no shell) |
| `MESTRO_ENV_FILE` | `/data/mestro.env` | the env file the init script sources before it starts mestro; the timezone picker writes `TZ=` there |
| `MESTRO_ZONEINFO_DIR` | `/usr/share/zoneinfo` | the zoneinfo database a chosen zone must exist in; the timezone picker shows only while it exists |
| `MESTRO_LOCALTIME` | `/etc/localtime` | the system zone link, read to learn the running zone when `TZ` is unset |
| `MESTRO_DIAG_LOGS` | `/tmp/mestro.log:/tmp/mestro.log.old:/tmp/ota.log` | colon-separated log files whose last 200 lines go into the diagnostics download (`.old` is the previous run); a missing file is skipped |
| `MESTRO_DIAG_STATUS` | `/usr/sbin/mestro-upgrade --status` | the command whose output joins the diagnostics download (split on whitespace, no shell, 5 s budget); empty = none |
| `MESTRO_WPA_CTRL_DIR` | `/run/wpa_supplicant` | wpa_supplicant's control socket directory (the wifi setup lane) |
| `MESTRO_WPA_CONF` | `/data/wpa_supplicant.conf` | the persistent STA conf; must match what the boot scripts start wpa_supplicant with |

## Formatting

`nix develop -c treefmt` formats the whole tree (Rust, Slint, Nix,
shell, the web app — see `treefmt.toml`; markdown and generated
files stay untouched). The dev shell installs the pre-commit hook
(`.githooks/`) that formats the staged files and refuses the commit
when anything changes.

## CI and supply-chain checks

CI (`.github/workflows/ci.yml`) runs the same commands as the dev
shell on every push to main and on every pull request: treefmt,
both clippy variants with `-D warnings`, both test feature sets (the
web app is built first — `build.rs` embeds `web/dist`), web
check/test/build, `cargo deny check` and `cargo machete`. CI runs
on plain GitHub-hosted toolchains, every tool pinned to the exact
dev-shell version (`rust-toolchain.toml` and the `*-VERSION`/
`*-SHA256` env pins in the workflow; Renovate bumps them — see
`renovate.json`). When a tool version changes, update the dev shell
(flake.nix) and CI in the same change so the pre-commit hook and
CI keep formatting identically; a Renovate CI-tool PR also needs
its SHA256 pin updated. Dependency updates arrive as Renovate PRs
(`renovate.json`); the slint crates move as one grouped PR.

Lint policy:

- no `unwrap`/`expect` outside tests — errors propagate to a caller
  that can handle them; a true invariant (a just-checked value, a
  validated const) gets a local `#[expect(clippy::…, reason = …)]`.
  `clippy.toml` allows both in tests
- `unsafe` only at audited sites with a `SAFETY:` comment and a local
  `#[allow(unsafe_code)]` — `unsafe_code` is denied crate-wide
  (see `[lints]` in `Cargo.toml`)
- modules parsing untrusted input (the web server's HTTP/WS code,
  `wpa_ctrl`, the wifi conf, config parsing) deny
  `clippy::indexing_slicing` — use `.get()` there

## Slint comes from crates.io at an exact pin

`Cargo.toml` pins every slint crate (`slint`, `slint-interpreter`,
`slint-build` and the internal `i-slint-core`/`i-slint-compiler`)
to the same exact version (`=x.y.z`) of the latest crates.io
release; the internal crates must match `slint`'s version exactly.
Renovate updates them all in one PR together with the `slint-lsp`
CI pin — never automerged, because a bump can change rendering:
re-run the snapshot tests (`--ignored`, see AGENTS.md) and
re-export the mock previews, then compare against the previous
renders. Consequences:

- build variants: base = player (linuxkms only,
  `--no-default-features`), the `desktop` feature adds winit

## Music Assistant protocol notes

Useful when touching `src/providers/ma/` or talking to MA's
WebSocket API elsewhere:

- the auth frame needs a `message_id`, or the session silently
  never authenticates
- push events only start flowing after a state change; idle systems
  are silent
- the event stream is incoherent during transitions: `queue_updated`
  replays the previous `current_item` for ~1.5s after a skip and
  replays the old clock on `queue_time`; natural advances surface
  seconds late. The state machine (`src/player/state.rs`) guards all of
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

## Web app (`web/`)

Svelte 5 + Vite, a PWA the binary serves over the LAN and embeds
at compile time.

- **dev loop**: `bun install && bun run dev` in `web/`, then open
  `http://localhost:5173/#pair=<PIN>` — Vite serves with HMR and
  proxies `/ws` to `ws://localhost:8787` (a running mestro; the PIN
  is in the player's connection page, the startup log or
  `~/.local/state/mestro/web-pin`). No rebuild needed, the app is
  always live.
- **release**: `bun run build` → `web/dist/` (generated, gitignored) —
  hashed assets plus the PWA manifest and icons. `build.rs` embeds
  `$MESTRO_WEB_DIST` (that dist directory, or a single index.html as
  a legacy fallback) — a release pipeline can pass its own dist.
- **checks**: `bun run check` (svelte-check), `bun test`, plus
  `bun run build` — and remember the binary embeds `web/dist/` at
  compile time: build the web app before `cargo build`, or the
  binary serves the old one.
