# AGENTS.md

mestro: a Music Assistant player for a small touch panel (Rust +
Slint), a companion web app (Svelte 5) served by the `mestro-remote`
crate, and a
Buildroot image for the Raspberry Pi 4. Layout, Slint pinning, the
MA protocol quirks and UI rules are in [CONTRIBUTING.md](CONTRIBUTING.md);
read the matching section before touching that area.

## Commands

Run everything through the dev shell (`nix develop -c …`); deps are
not on the host.

```sh
# Rust: both feature sets — CI-equivalent. The feature flags
# live on the binary crate (crates/mestro) and on mestro-os and
# mestro-remote
# (`wifi-setup`/`audio-output`/`timezone`, forwarded by the
# binary's features of the same names): -p scopes them
nix develop -c cargo test
nix develop -c cargo test -p mestro --features ota,wifi-setup,audio-output,timezone
nix develop -c cargo test -p mestro-os --features wifi-setup,audio-output,timezone
nix develop -c cargo test -p mestro-remote --features ota,wifi-setup,audio-output,timezone
nix develop -c cargo test -p mestro-ipc              # the wire protocol + its golden
nix develop -c cargo clippy --all-targets
nix develop -c cargo clippy -p mestro --no-default-features --features linuxkms,ota,wifi-setup,audio-output,timezone --all-targets
nix develop -c cargo clippy -p mestro-os --features wifi-setup,audio-output,timezone --all-targets
nix develop -c cargo clippy -p mestro-remote --features ota,wifi-setup,audio-output,timezone --all-targets
nix develop -c cargo test --test np_layout          # one integration test
nix develop -c cargo deny check                     # advisories, licenses, bans, sources
nix develop -c cargo machete                        # unused dependencies

# web (in web/)
nix develop -c bash -c 'cd web && bun run check && bun test && bun run build'

# run locally: kiosk-sized window, web app on :8787
nix develop -c bash -c 'SLINT_BACKEND=winit-skia ./target/debug/mestro --kiosk'

# the Pi image: one build container (podman), caches outside the
# tree — never nix, never on the host (image/README.md)
image/scripts/enter.sh make mestro_pi4
```

The `mestro-remote` crate embeds `web/dist/` at compile time: after a
web change,
`bun run build` **before** `cargo build`, or the binary serves the old app.

## Definition of done

- every check above green, clippy with zero warnings
- `cargo deny check` and `cargo machete` clean (both run in the dev
  shell — CI runs the same commands)
- new behavior has a test (Rust unit/integration or `bun test`)
- docs updated when behavior or config changes: `docs/configuration.md`
  (every config key and `MESTRO_*` env var), `docs/remote.md` (web
  protocol, auth model), `image/README.md` (image, OTA, recovery)
- CI (`.github/workflows/ci.yml`) runs on plain GitHub-hosted
  toolchains pinned to the exact dev-shell versions
  (`rust-toolchain.toml`, the `*-VERSION`/`*-SHA256` env pins) — when
  bumping a tool in the dev shell, bump CI in the same change or the
  pre-commit hook and CI disagree

## Boundaries

- format with `nix develop -c treefmt`; the pre-commit hook
  (installed by the dev shell) refuses unformatted commits
- no new runtime dependencies (Rust or web) without asking
- don't edit generated files: `web/dist/`, `crates/mestro/src/settings/schema-golden.json`
  (regenerate: `UPDATE_GOLDEN=1 cargo test default_schema_matches_the_golden_json`,
  then rerun without it and copy the file to `web/src/lib/mockSchema.json`
  — both must stay identical),
  `crates/mestro-ipc/tests/protocol-golden.json` (the wire shapes;
  regenerate: `UPDATE_GOLDEN=1 cargo test -p mestro-ipc`, then rerun
  without it; a change there is a protocol change — update
  `docs/remote.md` and `web/src/lib/protocol.ts` with it)
- don't open desktop windows to look at the UI; render offscreen:
  `MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test settings_snapshot -- --ignored`
- the Pi image build and anything touching a real device
  (ssh, OTA, reboot) only when asked

## Conventions that aren't obvious from the code

- **device logic lives in Rust**, not shell scripts; init scripts only
  start things. No hard-wired board paths: detect hardware at runtime
  (one binary for every board); where the *image* keeps files or how
  it runs commands is a build setting (`MESTRO_*` at build time, read
  by the crate that owns them — CONTRIBUTING.md, "Building for
  another image"), never a user config key
- **user-facing text says "player"**, never "kiosk"
- **web**: Svelte 5 runes, TypeScript strict, small components, own
  history router (`lib/router.ts`, no router dependency); every string
  goes through `lib/i18n.svelte.ts` with both `en` and `de`
- **web dev without a device**: `bun run dev`, then open `/?mock`
  (`&lang=de` for German); routes are paths (`/settings/vu`), not query
  params
- **settings** come from the Rust schema (`crates/mestro/src/settings/schema.rs`);
  the web app renders whatever the schema serves — add a setting there,
  not in Svelte
- **the MA event stream is incoherent during transitions**; guards live
  in the state machine, never in a provider (details in CONTRIBUTING.md)
- **Slint**: size elements at the instantiation site (cross-component
  sizing causes binding loops); verify Material Symbols codepoints,
  don't recall them
- commits: Conventional Commits in English (`feat(web): …`, `fix(wifi): …`)

## Security-relevant areas

Changes here need a test for the auth matrix and a note in `docs/remote.md`:

- `crates/mestro-remote/src/` — pairing cookie, unpaired hotspot access
  (`Ingress::of_peer`), the captive portal
- `crates/mestro-os/src/wifi/` — wifi credentials, the hotspot, the
  web↔wifi trust contract (the factory reset's wifi half lives
  here, its state half in `crates/mestro-os/src/reset.rs`)
- `image/` — root has no password and no console login; SSH is key-only
