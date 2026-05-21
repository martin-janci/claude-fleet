import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte({ hot: false })],
  resolve: {
    conditions: ['browser'],
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./vitest.setup.ts'],
    // `.worktrees/` holds git worktrees (full repo copies) — never run their
    // duplicate test suites as part of this project's run.
    exclude: ['**/node_modules/**', '**/.worktrees/**', '**/target/**'],
  },
});
