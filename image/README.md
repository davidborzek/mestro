# mestro kiosk image

A flash-and-go SD image for Raspberry Pi 4 (aarch64): mestro on
the DRM plane (linuxkms, GPU-accelerated via vc4/v3d + Skia GL),
squeezelite with the VISEXPORT shm ring for the VU meters, and a
system that runs entirely from RAM — piCorePlayer-style.

## Layout

| Partition | Type | Content |
|---|---|---|
| 1 (boot) | FAT32 | the system in two slots `Image.a`/`Image.b` (kernel with embedded initramfs), DTBs, firmware, `config.txt` |
| 2 (data) | FAT32 `MESTRODATA` | `wpa_supplicant.conf` (written by the setup hotspot), mestro state/config (settings, the env files mestro writes) — survives reflashes, editable on Windows/macOS |

- No root partition: the rootfs is an initramfs inside the kernel
  image. The SD card is only read at boot; zero wear afterwards.
- **No mestro config file, no CLI flags**: the service runs bare
  `mestro` — the linuxkms backend picks the connector's preferred
  DRM mode itself (`SLINT_DRM_MODE` overrides). First boot shows
  the setup hotspot's WiFi QR (no network yet) or the pairing QR;
  the phone + the settings UI write everything into `/data`; a plain
  flash seeds nothing but the wifi template
  - `wpa_supplicant.conf` — written by mestro when you pick a
    network on the setup page (a hand-written one works too;
    `/etc/mestro-defaults/wpa_supplicant.conf` is the template).
    mestro keeps the last working one as `wpa_supplicant.conf.good`
  - **no `squeezelite.env` here**: mestro owns squeezelite's config. It
    writes `/data/state/mestro/squeezelite.env` from the `audio.*`
    settings (Settings → Audio; `[audio]` in `config.toml`, incl. `extra`
    for further flags) and restarts squeezelite when the content changes —
    see [Sound card / HAT](#sound-card--hat). Never edit that file; mestro
    overwrites it. (The env files and the restart command are build-time
    settings: CONTRIBUTING.md, "Building for another image")
  - `/data/state/mestro/mestro.env` — written by mestro (`TZ`, from the
    timezone setting; the image bakes `/etc/localtime` = Europe/Berlin,
    `TZ` overrides it). Settings → System → Time zone writes it and
    restarts mestro to apply it — feature `timezone`; the first paired
    browser's zone is adopted once, later phones never change it. The
    legacy `/data/timezone` file (read by `S02data` into `/etc/localtime`)
    still works underneath; `TZ` wins
  - `/data/mestro.env` — **yours**, the expert escape hatch: environment
    for mestro, sourced by `S50mestro` after mestro's own file
    (`RUST_LOG`, `SLINT_DRM_OUTPUT`, `SLINT_DRM_MODE`, …; a `TZ` here wins).
    Optional, never created or written by mestro; CRLF and a UTF-8 BOM
    from Windows editors are handled
- Squeezelite: the **upstream Buildroot package** with
  `BR2_PACKAGE_SQUEEZELITE_VISEXPORT` (the shm ring mestro's VU
  reads, crates/mestro/src/vu/mod.rs) and **no resampler**: `hw:` output stays
  bit-perfect.

## Build

Everything builds inside one container — locally with podman, in
CI with docker, the same image (`image/Dockerfile`): a pinned
Buildroot submodule plus a BR2_EXTERNAL tree (this `image/`
directory) that owns the defconfig, the packages and the board
files. Nothing on the host is needed beyond the container runtime
and git (for the submodule); in particular the image build never
touches nix — the dev shells stay for the player itself.

```sh
git clone <mestro> && cd mestro
image/scripts/enter.sh make mestro_pi4
# → output/images/mestro.img
```

`enter.sh` builds the container, inits the `buildroot/` submodule
if needed, and runs the command with the repo mounted at `/build`
and the cache volume at `/cache`, as the calling user. Build from
a regular clone: in a `git worktree` the `.git` file points
outside the mount, so the image's version string loses its git
hash. With no
command it opens a shell in the container — that is where
`make menuconfig`, `make clean` and the Buildroot targets below
belong.

The cache volume (default `~/.cache/pi/mestro-cache`, override
with `MESTRO_CACHE_DIR`) holds everything that survives
`make clean`:

| dir | content |
|---|---|
| `dl/` | the Buildroot download cache, incl. the cargo crate cache (`dl/br-cargo-home`) |
| `cc/` | ccache |
| `bun/` | the bun package cache (the web app build) |

The build output (`output/`, gitignored) stays in the tree —
`make clean` keeps its `host/` toolchain, so only the target
packages rebuild. A first build with an empty cache needs network
(cargo fetches the crate set once); afterwards everything,
including `make clean` cycles, runs offline.

The root `Makefile` is the entry point:

| command | effect |
|---|---|
| `make mestro_pi4` | defconfig + build the image |
| `make mestro_pi4-config` | only (re)write `output/.config` |
| `make web` | rebuild `web/dist` (the mestro crate embeds it) |
| `make <buildroot target>` | falls through to Buildroot (`clean`, `menuconfig`, `mestro-dirclean`, `show-info`, …) |

`make mestro_pi4` rebuilds the mestro package first (`mestro-rebuild`):
a local-source package is rsynced into the build tree once, so
without that step the image would keep the sources of the previous
build. The rsync excludes `buildroot/`, `output/`, `target/` and
`web/node_modules` — `web/dist` is included on purpose.

### The Buildroot pin

`buildroot/` is a git submodule pinned to a stable tag
(2026.08.x). Buildroot releases quarterly; a regular series gets
bugfix releases until the next one, the February release of every
odd year is an LTS series with three years of bugfix and security
releases. We ride the current stable line and settle on the next
LTS (2027.02.x, spring 2027), following its point releases.

The pin defines the whole package set, toolchain included: the
image ships what the tag ships, not what Buildroot `master` has.
2026.08.x comes with the Bootlin 2025.08-1 toolchain (glibc 2.41)
— extensions with prebuilt binaries must not need a newer glibc
(python-build-standalone and static Go/Rust tools don't). Bumps are **manual** — commit the new
gitlink, never let automation touch it (Renovate does not manage
submodules) — and one bump = one PR that rebuilds the image and
re-checks the package versions (`make show-info`) plus the rootfs
file list against the previous build.

### How the player binary builds

`mestro` is a regular Buildroot cargo package
(`image/package/mestro`): the source is the repository itself
(local site method), Buildroot compiles it with its own Rust
toolchain (a newer rustc than the dev shells — check CI warnings
on bumps) and links it against the sysroot it just built. The
crate set is vendored into the build tree at package-extract time
(same as Buildroot's own cargo packages; the crate cache lives on
the /cache volume). The kiosk features are pinned in the package:
`linuxkms,ota,wifi-setup,audio-output,timezone`.

The fonts ship as their own packages (`image/package/font-inter`,
`font-material-symbols`), pinned upstream and hash-checked,
installing exactly the two files `board/mestro/common/fonts.conf`
lists.

### Quick tests on a running kiosk

Skip the image: `nix develop .#pi -c cargo build --profile pi`
links against the container-built toolchain in `output/host`
(build the image once first) and produces a binary for the exact
libs the image ships — swap `/usr/bin/mestro` live (the root is a
writable overlay; the swap is gone after a reboot).

## Extensions (tcz)

The kiosk boots through `kinit` (rdinit=/kinit): the initramfs
root is combined with every extension into one live root via
overlayfs (piCore's package model — see kinit in the rootfs
overlay). An extension is a squashfs image of a complete file
tree; installing one is a file copy:

```sh
scp my-thing.tcz root@<pi>:/data/tce/
ssh root@<pi> mestro-reboot   # active after the next boot
```

Loading order: name order; `/data/tce/onboot.lst` (one filename
per line, `#` comments) limits the selection. Earlier extensions
shadow later ones; the base system always loses against an
extension (leftmost layer wins).

Building a tcz needs any Linux box with `squashfs-tools`:

```sh
mksquashfs <dir> my-thing.tcz -comp xz -noappend
```

Three sourcing recipes, pick by situation:

| Source | Recipe |
|---|---|
| Buildroot package (`BR2_PACKAGE_*`) | `make <pkg>` in a copy of the buildroot tree with the same defconfig, extract the package's files from `output/target`, mksquashfs — exact toolchain match |
| Prebuilt standalone tarball (python-build-standalone, Go tools, …) | unpack, mksquashfs — binaries built against older glibc run against the image's glibc 2.41 |
| Your own files | arrange a directory tree as it should appear under /, mksquashfs |

Everything on `/data` survives re-flashes and OTA upgrades — the
upgrade only swaps the kernel image, never the data partition.

## Upgrades

From the web app: Settings → Info → System update, pick the
`Image` file. The upload streams straight into the inactive slot.

Over SSH — key-only access; root has no password and there is no
console login. Two ways to get a key in:

- **Bake it into your image** (git-ignored, never committed):
  put your public key at
  `board/mestro/rpi4/rootfs-overlay/root/.ssh/authorized_keys`
  before building.
- **Drop it on the data partition**: `/data/authorized_keys`
  (FAT, editable from any PC) overrides the baked one at boot
  (`S02data`).

The whole system is one file — the rootfs lives in the initramfs
embedded in the kernel image. The boot partition holds two of them,
`Image.a` and `Image.b`; `kernel=` in `config.txt` names the
running one. An upgrade writes the other slot and boots it once on
trial, no re-flashing:

```sh
# stream it from the build machine (nothing is buffered in RAM)
cat output/images/Image | \
	ssh root@<pi> "mestro-upgrade --reboot - $(sha256sum output/images/Image | cut -d' ' -f1)"
```

The trial uses the firmware's one-shot `tryboot`: that boot reads
`tryboot.txt` (a copy of `config.txt` pointing at the new slot)
and starts with the SoC watchdog armed. `S99ota` feeds the
watchdog while it checks that mestro runs stably for a minute,
then commits — `config.txt` switches to the new slot and the
watchdog turns off again. Anything else goes back to the previous
slot on its own:

| the new image … | what happens |
|---|---|
| runs, mestro stays up | committed after ~70 s |
| boots, mestro crashes | S99ota reboots → previous slot |
| hangs (kernel, init) | the watchdog resets after 60 s → previous slot |
| power cut during the trial | next boot reads `config.txt` → previous slot |

`mestro-upgrade --status` shows the running and a pending slot.
Without `--reboot` the trial starts at the next
`mestro-reboot --tryboot`; plain `mestro-reboot` (never busybox
`reboot`, it hangs the initramfs kiosk) stays on the current slot.

## WiFi setup (hotspot)

mestro owns the radio (feature `wifi-setup`, `crates/mestro-os/src/wifi/`):
without a usable `wpa_supplicant.conf` — or after 60 s without a
route on a configured player — it runs wpa_supplicant in AP mode
as the open hotspot `mestro-XXXX` (last four hex digits of the
MAC) on `10.42.0.1/24`, with dnsmasq for DHCP, a wildcard DNS
(every name resolves to the player, `mestro.setup` included) and
the RFC 8910 captive-portal option. Phones open the setup page by
themselves — the web endpoint runs on port 80 in the image
(`MESTRO_REMOTE_PORT=80` in S50mestro), so `mestro.setup` and the
portal URI carry no port. A fallback hotspot retries the saved
network every two minutes (never while a phone is connected to
it); a hotspot that cannot start falls back to the last working
network. The addresses are configurable in `[wifi]`
(docs/configuration.md).

The image ships `regulatory.db` (wireless-regdb) — without it the
kernel stays on the passive world regdom `00` and 5 GHz stays
restricted. mestro switches wifi power save off by default
(brcmfmac's default on state costs ~300 ms latency spikes and
stream reconnects) and re-applies it after every association. A
country pins the regdom via the config file
`/data/config/mestro/config.toml` (the data FAT — editable on any
OS):

```toml
[wifi]
power-save = false
# country = "DE"
```

## Sound card / HAT

squeezelite plays on a `hw:` ALSA device, bit-perfect. mestro owns its
config: it writes `/data/state/mestro/squeezelite.env` (`OUTPUT`, `NAME`,
`SERVER`, `EXTRA`) from the audio settings (Settings → Audio, or `[audio]`
in the config file); an empty setting falls back to what mestro derives,
and squeezelite restarts once when the file changes. Nothing on the data
partition to edit. The details:
[configuration.md](../docs/configuration.md#audio).

mestro picks the output at startup (feature `audio-output`):

- **USB DACs work out of the box**: plug it in before the boot, mestro
  derives `OUTPUT=hw:CARD=<id>` and restarts squeezelite once. Several
  cards: a USB DAC beats an I2S HAT; among equals the lower card
  number. A DAC that enumerates slowly gets ~10 s before mestro
  decides.
- **HATs (I2S) need their device-tree overlay**: edit `config.txt` on
  the boot partition (FAT, any OS) and add the overlay of your board
  — `dtoverlay=hifiberry-dacplus`, `hifiberry-digi`,
  `iqaudio-dacplus` or `allo-boss-dac-pcm512x-audio` (the full list
  is the `overlays/` folder next to it; its kernel driver must be in
  the image — the four above are). Reboot. The card appears as a
  sound card, and mestro picks it. The
  image ships `dtparam=audio=off` (no headphone jack card); remove
  that line to get the Pi's jack back, then choose it under
  Settings → Audio.
- The built-in headphone jack and the HDMI ports are never picked
  automatically — with only those present `OUTPUT` stays empty and
  squeezelite uses its own default (mestro logs a warning).
- To use another card: Settings → Audio → Audio output (it sets the
  `audio.output` setting). A DAC plugged in **later** is not picked up by
  itself (no hotplug); choose it there, or reboot.

`NAME` becomes `mestro-XXXX` (the hotspot's suffix, from the wifi
MAC) and `SERVER` the host of the Music Assistant url — this also
finds MA in another subnet, where the broadcast discovery cannot.
To pin your own, set `audio.name` / `audio.server` (Settings → Audio).

## Bug reports (diagnostics)

The web app's Settings → Info → *Download diagnostics* saves one
JSON file (`GET /diagnostics`, paired phones only): the version,
the redacted settings, the wifi status, the tail of `/tmp/mestro.log`
(and `.old`, the previous run) and `/tmp/ota.log`, `/proc/version`,
`/proc/asound/cards` and the output of `mestro-upgrade --status`.
Tokens, the pairing PIN and wifi passphrases are removed; host
keys, `wpa_supplicant.conf` and the settings overlay are never read.
The paths are `[diagnostics]` in the config
([configuration.md](../docs/configuration.md#diagnostics)); the log
files live in RAM, so the report has to be taken before a reboot
to contain the crash it is about.

## Known limits

- Raspberry Pi 4 only (Pi 3/5 and Zero 2 W defconfigs are
  follow-ups).
- The backlight at 0 % is dim, not fully off.
- The panel's touch firmware can hang the display bus under heavy
  backlight traffic; mestro routes backlight writes through the
  firmware mailbox to avoid it.

## Follow-up backlog

- settings UI: `[window] scale` picker (needs the app restart
  flow — SLINT_SCALE_FACTOR is read at backend init)
- USB-C gadget network as a rescue path
- release pipeline: versioned images + checksums as release assets
