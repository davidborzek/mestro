import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { viteSingleFile } from "vite-plugin-singlefile";

// the whole wizard ships as one index.html — mestro embeds it in
// the binary (src/web/server.rs include_str!) and serves it from
// memory; no runtime file deps on the kiosk
export default defineConfig({
  plugins: [svelte(), viteSingleFile()],
  build: {
    target: "es2020",
    cssCodeSplit: false,
  },
  server: {
    proxy: {
      // live dev: `bun run dev` serves the wizard with HMR and
      // forwards the WS channel to a running mestro. Open
      // http://localhost:5173/#pair=<PIN> (the PIN: kiosk
      // connection page, startup log or ~/.local/state/mestro/web-pin)
      "/ws": {
        target: "ws://localhost:8787",
        ws: true,
      },
    },
  },
});