<script lang="ts">
  // 6 px status dot before the player name
  let { tone }: { tone: "ok" | "wait" | "error" } = $props();
</script>

<span class="status {tone}" aria-hidden="true"></span>

<style>
  .status {
    flex: 0 0 auto;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    transition: background var(--t-accent) var(--ease);
  }
  .ok {
    background: var(--accent-live);
  }
  .wait {
    background: var(--mute);
  }
  .error {
    background: var(--danger);
  }
</style>
