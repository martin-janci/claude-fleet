import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
// The pinned run width lives in the component's <style>, which jsdom does not
// apply; read the source so the rule itself is regression-tested.
import terminalViewSource from './TerminalView.svelte?raw';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const clipboardReadText = vi.fn();
const clipboardWriteText = vi.fn();
type DragDropPayload = { type: string; position: { x: number; y: number }; paths?: string[] };
let dragDrop: ((e: { payload: DragDropPayload }) => void) | null = null;
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (cb: (e: { payload: DragDropPayload }) => void) => {
      dragDrop = cb;
      return () => {};
    },
  }),
}));

vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  readText: (...a: unknown[]) => clipboardReadText(...a),
  writeText: (...a: unknown[]) => clipboardWriteText(...a),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TerminalView from './TerminalView.svelte';
import { sessions, resetTombstonesForTests, type SessionRow } from './sessions';
import { selectSession, clearSelection } from './selection';
import { toasts, clearToasts } from './toasts';
import { get } from 'svelte/store';
import { copyOnSelect } from './prefs';
import { Screen } from './ansi';
import { DRAIN_MIN_MS } from './terminal_drain';

function makeSession(over: Partial<SessionRow>): SessionRow {
  return {
    id: 1,
    tmux_name: 'dev-martin-janci-claude-fleet',
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  };
}

// Two sessions with the SAME default (project-derived) tmux_name on two hosts —
// the normal case FE-1 was about.
const onAlpha = makeSession({ id: 1, host_alias: 'alpha' });
const onBeta = makeSession({ id: 2, host_alias: 'beta' });

type Inv = ReturnType<typeof vi.fn>;
const inv = () => mockedInvoke as Inv;
const calls = (cmd: string) => inv().mock.calls.filter((c) => c[0] === cmd);
const settle = async (n = 8) => {
  for (let i = 0; i < n; i++) await tick();
};

/** A drain result carrying every field the backend sends. */
const drained = (
  over: Partial<{ data: string; bytes: number; eof: boolean; overflowed: boolean }> = {},
) => ({ data: '', bytes: 0, eof: false, overflowed: false, ...over });

