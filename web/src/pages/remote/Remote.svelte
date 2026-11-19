<script lang="ts">
  import { connection, device } from "../../lib/connection.svelte";
  import { remoteState, remoteStateFeed } from "../../lib/remoteState.svelte";
  import { layout } from "../../lib/layout.svelte";
  import { optimistic } from "../../lib/optimistic.svelte";
  import { ctl, player as sendPlayer, setSetting } from "../../lib/commands";
  import { t } from "../../lib/i18n.svelte";
  import type { CtlCmd } from "../../lib/protocol";
  import Icon, { type IconName } from "../../components/Icon.svelte";
  import IconButton from "../../components/IconButton.svelte";
  import SliderRow from "../../components/SliderRow.svelte";

  // the device remote: one centered column (max 560 px), four
  // blocks top to bottom — now playing, kiosk screen, display,
  // Music Assistant (power sits in the top bar). Shape B (landscape phone) pairs
  // the blocks into two columns so nothing scrolls.
  const player = $derived(remoteState.player);
  const view = $derived(remoteState.view);
  const display = $derived(remoteState.display);
  const shape = $derived(layout.shape);
  const loading = $derived(!remoteStateFeed.hasSnapshot);
  const offline = $derived(connection.status !== "open");
  const kioskOn = $derived(Boolean(optimistic.pick("standby-toggle", player.powered)));
  const kioskOff = $derived(kioskOn === false);
  const maDown = $derived(!remoteState.connected);
  const kioskControls = $derived(offline || loading || kioskOff);
  const playback = $derived(offline || loading || maDown);
  const hasTrack = $derived(remoteState.connected && player.powered && player.has_media);

  type Tile = {
    cmd: CtlCmd;
    // the pushed flag the tile mirrors (the optimistic override
    // rides on the cmd key)
    view: "now_playing" | "queue" | "vu";
    icon: IconName;
    label: string;
  };

  const tiles = $derived<Tile[]>([
    { cmd: "now-playing", view: "now_playing", icon: "music", label: t("remote.tileNowPlaying") },
    { cmd: "queue", view: "queue", icon: "queue", label: t("remote.tileQueue") },
    { cmd: "vu", view: "vu", icon: "vu", label: t("remote.tileVu") },
  ]);

  const tileActive = (tile: Tile) => Boolean(optimistic.pick(tile.cmd, view[tile.view]));

  // a tap shows at once — the push (or the 3 s fallback) settles
  function tapTile(tile: Tile): void {
    optimistic.set(tile.cmd, !tileActive(tile));
    ctl(tile.cmd);
  }

  const vuIndex = $derived(Number(optimistic.pick("vu-index", view.vu_index)));

  function cycleVu(step: 1 | -1): void {
    const count = view.vu_count || 1;
    optimistic.set("vu-index", (vuIndex + step + count) % count);
    ctl(step > 0 ? "cycle-forward" : "cycle-back");
  }

  function wake(): void {
    optimistic.set("standby-toggle", true);
    ctl("standby-toggle");
  }

  const announcement = $derived(
    hasTrack
      ? t("remote.nowPlayingAnnouncement", { title: player.title, artist: player.artist })
      : "",
  );
</script>

