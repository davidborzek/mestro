# The embedded remote endpoint (`[remote]`)

mestro serves a small web endpoint on the LAN: the **setup wizard
SPA** (phone/desktop browser) plus a **WS JSON channel** the wizard
speaks. A future generic remote-settings editor rides the same
channel. One port (default `8787`), no TLS — LAN-only by design.

```
GET  /           the wizard SPA (web/dist/index.html, embedded)
GET  /ws         WS upgrade → JSON protocol (auth required)
anything else    404; /ws without auth → 401
```

## Configuration

```toml
[remote]
enabled = true    # serve at all (false: no server, no PIN file)
port = 8787       # on all interfaces
```

## Auth model

- A **pairing PIN** (8 chars, alphabet without `0/O/1/I`, from
  `/dev/urandom`) gates the WS channel. It is **restart-stable** —
  persisted `0600` in the XDG state dir (`web-pin`) so the URL a
  phone bookmarked survives app updates.
- The PIN travels in the **URL hash** (`http://host:port/#pair=…`),
  never in the request line: HTTP middlemen logs see the bare URL.
  The SPA moves it into the WS query (`/ws?pair=…`) where it is
  compared **constant-time**.
- The wizard URL is shown on the kiosk (Connection page + first-run
  QR code) and logged once at startup.

### Threat model (LAN)

| actor | can do |
|---|---|
| no PIN | fetch the SPA shell, nothing else (`401`) |
| with PIN | read the effective config **redacted** (secrets → `null` + `<key>-set`), write/reset any setting, reset-all, restart the app, run MA probe/login/players |
| on the wire | plaintext HTTP/WS on a trusted LAN — no TLS, treat the LAN as the trust boundary |

Secrets never leave the device over the wire: `settings.state`
redacts `token`/`password` keys. The stored MA token is only ever
attached to requests against the **stored** `music-assistant.url`
(`ma.players` on a client-chosen URL gets no bearer). The raw token
crosses the UI boundary only through the process-internal
`InternalOp::MaToken`, which the public protocol parser rejects.

## Protocol (WS, JSON)

Requests carry an `id` (echoed) and a domain-split vocabulary:

```jsonc
{"id": 1, "op": "hello"}
{"id": 2, "op": "settings", "action": "state"}
{"id": 3, "op": "settings", "action": "set", "key": "music-assistant.player", "value": "wohnzimmer"}
{"id": 4, "op": "settings", "action": "reset", "key": "music-assistant.token"}
{"id": 5, "op": "settings", "action": "reset-all"}
{"id": 6, "op": "settings", "action": "restart"}
{"id": 7, "op": "ma", "action": "probe",   "url": "http://ma.local:8095"}
{"id": 8, "op": "ma", "action": "login",   "url": "…", "username": "…", "password": "…"}
{"id": 9, "op": "ma", "action": "players", "url": "http://ma.local:8095"}
{"id": 10, "op": "ctl", "cmd": "play-pause"}
```

`ctl` accepts the same commands as `mestro ctl`: transport,
volume, navigation and standby. Settings commands are rejected;
use the separate `settings` op. Existing WS pairing authentication
applies. Success confirms UI-thread dispatch, not completion at
the player:

```json
{"id": 10, "ok": true, "data": {"cmd": "play-pause"}}
```

Replies: `{"id": …, "ok": true, "data": …}` or
`{"id": …, "ok": false, "error": …}`.

Settings writes map 1:1 onto the kiosk's `control::Command`
vocabulary (`SetSetting`/`ResetSetting`/`ResetAllSettings`) — one
dispatch path for touch UI, keyboard and web.

## Setup flow (wizard)

`intro → url (probe) → login (skipped for schema < 28) → player →
done`. Login persists `music-assistant.url` + `token`; the player
step persists `url` + `player`. Connection changes **rewire the live
provider session** (`SourceCommand::Reconnect`) — no app restart is
needed; the wizard's done screen polls until the provider reports
connected.

While `music-assistant.url` is empty the kiosk shows the first-run
overlay (QR code + wizard URL) until a verified connection exists.

## Threading

The socket side runs on its own tokio thread. Settings and slint
handles are `!Sync`, so every state op round-trips onto the UI
thread through the `web-op` callback (`Bridge`); blocking work (UI
roundtrips, `ureq`) runs on the blocking pool, never on the async
worker.