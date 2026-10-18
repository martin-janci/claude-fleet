import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

import { reticle } from '@reticlehq/vite-plugin';
export default defineConfig({
  plugins: [reticle({ port: 4400 }), svelte()],
  server: {
    port: 1420,
    strictPort: true,
  },
  clearScreen: false,
});
