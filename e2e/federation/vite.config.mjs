// The app's Vite config for the federation e2e harness: the same Svelte
// build, without the Reticle dev plugin (its bridge and overlay would be in
// every screenshot) and on whatever port the runner picks.
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  root: fileURLToPath(new URL('../..', import.meta.url)),
  plugins: [svelte()],
  logLevel: 'warn',
  clearScreen: false,
  server: { strictPort: false, hmr: false },
});