/** A promise a test resolves by hand, to hold one invoke in flight. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

// ResizeObserver stub that lets a test fire the observer callback by hand.
let resizeCallbacks: Array<() => void> = [];
class FakeResizeObserver {
  cb: () => void;
  constructor(cb: () => void) {
    this.cb = cb;
    resizeCallbacks.push(cb);
  }
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  inv().mockReset();
  inv().mockImplementation(async (cmd: string) => {
    if (cmd === 'pty_drain') return { data: '', bytes: 0 };
    return null;
  });
  resizeCallbacks = [];
  resetTombstonesForTests();
  // @ts-expect-error: test stub
  globalThis.ResizeObserver = FakeResizeObserver;
  sessions.set([onAlpha, onBeta]);
  clearSelection();
  clearToasts();
});

afterEach(() => {
  clearSelection();
});

describe('TerminalView session identity (FE-1)', () => {
  it('runs only the automatic workspace check (never explicit) before attaching', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    const repair = calls('repair_session');
    expect(repair).toHaveLength(1);
    expect(repair[0][1]).toEqual({ args: { session_id: 1, explicit: false } });
    const order = inv().mock.calls.map((c) => c[0]);
    expect(order.indexOf('repair_session')).toBeLessThan(order.indexOf('pty_open'));
  });

  it('attaches to the selected session by host + name', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    const opens = calls('pty_open');
    expect(opens).toHaveLength(1);
    expect((opens[0][1] as { args: { host_alias: string; session_name: string } }).args).toMatchObject({
      host_alias: 'alpha',
      session_name: onAlpha.tmux_name,
    });
    expect(screen.getByTestId('terminal-header').textContent).toContain('on alpha');
  });

  it('puts the Transfer chip on the host name for a movable session', async () => {
    const movable = makeSession({
      id: 1, host_alias: 'alpha', kind: 'work', worktree_id: 10, claude_session_id: 'c-1',
    });
    // selectedSession is derived from the `sessions` store by identity, not
    // from the object passed to selectSession — so the store must carry the
    // movable fields, or the lookup resolves to the plain `onAlpha` seeded
    // in beforeEach and canMoveSession sees worktree_id/claude_session_id: null.
    sessions.set([movable, onBeta]);
    render(TerminalView);
    selectSession(movable);
    await settle();
    const header = screen.getByTestId('terminal-header');
    expect(header.textContent?.replace(/\s+/g, ' ')).toContain('on alpha');
    expect(header.querySelector('[data-testid="transfer-chip"]')).not.toBeNull();
  });

  it("selecting the same-named session on another host reattaches to that host's PTY", async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    expect(calls('pty_open')).toHaveLength(1);

    selectSession(onBeta);
    await settle();

    const opens = calls('pty_open');
    expect(opens).toHaveLength(2);
    expect((opens[1][1] as { args: { host_alias: string } }).args.host_alias).toBe('beta');
    // The first PTY was closed before the second was opened.
    expect(calls('pty_close')).toHaveLength(1);
    expect(screen.getByTestId('terminal-header').textContent).toContain('on beta');
  });

  it('does not reattach when the selected row is merely updated (same host + name)', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    expect(calls('pty_open')).toHaveLength(1);

    // A session:updated for the selected row yields a new object with the
    // same identity — the open/attach guard must not re-open.
    sessions.set([{ ...onAlpha, claude_status: 'working', last_activity_at: 2 }, onBeta]);
    await settle();
    expect(calls('pty_open')).toHaveLength(1);
    expect(calls('pty_close')).toHaveLength(0);
  });
});

describe('TerminalView resize debounce (FE-11)', () => {
  it('coalesces a burst of ResizeObserver frames into one trailing pty_resize with the final size', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    expect(resizeCallbacks).toHaveLength(1);
    const host = screen.getByTestId('terminal-host');
    // Let the post-attach 150 ms "hint" resize go by so it can't be confused
    // with the debounced one. (Real timers up to here; fake from now on.)
    await new Promise((r) => setTimeout(r, 200));
    const before = calls('pty_resize').length;

    vi.useFakeTimers();
    try {
      // Simulate a drag: five frames, each with a different width. jsdom's
      // clientWidth is 0 by default; define it per frame.
      const widths = [400, 480, 560, 640, 720];
      for (const w of widths) {
        Object.defineProperty(host, 'clientWidth', { configurable: true, value: w });
        Object.defineProperty(host, 'clientHeight', { configurable: true, value: 300 });
        resizeCallbacks[0]();
        vi.advanceTimersByTime(10);
      }
      // 40 ms after the last frame: still inside the 50 ms window → nothing sent.
      expect(calls('pty_resize').length - before).toBe(0);
      vi.advanceTimersByTime(45);
      const after = calls('pty_resize').slice(before);
      expect(after).toHaveLength(1);
      // Final size only: (720 - 8) / 7.8 (fallback cell width) → 91 cols.
      const args = (after[0][1] as { args: { cols: number; rows: number } }).args;
      expect(args.cols).toBe(Math.floor((720 - 8) / 7.8));
      expect(args.rows).toBe(Math.floor((300 - 8) / 16));
    } finally {
      vi.useRealTimers();
    }
  });

  it('rate-limits a slow drag instead of resizing once per observer frame', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    const host = screen.getByTestId('terminal-host');
    await new Promise((r) => setTimeout(r, 200));
    const before = calls('pty_resize').length;

    vi.useFakeTimers();
    try {
      // A jerky drag: frames further apart than the debounce window, so each
      // one used to send its own pty_resize (a SIGWINCH + a full tmux redraw
      // over SSH).
      Object.defineProperty(host, 'clientHeight', { configurable: true, value: 300 });
      let width = 400;
      for (let frame = 0; frame < 10; frame++) {
        width += 30;
        Object.defineProperty(host, 'clientWidth', { configurable: true, value: width });
        resizeCallbacks[0]();
        await vi.advanceTimersByTimeAsync(67);
      }
      await vi.advanceTimersByTimeAsync(300);
      const sent = calls('pty_resize').slice(before);
      // ~one per 250 ms of drag, not one per frame.
      expect(sent.length).toBeLessThanOrEqual(5);
      // The settled size is never dropped.
      const args = (sent[sent.length - 1][1] as { args: { cols: number; rows: number } }).args;
      expect(args.cols).toBe(Math.floor((width - 8) / 7.8));
    } finally {
      vi.useRealTimers();
    }
  });

  it('skips the resize entirely when the settled size is unchanged', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    await new Promise((r) => setTimeout(r, 200));
    const before = calls('pty_resize').length;
    vi.useFakeTimers();
    try {
      // Same (default 0×0 → clamped 10×2) size as at open → no-op.
      resizeCallbacks[0]();
      resizeCallbacks[0]();
      vi.advanceTimersByTime(100);
      expect(calls('pty_resize').length - before).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('TerminalView run boxes are pinned to the cell grid (F3)', () => {
  it('publishes --cell-w and sizes every run box from it', () => {
    // Both halves matter: the custom property carries the measured cell, and
    // the rule multiplies it by the run's cell count. Drop either and glyph
    // drift returns while the cursor and selection overlays stay put.
    expect(terminalViewSource).toContain('--cell-w={cellWidth > 0');
    expect(terminalViewSource).toMatch(/width:\s*calc\(var\(--cell-w\)\s*\*\s*var\(--n,\s*1\)\)/);
  });
});

describe('TerminalView drain resilience (F1)', () => {
  it('runs the rest of the tick after a chunk blows up the parser', async () => {
    // The bytes are already consumed, so the eof the same drain reported must
    // still be acted on — otherwise a dead PTY goes unnoticed until a reopen.
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') return drained({ data: 'boom', bytes: 4, eof: true });
      return null;
    });
    const write = vi.spyOn(Screen.prototype, 'write').mockImplementation(() => {
      throw new Error('parser bug');
    });
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      const before = calls('pty_open').length;
      await vi.advanceTimersByTimeAsync(40); // the tick that throws
      await vi.advanceTimersByTimeAsync(2000); // auto-reconnect backoff
      await settle();
      expect(calls('pty_open').length).toBeGreaterThan(before);
    } finally {
      vi.useRealTimers();
      write.mockRestore();
      errors.mockRestore();
    }
  });

  it('keeps rendering after one chunk blows up the parser', async () => {
    const chunks = ['boom', 'after'];
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        const d = chunks.shift();
        return d === undefined ? { data: '', bytes: 0 } : { data: d, bytes: d.length };
      }
      return null;
    });
    const write = vi.spyOn(Screen.prototype, 'write');
    write.mockImplementationOnce(() => {
      throw new Error('parser bug');
    });
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      await vi.advanceTimersByTimeAsync(40); // tick 1: screen.write throws
      await vi.advanceTimersByTimeAsync(40); // tick 2: must still be polling
      await settle();
      expect(screen.getByTestId('terminal-host').textContent).toContain('after');
      expect(errors).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
      write.mockRestore();
      errors.mockRestore();
    }
  });
});

describe('TerminalView PTY death detection (F11/N8)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('session output that prints the EOF marker does not tear down a healthy attach', async () => {
    // Exactly what `grep -n "PTY EOF" pty.rs` shows inside an attached pane.
    const line = '[cf] PTY EOF after 0 bytes (tmux attach exited)';
    let served = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        if (served) return drained();
        served = true;
        return drained({ data: line, bytes: line.length });
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(calls('pty_open')).toHaveLength(1);
    expect(calls('pty_close')).toHaveLength(0);
    expect(screen.queryByTestId('terminal-autoreconnect-banner')).toBeNull();
    expect(screen.queryByTestId('terminal-reconnect-banner')).toBeNull();
  });

  it('re-attaches instead of rendering a stream the backend had to truncate (F6)', async () => {
    let overflowed = true;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        if (overflowed) {
          overflowed = false;
          return drained({ overflowed: true, data: 'garbage', bytes: 7 });
        }
        return drained();
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    await vi.advanceTimersByTimeAsync(40);
    await settle();
    expect(calls('pty_close')).toHaveLength(1);
    expect(calls('pty_open')).toHaveLength(2);
    // Whatever survived the trim is mid-sequence and mode-desynced: it must
    // never reach the screen.
    expect(screen.getByTestId('terminal-host').textContent).not.toContain('garbage');
  });

  it('an out-of-band eof with no bytes left reattaches', async () => {
    let dead = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_open') {
        dead = false;
        return null;
      }
      if (cmd === 'pty_drain') {
        if (dead) return drained();
        dead = true;
        return drained({ eof: true });
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    await vi.advanceTimersByTimeAsync(40);
    await settle();
    expect(screen.queryByTestId('terminal-autoreconnect-banner')).not.toBeNull();
    await vi.advanceTimersByTimeAsync(700);
    await settle();
    expect(calls('pty_open')).toHaveLength(2);
  });

  it('output from a doomed attach does not refill the self-heal budget', async () => {
    // Every attach prints something (a login profile, a tmux error) and then
    // dies. The output used to reset the retry count, so the cap was never
    // reached and the pane said "reconnecting…" forever.
    let served = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_open') {
        served = false;
        return null;
      }
      if (cmd === 'pty_drain') {
        if (!served) {
          served = true;
          return drained({ data: 'profile noise', bytes: 13 });
        }
        return drained({ eof: true });
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    await vi.advanceTimersByTimeAsync(8000);
    await settle();
    // 1 initial attach + MAX_AUTO_RECONNECT (3) tries, then the manual banner.
    expect(calls('pty_open')).toHaveLength(4);
    expect(screen.queryByTestId('terminal-reconnect-banner')).not.toBeNull();
  });

  it('an attach that stays up long enough earns a fresh budget', async () => {
    let dead = false;
    let deaths = 0;
    let openedAt = Date.now();
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_open') {
        dead = false;
        return null;
      }
      if (cmd === 'pty_drain') {
        // Die once per attach, but only after 12 s of healthy attachment.
        if (!dead && Date.now() - openedAt >= 12_000) {
          dead = true;
          deaths += 1;
          return drained({ eof: true });
        }
        return drained();
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    for (let i = 0; i < 5; i++) {
      openedAt = Date.now();
      await vi.advanceTimersByTimeAsync(13_000);
      await settle();
    }
    expect(deaths).toBeGreaterThan(3);
    // Every reattach stayed up past the health threshold, so the budget was
    // restored each time and the manual banner never appeared.
    expect(screen.queryByTestId('terminal-reconnect-banner')).toBeNull();
  });
});

describe('TerminalView open lifecycle (F12/N4)', () => {
  const openedHosts = () =>
    calls('pty_open').map((c) => (c[1] as { args: { host_alias: string } }).args.host_alias);

  it('a session switch during an open that then fails still attaches the new session', async () => {
    const gate = deferred<null>();
    inv().mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'pty_drain') return drained();
      if (cmd === 'pty_open') {
        const host = (args as { args: { host_alias: string } }).args.host_alias;
        if (host === 'alpha') return gate.promise;
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    expect(openedHosts()).toEqual(['alpha']);

    // The user switches while alpha's pty_open is still in flight…
    selectSession(onBeta);
    await settle();
    // …and alpha's open then fails.
    gate.reject({ message: 'ssh: Connection refused' });
    await settle(16);

    expect(openedHosts()).toEqual(['alpha', 'beta']);
    expect(screen.getByTestId('terminal-header').textContent).toContain('on beta');
    // Alpha's failure must not be shown under beta's header.
    expect(document.body.textContent).not.toContain('PTY error');
  });

  it('a failing auto-reconnect spends the whole budget, then offers the manual banner', async () => {
    let opened = 0;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_open') {
        opened += 1;
        if (opened > 1) throw { message: 'ssh: Connection refused' };
        return null;
      }
      if (cmd === 'pty_drain') return drained({ eof: true });
      return null;
    });
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      await vi.advanceTimersByTimeAsync(8000);
      await settle();
      // The initial attach plus MAX_AUTO_RECONNECT (3) backed-off retries.
      expect(calls('pty_open')).toHaveLength(4);
      expect(screen.queryByTestId('terminal-reconnect-banner')).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('a deselect during the workspace probe leaves no PTY behind', async () => {
    const probe = deferred<unknown>();
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'repair_session') return probe.promise;
      if (cmd === 'pty_drain') return drained();
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    expect(calls('repair_session')).toHaveLength(1);
    expect(calls('pty_open')).toHaveLength(0);

    clearSelection();
    await settle();
    probe.resolve({ actions: [], warnings: [] });
    await settle(16);
    // The open was abandoned mid-flight: nothing was attached, so nothing
    // has to be closed either.
    expect(calls('pty_open')).toHaveLength(0);

    // …and the session still attaches when it is selected again.
    selectSession(onAlpha);
    await settle(16);
    expect(calls('pty_open')).toHaveLength(1);
    expect(screen.getByTestId('terminal-size').textContent).not.toContain('measuring');
  });

  it('reselecting the SAME session mid-probe still attaches', async () => {
    // The user leaves and comes straight back while the SSH workspace probe is
    // still out. The in-flight open stands down (its generation is gone), so
    // the coalesced request is all that is left to attach the pane — and it is
    // for the very session that open targeted.
    const probe = deferred<unknown>();
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'repair_session') return probe.promise;
      if (cmd === 'pty_drain') return drained();
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    expect(calls('pty_open')).toHaveLength(0);

    clearSelection();
    await settle();
    selectSession(onAlpha);
    await settle();
    probe.resolve({ actions: [], warnings: [] });
    await settle(16);

    expect(calls('pty_open')).toHaveLength(1);
    expect(screen.getByTestId('terminal-size').textContent).not.toContain('measuring');
  });

  it('a stale open that already attached closes its own PTY', async () => {
    const gate = deferred<null>();
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_open') return gate.promise;
      if (cmd === 'pty_drain') return drained();
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    expect(calls('pty_open')).toHaveLength(1);
    clearSelection();
    await settle();
    gate.resolve(null); // the attach lands after the pane let it go
    await settle(16);
    expect(calls('pty_close')).toHaveLength(1);
  });

  it('destroying the pane mid-open neither attaches nor resizes afterwards', async () => {
    const probe = deferred<unknown>();
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'repair_session') return probe.promise;
      if (cmd === 'pty_drain') return drained();
      return null;
    });
    const view = render(TerminalView);
    selectSession(onAlpha);
    await settle();
    view.unmount();
    await settle();
    probe.resolve({ actions: [], warnings: [] });
    await settle(16);
    expect(calls('pty_open')).toHaveLength(0);
    expect(calls('pty_resize')).toHaveLength(0);
  });

  it('a stale post-attach resize hint cannot fire into the next attach', async () => {
    // The hint is armed 150 ms after an attach. Re-attaching inside that
    // window must not leave the previous one's timer to fire as well.
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      clearSelection();
      await settle();
      selectSession(onAlpha);
      await settle();
      const before = calls('pty_resize').length;
      await vi.advanceTimersByTimeAsync(400);
      expect(calls('pty_resize').length - before).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('the post-attach resize hint does not fire after the pane is closed', async () => {
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      clearSelection();
      await settle();
      await vi.advanceTimersByTimeAsync(400);
      expect(calls('pty_resize')).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('TerminalView drag-drop upload (N5)', () => {
  const written = () => calls('pty_write').map((c) => (c[1] as { args: { data: string } }).args.data);
  const drop = (paths: string[]) =>
    dragDrop?.({ payload: { type: 'drop', position: { x: 0, y: 0 }, paths } });

  it('pastes the uploaded paths into the session that started the upload', async () => {
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') return drained();
      if (cmd === 'upload_to_session') return ['/home/alpha/.cf-uploads/big.zip'];
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    drop(['/Users/me/big.zip']);
    await settle(16);
    expect(written().join('')).toContain('/home/alpha/.cf-uploads/big.zip');
  });

  it("never types one host's paths into the session attached later", async () => {
    const upload = deferred<string[]>();
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') return drained();
      if (cmd === 'upload_to_session') return upload.promise;
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    drop(['/Users/me/big.zip']);
    await settle();
    expect(calls('upload_to_session')).toHaveLength(1);

    // scp takes a while; the user moves to another host in the meantime.
    selectSession(onBeta);
    await settle(16);
    expect(screen.queryByTestId('terminal-drop-overlay')).toBeNull();

    upload.resolve(['/home/alpha/.cf-uploads/big.zip']);
    await settle(16);
    expect(written().join('')).not.toContain('big.zip');
    // The paths are not lost — they are reported instead.
    expect(get(toasts).some((t) => t.message.includes('/home/alpha/.cf-uploads/big.zip'))).toBe(true);
  });
});

describe('TerminalView idle cost (F17)', () => {
  it('an idle attached terminal does not rewrite the header', async () => {
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') return drained();
      return null;
    });
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      await vi.advanceTimersByTimeAsync(100);
      const counters = screen.getByTestId('terminal-counters');
      const text = counters.textContent;
      // Several seconds of empty polls (the loop backs off to 250 ms, so ~15).
      await vi.advanceTimersByTimeAsync(4000);
      await settle();
      expect(screen.getByTestId('terminal-counters').textContent).toBe(text);
    } finally {
      vi.useRealTimers();
    }
  });

  it('still reports the bytes that did arrive', async () => {
    let served = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        if (served) return drained();
        served = true;
        return drained({ data: 'hello', bytes: 5 });
      }
      return null;
    });
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      await vi.advanceTimersByTimeAsync(100);
      await settle();
      expect(screen.getByTestId('terminal-counters').textContent).toContain('5B');
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('TerminalView pty_write errors (FE-12)', () => {
  it('surfaces a rejected pty_write as one deduped error toast', async () => {
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') return { data: '', bytes: 0 };
      if (cmd === 'pty_write') throw { code: 'E_PTY', message: 'pty closed' };
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    const host = screen.getByTestId('terminal-host');
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'a', bubbles: true }));
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'b', bubbles: true }));
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'c', bubbles: true }));
    await settle();
    expect(calls('pty_write')).toHaveLength(3);
    const all = get(toasts);
    expect(all).toHaveLength(1);
    expect(all[0]).toMatchObject({ kind: 'error', code: 'E_PTY', count: 3 });
    expect(all[0].message).toContain('pty closed');
  });
});

describe('TerminalView keyboard (FE-6)', () => {
  const written = () => calls('pty_write').map((c) => (c[1] as { args: { data: string } }).args.data);

  beforeEach(() => {
    clipboardReadText.mockReset();
    clipboardWriteText.mockReset();
  });

  it('Ctrl+Shift+V pastes the native clipboard on Linux', async () => {
    clipboardReadText.mockResolvedValue('pasted text');
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    const host = screen.getByTestId('terminal-host');
    const ev = new KeyboardEvent('keydown', {
      key: 'V', code: 'KeyV', ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true,
    });
    host.dispatchEvent(ev);
    await settle();
    expect(ev.defaultPrevented).toBe(true);
    expect(clipboardReadText).toHaveBeenCalledTimes(1);
    expect(written()).toEqual(['pasted text']);
  });

  it('Ctrl+Shift+C does not send SIGINT; plain Ctrl+C still does', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    const host = screen.getByTestId('terminal-host');
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'C', ctrlKey: true, shiftKey: true, bubbles: true }));
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true, bubbles: true }));
    await settle();
    expect(written()).toEqual(['\x03']);
  });

  it('forwards Delete, F-keys, modifier-encoded arrows and Alt+key via the xterm key table', async () => {
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    const host = screen.getByTestId('terminal-host');
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'F5', bubbles: true }));
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', ctrlKey: true, bubbles: true }));
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', altKey: true, bubbles: true }));
    await settle();
    expect(written()).toEqual(['\x1b[3~', '\x1b[15~', '\x1b[1;5C', '\x1bx']);
  });

  it('answers a DSR 6 cursor-position query carried by the drained output', async () => {
    let served = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        if (served) return { data: '', bytes: 0 };
        served = true;
        // jsdom reports clientHeight 0, so the screen is the 2-row minimum:
        // row 2 is the last one that exists.
        return { data: '\x1b[2;4H\x1b[6n', bytes: 10 };
      }
      return null;
    });
    vi.useFakeTimers();
    try {
      render(TerminalView);
      selectSession(onAlpha);
      await settle();
      // First drain tick fires after DRAIN_MIN_MS (30 ms); the async variant
      // lets the mocked pty_drain promise resolve inside the tick.
      await vi.advanceTimersByTimeAsync(40);
      expect(written()).toContain('\x1b[2;4R');
    } finally {
      vi.useRealTimers();
    }
  });
});

// ─── Selection & cursor: text-input conventions ───────────────────────────
describe('TerminalView selection like a text input', () => {
  const written = () => calls('pty_write').map((c) => (c[1] as { args: { data: string } }).args.data);
  // jsdom: no layout, so the grid is the 10×2 minimum and cells use the
  // 7.8×16 fallback metrics with the container rect at (0,0).
  const CW = 7.8;
  const CH = 16;
  const xOf = (col: number) => 4 + col * CW + 1;
  const yOf = (row: number) => 4 + row * CH + 1;

  /** Mount, attach, and feed one chunk of output through pty_drain. */
  async function mountWith(data: string): Promise<HTMLElement> {
    let served = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        if (served) return { data: '', bytes: 0 };
        served = true;
        return { data, bytes: data.length };
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    await vi.advanceTimersByTimeAsync(40);
    await settle();
    return screen.getByTestId('terminal-host');
  }

  function mouse(type: string, init: MouseEventInit & { detail?: number }) {
    return new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, ...init });
  }

  function selectionRectsPx(): Array<{ left: number; width: number; top: number }> {
    return screen.queryAllByTestId('terminal-selection').map((el) => ({
      left: parseFloat(el.style.left),
      width: parseFloat(el.style.width),
      top: parseFloat(el.style.top),
    }));
  }

  beforeEach(() => {
    clipboardReadText.mockReset();
    clipboardWriteText.mockReset();
    clipboardWriteText.mockResolvedValue(undefined);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('double-click selects the word under the pointer; Ctrl+Shift+C copies it', async () => {
    const host = await mountWith('ab cd ef');
    host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(4), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(4), clientY: yOf(0) }));
    await settle();
    // "cd" = cols 3..4 → one rect from col 3, two cells wide.
    expect(selectionRectsPx()).toEqual([{ left: 4 + 3 * CW, width: 2 * CW, top: 4 }]);
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'C', ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }));
    await settle();
    expect(clipboardWriteText).toHaveBeenCalledWith('cd');
  });

  it('double-click, then drag into another word, extends by whole words', async () => {
    const host = await mountWith('ab cd ef');
    host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(4), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mousemove', { clientX: xOf(6), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(6), clientY: yOf(0) }));
    await settle();
    // From the start of "cd" (col 3) to the end of "ef" (col 7).
    expect(selectionRectsPx()).toEqual([{ left: 4 + 3 * CW, width: 5 * CW, top: 4 }]);
  });

  it('triple-click selects the whole line', async () => {
    const host = await mountWith('ab cd ef');
    host.dispatchEvent(mouse('mousedown', { detail: 3, clientX: xOf(4), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(4), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toEqual([{ left: 4, width: 10 * CW, top: 4 }]);
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'C', ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }));
    await settle();
    // Trailing blanks are trimmed on copy.
    expect(clipboardWriteText).toHaveBeenCalledWith('ab cd ef');
  });

  it('Shift+click extends the existing selection from its anchor', async () => {
    const host = await mountWith('ab cd ef');
    host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(0), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(0), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toEqual([{ left: 4, width: 2 * CW, top: 4 }]);
    host.dispatchEvent(mouse('mousedown', { detail: 1, shiftKey: true, clientX: xOf(6), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(6), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toEqual([{ left: 4, width: 7 * CW, top: 4 }]);
  });

  it('Ctrl+Shift+A selects the whole screen on Linux', async () => {
    const host = await mountWith('ab cd ef');
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'A', ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }));
    await settle();
    // 10×2 grid → one full-width rect per row.
    expect(selectionRectsPx()).toEqual([
      { left: 4, width: 10 * CW, top: 4 },
      { left: 4, width: 10 * CW, top: 4 + CH },
    ]);
    expect(written()).toEqual([]);
  });

  it('a plain click clears the selection; typing clears it too', async () => {
    const host = await mountWith('ab cd ef');
    host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(4), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(4), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toHaveLength(1);
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', bubbles: true, cancelable: true }));
    await settle();
    expect(selectionRectsPx()).toHaveLength(0);
    expect(written()).toContain('x');

    // Select again, then a single click with no drag drops it.
    host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(4), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(4), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toHaveLength(1);
    host.dispatchEvent(mouse('mousedown', { detail: 1, clientX: xOf(1), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(1), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toHaveLength(0);
  });

  it('a double-click selects locally even when the app has mouse reporting on', async () => {
    const host = await mountWith('\x1b[?1000h\x1b[?1006hab cd ef');
    host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(4), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(4), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toEqual([{ left: 4 + 3 * CW, width: 2 * CW, top: 4 }]);
    // Nothing was forwarded to the app as a click.
    expect(written().filter((d) => d.startsWith('\x1b[<'))).toHaveLength(0);
  });

  it('a wide glyph is rendered as a 2-cell span so columns after it line up', async () => {
    const host = await mountWith('a😀b');
    const wide = host.querySelectorAll('.row span.wide');
    expect(wide).toHaveLength(1);
    expect(wide[0].textContent).toBe('😀');
    // Double-click on the glyph selects the whole "a😀b" word (cols 0..3).
    host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(2), clientY: yOf(0) }));
    window.dispatchEvent(mouse('mouseup', { clientX: xOf(2), clientY: yOf(0) }));
    await settle();
    expect(selectionRectsPx()).toEqual([{ left: 4, width: 4 * CW, top: 4 }]);
  });

  it('pins every run to its cell count in units of the measured cell width', async () => {
    // ⏺ (U+23FA) is not in Menlo; its fallback advance must not move " ok".
    const host = await mountWith('\u23fa ok\u4e2d!');
    expect(host.style.getPropertyValue('--cell-w')).toBe(`${CW}px`);
    const row = host.querySelectorAll<HTMLElement>('.row')[0];
    const spans = Array.from(row.querySelectorAll<HTMLElement>('span'));
    expect(
      spans.map((el) => [
        el.textContent,
        el.style.getPropertyValue('--n'),
        el.classList.contains('glyph'),
        el.classList.contains('wide'),
      ]),
    ).toEqual([
      ['\u23fa', '1', true, false],
      [' ok', '3', false, false],
      ['\u4e2d', '2', false, true],
      // jsdom's grid is the 10-column minimum.
      ['!   ', '4', false, false],
    ]);
  });

  it('a plain click on either half of a wide glyph clears the selection and copies nothing', async () => {
    // The highlight snaps a press on a wide glyph to both of its cells, so
    // emptiness has to come from the gesture, not the snapped endpoints.
    const prev = get(copyOnSelect);
    copyOnSelect.set(true);
    try {
      const host = await mountWith('ab中cd');
      for (const col of [2, 3]) {
        host.dispatchEvent(mouse('mousedown', { detail: 2, clientX: xOf(4), clientY: yOf(0) }));
        window.dispatchEvent(mouse('mouseup', { clientX: xOf(4), clientY: yOf(0) }));
        await settle();
        expect(selectionRectsPx()).toHaveLength(1);
        clipboardWriteText.mockClear();
        host.dispatchEvent(mouse('mousedown', { detail: 1, clientX: xOf(col), clientY: yOf(0) }));
        window.dispatchEvent(mouse('mouseup', { clientX: xOf(col), clientY: yOf(0) }));
        await settle();
        expect(selectionRectsPx()).toHaveLength(0);
        expect(clipboardWriteText).not.toHaveBeenCalled();
      }
      // A drag that ends on the glyph still copies it whole.
      host.dispatchEvent(mouse('mousedown', { detail: 1, clientX: xOf(0), clientY: yOf(0) }));
      window.dispatchEvent(mouse('mousemove', { clientX: xOf(2), clientY: yOf(0) }));
      window.dispatchEvent(mouse('mouseup', { clientX: xOf(2), clientY: yOf(0) }));
      await settle();
      expect(selectionRectsPx()).toEqual([{ left: 4, width: 4 * CW, top: 4 }]);
      expect(clipboardWriteText).toHaveBeenCalledWith('ab中');
    } finally {
      copyOnSelect.set(prev);
    }
  });
});

