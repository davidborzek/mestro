# Configuration

mestro reads one TOML file and keeps the changes made on the
device in a second one. Everything has a working default — a
fresh device needs nothing but the connection to Music Assistant,
and the web app's setup wizard writes that.

[`mestro.example.toml`](../crates/mestro/mestro.example.toml) lists every key
with its default and an explanation. This page is the map.

## Where things live

| | Path | Written by |
|---|---|---|
| config file | `$MESTRO_CONFIG`, else `$XDG_CONFIG_HOME/mestro/config.toml` (on the player image: `/data/config/mestro/`) | you |
| device settings | `$XDG_STATE_HOME/mestro/settings.toml` (on the player image: `/data/state/mestro/`) | the settings page, the web app |

The **config file** is yours: mestro never writes it. It can also
be a directory — every `*.toml` inside is merged in name order
(`config.toml`, `theme.toml`, `vu.toml`, …); a key set in two
files takes the later file's value and logs a warning.

The **device settings** hold only what was changed on the device,
nothing else. A key there wins over the config file; **Reset** in
the settings (per key, or *Reset all*) removes it, and the config
file value is back.

## Precedence

```
built-in defaults  <  config file  <  device settings  <  MESTRO_* env  <  CLI flags
```

## When something is wrong

A broken key never takes the device down. An unknown key, a
misspelled value or a wrong type drops **that key only** — the
rest of the configuration stays in force. mestro then shows a
toast at startup and lists every ignored key with the reason at
the top of the settings page.

A file that is not valid TOML at all is ignored as a whole and
mestro starts on the defaults. If that leaves the connection to
Music Assistant empty, mestro says so instead of silently
starting the setup wizard — you can fix the file and restart, or
set the device up again on purpose.

## Sections

| Section | What it controls |
|---|---|
| `[window]` | size, fullscreen, UI scale for small panels |
| `[display]` | backlight brightness and standby dimming |
| `[music-assistant]` | server URL, player, access token |
| `[theme]` | accent: fixed amber or following the cover art |
| `[ui]` | nav rail style, on-screen keyboard layout, auto-open now playing |
| `[background]` | blurred cover behind the views |
| `[now-playing]` | layout, inline meter, quality line, transport buttons, fonts |
| `[hub]` | home screen layout and the order of its destinations |
| `[clock]` | standby clock face, custom faces, standby background |
| `[osd]` | which notifications show, where and how long |
| `[vu]` | VU source, the faceplates and their order, custom skins |
| `[keyboard]` | key bindings for USB remotes and keyboards |
| `[remote]` | the web app / web endpoint |
| `[wifi]` | the setup hotspot: interface, address, power save, country (the player image) |
| `[audio]` | squeezelite: output device, player name, server, extra flags (the player image) |
| `[system]` | the timezone (a device setting the web app writes) |

Deep dives:
[standby clocks and backgrounds](standby.md) ·
[VU faceplates and custom skins](vu.md) ·
[the web app and remote protocol](remote.md)

### `[window]`

```toml
[window]
size = "800x480"     # logical size; --size beats it
fullscreen = false
scale = 1.5          # 0.5..3.0 — larger UI on small panels (5" 800x480)
```

### `[display]`

The backlight. mestro picks the backend itself: the Raspberry Pi
firmware when the firmware drives the panel's touch (it owns that
panel's bus then), `/sys/class/backlight` elsewhere, nothing in a
VM. Without a backlight the keys are ignored and the settings hide
the page.

```toml
[display]
brightness = 80             # 1..100 %; unset = keep what the system booted with
standby-brightness = 15     # 0..100 % while the standby clock shows
backlight = "auto"          # auto | firmware | sysfs | none
```

### `[music-assistant]`

```toml
[music-assistant]
url = "ws://ma.example.com:8095/ws"
player = "Living Room"      # empty = the device's own squeezelite, else the first playing/available player
# token = "…"               # usually written by the web app
```

### `[clock]`

