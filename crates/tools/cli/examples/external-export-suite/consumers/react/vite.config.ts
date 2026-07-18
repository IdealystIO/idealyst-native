import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The components package is a `file:` dependency that lives outside this
// project's root (under the suite's `dist/external`). Allow Vite's dev
// server to read it, and don't try to pre-bundle the wasm glue.
export default defineConfig({
  plugins: [react()],
  server: { fs: { allow: [".", "../../dist/external"] } },
  optimizeDeps: { exclude: ["external-export-suite-components"] },
});
