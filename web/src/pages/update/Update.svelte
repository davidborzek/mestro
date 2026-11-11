<script lang="ts">
  import { device, socket } from "../../lib/connection.svelte";
  import type { HelloData } from "../../lib/protocol";
  import { t } from "../../lib/i18n.svelte";
  import { pairCode } from "../../lib/ws";

  type Phase = "idle" | "uploading" | "rebooting" | "done" | "error";

  let phase = $state<Phase>("idle");
  let file = $state<File | null>(null);
  let percent = $state(0);
  let reason = $state("");
  let newVersion = $state("");

  function pick(event: Event) {
    file = (event.target as HTMLInputElement).files?.[0] ?? null;
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
    xhr.open("POST", `/ota?pair=${encodeURIComponent(pairCode())}`);
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
        fail(wentDown ? "the kiosk did not come back" : "the kiosk did not restart");
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
    <label class="f">
      {t("update.file")}
      <input type="file" onchange={pick} />
    </label>
    {#if phase === "error"}
      <div class="err">⚠ {t("update.error", { reason })}</div>
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
  .update {
    flex: 1;
    margin: 16px;
  }
  progress {
    width: 100%;
    accent-color: var(--accent);
  }
</style>
