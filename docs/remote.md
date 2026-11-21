# The embedded remote endpoint (`[remote]`)

mestro serves a small web endpoint on the LAN: an **installable
PWA** whose main screen is a **device remote** — player power, the
player views, volume, brightness — plus a **WS JSON channel**.
One port (default `8787`; the player image runs 80 via
`MESTRO_REMOTE_PORT`, so URLs carry no port), no TLS — LAN-only by
design. One listener: the setup hotspot's captive portal rides the
same port, gated per connection on the peer address (below).

```
GET  /                    the app shell (web/dist/index.html, embedded)
GET  /assets/*            hashed app files — immutable cache
GET  /icons/*             PWA icons — 1-day cache
GET  <unknown route>      the shell (SPA fallback; no dot in the
                          last path segment)
GET  /pair/status         {"paired": true|false} (cookie check)
POST /pair                {"pin":"…"} → 204 + mestro_pair cookie
GET  /cover?v=<n>         the current cover as JPEG (auth);
                          ?blur=1 serves the pre-blurred,
                          pre-dimmed backdrop instead
GET  /vu-preview?skin=<name>&v=<hash>   a VU skin's preview PNG (auth)
GET  /clock-preview?style=<name>&v=<hash>   a clock style's preview PNG (auth)
GET  /ws                  WS upgrade → JSON protocol (auth)
POST /ota                 system-image upload (feature `ota`, auth)
anything else             404 (missing files, non-GET)
```

Static assets need no auth. `/ws`, `/ota`, `/cover`, `/vu-preview`
and `/clock-preview` accept either `?pair=<PIN>` or the pairing
cookie.

## Configuration

```toml
[remote]
enabled = true    # serve at all (false: no server, no PIN file)
port = 8787       # on all interfaces
```

`MESTRO_REMOTE_PORT` (env) overrides the port — the player image
exports `MESTRO_REMOTE_PORT=80` in its init script, so the wizard,
the QR code and the captive portal are reachable at
`http://<ip>/` without a port. mestro omits `:80` from every URL
it shows or advertises; any other port is spelled out.

## mDNS announcement

The player announces itself as a DNS-SD service, so clients (the
Home Assistant integration) find it without knowing an address:

```
_mestro._tcp.local.    port = the web port ([remote] port)
instance:              mestro-<id>
TXT  id=<mac>          the stable player id (as in hello)
     version=<app version>
     protocol=1        the wire protocol revision (as in hello)
     name=<label>      the MA player name, else the hostname
```

The announcement carries no secret: pairing still needs the PIN.
It runs on the regular networks (wifi STA, ethernet; multicast
UDP 5353) and announces **IPv4 addresses only** — the web endpoint
listens on `0.0.0.0`, an IPv6 address would not be reachable. It
is **stopped while the setup hotspot is in play** — the responder
never sits on the hotspot interface, dnsmasq owns that network.
The wifi manager asks the announcer to pause (a handshake, not the
auth `ap_mode` flag) at the start of the hotspot transition and
waits up to 3 s until the announcer confirms the daemon is shut
down, then builds the hotspot; a stuck announcer never blocks the
hotspot, the manager warns and goes on. The pause ends when the
station network is back (a failed hotspot stays paused until then).
Hotspot clients that are not paired see the same identity (`id`,
`name`, version) through `hello` anyway, so nothing is hidden by
the pause. Address changes (DHCP, link up/down) are followed
automatically. If the responder cannot start (UDP 5353 held
exclusively by another process), mestro logs a warning and retries
every 30 s; the player stays reachable by address. `[remote] enabled = false` turns it off
with the endpoint. Check with `avahi-browse -rt _mestro._tcp`.

## Auth model

- A **pairing PIN** (8 chars, alphabet without `0/O/1/I`, from
  `/dev/urandom`) gates the protocol. It is **restart-stable** —
  persisted `0600` in the XDG state dir (`web-pin`) so the URL a
  phone scanned survives app updates.
- The player URL carries the PIN in the **hash**
  (`http://host:port/#pair=…`): HTTP middlemen never log a
  fragment. The PWA consumes the hash once (drops it from the
  address bar) and `POST`s it to `/pair`.
