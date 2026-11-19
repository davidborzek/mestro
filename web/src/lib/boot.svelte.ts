// the boot sequence: hash PIN → cookie check → (unpaired) the
// hotspot probe → socket + push feed. `started` only stops boot()
// and the pair screen from double-starting those; the device
// handshake itself re-runs on every socket open (see
// connection.svelte).

import { consumeHashPair } from "./pairing";
import { checkPaired, connection, socket, startConnection } from "./connection.svelte";
import { startDiagnostics } from "./diagnostics.svelte";
import { startRemoteState } from "./remoteState.svelte";
import { navigate } from "./router.svelte";
import { unpairedLanding, withTimeout } from "./wifi";
import type { WifiStatusData } from "./protocol";

let started = false;

export function startApp(): void {
  if (started) return;
  started = true;
  startRemoteState();
  startDiagnostics();
  // a kiosk that is down keeps retrying inside the socket; each
  // successful open repeats the device handshake
  startConnection();
}

export async function boot(): Promise<void> {
  await consumeHashPair();
  await checkPaired();
  if (connection.paired) {
    startApp();
    return;
  }
  // unpaired: the phone may sit on the player's own hotspot —
  // the wifi status is the only fact that decides. The pair screen
  // renders meanwhile; this swaps the route behind it, so a LAN
  // client (probe fails or times out) never sees a difference
  const status = await withTimeout(
    socket.send<WifiStatusData>({ op: "wifi", action: "status" }),
    8000,
    "status timed out",
  ).catch(() => null);
  const landing = unpairedLanding(status);
  if (landing !== null) navigate(landing, { replace: true });
}
