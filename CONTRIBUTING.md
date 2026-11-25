# Contributing

## Layout

A Cargo workspace (see [docs/architecture.md](docs/architecture.md)):
the binary and UI in `crates/mestro`, focused crates for the
device/domain code beside it.

```text
crates/
  mestro/
    src/
      main.rs, app/  the binary: wiring, command dispatch, ticker
      config/          TOML config, the settings overlay merge, lenient
                       parsing (a broken key is dropped and reported)
      settings/        the settings page: overlay handle, UI sync
      player/          what plays: now playing, queue
      library/         browse, drill-down, search
      standby/         clock faces, custom skins, image backdrop
      vu/              VU faceplates and the spectrum
      display/         the backlight throttle (the backends live in
                       mestro-os)
      ui/              navigation stack, OSD, accent palette
      providers/       the Music Assistant client (the PlaybackSource
                       contract lives in mestro-player)
      remote/          keyboard map, the mestro-remote port
                       implementations (ports), QR
      restart.rs       self-restart after a settings change or update
    ui/                the slint views (mestro.slint is the root)
    tools/             dev tools: wscap (MA protocol capture), vu-feed
  mestro-remote/   the remote-control surface: HTTP/WS server, auth
                       matrix, pairing, captive portal, mDNS, OTA
                       upload, diagnostics, the push pipeline, the
                       local ctl socket, the embedded web app — UI
                       and provider cross through the UiPort/MaAuth
                       ports the binary implements
  mestro-os/          the OS crate: everything that talks to the
                       device's OS or hardware, a module per
                       capability — the base plumbing, the backlight
                       backends (display), the wifi lane (wifi),
                       squeezelite's audio lane (audio), the timezone
                       picker and the factory reset of the device
                       state — see docs/architecture.md
  mestro-ipc/         the wire protocol: the control vocabulary
                       (command), the web endpoint's JSON requests and
                       responses (web), the ctl socket's line framing
                       (ctl) — pure data, no sibling crate
  mestro-player/      the playback domain: source contract, state
                       machine, shell, cover seam, artwork palette
web/               the companion web app (Svelte PWA)
image/             the Buildroot player image (see image/README.md)
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
cargo build`); unset, the Buildroot image's value applies. Each
crate reads exactly its own variables with `option_env!` and its
`build.rs` reruns when one changes, so a cached build never keeps an
old value; the helper (`mestro_os::paths::setting`) is shared,
and the variables are global to cargo, so they reach every crate.
mestro-os's one `build.rs` reruns on every device variable
(the module that reads one pins it with a guard test); the
binary keeps only the diagnostics pair. Functions take their
paths as parameters (tests use temp dirs); only the production
call sites read the constants.

| variable | default | read in | what |
|---|---|---|---|
| `MESTRO_SQUEEZELITE_ENV` | `/data/state/mestro/squeezelite.env` | mestro-os (`audio::squeezelite`) | the env file mestro owns and writes whole (`OUTPUT`, `NAME`, `SERVER`, `EXTRA`, from the `audio.*` settings); the only file the init script sources — must match the path S60squeezelite sources |
| `MESTRO_SQUEEZELITE_RESTART` | `/etc/init.d/S60squeezelite restart` | mestro-os (`audio::squeezelite`) | the command that restarts squeezelite (split on whitespace, no shell); the audio output picker and the env file act only while its program exists |
| `MESTRO_ENV_FILE` | `/data/state/mestro/mestro.env` | mestro-os (`timezone`) | the env file mestro owns and writes whole (`TZ=`, from the timezone setting); S50mestro sources it before the user's `/data/mestro.env` — must match that path |
| `MESTRO_ZONEINFO_DIR` | `/usr/share/zoneinfo` | mestro-os (`timezone`) | the zoneinfo database a chosen zone must exist in; the timezone picker shows only while it exists |
| `MESTRO_LOCALTIME` | `/etc/localtime` | mestro-os (`timezone`) | the system zone link, read to learn the running zone when `TZ` is unset |
| `MESTRO_LOG_DIR` | `/tmp` | mestro-os (`logs`) | where the init scripts log: `mestro.log` and the previous run `mestro.log.old` (S50mestro), `ota.log` (S04trial, which starts `mestro-update trial`, and the update commands); the diagnostics download tails them, plus the last trial boot's copies under `$XDG_STATE_HOME/mestro/trial-boot/` — must match the scripts |
| `MESTRO_STATUS_COMMAND` | `/usr/sbin/mestro-update status` | mestro-os (`logs`) | the command whose output joins the diagnostics download (split on whitespace, no shell, 5 s budget); empty = none |
| `MESTRO_WPA_CTRL_DIR` | `/run/wpa_supplicant` | mestro-os (`wifi::paths`) | wpa_supplicant's control socket directory (the wifi setup lane) |
| `MESTRO_WPA_CONF` | `/data/wpa_supplicant.conf` | mestro-os (`wifi::paths`) | the persistent STA conf; must match what the boot scripts start wpa_supplicant with |
| `MESTRO_WPA_TEMPLATE` | `/etc/mestro-defaults/wpa_supplicant.conf` | mestro-os (`wifi::paths`) | the image's seeded default conf the factory reset restores (setup lane); a missing template means the credentials are removed instead |

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
web app is built first — mestro-remote's `build.rs` embeds `web/dist`), web
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

