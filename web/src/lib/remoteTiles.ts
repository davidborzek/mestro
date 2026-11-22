// which "On the display" tiles the remote offers: the queue view
// belongs to Music Assistant's queue, an external source (AirPlay,
// Spotify Connect, ...) has none — the server answers a `queue`
// ctl with an error then, so the tile is off instead

import type { CtlCmd } from "./protocol";

// `source` is player.source from the state push ("" = the MA queue)
export function tileAvailable(cmd: CtlCmd, source: string): boolean {
  return cmd !== "queue" || source === "";
}
