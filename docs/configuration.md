# Configuration

mestro reads one TOML file and keeps the changes made on the
device in a second one. Everything has a working default — a
fresh device needs nothing but the connection to Music Assistant,
and the web app's setup wizard writes that.

[`mestro.example.toml`](../mestro.example.toml) lists every key
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
| `[wifi]` | the setup hotspot's device paths (the player image) |
| `[audio]` | the squeezelite env file the audio output picker edits (the player image) |
| `[system]` | the timezone and the files it is applied through (the player image) |
| `[diagnostics]` | what the web app's diagnostics download collects (the player image) |

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
player = "Living Room"      # empty = the one playing
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

```toml
[wifi]
interface = "auto"            # auto | an interface name
ctrl-dir = "/run/wpa_supplicant" # wpa_supplicant's control socket dir
conf = "/data/wpa_supplicant.conf" # the persistent credentials (must match the boot scripts)
ap-address = "10.42.0.1/24"    # private a.b.c.d/16-29; DHCP pool = .10-.50
power-save = false            # off by default — the latency spikes
# country = "DE"              # ISO 3166-1 alpha-2 regdom; empty = kernel default
```

### `[audio]`

The audio output picker (web app → Settings → Info; player images
built with the `audio-output` feature, config-file only). Defaults
are the image's paths — a desktop or another board points them
elsewhere. The capability is on only while `env-file` exists: a
machine without squeezelite shows no audio row.

`env-file` is the file squeezelite's init script sources; the
picker rewrites its `OUTPUT=` line and nothing else — comments,
other keys, quoting and CRLF line endings stay as they were.
`restart-command` (split on whitespace, run without a shell) applies
the change. `asound-dir` is where the playback devices are listed
from (`cards` plus `card<N>/pcm<D>p`).

```toml
[audio]
env-file = "/data/squeezelite.env"
restart-command = "/etc/init.d/S60squeezelite restart"
asound-dir = "/proc/asound"
```

### `[system]`

The timezone picker (web app → Settings → Info; player images
built with the `timezone` feature). `timezone` is an IANA zone
name and a normal device setting: **empty means never chosen** —
the first paired browser may still set it (see
[remote.md](remote.md#timezone-feature-timezone)); once it holds a
zone, only an explicit choice in the picker changes it. A desktop
build has no picker and follows the OS timezone.

The other keys are paths, config-file only, defaulting to the
image: `env-file` is what the init script sources before it starts
mestro (the picker writes `TZ=<zone>` there — one line, the rest
of the file stays), `zoneinfo-dir` is the database a zone must
exist in (path traversal and non-zones are refused), `localtime`
is the system zone link read to learn the running zone when `TZ`
is unset. The capability is on only while `zoneinfo-dir` exists.

Applying a zone **restarts the player** (the same exec the
settings restart uses, with the new `TZ` in the new process's environment): a live
switch would need an `unsafe` `tzset`, and chrono caches the local
offset, so a restart is the one reliable way. It takes a few
seconds and the web app reconnects by itself.

```toml
[system]
# timezone = "Europe/Berlin"  # written by the web app; empty = never chosen
env-file = "/data/mestro.env"
zoneinfo-dir = "/usr/share/zoneinfo"
localtime = "/etc/localtime"
```

### `[diagnostics]`

What Settings → Info → *Download diagnostics* puts into the report
([remote.md](remote.md#diagnostics-report-get-diagnostics)) besides
the version, the redacted settings and the wifi status. Config file
only; the defaults are the player image's paths, and anything that
is not there is skipped — no key is required to exist.

`logs` are tailed (the last `log-lines`, 1–5000, of each; at most
256 KiB are read) — the image keeps mestro's log in RAM at
`/tmp/mestro.log` (`.old` is the previous run) and the OTA scripts'
at `/tmp/ota.log`. `files` are small text files copied whole (a
tail of the same size cap), `status-command` is run without a shell
for at most 5 s and its output added (`""` = none). Text from all
three passes the secret redaction; paths that are secrets by nature
(host keys, the wpa conf, `web-pin`, the settings overlay, …) are
refused whatever is configured.

```toml
[diagnostics]
logs = ["/tmp/mestro.log", "/tmp/mestro.log.old", "/tmp/ota.log"]
log-lines = 200
files = ["/proc/version", "/proc/asound/cards"]
status-command = "/usr/sbin/mestro-upgrade --status"
```

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

| flag | effect |
|---|---|
| `--config PATH` | config file or directory |
| `--size WxH` | window size |
| `--fullscreen` | fullscreen window |
| `--kiosk` | 5" panel sized window for trying it out on a desktop |
| `--mock` | fake playing track, no server needed |
| `--minimal` | UI only, no provider |
| `mestro ctl <command>` | send a command to the running instance |
