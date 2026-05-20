// Vite config for the Svelte variant. Library-mode build emits a
// single self-contained ES module at `pkg/main.js` that
// `index.html` loads — same shape as the wasm-pack output other
// variants use. Setting `lib.formats: ['es']` skips Vite's
// default UMD/CJS wrappers so the bundle is one clean module.
//
// `vite build` sets NODE_ENV=production, which `@sveltejs/vite-
// plugin-svelte` reads to run the compiler in production mode
// (`dev: false`). No dev-mode validators in the output.
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  build: {
    lib: {
      entry: './src/main.js',
      name: 'BenchmarkSvelte',
      formats: ['es'],
      fileName: 'main',
    },
    outDir: 'pkg',
    emptyOutDir: true,
    minify: 'esbuild',
    sourcemap: false,
    target: 'es2022',
  },
});
