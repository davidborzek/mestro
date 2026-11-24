# Architecture: crates and stages

mestro grows towards a Cargo workspace: the binary and UI in
`crates/mestro`, the playback domain in `crates/mestro-player`,
everything that touches the device in `crates/mestro-os`, the
wire protocol in `crates/mestro-ipc`. This
document is the target picture and the staged path there —
what exists today, what moves later, and what has to be true
before the later stages can happen.

## Why crates at all

Not structure for its own sake:

- **compiler-enforced boundaries.** `unsafe` is allowed only in
  the two audited leaf sites (the Pi firmware ioctl and the
  `kill(2)` wrapper, both in `mestro-os`); the domain and the
  protocol crates `#![forbid]` it, the binary crate denies it —
  slint's generated code carries its own local `allow`.
  Dependency direction is checked by cargo, not review:
  `mestro → {player, os, ipc, web}`, `os → ipc` and
  `web → {player, os, ipc}`; `ipc` depends on
  no sibling crate, and no crate ever reaches back into the app.
- **tests without Slint.** The player domain (state machine, shell,
  the source contract) is pure logic plus tokio channels; its tests
  used to drag in the whole UI build. In the workspace they compile
  and run without Slint (`cargo test -p mestro-player`). The same
  holds for the OS crate's device logic — and for the web
  endpoint's auth matrix, pairing, captive portal and push tests
  (`cargo test -p mestro-remote`).
- **blast radius.** A backlight regression can only be in
  `mestro-os::display`; a squeezelite env-file regression only in
  `mestro-os::audio::squeezelite`.

## The crates (this state of the tree)

```mermaid
graph TD
  mestro["mestro — binary + UI"] --> player["mestro-player — playback domain"]
  mestro --> os["mestro-os — device: OS + hardware"]
  mestro --> ipc["mestro-ipc — wire protocol"]
  mestro --> web["mestro-remote — remote surface: web + ctl + push"]
  os --> ipc
  web --> ipc
  web --> os
  web --> player
```

```
crates/
  mestro/            the binary + UI: app wiring,
                    config, settings, library, standby, vu (FFT,
                    skins, the meter loop), providers/ma, the
                    slint views (ui/, skins/), the remote
                    surface's port implementations (remote/ports: the
                    slint UiPort, the MA auth client, the web-op
                    and control-command handlers) and the
                    keyboard map + pairing QR (remote/)
  mestro-remote/   the device's remote-control surface, without
                    Slint or the config crate: the HTTP/WS server
                    (the PWA and Home Assistant speak it), the
                    auth matrix (the pairing cookie), the unpaired
                    hotspot gate (`Ingress::of_peer`), the captive
                    portal, mDNS discovery, the OTA upload,
                    diagnostics, the push pipeline, the local ctl
                    socket and the embedded web app (web/dist,
                    its build.rs embeds $MESTRO_WEB_DIST).
                    Everything UI- or provider-bound crosses
                    through the `UiPort` (ops, control commands,
                    the OTA overlay), the `MaAuth` port
                    (probe/login/players) or plain data
                    (`WebSettings`, the `UiView` watch) — the
                    binary implements the ports; the tests
                    (auth, pairing, captive, OTA, push) run
                    without the Slint build
  mestro-player/    the playback domain: the source contract
                    (TrackMeta, QueueItem, MediaEntry, SourceEvent,
                    PlaybackSource, SourceCaps, RepeatMode, …), the
                    state machine (player/state), the imperative
                    shell (player/shell), cover data + the
                    CoverFetcher seam (player/cover; the HTTP fetch
                    stays in the binary) and the artwork palette as
                    plain RGB (ui/palette without the slint::Color
                    conversions)
  mestro-ipc/       the wire protocol, the public crate (serde,
                    serde_json, toml's value type; no sibling
                    crate, `#![forbid(unsafe_code)]`):
                    - [command]: the control vocabulary (`Command`,
                      its kebab-case wire form)
                    - [web]: the web endpoint's JSON requests,
                      responses and ops (`Envelope`, `Request`,
                      `Response`, `SettingsOp`, `WifiOp`, …)
                    - [ctl]: the ctl socket's line framing (`ok` /
                      `err <reason>`, `answer_for`)
                    The keyboard map and everything that needs the
                    effective config (`state_data`) stay with the
                    binary; the web server and the ctl sockets live
                    in `mestro-remote`. A JSON golden test
                    (`tests/protocol-golden.json`) pins the shapes
  mestro-os/         everything that talks to the device's OS or
                    hardware; nothing about MA, the UI or the web
                    protocol. One OS crate, a module per capability,
                    the fakes beside the real code:
                    - the base: atomic file writes ([atomic_file]),
                      the XDG state dir and the shared build-setting
                      helper ([paths]), device identity
                      (MAC/device id/suffix, [identity]), [lan_ip]
                      and the safe signal wrapper for kill(2)
                    - [display]: the backlight backends (Pi firmware
                      mailbox ioctl, sysfs) — the only unsafe site
                      beside the signal wrapper
                    - [wifi]: the web↔wifi contract (the ops channel
                      carrying mestro-ipc's `WifiOp`, the
                      shutdown/children registry, the mDNS pause
                      gate, the [wifi] settings, the image's wpa
                      build settings) always compiled — the web
                      endpoint and the restart path name it in every
                      build; the setup lane itself (wpa_ctrl, the
                      manager, the backend, the factory reset's wifi
                      half) behind the `wifi-setup` feature
                    - [audio]: [outputs] (the renderer-neutral ALSA
                      output discovery, the picker, SwitchSlot — a
                      future in-process player reuses it),
                      [squeezelite] (the env file, derivation,
                      restart, behind `audio-output`) and
                      [visexport] (squeezelite's shm ring, always
                      built — [vu] mode = "shmem" is not gated)
                    - [timezone] (feature `timezone`): zone
                      validation, the zone list, `TZ` into mestro's
                      env file
                    - [reset] (feature `wifi-setup`): the factory
                      reset of the device state (settings overlay,
                      pairing PIN); the wifi credentials half stays
                      with the lane in `wifi::reset` — it knows the
                      wpa conf and its sidecars, the state half
                      knows the state files, and the settings op
                      calls both
