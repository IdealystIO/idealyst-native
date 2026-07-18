import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  server: { fs: { allow: [".", "../../dist/external"] } },
  optimizeDeps: { exclude: ["external-export-suite-components"] },
});