Useful when touching `crates/mestro/src/providers/ma/` or talking to MA's
WebSocket API elsewhere:

- the auth frame needs a `message_id`, or the session silently
  never authenticates
- push events only start flowing after a state change; idle systems
  are silent
- the event stream is incoherent during transitions: `queue_updated`
  replays the previous `current_item` for ~1.5s after a skip and
  replays the old clock on `queue_time`; natural advances surface
  seconds late. The state machine (`crates/mestro/src/player/state/`) guards all of
  this — providers only emit resolved events, guards never live in
  a provider
- an external source (AirPlay receiver, Spotify Connect, ...) is
  decided the way MA's own frontend does it: `active_source` is set,
  is not the player's own id (the queue) and is listed in
  `source_list` — anything else is the queue. The listed `name`
  ("AirPlay Receiver (HiFi)") is the source's name, shown without the
  player's " (<name>)" suffix. The queue then sends NOTHING and
  `current_media` (flat fields: title, artist, album, image_url,
  duration, elapsed_time with `elapsed_time_last_updated`) is the
  only metadata. It starts as a placeholder: the title is the
  source's name (so is the album of a source without one), later
  `AirPlay | HiFi | Music Assistant`; the title moves seconds before
  the elapsed clock on a skip; `image_url` is MA's public hostname
  with `size=0`. `current_media` is read ONLY while the source is
  external (`crates/mestro/src/providers/ma/external.rs`); on the queue it lags and
  stays ignored. The source's list entry also says what it can do
  (`can_play_pause`, `can_seek`, `can_next_previous`, `can_shuffle`,
  `can_repeat`, `passive`; AirPlay: all false, passive) — the
  transport greys out the rest, and what stays goes to MA as
  `players/cmd/{play,pause,next,previous,seek,shuffle,repeat}`, never
  to `player_queues/*`. Fixtures: `crates/mestro/tests/fixtures/ma/`
- queue lists must be fetched via `player_queues/items`;
  `queue_updated` carries only an item *count*, and a cleared queue
  arrives *without* `current_item`
- queue entries have a rich `media_item` (name, artists[], album,
  image.proxy_id); covers resolve through
  `{http_base}/imageproxy/{proxy_id}?size=512&fmt=jpg`

```sh
# replay a scripted session against your server and dump every
# frame with sanitized tokens — the tool this knowledge came from
# (crates/mestro/tools/wscap.rs)
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
- colors live in the `Theme` global (`crates/mestro/ui/common.slint`), including
  `danger`; palette changes should not introduce hardcodes

## Logging

`tracing` with `EnvFilter`: default `mestro=info`, override with
`RUST_LOG=mestro=debug`. Showing/track changes at info, cover fetches
and position corrections at debug, config problems at warn/error.
Keep stderr quiet in the normal path.

## Web app (`web/`)

Svelte 5 + Vite, a PWA the mestro-remote crate serves over the LAN
and embeds at compile time.

- **dev loop**: `bun install && bun run dev` in `web/`, then open
  `http://localhost:5173/#pair=<PIN>` — Vite serves with HMR and
  proxies `/ws` to `ws://localhost:8787` (a running mestro; the PIN
  is in the player's connection page, the startup log or
  `~/.local/state/mestro/web-pin`). No rebuild needed, the app is
  always live.
- **release**: `bun run build` → `web/dist/` (generated, gitignored) —
  hashed assets plus the PWA manifest and icons. mestro-remote's `build.rs` embeds
  `$MESTRO_WEB_DIST` (that dist directory, or a single index.html as
  a legacy fallback; a relative path is taken from the repository
  root) — a release pipeline can pass its own dist.
- **checks**: `bun run check` (svelte-check), `bun test`, plus
  `bun run build` — and remember mestro-remote embeds `web/dist/` at
  compile time: build the web app before `cargo build`, or the
  binary serves the old one.
