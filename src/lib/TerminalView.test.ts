import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const clipboardReadText = vi.fn();
const clipboardWriteText = vi.fn();
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
    safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
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