describe('TerminalView cursor like a text input', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  async function mountWith(data: string): Promise<HTMLElement> {
    let served = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        if (served) return { data: '', bytes: 0 };
        served = true;
        return { data, bytes: data.length };
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    await vi.advanceTimersByTimeAsync(40);
    await settle();
    return screen.getByTestId('terminal-host');
  }

  it('is a hollow outline until the terminal has focus, then a blinking block', async () => {
    const host = await mountWith('ab');
    let cur = screen.getByTestId('terminal-cursor');
    expect(cur.classList.contains('block')).toBe(true);
    expect(cur.classList.contains('unfocused')).toBe(true);
    expect(cur.classList.contains('blink')).toBe(false);
    host.focus();
    await settle();
    cur = screen.getByTestId('terminal-cursor');
    expect(cur.classList.contains('unfocused')).toBe(false);
    expect(cur.classList.contains('blink')).toBe(true);
    // Focus sits on the hidden IME proxy, not the grid (F9) — blur whatever
    // ended up holding it.
    (document.activeElement as HTMLElement | null)?.blur();
    await settle();
    cur = screen.getByTestId('terminal-cursor');
    expect(cur.classList.contains('unfocused')).toBe(true);
  });

  it('follows DECSCUSR: a steady bar (CSI 6 SP q) neither blinks nor fills the cell', async () => {
    const host = await mountWith('\x1b[6 qab');
    host.focus();
    await settle();
    const cur = screen.getByTestId('terminal-cursor');
    expect(cur.classList.contains('bar')).toBe(true);
    expect(cur.classList.contains('blink')).toBe(false);
    expect(parseFloat(cur.style.left)).toBeCloseTo(4 + 2 * 7.8, 5);
  });

  it('restarts the blink on each keystroke (the element is re-created)', async () => {
    const host = await mountWith('ab');
    host.focus();
    await settle();
    const before = screen.getByTestId('terminal-cursor');
    host.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', bubbles: true, cancelable: true }));
    await settle();
    const after = screen.getByTestId('terminal-cursor');
    expect(after).not.toBe(before);
    expect(after.classList.contains('blink')).toBe(true);
  });

  it('covers both cells of a wide glyph', async () => {
    // Cursor on the emoji head at col 1 → 2 cells wide.
    await mountWith('a😀b\x1b[2G');
    const cur = screen.getByTestId('terminal-cursor');
    expect(parseFloat(cur.style.width)).toBeCloseTo(2 * 7.8, 5);
  });

  it('never sits past the last column', async () => {
    // Ten chars fill the 10-column row: deferred wrap parks the cursor past
    // the edge; it is drawn on the last column.
    await mountWith('0123456789');
    const cur = screen.getByTestId('terminal-cursor');
    expect(parseFloat(cur.style.left)).toBeCloseTo(4 + 9 * 7.8, 5);
  });
});

