// the JSON vocabulary of the embedded endpoint — mirrors
// src/web/protocol.rs: `op` names the domain, `action` the verb

export type SettingsOp =
  | { action: "state" }
  | { action: "set"; key: string; value: unknown }
  | { action: "reset"; key: string }
  | { action: "reset-all" }
  | { action: "restart" };

export type MaOp =
  | { action: "probe"; url: string }
  | { action: "login"; url: string; username: string; password: string }
  | { action: "players"; url: string };

export type Request = { op: "hello" } | ({ op: "settings" } & SettingsOp) | ({ op: "ma" } & MaOp);

// the server's reply — Rust's `Envelope` is the request shape,
// this one is the response (kept apart on purpose)
export type Reply = {
  id: number;
  ok: boolean;
  data?: unknown;
  error?: string;
};

export type StateData = {
  config: Record<string, unknown>;
  overrides: string[];
  base_path: string | null;
  provider_status: string;
  remote_url: string;
};

export type ProbeData = {
  version: string;
  schema: number;
  auth: "login" | "token-only" | "none";
};

export type Player = { id: string; name: string };

export type HelloData = {
  app: string;
  version: string;
  remote_url: string;
  ota: boolean;
};
