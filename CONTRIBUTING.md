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
                   (behind the `wifi-setup` feature)
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

## Formatting

`nix develop -c treefmt` formats the whole tree (Rust, Slint, Nix,
shell, the web app — see `treefmt.toml`; markdown and generated
files stay untouched). The dev shell installs the pre-commit hook
(`.githooks/`) that formats the staged files and refuses the commit
when anything changes.

## CI and supply-chain checks

CI (`.github/workflows/ci.yml`) runs the dev-shell commands from
AGENTS.md on every push to main and on every pull request: treefmt,
both clippy variants with `-D warnings`, both test feature sets (the
web app is built first — `build.rs` embeds `web/dist`), web
check/test/build, `cargo deny check` and `cargo machete`. Dependency
updates arrive as Renovate PRs (`renovate.json`); the slint rev stays
pinned.

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

## Slint is pinned to a git rev

`Cargo.toml` pins slint to a commit of its main branch for the
linuxkms fixes mestro relies on. Consequences:

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
