# Architecture: crates and stages

mestro grows towards a Cargo workspace: the binary and UI in
`crates/mestro`, the device/domain code in focused crates below it.
This document is the target picture and the staged path there —
what exists today (stage 1), what moves later, and what has to be
true before the later stages can happen.

## Why crates at all

Not structure for its own sake:

- **compiler-enforced boundaries.** `unsafe` is allowed only in the
  two audited leaf sites (the Pi firmware ioctl in `mestro-display`,
  the `kill(2)` wrapper in `mestro-system`); every other crate
  `#![forbid]`s it. Dependency direction is checked by cargo, not
  review: `mestro → {player, squeezelite, display, system}`,
  `squeezelite → system`, and the domain crates never reach back
  into the app.
- **tests without Slint.** The player domain (state machine, shell,
  the source contract) is pure logic plus tokio channels; its tests
  used to drag in the whole UI build. In the workspace they compile
  and run without Slint (`cargo test -p mestro-player`).
- **blast radius.** A backlight regression can only be in
  `mestro-display`; a squeezelite env-file regression only in
  `mestro-squeezelite`.

## Stage 1 (this state of the tree)

```
crates/
  mestro/            the binary + UI + web endpoint: app wiring,
                    config, settings, library, standby, vu (FFT,
                    skins, the meter loop), providers/ma, remote/,
                    system/wifi, the slint views (ui/, skins/)
  mestro-player/    the playback domain: the source contract
                    (TrackMeta, QueueItem, MediaEntry, SourceEvent,
                    PlaybackSource, SourceCaps, RepeatMode, …), the
                    state machine (player/state), the imperative
                    shell (player/shell), cover data + the
                    CoverFetcher seam (player/cover; the HTTP fetch
                    stays in the binary) and the artwork palette as
                    plain RGB (ui/palette without the slint::Color
                    conversions)
  mestro-system/    device plumbing: atomic file writes, the
                    poison-tolerant lock helpers, the image paths
                    (system/paths, incl. the XDG state dir),
                    factory reset (feature `wifi-setup`), the
                    timezone picker (feature `timezone`), device
                    identity (MAC/device id/suffix), lan_ip, the
                    safe signal wrapper for kill(2)
  mestro-squeezelite/  squeezelite specifics: the ALSA output picker
                    and the env-file writer (feature
                    `audio-output`, fed an AudioSettings value
                    instead of the config types) and the VISEXPORT
                    shm reader (always built — [vu] mode = "shmem"
                    is not feature-gated)
  mestro-display/   the backlight backends (Pi firmware mailbox
                    ioctl, sysfs) — the only unsafe block outside
                    mestro-system
```

The 60 MA scenarios stay in the binary for now: they need the
private MA parser, which is a later stage. The slint-facing
throttling of backlight writes (`Display`) stays with the UI.

Feature flags are unchanged: `desktop`/`linuxkms` on the binary,
and `wifi-setup`/`audio-output`/`timezone` forward into the
member crates (`mestro-system/wifi-setup`, …) so the desktop
build still compiles only what the desktop needs.

## Later stages

The end picture, in rough order — each stage keeps the tree green
and shippable:

- **mestro-wifi**: the wifi lane out of the binary — `wpa_ctrl`,
  the AP/captive-portal policy, the join state machine
  (`system/wifi`). Prerequisite: the trust-boundary tests around
  `Ingress::of_peer` move with it (see docs/remote.md).
- **mestro-web**: the embedded web endpoint (`remote/web`) minus
  the UI-bound bridge; the push pipeline becomes an event feed the
  crate owns. Prerequisite: `remote/web` stops reaching into
  `crate::Mestro` for previews — an injected renderer seam already
  exists (`PreviewRenderer`).
- **mestro-ma**: the Music Assistant client (`providers/ma`) as a
  PlaybackSource implementation, then the scenarios move out of the
  binary into that crate. Prerequisite: the private parser types
  (`parse`) settle behind the crate's public API.
- **mestro-settings**: the schema + overlay engine behind the web
  endpoint (`settings/schema.rs` today), so the web app contract
  and the Rust schema can be tested together without the UI.
- **mestro-protocol**: `remote/web/protocol.rs` + the ctl vocabulary
  (`remote/control.rs`, `remote/ipc`) — the wire surface of the
  player, shared by the kiosk, the web app and external tools.
- **mestro-testkit**: the scenario harness (fixtures, the scripted
  MA server) for out-of-tree provider tests.
- **mestro-ota**: the update lane (`remote/web/server/ota.rs`,
  the image-side scripts) once the upload path is a stable seam.
- **xtask**: the image/tooling helpers (snapshot exports, the
  golden-schema check, the aarch64 staging script) behind `cargo
  xtask`, replacing ad-hoc shell invocations.
- **plugins**: runtime VU skins already load from TOML manifests;
  a plugin trait for playback sources would land on the
  `PlaybackSource` contract in mestro-player.

## Prerequisites for stage 2 (beyond stage 1)

Stage 2 (web, ma, wifi out of the binary) cannot be a pure move:
these must be resolved first.

1. **Remove the module cycles.** `settings ↔ ui` (the schema
   golden test reads `ui/settings/*.slint` by path) and
   `remote/web ↔ player` (the bridge reaches into `Mestro`
   properties) are the two remaining reach-backs; each needs an
   injected seam, not a crate edge.
2. **Push `Mestro` out of the domain logic.** `nowplaying`,
   `osd`, `vu/wire` take `&Mestro` deep into rendering helpers;
   until they take narrow ports (an update sink, a model handle),
   they keep the UI crate and the domain glued together.
3. **The MA parser is private.** The scenarios compile against
   `providers::ma::parse` internals; a public event surface for
   mestro-ma has to come first, or the scenario crate duplicates
   the harness.
4. **Config types cross the seams as data.** Stage 1 introduced
   `AudioSettings` for this; the same treatment (plain structs at
   the boundary) is needed for the wifi and web settings before
   those crates can stand alone.
