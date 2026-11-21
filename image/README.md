# mestro kiosk image

A flash-and-go SD image for Raspberry Pi 4 (aarch64): mestro on
the DRM plane (linuxkms, GPU-accelerated via vc4/v3d + Skia GL),
squeezelite with the VISEXPORT shm ring for the VU meters, and a
system that runs entirely from RAM — piCorePlayer-style.

## Layout

| Partition | Type | Content |
|---|---|---|
| 1 (boot) | FAT32 | the system in two slots `Image.a`/`Image.b` (kernel with embedded initramfs), DTBs, firmware, `config.txt` |
| 2 (data) | FAT32 `MESTRODATA` | `squeezelite.env`, `wpa_supplicant.conf` (written by the setup hotspot), mestro state/config — survives reflashes, editable on Windows/macOS |

- No root partition: the rootfs is an initramfs inside the kernel
  image. The SD card is only read at boot; zero wear afterwards.
- **No mestro config file, no CLI flags**: the service runs bare
  `mestro` — the linuxkms backend picks the connector's preferred
  DRM mode itself (`SLINT_DRM_MODE` overrides). First boot shows
  the setup hotspot's WiFi QR (no network yet) or the pairing QR;
  the phone + the settings UI write everything into `/data`.
  `squeezelite.env` is the one file a plain flash seeds
  - `wpa_supplicant.conf` — written by mestro when you pick a
    network on the setup page (a hand-written one works too;
    `/etc/mestro-defaults/wpa_supplicant.conf` is the template).
    mestro keeps the last working one as `wpa_supplicant.conf.good`
  - `squeezelite.env` — `OUTPUT` (bit-perfect ALSA device),
    `SERVER` (Music Assistant host, empty = discovery), `NAME`,
    `EXTRA`; applied with
    `/etc/init.d/S60squeezelite restart`
    (the web app's Settings → Info → Audio output does exactly that:
    it rewrites only the `OUTPUT=` line, keeps comments, other keys
    and CRLF, and reverts to the previous output when squeezelite
    does not start on the new one — feature `audio-output`)
  - `mestro.env` — environment for mestro, sourced by `S50mestro`
    (`RUST_LOG`, `SLINT_DRM_OUTPUT`, `MESTRO_REMOTE_PORT`, …) and
    `TZ`, the timezone: the image bakes `/etc/localtime` =
    Europe/Berlin, `TZ` overrides it. The web app's Settings → Info →
    Time zone writes exactly that one line (every other line,
    comment and CRLF stays) and restarts mestro to apply it —
    feature `timezone`; the first paired browser's zone is adopted
    once, later phones never change it. The legacy `/data/timezone`
    file (read by `S02data` into `/etc/localtime`) still works
    underneath; `TZ` wins
- Squeezelite: the **upstream Buildroot package** with
  `BR2_PACKAGE_SQUEEZELITE_VISEXPORT` (the shm ring mestro's VU
  reads, src/vu/mod.rs) and **no resampler**: `hw:` output stays
  bit-perfect.

## Build

First image, from scratch (see [README](../README.md) for the
player itself):

```sh
git clone https://github.com/buildroot/buildroot
cd buildroot
# NixOS only: buildroot insists on a literal /usr/bin/file
# (legacy libtool workaround). Plain PATH lookup works fine here:
sed -i 's|^check_prog_host "/usr/bin/file"$|check_prog_host "file"|' \
  support/dependencies/dependencies.sh
nix develop /path/to/mestro#image -c \
  make BR2_EXTERNAL=/path/to/mestro/image mestro_pi4_defconfig

# stages bin/mestro + fonts behind image/bin — on the fresh tree
# it bootstraps toolchain + the binary's runtime libs
# (`make mestro-depends`) before cross-building the binary
nix develop -c /path/to/mestro/image/scripts/build-mestro-aarch64.sh

nix develop /path/to/mestro#image -c make -j$(nproc)
# → output/images/mestro.img
```

The script runs a plain `cargo build` for aarch64 in the flake's
`.#pi` shell, linked against this Buildroot tree's toolchain and
sysroot — so it needs the tree configured first (the `mestro_pi4_defconfig`
step above), but nothing built: the script's `make mestro-depends`
bootstrap produces the toolchain and the sysroot libs. The staged
payload lands in `~/.cache/pi/mestro-image-bin` (or the stage-dir
argument), linked from `image/bin`. `mestro-dirclean` before a
`make` is not optional once an image exists: the package rsyncs
with `-u` and skips a binary that is not newer than the copy it
already has.

```sh
image/scripts/build-mestro-aarch64.sh   # → ~/.cache/pi/mestro-image-bin, links image/bin
cd buildroot && nix develop /path/to/mestro#image -c \
  bash -c 'make mestro-dirclean && make -j$(nproc)'
```

For quick tests on a running kiosk, skip the image: build with
`cargo build --profile pi` in `.#pi` and swap `/usr/bin/mestro`
live (the root is a writable overlay; the swap is gone after a
reboot).

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
| Prebuilt standalone tarball (python-build-standalone, Go tools, …) | unpack, mksquashfs — binaries built against older glibc run against the image's glibc 2.44 |
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
cat ~/.cache/pi/buildroot/output/images/Image | \
	ssh root@<pi> "mestro-upgrade --reboot - $(sha256sum ~/.cache/pi/buildroot/output/images/Image | cut -d' ' -f1)"
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

mestro owns the radio (feature `wifi-setup`, `src/system/wifi/`):
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
network. Addresses and paths are configurable in `[wifi]`
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
- The first network scan after boot can be incomplete.

## Follow-up backlog

- settings UI: `[window] scale` picker (needs the app restart
  flow — SLINT_SCALE_FACTOR is read at backend init)
- kiosk polish: hide the clock until the first NTP sync (boots
  into 1970 for ~a minute on a fresh image)
- USB-C gadget network as a rescue path
- release pipeline: versioned images + checksums as release assets