- `POST /pair` (after 5 wrong attempts: **429 for 30 s**) trades
  the PIN for a cookie:

  ```
  Set-Cookie: mestro_pair=<token>; Path=/; Max-Age=31536000;
              HttpOnly; SameSite=Strict
  ```

  The token is `sha256("mestro-pair:" + PIN)` in hex — the PIN
  itself never sits in a header a browser replays. A paired phone
  then opens `/ws`, `/ota` and `/cover` without the PIN; the PIN
  compare stays constant-time. The PIN is never rotated
  automatically (`reset-connection` keeps it; re-pairing means a
  new PIN file).
- The pairing URL is shown on the player (Connection page +
  first-run QR code) and logged once at startup. `GET /pair/status`
  tells the PWA whether it is paired.

### The setup hotspot (AP mode, feature `wifi-setup`)

While the player runs its setup hotspot (no usable wifi conf, or a
configured player lost its route), an **unpaired** client may use
exactly two things: `hello` and the wifi vocabulary
(`scan`/`join`/`status`). There is no second listener for this —
the single port decides per connection from the **peer address**:

- the hotspot is confirmed up (supplicant answering, address on,
  dnsmasq running — a failed start leaves the gate closed), and
- the peer lies inside the hotspot subnet (`[wifi] ap-address`,
  default `10.42.0.1/24`), and
- the peer is not the gateway itself.

The address rule cannot be spoofed from outside: completing a TCP
handshake needs our SYN-ACK to reach the peer, and a route into
the hotspot subnet exists only over the hotspot interface — a LAN
host cannot finish a connection with a forged `10.42.0.x` source.
Known limit: if another interface gains an address inside the
hotspot subnet *after* the hotspot started (the clash check runs at
start), a host on that network could pass as a hotspot client. It
would get exactly what any phone in the open hotspot gets — the
wifi ops — so this is accepted rather than firewalled.

`scan` returns the **complete** list: right after boot the supplicant
answers `SCAN_RESULTS` at once with the cache of an earlier, partial
scan, so the manager attaches to the supplicant's event stream
(`ATTACH`, before the scan starts), waits for
`CTRL-EVENT-SCAN-RESULTS` (or `-SCAN-FAILED`) and only then reads
the results; it detaches afterwards. Without an event stream it
polls until the network count is unchanged over two consecutive
polls. Both are bounded by the 13 s scan budget, after which the
op returns what the supplicant has.

The classic captive-portal probes go to port 80: on another
`[remote] port`, only clients that honor DHCP option 114 open the
setup page on their own.
The verdict is re-checked when a wifi op executes (a request that
queued while the hotspot was up must not run after it ended), and
the socket closes the moment AP mode ends.

The captive portal rides the same rule: for an AP-trusted
connection, the OS connectivity probes (`/generate_204`,
`/hotspot-detect.html`, `/ncsi.txt`, …) and any request naming a
foreign host 302 into `/setup/wifi`; dnsmasq announces the portal
URI up front via DHCP option 114 (RFC 8910). Every other
connection — the LAN included, even while the hotspot is up —
keeps the regular behavior.

### Threat model (LAN)

| actor | can do |
|---|---|
| no PIN, no cookie | fetch the app shell and assets, nothing else (`401`) |
| paired | read the effective config **redacted**, write/reset any setting, reset-all, restart, run MA probe/login/players, drive the transport (`ctl`, incl. `vu-select`) and volume, mirror the player views, upload a system image (`ota`), switch the audio output (`audio`) |
| unpaired, from the setup hotspot (peer inside the hotspot subnet, hotspot confirmed up) | wifi ops only (`scan`/`join`/`status`) and `hello` — no settings, no player data, **no pushes**; the socket closes the moment AP mode ends (a successful join leaves it) |
| on the wire | plaintext HTTP/WS on a trusted LAN — no TLS, treat the LAN as the trust boundary |

