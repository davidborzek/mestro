// pure Music Assistant connection logic the setup wizard and
// the settings connection section share: URL normalization, the
// probe → login decision and the section-row summary. DOM-free
// so bun tests it all.

import type { ProbeData } from "./protocol";

export function normalizeMaUrl(value: string): string {
  const trimmed = value.trim().replace(/\/+$/, "");
  return trimmed.includes("://") ? trimmed : `http://${trimmed}`;
}

// schema < 28 servers need no login — the login form is skipped
export function needsLogin(probe: ProbeData | null): boolean {
  return probe === null || probe.auth !== "none";
}

// where MA keeps the long-lived tokens (the manual fallback)
export function maProfileUrl(url: string): string {
  return `${url.replace(/\/+$/, "")}/#/settings/profile`;
}

export type MaStatus = {
  connected: boolean;
  player: string;
  // the provider's error text, "" while it is fine
  providerStatus: string;
};

// the settings-section row: connected · player, else the error
export function maSummary(
  status: MaStatus,
  labels: { connected: string; notConnected: string },
): string {
  if (status.connected) {
    return status.player ? `${labels.connected} · ${status.player}` : labels.connected;
  }
  return status.providerStatus || labels.notConnected;
}

// the device relays the provider's error verbatim (an English
// Rust string); the known lead-ins get a localized form, a known
// detail after the colon too — anything else stays technical
export const PROVIDER_LEADINS = {
  unreachable: "Music Assistant unreachable: ",
  auth: "Music Assistant: ",
} as const;

export function localizeProviderStatus(
  status: string,
  leadins: { unreachable: string; auth: string },
  details: Record<string, string> = {},
): string {
  const detail = (rest: string) => details[rest.trim()] ?? rest;
  if (status.startsWith(PROVIDER_LEADINS.unreachable))
    return leadins.unreachable + detail(status.slice(PROVIDER_LEADINS.unreachable.length));
  if (status.startsWith(PROVIDER_LEADINS.auth))
    return leadins.auth + detail(status.slice(PROVIDER_LEADINS.auth.length));
  return status;
}

// the provider's raw detail strings (std::io / tungstenite
// Display texts) → i18n keys
export const PROVIDER_DETAILS = {
  "connect timeout": "ma.detail.timeout",
  "IO error: Connection refused (os error 111)": "ma.detail.refused",
  "IO error: Network is unreachable (os error 101)": "ma.detail.network",
} as const;
