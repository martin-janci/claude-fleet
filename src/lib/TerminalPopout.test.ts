// A pop-out terminal window (redesign step 5.4): it loads what the pane
// needs, picks its session without overwriting the main window's remembered
// one, and attaches under its own label — never the main window's pty ids.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ label: 'term-1-sh2', onDragDropEvent: async () => () => {} }),
}));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ readText: vi.fn(), writeText: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TerminalPopout from './TerminalPopout.svelte';
import { sessions, resetTombstonesForTests, type SessionRow } from './sessions';
import { clearSelection } from './selection';
import { readPref } from './prefs';

const row = {
  id: 1, tmux_name: 'api', host_alias: 'alpha', project_id: null, worktree_id: null, created_at: 1,
  last_activity_at: 1, status: 'running', notes: null, account_uuid: null, kind: 'work',
  reviews_session_id: null, worktree_key: 'main', lost_at: null, claude_session_id: null,
  claude_status: null, effort_level: null, pr_url: null, current_activity: null, friendly_name: null,
  safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
  context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null,
  last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0,
  last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null,
  context_window: null, context_source: null, context_at: null, context_stale: false,
  tmux_pane_id: null, pending_input: null,
} as unknown as SessionRow;

type Inv = ReturnType<typeof vi.fn>;
const inv = () => mockedInvoke as Inv;
const calls = (cmd: string) => inv().mock.calls.filter((c) => c[0] === cmd);
const args = (c: unknown[]) => (c[1] as { args: Record<string, unknown> }).args;
const settle = async (n = 16) => {
  for (let i = 0; i < n; i++) {
    await tick();
    await new Promise((r) => setTimeout(r, 0));
  }
};

class FakeResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

let rows: SessionRow[] = [];

beforeEach(() => {
  rows = [row];
  inv().mockReset();
  inv().mockImplementation(async (cmd: string) => {
    if (cmd === 'hub_status') return { remote: false };
    if (cmd === 'list_sessions') return rows;
    if (cmd === 'list_hosts') return [];
    if (cmd === 'my_grants') return { person_id: null, grants: [] };
    if (cmd === 'pty_drain') return { data: '', bytes: 0 };
    if (cmd === 'shell_terminals') return { session_id: 1, host_alias: 'alpha', terminals: [{ n: 2, tmux_name: 'api--sh2' }], opened: 2 };
    return null;
  });
  globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
  resetTombstonesForTests();
  sessions.set([]);
  clearSelection();
});

afterEach(() => {
  clearSelection();
});

describe('pop-out terminal window (step 5.4)', () => {
  it('attaches its shell under the window label, with no strip and no microphone', async () => {
    render(TerminalPopout, { popout: { label: 'term-1-sh2', sessionId: 1, shell: 2 } });
    await settle();
    const opens = calls('pty_open').map((c) => args(c));
    expect(opens).toHaveLength(1);
    expect(opens[0]).toMatchObject({ id: 'term-1-sh2', session_name: 'api--sh2', host_alias: 'alpha' });
    expect(screen.queryByTestId('terminal-strip')).toBeNull();
    expect(calls('voice_claim')).toHaveLength(0);
    // The main window's next launch still reopens what it had.
    expect(readPref('session.last', null, (_v): _v is unknown => true)).toBeNull();
  });

  it("pops the agent's pane out under its own id, never `agent`", async () => {
    render(TerminalPopout, { popout: { label: 'term-1-agent', sessionId: 1, shell: null } });
    await settle();
    expect(calls('pty_open').map((c) => args(c))).toEqual([
      expect.objectContaining({ id: 'term-1-agent', session_name: 'api' }),
    ]);
  });

  it('says so when the session is gone', async () => {
    rows = [];
    render(TerminalPopout, { popout: { label: 'term-1-agent', sessionId: 1, shell: null } });
    await settle();
    expect(screen.getByTestId('terminal-popout-gone')).toBeTruthy();
    expect(calls('pty_open')).toHaveLength(0);
  });
});
