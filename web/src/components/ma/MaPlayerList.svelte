<script lang="ts">
  import Icon from "../Icon.svelte";
  import { t } from "../../lib/i18n.svelte";
  import type { Player } from "../../lib/protocol";

  // the MA player list, shared by the wizard's player step and
  // the settings connection section's picker sheet. With
  // `active` set (the picker) every row gets a fixed leading
  // check slot — the selected row's mark, an empty slot on the
  // others — so all rows align left; the wizard step keeps its
  // plain rows with the trailing chevron.

  let {
    players,
    busy = false,
    autoLabel = "",
    active = undefined,
    onpick,
  }: {
    players: Player[];
    busy?: boolean;
    autoLabel?: string;
    // the currently stored player name ("" = automatic)
    active?: string;
    onpick: (name: string) => void;
  } = $props();

  const picker = $derived(active !== undefined);
</script>

{#if busy && players.length === 0}
  <div class="pill"><span class="dot"></span> {t("player.loading")}</div>
{:else}
  <ul class="players">
    {#if autoLabel}
      <li>
        <button class="row" class:current={active === ""} onclick={() => onpick("")}>
          {#if picker}
            <span class="lead">
              {#if active === ""}<Icon name="check" size={20} />{/if}
            </span>
          {/if}
          <Icon name="music" size={20} />
          <span class="grow">{autoLabel}</span>
        </button>
      </li>
    {/if}
    {#each players as player (player.id)}
      <li>
        <button
          class="row"
          class:current={player.name === active}
          onclick={() => onpick(player.name)}
        >
          {#if picker}
            <span class="lead">
              {#if player.name === active}<Icon name="check" size={20} />{/if}
            </span>
          {/if}
          <Icon name="music" size={20} />
          <span class="grow">{player.name}</span>
          {#if !picker}
            <span class="chev"><Icon name="chevronRight" size={20} /></span>
          {/if}
        </button>
      </li>
    {:else}
      {#if !autoLabel}
        <li class="empty">{t("player.empty")}</li>
      {/if}
    {/each}
  </ul>
{/if}

<style>
  .players {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    background: var(--panel);
    border: 1px solid var(--hairline);
    justify-content: flex-start;
    padding: 0 var(--sp-3);
    min-height: 48px;
    border-radius: var(--r-8);
    font-size: 15px;
    gap: var(--sp-3);
  }
  .row.current {
    color: var(--accent-live);
    font-weight: 650;
  }
  .lead {
    flex: 0 0 auto;
    width: 20px;
    height: 20px;
    display: inline-flex;
    align-items: center;
    color: var(--accent-live);
  }
  .grow {
    flex: 1;
    text-align: left;
  }
  .chev {
    color: var(--mute);
    display: flex;
  }
  .empty {
    color: var(--mute);
    font-size: 12.5px;
    padding: 6px 2px;
  }
</style>
