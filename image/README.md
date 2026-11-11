# mestro kiosk image

A flash-and-go SD image for Raspberry Pi 4 (aarch64): mestro on
the DRM plane (linuxkms, GPU-accelerated via vc4/v3d + Skia GL),
squeezelite with the VISEXPORT shm ring for the VU meters, and a
system that runs entirely from RAM — piCorePlayer-style.

## Layout

| Partition | Type | Content |
|---|---|---|
| 1 (boot) | FAT32 | kernel Image (with embedded initramfs), DTBs, firmware, `config.txt` |
| 2 (data) | FAT32 `MESTRODATA` | `mestro.toml`, `squeezelite.env`, mestro state/config — survives reflashes, editable on Windows/macOS |

- No root partition: the rootfs is an initramfs inside the kernel
  image. The SD card is only read at boot; zero wear afterwards.
- **No mestro config file, no CLI flags**: the service runs bare
  `mestro` — the linuxkms backend picks the connector's preferred
  DRM mode itself (`SLINT_DRM_MODE` overrides). First boot shows
  the pairing QR, the phone wizard + the settings UI write
  everything into `/data`. `squeezelite.env` is the one file a
  plain flash seeds
  - `squeezelite.env` — `OUTPUT` (bit-perfect ALSA device),
    `SERVER` (LMS), `NAME`, `EXTRA`; applied with
    `/etc/init.d/S60squeezelite restart`
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

## Upgrades over SSH

SSH access is key-only. Two ways to get a key in:

- **Bake it into your image** (git-ignored, never committed):
  put your public key at
  `board/mestro/rpi4/rootfs-overlay/root/.ssh/authorized_keys`
  before building.
- **Drop it on the data partition**: `/data/authorized_keys`
  (FAT, editable from any PC) overrides the baked one at boot
  (`S02data`).

The whole system is one file — the rootfs lives in the initramfs
embedded in the kernel `Image` on the boot partition. So an
upgrade is a file swap, no re-flashing:

```sh
# on the build machine
scp ~/.cache/pi/buildroot/output/images/Image root@<pi>:/tmp/new-Image
# optional: verify the transfer
sha256sum ~/.cache/pi/buildroot/output/images/Image

# on the pi (Image.old is the recovery: mv it back and reboot)
mestro-upgrade --reboot /tmp/new-Image <sha256>
```

Without `--reboot` the new image takes over at the next
`mestro-reboot` (never plain `reboot`: it hangs the initramfs
kiosk).

`mestro-upgrade` copies to `Image.new`, re-verifies the checksum
on the FAT before the swap, and keeps the previous kernel as
`Image.old`. Follow-up: an update button in the mestro web UI
(tryboot fallback as the next safety layer).

## Status / known untested

This tree is the skeleton; a full build has **not** been run yet.
Specifically:

- `configs/mestro_pi4_defconfig`: the mesa3d symbol set
  (`_GALLIUM_DRIVER_V3D`, `_OPENGL_EGL`, `_OPENGL_ES`, GBM)
  and the exact boot firmware file list need a first real
  `make` pass to verify.
- Squeezelite pin `45e711bb…` compiles VISEXPORT in theory
  (Makefile verified); unverified in a real toolchain.
- DSI panel backlight/touch overlay is not configured
  (`config.txt`), Pi 3/5 and Pi Zero 2 W configs are follow-ups.
- A release pipeline (GitHub Releases → `MESTRO_SITE`) replaces
  the local tarball flow later.

## Follow-up backlog

User-facing configuration should converge into the mestro UI
(phone/panel) instead of editing files on the data partition:

- settings UI: `[window] scale` picker (needs the app restart
  flow — SLINT_SCALE_FACTOR is read at backend init)
- settings UI: timezone picker writing `/data/timezone`
- settings UI: WiFi credentials writing `/data/wpa_supplicant.conf`
  + `S39wifi restart`
- settings UI: squeezelite.env editor + `S60squeezelite restart`
- web UI: OTA update button — upload an Image, verify its sha256,
  run mestro-upgrade, reboot (tryboot as the fallback layer)
- kiosk polish: hide the clock until the first NTP sync (boots
  into 1970 for ~a minute on a fresh image)

Infra follow-ups:

- Pi 5 (cortex-a76) and Zero 2 W (armv7) defconfigs
- release pipeline (GitHub Releases → `MESTRO_SITE`) replacing the
  local tarball flow
