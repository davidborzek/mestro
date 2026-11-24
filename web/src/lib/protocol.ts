// the JSON vocabulary of the embedded endpoint — mirrors
// crates/mestro-ipc/src/web.rs: `op` names the domain, `action` the verb

export type SettingsOp =
  | { action: "state" }
  | { action: "schema" }
  | { action: "set"; key: string; value: unknown }
  | { action: "reset"; key: string }
  | { action: "reset-all" }
  | { action: "restart" }
  // feature `wifi-setup` on the device: settings overlay, pairing
  // PIN and the MA connection keys go, then the app restarts into
  // first run. `wifi` also restores the wpa_supplicant.conf
  // template — or, without a template, removes the credentials
  // (the reply's `wifi` field says which)
  | { action: "factory-reset"; wifi: boolean };

// the `factory-reset` reply: the app restarts into first run;
// `wifi` says what happened to the credentials
export type FactoryResetData = {
  restarting: boolean;
  wifi: "kept" | "restored" | "removed";
};

export type MaOp =
  | { action: "probe"; url: string }
  | { action: "login"; url: string; username: string; password: string }
  | { action: "players"; url: string };

export type CtlCmd =
  | "play-pause"
  | "next-track"
  | "prev-track"
  | "now-playing"
  | "queue"
  | "vu"
  | "home"
  | "standby-toggle"
  | "cycle-forward"
  | "cycle-back";

export type PlayerOp = { action: "volume"; value: number };

// the wifi setup lane (device feature `wifi-setup`): join takes
// no `psk` for open networks; the device hashes the passphrase
// itself, it never leaves the request
export type WifiOp =
  { action: "scan" } | { action: "join"; ssid: string; psk?: string } | { action: "status" };

// the audio output picker (device feature `audio-output`, paired
// only): `set` rewrites squeezelite's OUTPUT and restarts it; the
// device restores the previous output when the new one fails
export type AudioOp = { action: "outputs" } | { action: "set"; output: string };

export type AudioOutput = { id: string; name: string; current: boolean };

export type AudioOutputsData = { outputs: AudioOutput[]; current: string | null };

// the timezone picker (device feature `timezone`, paired only):
// `set` is the user's choice, `adopt` offers the browser's zone —
// taken only while no zone was ever chosen. A changed zone restarts
// the player (`restarting`)
export type TimezoneOp =
  { action: "zones" } | { action: "set"; zone: string } | { action: "adopt"; zone: string };

export type TimezoneZonesData = {
  zones: string[];
  // the zone the user chose ("" = automatic)
  chosen: string;
  // the zone the player runs in (null = unknown)
  current: string | null;
};

export type TimezoneSetData = { zone: string; taken: boolean; restarting: boolean };

export type Request =
  | { op: "hello" }
  | ({ op: "settings" } & SettingsOp)
  | ({ op: "ma" } & MaOp)
  | { op: "ctl"; cmd: CtlCmd }
  | ({ op: "player" } & PlayerOp)
  | ({ op: "wifi" } & WifiOp)
  | ({ op: "audio" } & AudioOp)
  | ({ op: "timezone" } & TimezoneOp);

// the server's reply — Rust's `Envelope` is the request shape,
// this one is the response (kept apart on purpose)
export type Reply = {
  id: number;
  ok: boolean;
  data?: unknown;
  error?: string;
};

// push frames have no `id` — the server sends them on every
// state change; `state` is the remote-control snapshot, a bare
// `settings` announces that some client changed device settings
export type PushFrame = { event: "state"; data: RemoteState } | { event: "settings" };

export type SourceCaps = {
  play_pause: boolean;
  seek: boolean;
  next_previous: boolean;
  shuffle: boolean;
  repeat: boolean;
};

export type RemoteState = {
  connected: boolean;
  // MA web UI base URL ("" = unknown)
  ma_url: string;
  player: {
    name: string;
    powered: boolean;
    playing: boolean;
    loading: boolean;
    has_media: boolean;
    // the external source playing ("AirPlay", ...), "" = the MA queue
    source: string;
    // what the source can do; absent (older servers) or the queue = all
    source_caps?: SourceCaps;
    volume: number;
    title: string;
    artist: string;
    cover: string;
    accent: string;
  };
  view: {
    now_playing: boolean;
    queue: boolean;
    vu: boolean;
    standby: boolean;
    vu_index: number;
    vu_count: number;
    vu_skins: string[];
  };
  display: { supported: boolean; brightness: number };
};

export type StateData = {
  config: Record<string, unknown>;
  // the merged config with defaults folded in — every schema
  // field's current value reads from here (unset = null)
  effective: Record<string, unknown>;
  overrides: string[];
  base_path: string | null;
  provider_status: string;
  remote_url: string;
};

// the settings UI the device serves: sections = the kiosk's
// settings pages, fields = the keys it writes. The schema is the
// single source — the web keeps no key list of its own
export type Schema = { sections: Section[] };
export type Section = { id: string; title: string; icon: string; fields: Field[] };
export type Field = {
  // the dotted settings key (as `set`/`reset` accept it)
  key: string;
  label: string;
  help?: string;
  kind: FieldKind;
  visible_if?: VisibleIf;
};
export type FieldKind =
  | { type: "bool" }
  | { type: "choice"; options: Option[] }
  | { type: "number"; min: number; max: number; step: number; unit?: string }
  // stored 0..1, shown 0..100 %
  | { type: "percent" }
  | { type: "ordered-subset"; options: Option[] }
  | { type: "permutation"; options: Option[] }
  // unset = inherit the base value (osd.<context>.*)
  | { type: "inherit-bool" }
  // free text, empty = automatic (audio.*)
  | { type: "text" };
export type Option = {
  value: string;
  label: string;
  // thumbnail URL (the device's /vu-preview or /clock-preview
  // endpoints) — absent when the option has no preview
  preview?: string;
};
export type VisibleIf =
  { key: string; not_in?: string[]; in?: string[] } | { capability: "display" };

export type ProbeData = {
  version: string;
  schema: number;
  auth: "login" | "token-only" | "none";
};

export type Player = { id: string; name: string };

export type HelloData = {
  app: string;
  version: string;
  // the wizard URL incl. the pairing PIN in its hash — EMPTY for
  // an unpaired socket (an AP-mode setup client must never learn
  // the PIN)
  remote_url: string;
  ota: boolean;
  // newer servers report build capabilities here; `ota` is kept
  // for the pre-PWA backend and stays the fallback
  capabilities?: {
    ota: boolean;
    display: boolean;
    wifi?: boolean;
    factory_reset?: boolean;
    audio_output?: boolean;
    timezone?: boolean;
  };
};

// the `wifi scan` payload: one entry per SSID (hidden networks
// join by hand), strongest first
export type WifiNetwork = {
  ssid: string;
  // dBm, negative — the web derives the signal bars
  signal: number;
  secured: boolean;
};

// the `wifi status` payload: "sta" | "ap" | "joining" | "failed";
// `ap_ssid` is the setup hotspot (relevant in ap/failed), `ssid`
// the current (or joining) network, `error` the last join failure
export type WifiStatusData = {
  state: "sta" | "ap" | "joining" | "failed";
  ap_ssid: string;
  ssid: string;
  error: string;
};

// the `wifi join` reply after a successful join (the phone loses
// the hotspot when the player leaves AP mode)
export type WifiJoinedData = { ssid: string; state: "sta" };