```toml
[clock]
style = "digital"           # digital | analog | a [clock.skins] name

[clock.background]
source = "black"            # black | cover | folder
dim = 0.6
blur = false               # blurred backdrop instead of sharp
folder = "/data/backgrounds"
interval = 10               # minutes per picture
```

### `[keyboard]`

USB remotes and keyboards send keys; mestro maps them to
commands. The defaults follow VLC/Kodi conventions (arrows,
Enter, Escape, Home, +/-). Override single keys:

```toml
[keyboard]
right = "cycle-forward"
escape = "back"
m = "home"
```

Commands: `cycle-forward` `cycle-back` `back` `home` `play-pause`
`next-track` `prev-track` `volume-up` `volume-down`.

### `[wifi]`

The first-boot WiFi setup (player images only; config-file only —
never in the settings editor). All values resolve at runtime, so
one binary covers every board: `interface = "auto"` picks the
first wireless netdev (an entry under
`/sys/class/net/<if>/wireless`, an up one preferred); the setup
hotspot's DHCP pool and the captive portal derive from
`ap-address`. Garbage falls back to the default with a config
problem.

`power-save = false` (the default) switches the interface's wifi
power save off — brcmfmac enables it by default and its periodic
wakeup costs the link ~300 ms latency spikes and stream
reconnects; mestro re-applies the state after every association.
`country` pins the regulatory domain (ISO 3166-1 alpha-2, e.g.
`"DE"`): without it the kernel stays on the passive world regdom
`00` (5 GHz restricted). The image ships `regulatory.db` — an
empty `country` leaves the kernel's regdom alone.

wpa_supplicant's control socket directory and the persistent
credentials file are build settings of the image, not config
(CONTRIBUTING.md, "Building for another image"). A config that still
carries `ctrl-dir` or `conf` loads: both keys are reported as
unknown and ignored.

```toml
[wifi]
interface = "auto"            # auto | an interface name
ap-address = "10.42.0.1/24"    # private a.b.c.d/16-29; DHCP pool = .10-.50
power-save = false            # off by default — the latency spikes
# country = "DE"              # ISO 3166-1 alpha-2 regdom; empty = kernel default
```

### `[system]`

