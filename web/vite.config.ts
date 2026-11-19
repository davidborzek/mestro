import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// a real PWA build: hashed assets + manifest + icons from
// web/public — mestro embeds the whole dist dir (build.rs) and
// serves it from memory; no runtime file deps on the kiosk
export default defineConfig({
  plugins: [svelte()],
  build: {
    target: "es2022",
    cssCodeSplit: false,
  },
  server: {
    proxy: {
      // live dev: `bun run dev` serves the app with HMR and
      // forwards the API to a running mestro. Open
      // http://localhost:5173 (the pairing cookie rides along).
      // No kiosk around? Append ?mock for a fake state feed.
      "/ws": {
        target: "ws://localhost:8787",
        ws: true,
      },
      "/pair": "http://localhost:8787",
      "/cover": "http://localhost:8787",
      "/ota": "http://localhost:8787",
    },
  },
});
