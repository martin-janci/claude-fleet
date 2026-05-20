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
    if (cmd === 'rename_session') return null;
    if (cmd === 'restart_session') return null;
    if (cmd === 'pty_open') return null;
    if (cmd === 'pty_write') return null;
    if (cmd === 'pty_resize') return null;
    if (cmd === 'pty_close') return null;
    if (cmd === 'pty_drain') return { data: '', bytes: 0 };
    if (cmd === 'list_hosts') return [{
      alias: 'local',
      ssh_alias: null,
      reachable: true,
      claude_version: '2.1.145',
      tmux_version: '3.5a',
      hidden: false,
      last_pinged_at: 1,
      account_uuid: null,
    }];
    if (cmd === 'discover_hosts') return [];
    if (cmd === 'add_host') return null;
    if (cmd === 'probe_host') return null;
    if (cmd === 'remove_host') return null;
    if (cmd === 'hide_host') return null;
    if (cmd === 'list_accounts') return [];
    if (cmd === 'probe_ssh_alias') return {
      reachable: true,
      claude_version: '2.1.144',
      tmux_version: '3.6a',
      account: null,
    };
    return null;
  }),
}));

// pty-data and other Tauri events: listen returns an unlisten fn that does
// nothing. Tests don't drive PTY events.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

// ResizeObserver isn't implemented by jsdom. Our TerminalView attaches one
// to re-fit the screen buffer on container resize; stub it as a no-op so
// tests that mount the component don't blow up.
if (typeof globalThis !== 'undefined') {
  // @ts-expect-error: jsdom stub
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
