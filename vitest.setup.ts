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
    if (cmd === 'related_sessions') return [];
    if (cmd === 'send_prompt') return null;
    if (cmd === 'spawn_review') return null;
    if (cmd === 'mcp_status' || cmd === 'mcp_configure')
      return {
        enabled: false,
        running: false,
        port: 4180,
        token: 'test-token',
        url: 'http://127.0.0.1:4180/mcp',
        bind_error: null,
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

// The real getCurrentWebview() reads Tauri window internals that don't exist
// in jsdom, so it throws on access. TerminalView subscribes to drag-drop
// events through it on mount; stub it so onDragDropEvent resolves to a no-op
// unlisten fn and mounting the component doesn't crash.
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn(async () => () => {}),
  }),
}));

// vitest's jsdom environment does not expose a usable global `localStorage`
// (the document origin is opaque, so jsdom omits the Web Storage API).
// Provide a minimal in-memory Storage so modules that read prefs / theme /
// session-ui state — some at import time — don't crash. Setup files load
// before any test module is imported.
//
// We replace it both when it's missing AND when it's present-but-unusable:
// Node 22+ ships an experimental global `localStorage` that throws unless the
// process was started with `--localstorage-file`, so a bare `typeof` check
// isn't enough — probe a real call.
function localStorageUsable(): boolean {
  try {
    globalThis.localStorage?.getItem('__probe__');
    return typeof globalThis.localStorage?.getItem === 'function';
  } catch {
    return false;
  }
}
if (!localStorageUsable()) {
  const mem = new Map<string, string>();
  const storage: Storage = {
    get length() {
      return mem.size;
    },
    clear() {
      mem.clear();
    },
    getItem(key: string) {
      return mem.has(key) ? mem.get(key)! : null;
    },
    setItem(key: string, value: string) {
      mem.set(key, String(value));
    },
    removeItem(key: string) {
      mem.delete(key);
    },
    key(index: number) {
      return Array.from(mem.keys())[index] ?? null;
    },
  };
  // Use defineProperty: Node's built-in `localStorage` is a non-writable
  // accessor, so a plain assignment would throw.
  Object.defineProperty(globalThis, 'localStorage', {
    value: storage,
    configurable: true,
    writable: true,
  });
}

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
