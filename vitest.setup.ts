import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// Global Tauri IPC mock: keeps components that call invoke() on mount
// (e.g. App.svelte's healthCheck(), Sidebar's loadProjects() and
// loadSessions()) from crashing in tests. Individual test files can
// override with their own vi.mock for specific commands.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.0.0', db_ready: true, schema_version: 1 };
    if (cmd === 'list_projects') return [];
    if (cmd === 'refresh_projects') return [];
    if (cmd === 'list_sessions') return [];
    if (cmd === 'kill_session') return null;
    if (cmd === 'new_session') return null;
    if (cmd === 'pty_open') return null;
    if (cmd === 'pty_write') return null;
    if (cmd === 'pty_resize') return null;
    if (cmd === 'pty_close') return null;
    return null;
  }),
  // Minimal Channel stub kept for backward-compat with any test that still
  // touches the older code path; current TerminalView uses listen() instead.
  Channel: class {
    onmessage: ((data: unknown) => void) | null = null;
  },
}));

// pty-data and other Tauri events: listen returns an unlisten fn that does
// nothing. Tests don't drive PTY events.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

// xterm.js relies on DOM measurement APIs that jsdom doesn't implement.
// Stub them as no-ops so components that mount xterm don't blow up tests.
if (typeof globalThis !== 'undefined') {
  // @ts-expect-error: jsdom stub
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
