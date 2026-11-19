<script lang="ts">
  import { t } from "../../../lib/i18n.svelte";
  import MaPlayerList from "../../../components/ma/MaPlayerList.svelte";
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
  <MaPlayerList {players} {busy} {onpick} />
</section>

<footer class="actions">
  <button class="ghost" onclick={onback}>{t("common.back")}</button>
</footer>