Secrets never leave the device over the wire: `settings.state`
redacts `token`/`password` keys. The stored MA token is only ever
attached to requests against the **stored**
`music-assistant.url`. The raw token crosses the UI boundary only
through the process-internal `InternalOp::MaToken`, which the
public protocol parser rejects.

## Installing on a phone

- **Android/Chrome:** open the pairing URL once, sign in to the
  Wi-Fi landing page if needed, then *Add to Home screen* /
  *Install app*. Chrome allows installs over plain `http` on LAN
  addresses but shows no lock icon; a service worker (offline
  cache) is **not** registered — mestro deliberately ships none,
  because `http://<lan-ip>` is not a secure context and SWs would
  be ignored anyway.
- **iOS/Safari:** open the URL, *Share → Add to Home Screen*.
  iOS installs PWAs from Safari regardless of the scheme; the
  app opens full-screen via the `apple-mobile-web-app-*` metas
  and keeps the pairing cookie in its own storage partition.
- The cookie survives restarts of mestro and the phone (1 year,
  bound to the PIN — a new PIN file invalidates it).
- **HTTPS limits:** what needs a secure context (service worker,
  `MediaSession`, persistent storage) is unavailable over plain
  HTTP. For a locked-down deployment, put mestro behind a
  **reverse proxy with TLS** (e.g. Caddy/nginx on the LAN, or a
  VPN with a DNS name): forward `/` (assets), `/ws` (with
  `Upgrade`/`Connection` headers) and `/cover`; add
  `X-Forwarded-Proto` handling is not needed — mestro only builds
  `http://` URLs for the QR code. Do not expose the port to the
  internet: the protocol has no TLS and no rate limit beyond
  `/pair`.

## Protocol (WS, JSON)

Requests carry an `id` (echoed) and a domain-split vocabulary:

```jsonc
{"id": 1, "op": "hello"}
{"id": 2, "op": "settings", "action": "state"}
{"id": 3, "op": "settings", "action": "schema"}
{"id": 4, "op": "settings", "action": "set", "key": "music-assistant.player", "value": "wohnzimmer"}
{"id": 5, "op": "settings", "action": "reset", "key": "music-assistant.token"}
{"id": 6, "op": "settings", "action": "reset-all"}
{"id": 7, "op": "settings", "action": "restart"}
{"id": 8, "op": "ma", "action": "probe",   "url": "http://ma.local:8095"}
{"id": 9, "op": "ma", "action": "login",   "url": "…", "username": "…", "password": "…"}
{"id": 10, "op": "ma", "action": "players", "url": "http://ma.local:8095"}
{"id": 11, "op": "ctl", "cmd": "play-pause"}
{"id": 12, "op": "player", "action": "volume", "value": 42}
{"id": 13, "op": "audio", "action": "outputs"}
{"id": 14, "op": "audio", "action": "set", "output": "hw:CARD=S3"}
```

`settings` `state` reports the merged config (defaults not
folded in) plus `effective` — the same config with the defaults
folded in, so an editor can show every field's current value —
both redacted, alongside `overrides` (the keys that deviate
from the config file). `schema` serves the settings schema the
web settings page renders from: sections mirror the device's
settings pages, fields carry the dotted key, label, kind and
range.

`hello` reports the app version, the pairing URL, the player
identity and `capabilities` (the flat `ota` field stays for the
old wizard bundle):

```jsonc
{"id": 1, "ok": true, "data": {
  "app": "0.1.0 (abc1234)", "version": "0.1.0 (abc1234)",
  "protocol": 1,           // wire revision, bumped only on a breaking change
  "id": "a8724d0c4171",    // stable player id: a MAC, lowercase hex, no colons
  "name": "Wohnzimmer",    // the configured MA player name, else the hostname
  "remote_url": "…", "ota": true,
  "capabilities": {"ota": true, "display": true, "wifi": true, "factory_reset": true,
                   "audio_output": true}
}}
```

