# AGENTS.md

mestro: a Music Assistant player for a small touch panel (Rust +
Slint), a companion web app (Svelte 5) served by the binary, and a
Buildroot image for the Raspberry Pi 4. Layout, Slint pinning, the
MA protocol quirks and UI rules are in [CONTRIBUTING.md](CONTRIBUTING.md);
read the matching section before touching that area.

## Commands

Run everything through the dev shell (`nix develop -c …`); deps are
not on the host.

```sh
# Rust: both feature sets — CI-equivalent
nix develop -c cargo test
nix develop -c cargo test --features ota,wifi-setup,audio-output
nix develop -c cargo clippy --all-targets
nix develop -c cargo clippy --no-default-features --features linuxkms,ota,wifi-setup,audio-output --all-targets
nix develop -c cargo test --test np_layout          # one integration test
nix develop -c cargo deny check                     # advisories, licenses, bans, sources
nix develop -c cargo machete                        # unused dependencies

# web (in web/)
nix develop -c bash -c 'cd web && bun run check && bun test && bun run build'

# run locally: kiosk-sized window, web app on :8787
nix develop -c bash -c 'SLINT_BACKEND=winit-skia ./target/debug/mestro --kiosk'
```

The binary embeds `web/dist/` at compile time: after a web change,
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
- don't edit generated files: `web/dist/`, `src/settings/schema-golden.json`
  (regenerate: `UPDATE_GOLDEN=1 cargo test default_schema_matches_the_golden_json`,
  then rerun without it and copy the file to `web/src/lib/mockSchema.json`
  — both must stay identical)
- don't open desktop windows to look at the UI; render offscreen:
  `MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test settings_snapshot -- --ignored`
- the Pi image build and anything touching a real device
  (ssh, OTA, reboot) only when asked

## Conventions that aren't obvious from the code

- **device logic lives in Rust**, not shell scripts; init scripts only
  start things. No hard-wired board paths: detect at runtime, allow a
  config override — one binary for every board
- **user-facing text says "player"**, never "kiosk"
- **web**: Svelte 5 runes, TypeScript strict, small components, own
  history router (`lib/router.ts`, no router dependency); every string
  goes through `lib/i18n.svelte.ts` with both `en` and `de`
- **web dev without a device**: `bun run dev`, then open `/?mock`
  (`&lang=de` for German); routes are paths (`/settings/vu`), not query
  params
- **settings** come from the Rust schema (`src/settings/schema.rs`);
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

- `src/remote/web/` — pairing cookie, unpaired hotspot access
  (`Ingress::of_peer`), the captive portal
- `src/system/wifi/` — credentials, hotspot, factory reset
- `image/` — root has no password and no console login; SSH is key-only
