// the live remote-control state: the server pushes a snapshot on
// connect and a new frame on every change

import type { RemoteState } from "./protocol";
import { socket } from "./connection.svelte";
import { confirmOptimistic } from "./optimistic.svelte";

const emptyState: RemoteState = {
  connected: false,
  ma_url: "",
  player: {
    name: "",
    powered: false,
    playing: false,
    loading: false,
    has_media: false,
    volume: 0,
    title: "",
    artist: "",
    cover: "",
    accent: "",
  },
  view: { now_playing: false, queue: false, vu: false, standby: false, vu_index: 0, vu_count: 0 },
  display: { supported: false, brightness: 0 },
};

export const remoteState = $state<RemoteState>(structuredClone(emptyState));

// true once the first push lands — the remote page shows a
// skeleton instead of a stale/empty state until then
export const remoteStateFeed = $state({ hasSnapshot: false, revision: 0 });

export function startRemoteState(): void {
  socket.onPush((data) => {
    remoteState.connected = data.connected;
    remoteState.ma_url = data.ma_url;
    remoteState.player = data.player;
    remoteState.view = data.view;
    remoteState.display = data.display;
    remoteStateFeed.hasSnapshot = true;
    remoteStateFeed.revision += 1;
    confirmOptimistic(data);
  });
}

export function resetRemoteState(): void {
  remoteState.connected = structuredClone(emptyState.connected);
  remoteState.ma_url = structuredClone(emptyState.ma_url);
  remoteState.player = structuredClone(emptyState.player);
  remoteState.view = structuredClone(emptyState.view);
  remoteState.display = structuredClone(emptyState.display);
  remoteStateFeed.hasSnapshot = false;
}
