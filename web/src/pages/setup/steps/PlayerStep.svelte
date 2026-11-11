<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";
  import type { Player } from "../../../lib/protocol";

  let {
    players,
    busy,
    onpick,
    onback,
  }: {
    players: Player[];
    busy: boolean;
    onpick: (name: string) => void;
    onback: () => void;
  } = $props();
</script>

<section class="card">
  <h2>{t("player.title")}</h2>
  <p>{t("player.lead")}</p>
  {#if busy && players.length === 0}
    <div class="pill"><span class="dot"></span> {t("player.loading")}</div>
  {:else}
    <ul class="players">
      {#each players as player}
        <li>
          <button class="row" onclick={() => onpick(player.name)}>
            <span class="ic">&#128266;</span>
            <span class="grow">{player.name}</span>
            <span class="chev">&rsaquo;</span>
          </button>
        </li>
      {:else}
        <li class="empty">{t("player.empty")}</li>
      {/each}
    </ul>
  {/if}
</section>

<footer class="actions">
  <button class="ghost" onclick={onback}>{t("common.back")}</button>
</footer>

<style>
  .players {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .row {
    width: 100%;
    background: var(--panel);
    border: 1px solid var(--hairline);
    justify-content: flex-start;
    padding: 0 12px;
    font-weight: 400;
    gap: 10px;
  }
  .grow {
    flex: 1;
    text-align: left;
  }
  .chev {
    color: var(--mute);
  }
  .empty {
    color: var(--mute);
    font-size: 12.5px;
    padding: 6px 2px;
  }
</style>
