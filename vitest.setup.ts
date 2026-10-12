import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// Global Tauri IPC mock: keeps components that call invoke() on mount
// (e.g. App.svelte's healthCheck(), Sidebar's loadProjects() and
// loadSessions()) from crashing in tests. Individual test files can
// override with their own vi.mock for specific commands.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.0.0', db_ready: true, schema_version: 1 };
    // Standalone unless a test says otherwise, which is what every existing
    // test assumes: the hub client mode must change nothing by default.
    if (cmd === 'hub_status')
      return {
        remote: false,
        url: null,
        client_name: null,
        client_mode: null,
        configured_url: null,
        configured_client_name: null,
        allow_plaintext: false,
        warning: null,
        restart_required: false,
        // Spelled out rather than left undefined: `access.ts::backendMode`
        // reads it first, and a HubStatus missing the field is a shape the
        // real backend never sends.
        unavailable: null,
      };
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
    // ── multi-user M1 ──────────────────────────────────────────────────
    // `my_grants` is what a client derives its per-session access from
    // (`src/lib/access.ts`). The default answer is a person with NO grants,
    // which is the ordinary single-user shape; every existing test is
    // standalone, where the derivation answers `own` from the backend mode
    // alone and never reads this at all.
    if (cmd === 'my_grants') return { person_id: 1, grants: [] };
    // An ARRAY, not null: the Share sheet renders the grant list, and `null`
    // would make a component that maps over it throw instead of showing the
    // "not shared with anyone" state.
    if (cmd === 'session_access') return [];
    if (cmd === 'session_share') return null;
    if (cmd === 'session_unshare') return null;
    if (cmd === 'session_narrow') return null;
    // Gap plan G4.2: no open asks by default.
    if (cmd === 'access_requests') return [];
    // Plain text, not JSON (the watcher's read-only pane snapshot).
    if (cmd === 'capture_session') return '';
    if (cmd === 'send_prompt') return null;
    // The fleet agent's status, which Control (redesign step 9.1) reads when
    // it opens: blocked on the control API being off, which is the state a
    // fresh install is in and the one that starts nothing.
    if (cmd === 'operator_status') return { ready: false, session: null, blocked: 'no_mcp' };
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
  emitTo: vi.fn(async () => {}),
}));

// `getVersion()` is an IPC call into the Tauri runtime, which jsdom has none
// of. The footer's version line reads it (`src/lib/app_version.ts`); a test
// that cares about the value mocks this module itself, and everything else
// gets a stable stand-in rather than a rejected promise on mount.
vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(async () => '0.0.0-test'),
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

// jsdom ships an empty HTMLDialogElement (no show/showModal/close). Modal.svelte
// opens itself with showModal() on mount; polyfill the three methods with the
// `open` attribute so the dialog renders visible (and role="dialog" queries
// resolve) in tests. A real browser's top-layer/focus-trap behaviour is not
// emulated — those are covered by the engine, not by us.
if (typeof HTMLDialogElement !== 'undefined') {
  const proto = HTMLDialogElement.prototype as HTMLDialogElement & {
    showModal?: () => void;
    show?: () => void;
    close?: (v?: string) => void;
  };
  if (typeof proto.showModal !== 'function') {
    proto.showModal = function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
    };
  }
  if (typeof proto.show !== 'function') {
    proto.show = function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
    };
  }
  if (typeof proto.close !== 'function') {
    proto.close = function (this: HTMLDialogElement, returnValue?: string) {
      if (!this.hasAttribute('open')) return;
      this.removeAttribute('open');
      if (returnValue !== undefined) this.returnValue = returnValue;
      this.dispatchEvent(new Event('close'));
    };
  }
  if (!Object.getOwnPropertyDescriptor(proto, 'open')) {
    Object.defineProperty(proto, 'open', {
      configurable: true,
      get(this: HTMLDialogElement) {
        return this.hasAttribute('open');
      },
      set(this: HTMLDialogElement, v: boolean) {
        if (v) this.setAttribute('open', '');
        else this.removeAttribute('open');
      },
    });
  }
}