`id` is the MAC of the first physical network interface (by
name; one with a `device` entry in `/sys/class/net`, so no
bridges, veth or tunnels). sysfs keeps the address while a link
is down and the hotspot runs on the same radio, so the id
survives restarts and interface up/down. A host without any
physical interface (container, minimal VM) generates a random
12-hex id once and keeps it in the state dir
(`$XDG_STATE_HOME/mestro/device-id`, written atomically), so it is
stable across restarts and two installs never share one. If the
state dir is not writable the id derives from the hostname
instead (stable, warned once).
`id` and `name` are also in the mDNS record (below).

Replies: `{"id": …, "ok": true, "data": …}` or
`{"id": …, "ok": false, "error": …}`. Acks (e.g. `player`) reply
`{"id": …, "ok": true}` with no `data`.

`ctl` accepts the same commands as `mestro ctl`: transport,
volume, navigation and standby (`play-pause`, `next-track`,
`prev-track`, `home`, `now-playing`, `queue`, `vu`,
`standby-toggle`, …). Settings commands are rejected; use the
`settings` op.

`vu-select:<index>` opens the VU view on the skin at that
0-based index of `view.vu_skins` (the state frame lists the
skins in cycle order, so `vu_skins[vu_index]` is the active
one). An index outside the cycle, or one that is not a plain
non-negative integer, is an error reply and changes nothing
(`{"ok": false, "error": "vu-select: no skin at index 7"}`,
respectively `unknown command`):

```jsonc
{"id": 13, "op": "ctl", "cmd": "vu-select:2"}
```

`player` carries the one transport value `ctl` cannot express:
absolute volume. It maps onto the same source command the
player's own slider uses (the volume OSD suppression included).
Playback beyond volume — queue, seek, shuffle — lives in
Music Assistant's own web UI (`ma_url` in the state frame),
and player power is `ctl`'s `standby-toggle`:

```jsonc
{"op": "player", "action": "volume", "value": 0..100}
```

Success confirms UI-thread dispatch, not completion at the
player — the state frame reports the truth.

### Audio output (feature `audio-output`)

The picker behind Settings → Audio → Audio output. Paired only —
an unpaired hotspot client gets `pairing required`, like for every
other device op. `hello` reports `capabilities.audio_output`: true
only when the build has the feature **and** the squeezelite env file
exists (`[audio] env-file`); without it both ops answer
`audio output not available` and no file is touched.

```jsonc
// → {"id": 13, "op": "audio", "action": "outputs"}
{"id": 13, "ok": true, "data": {
  "outputs": [
    {"id": "hw:CARD=Headphones", "name": "bcm2835 Headphones", "current": false},
    {"id": "hw:CARD=S3", "name": "Topping S3", "current": true}
  ],
  "current": "hw:CARD=S3"    // OUTPUT in the env file, null when unset
}}
// → {"id": 14, "op": "audio", "action": "set", "output": "hw:CARD=Headphones"}
{"id": 14, "ok": true, "data": {"output": "hw:CARD=Headphones"}}
// a device that does not start:
{"id": 14, "ok": false, "error": "squeezelite could not start on hw:CARD=S3 (squeezelite stopped right after the start) — back on the previous output"}
```

`outputs` lists the ALSA playback devices (`/proc/asound/cards`
plus `card<N>/pcm<D>p`; capture-only cards are no output). A
card's first playback device is `hw:CARD=<id>`, further ones
`hw:CARD=<id>,DEV=<n>`; `current` marks the entry the file's
`OUTPUT` names (`,DEV=0` is the plain card).

`set` accepts only an id from that list (anything else:
`unknown audio output …`, the file untouched), rewrites only the
`OUTPUT=` line of the env file (atomic; comments, other keys,
quoting and CRLF stay) and runs `[audio] restart-command`. If
squeezelite is not running 3 s after the restart (or the command
fails), the previous file is restored byte for byte, squeezelite
is restarted again and the error names the device that failed —
a bad pick never leaves the player silent. The reply arrives after
that watch (about 3.5 s). One switch runs at a time: a second
`set` while one is in flight fails at once with `another switch is
in progress` and touches nothing.

### Push frames (no `id`)