```

Why one OS crate rather than one crate per device capability: a
device capability is not a domain boundary — the omicron
`illumos-utils` crate is the shape to copy (one crate, a module
per system it talks to, the fakes beside the real code), and
librespot/spotifyd keep alternative backends as features of one
crate rather than as crates. Separate crates pay off for a domain
(player), a reusable protocol (mestro-ipc) — not per device
seam, where they only multiply manifests and build-matrix
entries for code that always moves together.

The 60 MA scenarios stay in the binary for now: they need the
private MA parser, which is a later stage. The slint-facing
throttling of backlight writes (`Display`) stays with the UI.

Feature flags are unchanged in shape: `desktop`/`linuxkms` on the
binary, and the capability features `wifi-setup`,
`audio-output` and `timezone` live on `mestro-os` and
`mestro-remote` (the endpoint's own gates: the ota upload, the
audio picker, the timezone picker, the capability hello
reports) — the binary
forwards each (`wifi-setup = ["mestro-os/wifi-setup",
"mestro-remote/wifi-setup"]`, …) and
its own features carry the same names. Without them mestro-os
compiles the base, the display backends, the wifi contract and
the visexport ring — exactly what the desktop build needs; the
OS crate's own test/clippy runs are `-p mestro-os --features
wifi-setup,audio-output,timezone`, the web crate's are
`-p mestro-remote --features ota,wifi-setup,audio-output,timezone`.

## Later stages

The end picture, in order — each stage keeps the tree green and
shippable:

1. **Mestro out of the domain logic.** `nowplaying`, `osd` and
   `vu/wire` take `&Mestro` deep into rendering helpers; narrow
   ports (an update sink, a model handle) replace it.
2. **config as a leaf.** The config types stop reaching into the
   app, so the schema and the protocol can name them without the
   binary.
3. **The settings schema into mestro-ipc** (`settings/schema.rs`
   and its golden): the web app contract and the Rust schema are
   tested together without the UI. The schema golden then moves
   with it.
4. **mestro-ma**, later: the Music Assistant client
   (`providers/ma`) as a PlaybackSource implementation, then the
   scenarios move out of the binary into that crate. Prerequisite:
   the private parser types (`parse`) settle behind the crate's
   public API.

Beside that line, without a fixed slot:

- **mestro-testkit**: the scenario harness (fixtures, the scripted
  MA server) for out-of-tree provider tests.
- **mestro-ota**: the update lane (`mestro-remote`'s
  `server/ota.rs`,
  the image-side scripts) once the upload path is a stable seam.
- **xtask**: the image/tooling helpers (snapshot exports, the
  golden-schema check, the aarch64 staging script) behind `cargo
  xtask`, replacing ad-hoc shell invocations.
- **plugins**: runtime VU skins already load from TOML manifests;
  a plugin trait for playback sources would land on the
  `PlaybackSource` contract in mestro-player.

## Prerequisites for the ma stage

The remote surface has left the binary (`mestro-remote`: the
web endpoint and the ctl socket crossed behind the
`UiPort`/`MaAuth` ports and plain-data inputs; the push pipeline
owns its event feed — the binary publishes view snapshots into a
watch channel). What remains before `providers/ma` can follow:

1. **Remove the module cycle.** `settings ↔ ui` (the schema
   golden test reads `ui/settings/*.slint` by path) is the
   remaining reach-back; it needs an injected seam, not a crate
   edge. (`remote/web ↔ player` is resolved: the bridge became
   the `UiPort` port, the push context a `UiView` watch.)
2. **Push `Mestro` out of the domain logic.** `nowplaying`,
   `osd`, `vu/wire` take `&Mestro` deep into rendering helpers;
   until they take narrow ports (an update sink, a model handle),
   they keep the UI crate and the domain glued together.
3. **The MA parser is private.** The scenarios compile against
   `providers::ma::parse` internals; a public event surface for
   mestro-ma has to come first, or the scenario crate duplicates
   the harness.