<div class="remote shape-{shape}">
  <span class="sr-only" aria-live="polite">{announcement}</span>

  <section class="np">
    {#if loading}
      <div class="head">
        <span class="thumb shimmer"></span>
        <div class="meta">
          <span class="bar shimmer"></span>
          <span class="bar shimmer"></span>
        </div>
      </div>
    {:else if kioskOff}
      <!-- the header's power button is the everyday control; off,
           the card offers the obvious way back -->
      <div class="off">
        <p class="line">{t("remote.playerIsOff")}</p>
        <button class="wake" disabled={offline} onclick={wake}>
          <Icon name="power" size={20} />
          {t("remote.powerOn")}
        </button>
      </div>
    {:else if maDown}
      <p class="line">{t("remote.maDisconnected")}</p>
    {:else}
      <div class="head">
        <span class="thumb">
          {#if player.cover}
            <img src={player.cover} alt="" draggable="false" />
          {:else}
            <Icon name="music" size={26} />
          {/if}
        </span>
        <div class="meta">
          {#if player.has_media}
            <h1 class="title">{player.title}</h1>
            <p class="artist">{player.artist}</p>
          {:else}
            <p class="title none">{t("remote.nothingPlaying")}</p>
          {/if}
        </div>
        {#if player.has_media}
          <button
            class="play"
            disabled={playback}
            aria-label={player.playing ? t("remote.pause") : t("remote.play")}
            onclick={() => ctl("play-pause")}
          >
            <Icon name={player.playing ? "pause" : "play"} size={26} />
          </button>
        {/if}
      </div>
      <div class="volume">
        <SliderRow
          icon={(value) => (value === 0 ? "volumeX" : value < 50 ? "volumeLow" : "volumeHigh")}
          label={t("remote.volume")}
          value={player.volume}
          disabled={playback}
          oncommit={(value) => sendPlayer({ action: "volume", value }).catch(() => {})}
        />
      </div>
    {/if}
  </section>

  <section class="kiosk">
    <h2 class="label">{t("remote.onDisplay")}</h2>
    <div class="tiles" role="group" aria-label={t("remote.onDisplay")}>
      {#each tiles as tile (tile.cmd)}
        <button
          class="tile"
          class:active={tileActive(tile)}
          disabled={kioskControls}
          aria-pressed={tileActive(tile)}
          onclick={() => tapTile(tile)}
        >
          <Icon name={tile.icon} size={28} />
          <span class="tile-label">{tile.label}</span>
        </button>
      {/each}
    </div>
    {#if view.vu}
      <div class="vu-row">
        <span class="vu-skin">{t("remote.vuSkin")}</span>
        <span class="vu-pos">{vuIndex} / {view.vu_count}</span>
        <IconButton
          icon="chevronLeft"
          label={t("remote.vuPrev")}
          disabled={kioskControls}
          onclick={() => cycleVu(-1)}
        />
        <IconButton
          icon="chevronRight"
          label={t("remote.vuNext")}
          disabled={kioskControls}
          onclick={() => cycleVu(1)}
        />
      </div>
    {/if}
  </section>

  <div class="side">
    {#if device.display}
      <section class="display">
        <h2 class="label">{t("remote.display")}</h2>
        <SliderRow
          icon="sun"
          label={t("remote.brightness")}
          value={display.brightness}
          min={1}
          throttleMs={200}
          disabled={kioskControls}
          oncommit={(value) => setSetting("display.brightness", value).catch(() => {})}
        />
      </section>
    {/if}

    {#if remoteState.ma_url}
      <a
        class="ma"
        href={remoteState.ma_url}
        target="_blank"
        rel="noreferrer"
        aria-label={t("remote.maOpen")}
        title={t("remote.maOpen")}
      >
        <Icon name="ma" size={22} />
        <span class="ma-label">{t("remote.maOpen")}</span>
        <Icon name="external" size={18} />
      </a>
    {/if}
  </div>
</div>

<style>
  .remote {
    width: 100%;
    max-width: 560px;
    margin: 0 auto;
    min-height: calc(100dvh - var(--bar-total));
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: var(--sp-3);
    padding: var(--sp-2) max(24px, var(--safe-r)) calc(var(--sp-3) + var(--safe-b))
      max(24px, var(--safe-l));
  }

  .label {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.8px;
    text-transform: uppercase;
    color: var(--mute);
  }

  /* ── now playing card ── */
  .np {
    background: var(--glass);
    border: 1px solid var(--hairline);
    border-radius: var(--r-18);
    padding: var(--sp-4);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    min-height: 64px;
  }
  .thumb {
    flex: 0 0 auto;
    width: 64px;
    height: 64px;
    border-radius: 10px;
    background: var(--pill);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--mute);
    overflow: hidden;
  }
  .thumb img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .meta {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .title {
    font-size: 16px;
    font-weight: 600;
    line-height: 1.2;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .artist {
    font-size: 14px;
    color: var(--dim);
    line-height: 1.2;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title.none {
    color: var(--dim);
    font-weight: 600;
  }
  .play {
    flex: 0 0 auto;
    width: 52px;
    height: 52px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--accent-live);
    color: var(--accent-ink);
    transition:
      transform var(--t-press) var(--ease),
      background var(--t-accent) var(--ease);
  }
  .play:active:not(:disabled) {
    transform: scale(0.94);
  }
  .play:disabled {
    opacity: 0.45;
  }
  .volume {
    margin-top: var(--sp-2);
  }
  .line {
    min-height: 32px;
    display: flex;
    align-items: center;
    font-size: 15px;
    color: var(--dim);
  }
  .bar {
    height: 16px;
    border-radius: var(--r-8);
  }
  .bar + .bar {
    margin-top: 6px;
    width: 60%;
  }
  .shimmer {
    background: linear-gradient(100deg, var(--panel) 30%, var(--panel-hover) 50%, var(--panel) 70%)
      0 0 / 300% 100%;
    animation: shimmer 1.6s linear infinite;
  }
  .thumb.shimmer {
    width: 64px;
    height: 64px;
  }
  @keyframes shimmer {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: -100% 0;
    }
  }

  /* ── off: the way back ── */
  .off {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
  }
  .wake {
    flex: 0 0 auto;
    min-height: 44px;
    padding: 0 var(--sp-4);
    border-radius: 22px;
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    background: var(--accent-live);
    color: var(--accent-ink);
    font-size: 14px;
    font-weight: 650;
    transition: transform var(--t-press) var(--ease);
  }
  .wake:active:not(:disabled) {
    transform: scale(0.96);
  }
  .wake:disabled {
    opacity: 0.45;
  }

  /* ── kiosk screen ── */
  .kiosk {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--sp-3);
  }
  .tile {
    height: 88px;
    border: 1px solid var(--hairline);
    border-radius: 16px;
    background: var(--glass);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    color: var(--dim);
    transition:
      transform var(--t-press) var(--ease),
      background var(--t-press) var(--ease),
      color var(--t-accent) var(--ease);
  }
  .tile-label {
    font-size: 13px;
    font-weight: 600;
  }
  .tile:active:not(:disabled) {
    transform: scale(0.96);
  }
  .tile.active {
    background: color-mix(in srgb, var(--accent-live) 14%, transparent);
    border-color: color-mix(in srgb, var(--accent-live) 40%, transparent);
    color: var(--accent-live);
  }
  .tile:disabled {
    opacity: 0.4;
  }
  .vu-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-height: 44px;
  }
  .vu-skin {
    font-size: 13px;
    font-weight: 600;
    color: var(--dim);
  }
  .vu-pos {
    flex: 1 1 auto;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    color: var(--mute);
  }

  /* ── display + Music Assistant ── */
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .display {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .ma {
    min-height: 48px;
    padding: 0 var(--sp-4);
    border: 1px solid var(--hairline);
    border-radius: var(--r-14);
    background: var(--glass);
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
    white-space: nowrap;
  }
  .ma .ma-label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ma :global(svg) {
    flex: 0 0 auto;
    color: var(--dim);
  }

  /* ── B: landscape phone — two columns, nothing scrolls ── */
  .shape-B {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(0, 1fr);
    grid-template-rows: auto auto;
    gap: var(--sp-2);
    align-content: center;
    /* the landscape phone is the one shape where every pixel of
       height counts — the paddings/gaps slim down with it */
    padding-top: var(--sp-1);
    padding-bottom: var(--sp-1);
  }
  .shape-B .np {
    grid-area: 1 / 1;
  }
  .shape-B .np .volume {
    margin-top: var(--sp-1);
  }
  /* too narrow for the label: icons only, the name is the tooltip */
  .shape-B .ma {
    justify-content: center;
  }
  .shape-B .ma .ma-label {
    display: none;
  }
  .shape-B .kiosk {
    grid-area: 2 / 1;
    gap: var(--sp-1);
  }
  .shape-B .side {
    grid-area: 1 / 2 / 3 / 3;
    gap: var(--sp-2);
    align-self: start;
  }
</style>