Every **paired** socket receives `{"event":"state","data":…}` right
after the upgrade (a snapshot, always the first frame: the hub
publishes an initial one — `connected: false`, an empty player —
before the listener accepts connections) and on every change afterwards —
coalesced to at most one frame per 100 ms, except player view
changes (np/queue/vu/standby/vu-index), which are published at
once. Unpaired sockets (the hotspot's setup clients) receive no
pushes. A lagged socket gets a fresh snapshot instead of a replay
burst. After any successful settings write (any source: web,
player, `ctl`) every paired socket also receives
`{"event":"settings"}` — no payload, clients refetch the
settings state.

```jsonc
{
  "connected": true,            // player ↔ Music Assistant
  "ma_url": "http://ma.local:8095",  // the MA web UI, "" = unknown
  "player": {
    "name": "wohnzimmer",      // configured/auto-picked player
    "powered": true, "playing": true, "loading": false,
    "has_media": true, "volume": 42,
    "title": "…", "artist": "…",
    "cover": "/cover?v=3",     // "" = none
    "accent": "#f3bd6e"        // from the cover palette, "" = default
  },
  "view": {
    "now_playing": false, "queue": false, "vu": false, "standby": true,
    "vu_index": 2,             // the active VU skin, 0-based
    "vu_count": 10,
    "vu_skins": ["night", "cream", "round", "wall", "wall-light",
                 "arc", "orbit", "modern", "spectrum", "bars"]  // cycle order = vu_index order
  },
  "display": {
    "supported": true,         // a backlight exists (capabilities.display)
    "brightness": 80           // 1..100, writes: settings set display.brightness
  }
}
```

`ma_url` derives from the configured `music-assistant.url`'s
ws endpoint: `ws`→`http`, `wss`→`https`, the trailing `/ws`
stripped. `vu_index`/`vu_count` mirror the player's
`vu-view-index`/`vu-view-count`; `vu_skins` names the cycle's
entries in the same order. VU skin switching rides `ctl`:
`cycle-forward`/`cycle-back` (they cycle the skin only while the
VU view is up) or `vu-select:<index>` (any time, opens the VU
view).

`GET /cover?v=<n>` serves the current cover as a JPEG (quality
85, encoded once per artwork version, `private` cache — the
versioned URL makes it effectively immutable). `404` while no
cover exists.

## Preview thumbnails

The schema's `vu.cycle` and `clock.style` options carry a
`preview` URL — `/vu-preview?skin=<name>&v=<hash>` and
`/clock-preview?style=<name>&v=<hash>` — that serves a PNG of
the render-once preview the device's own pickers show. The `v`
hash versions the render: it changes whenever a
preview-relevant setting does (the cycle, info lines, glow, LED
meters, gradient; the style list and palette for clocks), so an
edit changes the URL and the client refetches. The web thread
serves these from a PNG cache keyed by (kind, name, version); a
cache miss renders on the UI thread through the bridge (the
internal `preview` op — the public protocol rejects it) once
per key. A custom skin whose art files changed on disk without
a config edit stays stale until the day-long browser cache
expires.

Settings writes map 1:1 onto the player's `control::Command`
vocabulary (`SetSetting`/`ResetSetting`/`ResetAllSettings`) — one
dispatch path for touch UI, keyboard and web.

## Setup flow (wizard)

`intro → url (probe) → login (skipped for schema < 28) → player →
done`. Login persists `music-assistant.url` + `token`; the player
step persists `url` + `player`. Connection changes **rewire the
live provider session** (`SourceCommand::Reconnect`) — no app
restart is needed; the done screen polls until the provider
reports connected. The wizard takes over the app on an
unconfigured player; afterwards the remote tab is the default.

While `music-assistant.url` is empty the player shows the first-run
overlay (QR code + wizard URL) until a verified connection exists.

## Threading

The socket side runs on its own tokio thread. Settings and slint
handles are `!Sync`, so every state op round-trips onto the UI
thread through the `web-op` callback (`Bridge`); blocking work (UI
roundtrips, `ureq`, cover JPEG encodes) runs on the blocking pool
or the socket thread, never on the async worker. The state frames
are built on the UI thread (the player update loop plus a 250 ms
poll of the player view props) and cross into the web thread via a
broadcast channel.
