# mestro

[![CI](https://github.com/davidborzek/mestro/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/davidborzek/mestro/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/github/license/davidborzek/mestro)](LICENSE)
[![Release](https://img.shields.io/github/v/release/davidborzek/mestro?include_prereleases&sort=semver)](https://github.com/davidborzek/mestro/releases)

A touchscreen now-playing display for
[Music Assistant](https://music-assistant.io/). Put a small panel
next to your hi-fi, point mestro at your Music Assistant server,
and it shows and controls whatever plays there — cover art, the
track, a VU meter, the queue and your library.

mestro is a native Rust/[Slint](https://slint.dev) app that draws
straight to the display: no browser, no desktop, boots in seconds.
It ships as a ready-to-flash image for the Raspberry Pi 4 and runs
on any Linux desktop for trying it out.

> **Status: alpha.** Daily use on one setup (Pi 4 with a 5" DSI
> panel). Expect rough edges, and please report them.

<p align="center">
  <img src="docs/screenshots/now-playing.png" width="32%" alt="Now playing">
  <img src="docs/screenshots/hub.png" width="32%" alt="Home screen">
  <img src="docs/screenshots/standby.png" width="32%" alt="Standby clock">
</p>

## Features

- **Now playing** — cover art over its own blurred backdrop,
  title/artist/album, stream quality (codec, sample rate, bit
  depth), seekable progress, transport, volume. Two layouts: cover
  beside the text, or the cover edge to edge.
- **Accent from the cover** — the whole UI can take its accent
  color from the artwork, or stay in a warm amber.
- **VU meters** — analog faceplates, a spectrum analyzer, an inline
  LED meter; fed from squeezelite. Custom faceplates are a manifest
  plus SVG art ([docs/vu.md](docs/vu.md)).
- **Queue and library** — browse, search with an on-screen
  keyboard, play, add to the queue.
- **Standby** — a dim clock (digital, analog or your own face) over
  black, the cover or a slideshow of your pictures
  ([docs/standby.md](docs/standby.md)). Dims the backlight.
- **Set up from your phone** — on first boot the player opens its own
  WiFi hotspot; join it and pick your network. Then scan the QR code,
  sign in to Music Assistant and pick the player. No SSH, no editing
  files on the SD card.
- **Companion web app** — an installable app (add it to your home
  screen) for what Music Assistant doesn't cover: the player's power,
  what the display shows, the VU skin, the backlight, every device
  setting, system updates ([docs/remote.md](docs/remote.md)).
- **Remote control** — USB remotes and keyboards, `mestro ctl`, and a
  WebSocket channel on the LAN.
- **Robust config** — a broken setting is skipped and reported,
  never a black screen.

## Raspberry Pi image

The image boots straight into mestro (Buildroot, runs from RAM,
about 80 MB). Your settings, WiFi and pictures live on a small FAT
partition you can edit from any computer.

**Hardware:** Raspberry Pi 4 with a DSI touch panel (the official
7" display or a compatible 5"/7" panel). Audio out through
squeezelite to a USB DAC or HAT.

1. Flash the image to an SD card and boot.
2. The screen shows a WiFi QR code: join the hotspot `mestro-XXXX`
   with your phone. The setup page opens by itself (otherwise open
   `mestro.setup` in the browser) — pick your network and enter its
   password.
3. Back on your home WiFi, scan the new QR code on the screen, sign
   in to Music Assistant and pick the player.

If the player later loses its network, it opens the hotspot again
and keeps retrying the saved network every two minutes.

**Updates** install from the web app (Settings → Info → System
update) or over SSH, without re-flashing. Each update boots once on
trial and falls back to the previous version by itself if it does
not come up. Building the image yourself and adding extensions:
[image/README.md](image/README.md).

## Try it on a desktop

The binary embeds the web app at compile time — build it first
(`bun` comes with the nix dev shell):

```sh
cd web && bun install && bun run build

# nix dev shell (NixOS/nix): Rust, bun and the build deps
nix develop -c cargo run -- --kiosk

# or plain cargo, when your system provides Rust and the build deps
cd .. && cargo run -- --kiosk
```

`--kiosk` opens a window the size of a 5" panel. On first start
mestro shows the setup QR code — or set the server directly:

```sh
MESTRO_MA_URL=ws://my-ma-server:8095/ws cargo run -- --kiosk
```

`--mock` shows a fake playing track without any server.

## Configuration

Most things are set on the device (Settings) or from the phone.
Everything is also a TOML file — see
[docs/configuration.md](docs/configuration.md) and the commented
[`mestro.example.toml`](mestro.example.toml).

## Contributing

Dev setup, the code layout and the Music Assistant protocol notes:
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

mestro is free software under the
[GNU General Public License v3.0 or later](LICENSE).

It is built on [Slint](https://slint.dev), used under Slint's
GPLv3 option — which is what allows mestro to run on dedicated
devices like the player image at no cost.
