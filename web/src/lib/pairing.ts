// pairing: the PIN arrives once via the URL hash (the kiosk QR),
// is exchanged for an HttpOnly cookie (POST /pair) and the hash
// is stripped so it never leaks into history or shares.

// the kiosk PIN alphabet — no 0/1/I/O, 8 characters
export const PIN_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
export const PIN_LENGTH = 8;

export function normalizePin(input: string): string {
  return input
    .toUpperCase()
    .split("")
    .filter((c) => PIN_ALPHABET.includes(c))
    .slice(0, PIN_LENGTH)
    .join("");
}

export function isValidPin(pin: string): boolean {
  return pin.length === PIN_LENGTH && pin.split("").every((c) => PIN_ALPHABET.includes(c));
}

export class PairError extends Error {
  constructor(public readonly kind: "wrong" | "limited" | "unreachable") {
    super(kind);
  }
}

async function postPair(pin: string): Promise<void> {
  let reply: Response;
  try {
    reply = await fetch("/pair", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pin }),
    });
  } catch {
    throw new PairError("unreachable");
  }
  if (reply.status === 401) throw new PairError("wrong");
  if (reply.status === 429) throw new PairError("limited");
  if (!reply.ok) throw new PairError("unreachable");
}

export async function pair(pin: string): Promise<void> {
  if (!isValidPin(pin)) throw new PairError("wrong");
  await postPair(pin);
}

// tri-state: true = the cookie is accepted, false = the player
// answered and rejected it (a factory reset drew a new PIN),
// null = no answer. Only an explicit false may unpair — an
// unreachable player must not log the app out
export type FetchLike = (url: string) => Promise<Response>;

export async function pairStatus(fetchFn: FetchLike = fetch): Promise<boolean | null> {
  let reply: Response;
  try {
    reply = await fetchFn("/pair/status");
  } catch {
    return null;
  }
  if (!reply.ok) return null;
  try {
    const data = (await reply.json()) as { paired?: boolean };
    return data.paired === true;
  } catch {
    return null;
  }
}

export async function isPaired(): Promise<boolean> {
  return (await pairStatus()) === true;
}

// the hash form `#pair=<PIN>` comes from the kiosk QR code
function pinFromHash(): string {
  const pin = location.hash.match(/pair=([A-Za-z0-9]+)/)?.[1] ?? "";
  return normalizePin(pin);
}

// exchange a hash PIN for the cookie, then drop the hash. A bad
// PIN still strips the hash — the manual pair screen takes over.
// The router's entry state rides along so a deep link keeps its
// place in the history.
export async function consumeHashPair(): Promise<void> {
  const pin = pinFromHash();
  if (!pin) return;
  history.replaceState(history.state, "", location.pathname + location.search);
  if (isValidPin(pin)) await postPair(pin).catch(() => {});
}
