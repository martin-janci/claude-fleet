import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// Global Tauri IPC mock: keeps components that call invoke() on mount
// (e.g. App.svelte's healthCheck()) from crashing in tests. Individual
// test files can override with their own vi.mock for specific commands.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => ({ version: '0.0.0', db_ready: true, schema_version: 1 })),
}));
