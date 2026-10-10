// Shell terminals (redesign step 5.3): the strip in the new layout, one pty
// id per terminal, Split, Close, and the ⌥⌘T / ⌘` chords.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ readText: vi.fn(), writeText: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TerminalView from './TerminalView.svelte';
import { sessions, resetTombstonesForTests, type SessionRow } from './sessions';
import { selectSession, clearSelection } from './selection';
import { clearToasts } from './toasts';
import { get } from 'svelte/store';
import {
  nextTerminalTab,
  requestTerminalTab,
  shellActivity,
  shellTerminalName,
  SHELL_ACTIVITY_POLL_MS,
  terminalOpensOn,
  terminalPane,
  terminalPtyId,
} from './terminals';

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
const settle = async (n = 12) => {
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

/** The terminals tmux has, as the backend would answer. */
let open: number[] = [];
/** What runs in each, as tmux's `pane_current_command` (M15 G4.4). */
let cmds: Record<number, string> = {};
const answer = (opened: number | null = null) => ({
  session_id: 1,
  host_alias: 'alpha',
  terminals: open.map((n) => (cmds[n] ? { n, tmux_name: `api--sh${n}`, command: cmds[n] } : { n, tmux_name: `api--sh${n}` })),
  opened,
});

beforeEach(() => {
  open = [];
  cmds = {};
  inv().mockReset();
  inv().mockImplementation(async (cmd: string, payload?: { args?: { action?: string; n?: number | null } }) => {
    if (cmd === 'pty_drain') return { data: '', bytes: 0 };
    if (cmd === 'shell_terminals') {
      const a = payload?.args ?? {};
      if (a.action === 'open') {
        const n = a.n ?? [1, 2, 3].find((x) => !open.includes(x))!;
        if (!open.includes(n)) open.push(n);
        return answer(n);
      }
      if (a.action === 'close') open = open.filter((x) => x !== a.n);
      return answer();
    }
    return null;
  });
  globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
  resetTombstonesForTests();
  sessions.set([row]);
  clearSelection();
  clearToasts();
});

afterEach(() => {
  clearSelection();
  vi.useRealTimers();
});

describe('shell terminals strip (step 5.3)', () => {
  it('+ New opens a terminal and attaches it under its own pty id; the agent tab goes back', async () => {
    render(TerminalView);
    selectSession(row);
    await settle();
    expect(screen.getByTestId('terminal-strip')).toBeTruthy();
    expect(args(calls('shell_terminals')[0])).toMatchObject({ session_id: 1, action: 'list' });

    await fireEvent.click(screen.getByTestId('terminal-new'));
    await settle();
    expect(screen.getByTestId('terminal-tab-1').getAttribute('aria-selected')).toBe('true');
    const opens = calls('pty_open').map((c) => args(c));
    expect(opens.at(-1)).toMatchObject({ id: 'sh1', session_name: 'api--sh1', host_alias: 'alpha' });
    // The agent's attach was closed by its own id, never replaced.
    expect(calls('pty_close').map((c) => args(c).id)).toContain('agent');
    expect(screen.getByTestId('terminal-shell-tag').textContent).toContain('Shell 1');

    await fireEvent.click(screen.getByTestId('terminal-tab-agent'));
    await settle();
    expect(calls('pty_open').map((c) => args(c)).at(-1)).toMatchObject({ id: 'agent', session_name: 'api' });
    expect(calls('pty_close').map((c) => args(c).id)).toContain('sh1');
  });

  it('Split shows the agent and the picked shell side by side, each on its own pty', async () => {
    open = [2];
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-tab-2'));
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-split-toggle'));
    await settle(20);
    expect(screen.getByTestId('terminal-split')).toBeTruthy();
    const ids = calls('pty_open').map((c) => args(c).id);
    expect(ids.slice(-2).sort()).toEqual(['agent', 'sh2']);
  });

  it('closing a terminal lets go of it first and never stops the session', async () => {
    open = [1];
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-tab-1'));
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-close-1'));
    await settle();
    expect(args(calls('shell_terminals').at(-1)!)).toMatchObject({ action: 'close', n: 1 });
    expect(screen.queryByTestId('terminal-tab-1')).toBeNull();
    expect(calls('pty_open').map((c) => args(c)).at(-1)).toMatchObject({ id: 'agent' });
    expect(calls('kill_session')).toHaveLength(0);
  });

  it('Kill terminal… in the shell\'s menu asks first, then closes only that terminal', async () => {
    open = [1, 2];
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-tab-2'));
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-menu-toggle'));
    const menu = screen.getByTestId('terminal-menu');
    expect(menu.getAttribute('role')).toBe('menu');
    expect(Array.from(menu.querySelectorAll('[role=menuitem]')).map((m) => m.textContent?.trim())).toEqual([
      'Clear',
      'Split right',
      'Pop out ↗',
      'Kill terminal…the session keeps running',
    ]);
    await fireEvent.click(screen.getByTestId('terminal-kill'));
    // Nothing is closed until the person confirms.
    expect(calls('shell_terminals').filter((c) => args(c).action === 'close')).toHaveLength(0);
    expect(screen.getByTestId('confirm-dialog').textContent).toContain('Kill Shell 2?');
    await fireEvent.click(screen.getByTestId('terminal-kill-confirm'));
    await settle();
    expect(args(calls('shell_terminals').at(-1)!)).toMatchObject({ action: 'close', n: 2 });
    expect(screen.queryByTestId('terminal-tab-2')).toBeNull();
    expect(screen.getByTestId('terminal-tab-1')).toBeTruthy();
    expect(calls('kill_session')).toHaveLength(0);
  });

  it('a right-click on a shell tab opens the same menu, and Cancel kills nothing', async () => {
    open = [1];
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.contextMenu(screen.getByTestId('terminal-tab-1'));
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-kill'));
    await fireEvent.click(screen.getByText('Cancel'));
    await settle();
    expect(calls('shell_terminals').filter((c) => args(c).action === 'close')).toHaveLength(0);
    expect(screen.getByTestId('terminal-tab-1')).toBeTruthy();
  });

  it('"New terminal opens on" names the session\'s host, and + New starts where it says', async () => {
    terminalOpensOn.set('worktree');
    render(TerminalView);
    selectSession(row);
    await settle();
    const pick = screen.getByTestId('terminal-opens-on') as HTMLSelectElement;
    expect(Array.from(pick.options).map((o) => o.textContent)).toEqual(['This worktree · alpha', 'Home folder · alpha']);
    expect(pick.value).toBe('worktree');
    await fireEvent.click(screen.getByTestId('terminal-new'));
    await settle();
    // The default says what it always said: no `at` on the wire.
    // (An open naming its `n` is the pane re-ensuring a terminal it shows.)
    const news = () => calls('shell_terminals').filter((c) => args(c).action === 'open' && args(c).n == null);
    expect(args(news()[0])).not.toHaveProperty('at');

    await fireEvent.change(pick, { target: { value: 'home' } });
    expect(get(terminalOpensOn)).toBe('home');
    await fireEvent.click(screen.getByTestId('terminal-new'));
    await settle();
    expect(news()).toHaveLength(2);
    expect(args(news()[1])).toMatchObject({ action: 'open', at: 'home' });
    terminalOpensOn.set('worktree');
  });

  it('Ctrl+Alt+T opens a terminal and Ctrl+Alt+` walks the tabs, neither reaching the pty', async () => {
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.keyDown(window, { key: 't', ctrlKey: true, altKey: true });
    await settle();
    expect(screen.getByTestId('terminal-tab-1').getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(window, { key: '`', ctrlKey: true, altKey: true });
    await settle();
    expect(screen.getByTestId('terminal-tab-agent').getAttribute('aria-selected')).toBe('true');
    // The older plain Ctrl+` still walks them.
    await fireEvent.keyDown(window, { key: '`', ctrlKey: true });
    await settle();
    expect(screen.getByTestId('terminal-tab-1').getAttribute('aria-selected')).toBe('true');
    expect(calls('pty_write')).toHaveLength(0);
  });
});

describe('the session bar\'s Terminals tab (step 5.3)', () => {
  it('with no terminal open it opens one; Agent goes back; the pane publishes what it shows', async () => {
    render(TerminalView);
    selectSession(row);
    await settle();
    expect(get(terminalPane)).toEqual({ sessionId: 1, shells: [], active: null });

    requestTerminalTab('shells');
    await settle();
    expect(args(calls('shell_terminals').at(-1)!)).toMatchObject({ action: 'open' });
    expect(calls('pty_open').map((c) => args(c)).at(-1)).toMatchObject({ id: 'sh1', session_name: 'api--sh1' });
    expect(get(terminalPane)).toEqual({ sessionId: 1, shells: [1], active: 1 });

    requestTerminalTab('agent');
    await settle();
    expect(calls('pty_open').map((c) => args(c)).at(-1)).toMatchObject({ id: 'agent' });
    expect(get(terminalPane).active).toBeNull();
  });

  it('goes back to the terminal last picked rather than opening another', async () => {
    open = [1, 3];
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-tab-3'));
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-tab-agent'));
    await settle();
    requestTerminalTab('shells');
    await settle();
    expect(screen.getByTestId('terminal-tab-3').getAttribute('aria-selected')).toBe('true');
    // The pane's own re-open before each attach names its terminal; no
    // open asked for a new one.
    const opens = calls('shell_terminals').filter((c) => args(c).action === 'open');
    expect(opens.map((c) => args(c).n)).toEqual(opens.map(() => 3));
  });

});

describe('what runs in each shell (Terminals board, M15 G4.4)', () => {
  it('a shell running something says what; one at its prompt does not; the list is asked again while shown', async () => {
    // Only the interval is faked: the pane's own timeouts keep running.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    open = [1, 2];
    cmds = { 1: 'zsh', 2: 'node' };
    render(TerminalView);
    selectSession(row);
    await settle();
    expect(screen.getByTestId('terminal-running-2').textContent).toContain('node');
    expect(screen.getByTestId('terminal-tab-2').getAttribute('title')).toContain('node running');
    expect(screen.queryByTestId('terminal-running-1')).toBeNull();
    expect(screen.getByTestId('terminal-tab-1').getAttribute('title')).toContain('zsh, at its prompt');

    // `pnpm dev` stops in Shell 2 and starts in Shell 1: no event says so,
    // the next poll does.
    cmds = { 1: 'pnpm', 2: '-zsh' };
    const lists = calls('shell_terminals').length;
    vi.advanceTimersByTime(SHELL_ACTIVITY_POLL_MS);
    await settle();
    expect(calls('shell_terminals').length).toBe(lists + 1);
    expect(args(calls('shell_terminals').at(-1)!)).toMatchObject({ session_id: 1, action: 'list' });
    expect(screen.getByTestId('terminal-running-1').textContent).toContain('pnpm');
    expect(screen.queryByTestId('terminal-running-2')).toBeNull();
    // Asking never typed into a terminal.
    expect(calls('pty_write')).toHaveLength(0);
  });

  it('an older hub that does not say leaves the tabs as they were', async () => {
    open = [1];
    render(TerminalView);
    selectSession(row);
    await settle();
    expect(screen.getByTestId('terminal-tab-1').textContent?.trim()).toBe('Shell 1');
    expect(screen.queryByTestId('terminal-running-1')).toBeNull();
  });

  it('reads a command as running or idle', () => {
    expect(shellActivity('node')).toEqual({ state: 'running', command: 'node' });
    expect(shellActivity('-zsh')).toEqual({ state: 'idle', command: 'zsh' });
    expect(shellActivity('bash')).toEqual({ state: 'idle', command: 'bash' });
    expect(shellActivity('/usr/bin/vim')).toEqual({ state: 'running', command: 'vim' });
    expect(shellActivity('')).toBeNull();
    expect(shellActivity(undefined)).toBeNull();
  });
});

describe('popping back in to a named terminal (M15 G4.4)', () => {
  it('shows the shell the pop-out had, whether or not the list is in yet', async () => {
    open = [1, 3];
    render(TerminalView);
    selectSession(row);
    // Before the list has come back.
    requestTerminalTab('shells', 3);
    await settle();
    expect(screen.getByTestId('terminal-tab-3').getAttribute('aria-selected')).toBe('true');

    await fireEvent.click(screen.getByTestId('terminal-tab-agent'));
    await settle();
    requestTerminalTab('shells', 1);
    await settle();
    expect(screen.getByTestId('terminal-tab-1').getAttribute('aria-selected')).toBe('true');
    // Neither opened a new terminal.
    expect(calls('shell_terminals').filter((c) => args(c).action === 'open' && args(c).n == null)).toHaveLength(0);
  });
});

describe('terminals helpers', () => {
  it('names terminals and their ptys like the backend', () => {
    expect(shellTerminalName('api', 3)).toBe('api--sh3');
    expect(terminalPtyId(null)).toBe('agent');
    expect(terminalPtyId(4)).toBe('sh4');
  });

  it('next tab goes agent, 1, 2, … and wraps', () => {
    expect(nextTerminalTab(null, [2, 1])).toBe(1);
    expect(nextTerminalTab(1, [1, 2])).toBe(2);
    expect(nextTerminalTab(2, [1, 2])).toBeNull();
    expect(nextTerminalTab(null, [])).toBeNull();
  });
});

describe('pop out (step 5.4)', () => {
  it('Pop out opens a window for the picked tab, and the tab stays here', async () => {
    open = [1];
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-tab-1'));
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-popout'));
    await settle();
    expect(args(calls('open_terminal_window')[0])).toEqual({ session_id: 1, shell: 1, title: 'api · Shell 1' });
    expect(screen.getByTestId('terminal-tab-1').getAttribute('aria-selected')).toBe('true');

    await fireEvent.click(screen.getByTestId('terminal-tab-agent'));
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-popout'));
    await settle();
    expect(args(calls('open_terminal_window')[1])).toEqual({ session_id: 1, shell: null, title: 'api' });
  });
});

describe('context help on a shell', () => {
  /** Answer `context_help` with `reply`; every other command as before. */
  function helpAnswers(reply: unknown) {
    const base = inv().getMockImplementation() as (cmd: string, payload?: unknown) => unknown;
    inv().mockImplementation(async (cmd: string, payload?: unknown) =>
      cmd === 'context_help' ? reply : base(cmd, payload),
    );
  }
  it('asks about the picked shell with its own history and types the proposal without Enter', async () => {
    open = [1, 2];
    cmds = { 1: 'zsh', 2: 'vim' };
    render(TerminalView);
    selectSession(row);
    await settle();
    expect(screen.queryByTestId('terminal-help')).toBeNull();
    await fireEvent.click(screen.getByTestId('terminal-tab-1'));
    await settle();
    helpAnswers({
      answer: 'No upstream yet.',
      command: 'git push -u origin HEAD',
      model: 'haiku',
      host_alias: 'alpha',
      history_items: 40,
      at: 1,
    });
    await fireEvent.click(screen.getByTestId('terminal-help'));
    await settle();
    const q = screen.getByTestId('terminal-help-panel-question');
    await fireEvent.input(q, { target: { value: 'why?' } });
    await fireEvent.keyDown(q, { key: 'Enter' });
    await settle();
    expect(args(calls('context_help')[0])).toMatchObject({
      surface: 'shell',
      host_alias: 'alpha',
      session_name: 'api',
      terminal: 1,
      question: 'why?',
      model: 'haiku',
    });
    expect(calls('pty_write')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('terminal-help-panel-insert'));
    await settle();
    // Ctrl+U clears what was typed; no newline follows.
    expect(args(calls('pty_write').at(-1)!)).toEqual({ id: 'sh1', data: '\x15git push -u origin HEAD' });
    expect(screen.queryByTestId('terminal-help-panel')).toBeNull();
  });

  it('keeps Insert off while a program runs in the shell', async () => {
    open = [2];
    cmds = { 2: 'vim' };
    render(TerminalView);
    selectSession(row);
    await settle();
    await fireEvent.click(screen.getByTestId('terminal-tab-2'));
    await settle();
    helpAnswers({
      answer: 'Quit with :q',
      command: 'ls',
      model: 'haiku',
      host_alias: 'alpha',
      history_items: 3,
      at: 1,
    });
    await fireEvent.click(screen.getByTestId('terminal-help'));
    await settle();
    await fireEvent.input(screen.getByTestId('terminal-help-panel-question'), { target: { value: 'how do I quit?' } });
    await fireEvent.click(screen.getByTestId('terminal-help-panel-ask'));
    await settle();
    const insert = screen.getByTestId('terminal-help-panel-insert') as HTMLButtonElement;
    expect(insert.disabled).toBe(true);
    expect(insert.title).toContain('vim is running');
  });
});