The timezone picker (web app → Settings → System; player images
built with the `timezone` feature). `timezone` is an IANA zone
name and a normal device setting: **empty means never chosen** —
the first paired browser may still set it (see
[remote.md](remote.md#timezone-feature-timezone)); once it holds a
zone, only an explicit choice in the picker changes it. A desktop
build has no picker and follows the OS timezone.

mestro writes `TZ=<zone>` into an env file of its own, `/data/state/mestro/mestro.env`
(whole file, rewritten only when the zone changes; never edit it), which the
init script sources before it starts mestro. Your own `/data/mestro.env`
(`RUST_LOG`, `SLINT_DRM_*`, …) is sourced after it and wins — mestro never
writes it. Where these files, the zoneinfo database and the `/etc/localtime`
link live is a build setting of the image, not config
(CONTRIBUTING.md, "Building for another image"). The capability is
on only while the zoneinfo database exists.

Applying a zone **restarts the player** (the same exec the
settings restart uses, with the new `TZ` in the new process's environment): a live
switch would need an `unsafe` `tzset`, and chrono caches the local
offset, so a restart is the one reliable way. It takes a few
seconds and the web app reconnects by itself.

```toml
[system]
# timezone = "Europe/Berlin"  # written by the web app; empty = never chosen
```

### `[audio]`

squeezelite's settings (the player image, built with the `audio-output`
feature; web app → Settings → Audio). Every value is optional: **empty
means mestro derives it**. `output`, `name` and `server` are normal
device settings the web app writes; `extra` is config-file only.

| key | empty means | set |
|---|---|---|
| `output` | `hw:CARD=<id>` of a USB DAC (card driver `USB-Audio`), else an I2S/HAT card — never the built-in jack (`bcm2835…`) or HDMI (`vc4-hdmi`); nothing to pick → squeezelite's own default (a warning is logged). A DAC that enumerates slowly gets ~10 s before mestro decides; no hotplug — a DAC plugged in later is chosen in the web app, or found on the next boot | an ALSA device, e.g. `hw:CARD=S3`. The output picker in the web app sets it |
| `name` | `mestro-XXXX`: the last 4 hex digits of the wifi interface's MAC (the player id when there is no wifi), uppercase — the hotspot's name uses the same digits | the player's name in Music Assistant |
| `server` | the host of `[music-assistant] url` (`http://192.168.3.13:8095` → `192.168.3.13`; slimproto's port 3483 is squeezelite's default); none without a url → squeezelite discovers the server itself | the Music Assistant host (or `host`) to connect to |
| `extra` | none | further squeezelite flags, passed verbatim and split on whitespace (`-r -Z 100`); not in the web app |

```toml
[audio]
# output = "hw:CARD=S3"
# name = "HiFi"
# server = "192.168.3.13"
# extra = "-r -Z 100"
```

## squeezelite's env file (image only)

On the player image mestro owns squeezelite's config. It composes
`/data/state/mestro/squeezelite.env` — `OUTPUT`, `NAME`, `SERVER`, `EXTRA` — from the
`[audio]` settings (a non-empty setting wins, an empty one falls back to
the derived value above) and writes it whole, only when its content
changes. The init script sources **only that file**; without it squeezelite
runs on its own defaults. Nothing about it is a user file: edit the settings,
not the env file (mestro would overwrite it).

mestro derives the file at startup and again whenever an `audio.*` or
`music-assistant.*` setting changes (an info line in the log). When the content
changed and squeezelite runs, it restarts **once** (after the boot's own start
has finished); nothing changed, nothing is written and nothing restarts.
Changes are coalesced: a burst restarts squeezelite once, for the newest
settings. While an output switch from the web app is in flight, the update
waits (up to ~60 s) instead of being dropped.

The output picker (Settings → Audio) sets `audio.output` through the normal
settings path and the file follows. If squeezelite does not stay up on the
new device, the previous setting value returns and the picker shows an error
([remote.md](remote.md#audio-output-feature-audio-output)). A factory reset
clears the settings; the file is derived again.

## Environment and command line

| env | effect |
|---|---|
| `MESTRO_CONFIG` | config file or directory |
| `MESTRO_TOKEN` | Music Assistant access token (when not in the files) |
| `MESTRO_MA_URL` | overrides `[music-assistant] url` |
| `MESTRO_PLAYER` | overrides `[music-assistant] player` |
| `MESTRO_CLOCK` | overrides `[clock] style` |
| `MESTRO_REMOTE_PORT` | overrides `[remote] port` (the player image sets 80) |
| `MESTRO_OUTPUT` | DRM output for the direct-render backend (`DSI-1`, `HDMI-A-1`) |
| `RUST_LOG` | log filter, default `mestro=info` |

The image's paths and commands are build settings (`MESTRO_*` at
`cargo build` time, not run-time env). `mestro-os`'s one `build.rs`
owns the wpa trio (`MESTRO_WPA_CTRL_DIR`, `MESTRO_WPA_CONF`,
`MESTRO_WPA_TEMPLATE`), the squeezelite pair
(`MESTRO_SQUEEZELITE_ENV`, `MESTRO_SQUEEZELITE_RESTART`), the
timezone trio (`MESTRO_ENV_FILE`, `MESTRO_ZONEINFO_DIR`,
`MESTRO_LOCALTIME`) and the log locations (`MESTRO_LOG_DIR`,
`MESTRO_STATUS_COMMAND`). Defaults and meaning: CONTRIBUTING.md,
"Building for another image".
| flag | effect |
|---|---|
| `--config PATH` | config file or directory |
| `--size WxH` | window size |
| `--fullscreen` | fullscreen window |
| `--kiosk` | 5" panel sized window for trying it out on a desktop |
| `--mock` | fake playing track, no server needed |
| `--minimal` | UI only, no provider |
| `mestro ctl <command>` | send a command to the running instance |
