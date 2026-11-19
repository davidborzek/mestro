<script lang="ts">
  import StatusDot from "./StatusDot.svelte";

  // top-bar left side: status dot + player name (a label, not a
  // control — tapping it does nothing)
  let {
    name,
    tone,
  }: {
    name: string;
    tone: "ok" | "wait" | "error";
  } = $props();
</script>

<div class="title">
  <StatusDot {tone} />
  <span class="name">{name}</span>
</div>

<style>
  .title {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-width: 0;
  }
  .name {
    font-size: 15px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
