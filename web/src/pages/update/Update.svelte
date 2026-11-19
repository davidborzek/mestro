<script lang="ts">
  import { device, socket } from "../../lib/connection.svelte";
  import type { HelloData } from "../../lib/protocol";
  import { t } from "../../lib/i18n.svelte";
  import Icon from "../../components/Icon.svelte";

  type Phase = "idle" | "uploading" | "rebooting" | "done" | "error";

  let phase = $state<Phase>("idle");
  let file = $state<File | null>(null);
  let percent = $state(0);
  let reason = $state("");
  let newVersion = $state("");

  function pick(event: Event) {
    file = (event.target as HTMLInputElement).files?.[0] ?? null;
  }

  // image files run into megabytes — keep the unit readable
  function formatSize(bytes: number): string {
    if (bytes >= 1_000_000) return `${Math.round(bytes / 1_000_000)} MB`;
    if (bytes >= 1_000) return `${Math.round(bytes / 1_000)} kB`;
    return `${bytes} B`;
  }

  function fail(message: string) {
    reason = message;
    phase = "error";
  }

  // XHR, not fetch: only XHR reports upload progress
  function upload() {
    if (!file) return;
    phase = "uploading";
    percent = 0;
    const xhr = new XMLHttpRequest();
    // the pairing cookie rides along automatically
    xhr.open("POST", "/ota");
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) percent = Math.floor((e.loaded * 100) / e.total);
    };
    xhr.onload = () => {
      if (xhr.status === 200) {
        phase = "rebooting";
        void waitForDevice();
      } else {
        fail(xhr.responseText || `HTTP ${xhr.status}`);
      }
    };
    xhr.onerror = () => fail("connection lost");
    xhr.send(file);
  }

  const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

  async function hello(): Promise<HelloData | null> {
    try {
      return await Promise.race([
        socket.send<HelloData>({ op: "hello" }),
        sleep(3000).then(() => null),
      ]);
    } catch {
      return null;
    }
  }

  // the device answers until it reboots: wait for it to go down
  // first, then for it to come back. A trial boot plus its health
  // check takes ~2 min; a device that never went down did not reboot
  async function waitForDevice() {
    let wentDown = false;
    const deadline = Date.now() + 5 * 60_000;
    for (;;) {
      if (Date.now() > deadline) {
        fail(wentDown ? t("update.notBack") : t("update.notRestarted"));
        return;
      }
      await sleep(3000);
      const answer = await hello();
      if (!answer) {
        wentDown = true;
      } else if (wentDown) {
        newVersion = answer.version;
        device.version = answer.version;
        phase = "done";
        return;
      }
    }
  }
</script>

<section class="card update">
  <h2>{t("update.title")}</h2>
  <p>{t("update.current", { version: device.version })}</p>

  {#if phase === "idle" || phase === "error"}
    <p>{t("update.lead")}</p>
    <label class="picker">
      <!-- the native input is invisible; the button carries it -->
      <input class="file" type="file" onchange={pick} aria-label={t("update.chooseFile")} />
      <span class="choose"><Icon name="upload" size={20} /> {t("update.chooseFile")}</span>
      {#if file}
        <span class="picked">
          <strong class="name">{file.name}</strong>
          <span class="size">{formatSize(file.size)}</span>
        </span>
      {/if}
    </label>
    {#if phase === "error"}
      <div class="err"><Icon name="alert" size={16} /> {t("update.error", { reason })}</div>
    {/if}
    <button class="primary" disabled={!file} onclick={upload}>
      {phase === "error" ? t("update.retry") : t("update.start")}
    </button>
  {:else if phase === "uploading"}
    <p>{t("update.uploading", { percent: String(percent) })}</p>
    <progress max="100" value={percent}></progress>
    <div class="note">{t("update.keepOpen")}</div>
  {:else if phase === "rebooting"}
    <div class="pill"><span class="dot pulse"></span> {t("update.rebooting")}</div>
  {:else}
    <div class="pill ok"><span class="dot"></span> {t("update.done", { version: newVersion })}</div>
  {/if}
</section>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .file {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
  }
  .choose {
    align-self: flex-start;
    min-height: 44px;
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0 var(--sp-4);
    border: 1px solid var(--hairline);
    border-radius: var(--r-14);
    color: var(--text);
    font-size: 15px;
    font-weight: 600;
    cursor: pointer;
    transition: border-color var(--t-press) var(--ease);
  }
  .picker:focus-within .choose {
    border-color: var(--accent-live);
  }
  .picked {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 13px;
    overflow-wrap: anywhere;
  }
  .picked .name {
    font-weight: 600;
  }
  .picked .size {
    color: var(--mute);
    font-variant-numeric: tabular-nums;
  }
  progress {
    width: 100%;
    accent-color: var(--accent);
  }
</style>
