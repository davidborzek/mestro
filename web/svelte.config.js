import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

// svelte-check and tooling resolve the Svelte config from here;
// the build itself runs through vite.config.ts
export default {
  preprocess: vitePreprocess(),
};