// ─── IME / dead-key input through the hidden proxy (F9) ───────────────────
describe('TerminalView IME input proxy (F9)', () => {
  const written = () => calls('pty_write').map((c) => (c[1] as { args: { data: string } }).args.data);

  /** Mount, attach, and hand back the proxy the input method types into.
   *  With `data`, wait out one real drain tick (DRAIN_MIN_MS) so the escape
   *  sequence has actually reached the screen — this describe runs on real
   *  timers, and `settle()` alone only flushes Svelte, not the drain loop. */
  async function mountProxy(data = ''): Promise<HTMLTextAreaElement> {
    let served = false;
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') {
        if (served) return drained();
        served = true;
        return drained({ data, bytes: data.length });
      }
      return null;
    });
    render(TerminalView);
    selectSession(onAlpha);
    await settle();
    if (data !== '') {
      await new Promise((r) => setTimeout(r, DRAIN_MIN_MS + 30));
      await settle();
    }
    return screen.getByTestId('terminal-ime') as HTMLTextAreaElement;
  }

  const key = (init: KeyboardEventInit) =>
    new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
  /** What the browser leaves behind for us: the text is in the proxy's value
   *  by the time either event fires. */
  const compose = (el: HTMLTextAreaElement, text: string) => {
    el.value = text;
    return new CompositionEvent('compositionend', { data: text, bubbles: true });
  };
  const inputEvent = (inputType: string, data: string) =>
    new InputEvent('input', { inputType, data, bubbles: true });
  /** Let the compositionJustEnded flag's setTimeout(0) run. */
  const nextMacrotask = () => new Promise((r) => setTimeout(r, 0));

  it('renders a focusable proxy inside the grid', async () => {
    const ime = await mountProxy();
    expect(ime.tagName).toBe('TEXTAREA');
    expect(screen.getByTestId('terminal-host').contains(ime)).toBe(true);
    // Nothing the OS adds on its own may reach the PTY.
    expect(ime.getAttribute('autocapitalize')).toBe('off');
    expect(ime.getAttribute('autocomplete')).toBe('off');
    expect(ime.getAttribute('autocorrect')).toBe('off');
    expect(ime.getAttribute('spellcheck')).toBe('false');
  });

  it('a dead-key composition reaches the PTY exactly once (compositionend first)', async () => {
    const ime = await mountProxy();
    ime.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    // The keystrokes the input method swallowed while composing.
    ime.dispatchEvent(key({ key: 'Dead', keyCode: 229 }));
    ime.dispatchEvent(key({ key: 'a', keyCode: 229, isComposing: true }));
    ime.dispatchEvent(compose(ime, 'á'));
    // Chromium follows compositionend with an input event for the same text.
    ime.dispatchEvent(inputEvent('insertCompositionText', 'á'));
    await settle();
    expect(written()).toEqual(['á']);
  });

  it('…and once when the input event lands before compositionend (WebKit order)', async () => {
    const ime = await mountProxy();
    ime.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    ime.value = 'á';
    ime.dispatchEvent(inputEvent('insertCompositionText', 'á'));
    ime.dispatchEvent(compose(ime, 'á'));
    await settle();
    expect(written()).toEqual(['á']);
  });

  it('drops the keystrokes an IME is still holding', async () => {
    const ime = await mountProxy();
    ime.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    ime.dispatchEvent(key({ key: 'Process', keyCode: 229 }));
    ime.dispatchEvent(key({ key: 'n', keyCode: 229, isComposing: true }));
    ime.dispatchEvent(key({ key: 'i', keyCode: 229, isComposing: true }));
    await settle();
    expect(written()).toEqual([]);
  });

  it('swallows the Enter that commits a composition, but not the next one', async () => {
    const ime = await mountProxy();
    ime.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    ime.dispatchEvent(compose(ime, '日本'));
    // WebKit delivers the committing key AFTER compositionend, isComposing
    // already false. Sending it too would submit Claude Code's prompt.
    ime.dispatchEvent(key({ key: 'Enter' }));
    await settle();
    expect(written()).toEqual(['日本']);

    await nextMacrotask();
    ime.dispatchEvent(key({ key: 'Enter' }));
    await settle();
    expect(written()).toEqual(['日本', '\r']);
  });

  it('swallows the Space that commits a composition, but not the next one', async () => {
    // Space commits the first conversion step in most Japanese and Chinese
    // IMEs; sending it too would type a stray space into the prompt.
    const ime = await mountProxy();
    ime.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    ime.dispatchEvent(compose(ime, 'にほん'));
    ime.dispatchEvent(key({ key: ' ' }));
    await settle();
    expect(written()).toEqual(['にほん']);

    await nextMacrotask();
    ime.dispatchEvent(key({ key: ' ' }));
    await settle();
    expect(written()).toEqual(['にほん', ' ']);
  });

  it('sends an emoji-picker / accent-popup insert once', async () => {
    const ime = await mountProxy();
    ime.value = '🤖';
    ime.dispatchEvent(inputEvent('insertText', '🤖'));
    await settle();
    expect(written()).toEqual(['🤖']);
    // Nothing is left behind to be sent a second time.
    ime.dispatchEvent(inputEvent('insertText', ''));
    await settle();
    expect(written()).toEqual(['🤖']);
  });

  it('keeps forwarding ordinary keys, which never reach the proxy as text', async () => {
    const ime = await mountProxy();
    ime.dispatchEvent(key({ key: 'x' }));
    ime.dispatchEvent(key({ key: 'ArrowLeft' }));
    await settle();
    expect(written()).toEqual(['x', '\x1b[D']);
  });

  it('hands keyboard focus to the proxy, from the grid and from a click', async () => {
    const ime = await mountProxy();
    const host = screen.getByTestId('terminal-host');
    host.focus();
    await settle();
    expect(document.activeElement).toBe(ime);
    expect(screen.getByTestId('terminal-cursor').classList.contains('unfocused')).toBe(false);

    ime.blur();
    await settle();
    host.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0, detail: 1 }));
    await settle();
    expect(document.activeElement).toBe(ime);
  });

  it('parks the proxy on the cursor cell so the IME popup opens there', async () => {
    const ime = await mountProxy('\x1b[2;4H');
    const cur = screen.getByTestId('terminal-cursor');
    expect(parseFloat(cur.style.left)).toBeCloseTo(4 + 3 * 7.8, 5);
    expect(ime.style.left).toBe(cur.style.left);
    expect(ime.style.top).toBe(cur.style.top);
  });

  /** `isMac` is read once per component instance at render time, so shadowing
   *  navigator.platform before mounting is enough to make the pane behave like
   *  the macOS build. */
  function asMac(): () => void {
    Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true });
    return () => {
      delete (window.navigator as unknown as { platform?: string }).platform;
    };
  }

  it('lets a held key repeat through the proxy so the accent popup can open (macOS)', async () => {
    const restore = asMac();
    try {
      const ime = await mountProxy();
      // The first press is an ordinary keystroke: straight out through the key
      // table, preventDefault and all.
      const first = key({ key: 'e' });
      ime.dispatchEvent(first);
      await settle();
      expect(first.defaultPrevented).toBe(true);
      expect(written()).toEqual(['e']);

      // The repeat is NOT prevented — a prevented keydown never reaches AppKit's
      // interpretKeyEvents:, which is what opens the press-and-hold popup — and
      // it sends nothing by itself.
      const repeat = key({ key: 'e', repeat: true });
      ime.dispatchEvent(repeat);
      await settle();
      expect(repeat.defaultPrevented).toBe(false);
      expect(written()).toEqual(['e']);

      // Whatever the OS does with it arrives as text in the proxy: a plain
      // repeat when press-and-hold is off…
      ime.value = 'e';
      ime.dispatchEvent(inputEvent('insertText', 'e'));
      await settle();
      expect(written()).toEqual(['e', 'e']);

      // …or the accent the user picked from the popup, exactly once.
      ime.value = 'é';
      ime.dispatchEvent(inputEvent('insertReplacementText', 'é'));
      await settle();
      expect(written()).toEqual(['e', 'e', 'é']);
    } finally {
      restore();
    }
  });

  it('still forwards repeats that no popup can claim (macOS)', async () => {
    const restore = asMac();
    try {
      const ime = await mountProxy();
      // Held arrows and chords have no accent alternatives; letting them fall
      // through would drop key-repeat scrolling and Ctrl+C.
      const arrow = key({ key: 'ArrowDown', repeat: true });
      const chord = key({ key: 'e', ctrlKey: true, repeat: true });
      ime.dispatchEvent(arrow);
      ime.dispatchEvent(chord);
      await settle();
      expect(arrow.defaultPrevented).toBe(true);
      expect(chord.defaultPrevented).toBe(true);
      expect(written()).toEqual(['\x1b[B', '\x05']);
    } finally {
      restore();
    }
  });

  it('sends a repeating letter through the key table off macOS', async () => {
    const ime = await mountProxy();
    const repeat = key({ key: 'e', repeat: true });
    ime.dispatchEvent(repeat);
    await settle();
    // No press-and-hold popup exists here, so the repeat is a plain keystroke.
    expect(repeat.defaultPrevented).toBe(true);
    expect(written()).toEqual(['e']);
  });

  it('…and still rides the caret when the app hides the cursor (?25l)', async () => {
    // Full-screen TUIs — Ink-based Claude Code included — keep the cursor
    // hidden while they redraw, which is the state this terminal spends most
    // of its life in. The popup has to open at the caret there too, so the
    // proxy follows the cursor cell, not the cursor OVERLAY.
    const ime = await mountProxy('\x1b[?25l\x1b[2;4H');
    expect(screen.queryByTestId('terminal-cursor')).toBeNull();
    expect(parseFloat(ime.style.left)).toBeCloseTo(4 + 3 * 7.8, 5);
    expect(parseFloat(ime.style.top)).toBeCloseTo(4 + 1 * 16, 5);
    expect(parseFloat(ime.style.height)).toBeCloseTo(16, 5);
  });
});
