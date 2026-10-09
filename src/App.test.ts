import { findByTestId, render, screen, waitFor } from '@testing-library/svelte';
import { fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { vi } from 'vitest';
import App from './App.svelte';
import { onboardingDismissed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { get } from 'svelte/store';
import { assetsViewRequest, requestAssetsView } from './lib/app_views';

beforeEach(() => {
  // Suppress the OnboardingCard so tests don't need stubs for its IPC calls
  // (check_local_prereqs, tunnel_status, mcp_status).
  onboardingDismissed.set(true);
  clearToasts();
});

// FE-12: a bootstrap failure (e.g. broken DB) used to be swallowed, leaving an
// innocent "No projects yet". It must surface with its code.
describe('App bootstrap failure', () => {
  it('shows a sticky error toast with the IpcError code and a footer banner', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const inv = invoke as ReturnType<typeof vi.fn>;
    const original = inv.getMockImplementation() as
      | ((cmd: string, ...rest: unknown[]) => Promise<unknown>)
      | undefined;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'list_sessions') throw { code: 'E_DB', message: 'database is locked' };
      return original ? original(cmd, ...rest) : null;
    });
    // The user had a session open last time. A failed list_sessions must not
    // be mistaken for "that session is gone" — the pref has to survive.
    const remembered = JSON.stringify({ host_alias: 'mefistos', tmux_name: 'dev-foo' });
    localStorage.setItem('cf:pref:session.last', remembered);
    try {
      const { container } = render(App);
      const toast = await findByTestId(container.ownerDocument.body, 'toast');
      expect(toast.getAttribute('data-kind')).toBe('error');
      expect(toast.textContent).toContain('E_DB');
      expect(toast.textContent).toContain('database is locked');
      const banner = await findByTestId(container, 'bootstrap-error');
      expect(banner.textContent).toContain('sessions: E_DB');
      // The store stays empty; the UI must not pretend there are simply no sessions.
      expect(screen.getByTestId('toasts')).toBeInTheDocument();
      expect(localStorage.getItem('cf:pref:session.last')).toBe(remembered);
    } finally {
      inv.mockImplementation(original!);
      localStorage.removeItem('cf:pref:session.last');
    }
  });

  it('restores the remembered session when the sessions bootstrap succeeds', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const inv = invoke as ReturnType<typeof vi.fn>;
    const original = inv.getMockImplementation() as
      | ((cmd: string, ...rest: unknown[]) => Promise<unknown>)
      | undefined;
    const row = {
      id: 5, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: null, worktree_id: null,
      created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
      kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null,
      claude_session_id: null, claude_status: null, effort_level: null, pr_url: null,
      current_activity: null, friendly_name: null, safe_kill_state: null, safe_kill_nonce: null,
      safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
    };
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'list_sessions') return [row];
      return original ? original(cmd, ...rest) : null;
    });
    localStorage.setItem('cf:pref:session.last', JSON.stringify({ host_alias: 'mefistos', tmux_name: 'dev-foo' }));
    try {
      render(App);
      const { selectedSession } = await import('./lib/selection');
      const { get } = await import('svelte/store');
      for (let i = 0; i < 10 && get(selectedSession) === null; i++) await new Promise((r) => setTimeout(r, 5));
      expect(get(selectedSession)?.id).toBe(5);
    } finally {
      inv.mockImplementation(original!);
      localStorage.removeItem('cf:pref:session.last');
      const { clearSelection } = await import('./lib/selection');
      clearSelection();
    }
  });
});

describe('App startup order', () => {
  it('subscribes to row events before the first list resolves', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const { listen } = await import('@tauri-apps/api/event');
    const inv = invoke as ReturnType<typeof vi.fn>;
    const lis = listen as ReturnType<typeof vi.fn>;
    const original = inv.getMockImplementation() as
      | ((cmd: string, ...rest: unknown[]) => Promise<unknown>)
      | undefined;
    const order: string[] = [];
    lis.mockImplementation(async (name: string) => {
      order.push(`listen:${name}`);
      return () => {};
    });
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'list_sessions') {
        order.push('list_sessions');
        return [];
      }
      return original ? original(cmd, ...rest) : null;
    });
    try {
      render(App);
      await waitFor(() => expect(order).toContain('list_sessions'));
      const firstListen = order.findIndex((o) => o === 'listen:session:updated');
      const list = order.indexOf('list_sessions');
      expect(firstListen).toBeGreaterThanOrEqual(0);
      expect(firstListen).toBeLessThan(list);
    } finally {
      inv.mockImplementation(original!);
      lis.mockImplementation(async () => () => {});
    }
  });
});

describe('App layout', () => {
  it('renders the sidebar and terminal panes, and no center pane (13.1)', () => {
    const { getByTestId, queryByTestId } = render(App);
    expect(getByTestId('pane-sidebar')).toBeInTheDocument();
    expect(getByTestId('pane-terminal')).toBeInTheDocument();
    expect(queryByTestId('pane-center')).toBeNull();
  });

  it('contains both panes inside the layout container', () => {
    const { container } = render(App);
    const layout = container.querySelector('.layout') as HTMLElement;
    expect(layout).not.toBeNull();
    const panes = layout.querySelectorAll('[data-testid^="pane-"]');
    expect(panes).toHaveLength(2);
  });

  it('mounts the sidebar tree inside the sidebar pane', async () => {
    const { container } = render(App);
    const sidebarTree = await findByTestId(container, 'sidebar-tree');
    expect(sidebarTree).toBeInTheDocument();
  });

  it('refreshes projects and sessions when the window regains focus', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    render(App);
    const before = (invoke as ReturnType<typeof vi.fn>).mock.calls.length;
    await fireEvent(window, new FocusEvent('focus'));
    const after = (invoke as ReturnType<typeof vi.fn>).mock.calls.length;
    expect(after).toBeGreaterThan(before);
    const cmds = (invoke as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0]);
    expect(cmds).toEqual(expect.arrayContaining(['list_projects', 'list_sessions']));
  });

  // Transfer fix wave I1(d): a wait's end missed while the window was away is
  // found again from the session's timeline when it regains focus.
  it('re-checks a waiting transfer against its timeline when the window regains focus', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const { putRunForTest, resetMovesForTest } = await import('./lib/moves');
    const { MOVE_STEPS } = await import('./lib/moveProgress');
    putRunForTest({
      sessionId: 77, sessionName: 's', fromHost: 'alpha', toHost: 'beta', keepSource: null,
      origin: 'local', steps: MOVE_STEPS.map((step) => ({ step, state: 'pending' as const, detail: null })),
      status: 'waiting', report: null, error: null, resolveError: null, startedAt: 1, settledAt: null,
      cleanTarget: false, forceCrossOrg: false, attempt: 1, resolving: false, awaitingStart: false,
      deadlineUnix: 2_000_000_000, waitEnded: null, waitRefusal: null,
    });
    try {
      render(App);
      await fireEvent(window, new FocusEvent('focus'));
      const calls = (invoke as ReturnType<typeof vi.fn>).mock.calls;
      expect(calls).toContainEqual(['session_history', { args: { session_id: 77, limit: null } }]);
    } finally {
      resetMovesForTest();
    }
  });

  it('marks only Assets active (no session tab) when Assets is open', async () => {
    const { getByTestId } = render(App);
    await fireEvent.click(getByTestId('stab-assets'));
    expect(getByTestId('stab-assets').getAttribute('aria-pressed')).toBe('true');
    for (const t of ['conversation', 'agent', 'files', 'details']) {
      expect(getByTestId(`stab-${t}`).getAttribute('aria-selected')).toBe('false');
    }
  });

  it('a quick-switcher request opens the Assets overlay, and the panel takes the request', async () => {
    const { getByTestId, queryByTestId } = render(App);
    expect(queryByTestId('assets-overlay')).toBeNull();
    requestAssetsView({ select: 'asset:personal:skill/w' });
    await waitFor(() => expect(queryByTestId('assets-overlay')).not.toBeNull());
    expect(getByTestId('stab-assets').getAttribute('aria-pressed')).toBe('true');
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
  });

  it('swallows a drop outside a drop target so the webview cannot navigate', async () => {
    render(App);
    await tick();
    const ev = new Event('drop', { bubbles: true, cancelable: true });
    window.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);

    const over = new Event('dragover', { bubbles: true, cancelable: true });
    window.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
  });
});

// Spec §6: the Conversation tab. Reuses the Files-overlay mechanism for tmux
// rows; bg / external rows (no pane) open it by default and have Terminal and
// Files disabled.
describe('App: the Conversation tab', () => {
  const base = {
    project_id: null, worktree_id: null, created_at: 1, last_activity_at: 1, status: 'running',
    notes: null, account_uuid: null, reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_status: 'idle', effort_level: null, pr_url: null, current_activity: null,
    friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null,
    safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null,
    stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null,
    last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null,
    tags: [],
  };
  const work = { ...base, id: 101, tmux_name: 'dev-work', host_alias: 'local', kind: 'work', claude_session_id: 'c-work' };
  const noId = { ...base, id: 102, tmux_name: 'dev-noid', host_alias: 'local', kind: 'work', claude_session_id: null };
  const bg = { ...base, id: 103, tmux_name: 'bg:c-bg', host_alias: 'local', kind: 'bg', claude_session_id: 'c-bg' };
  const ext = { ...base, id: 104, tmux_name: 'bg:c-ext', host_alias: 'local', kind: 'external', claude_session_id: 'c-ext' };

  let inv: ReturnType<typeof vi.fn>;
  let original: ((cmd: string, ...rest: unknown[]) => Promise<unknown>) | undefined;

  beforeEach(async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    inv = invoke as ReturnType<typeof vi.fn>;
    original = inv.getMockImplementation() as typeof original;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'list_sessions') return [work, noId, bg, ext];
      if (cmd === 'session_conversation') return { turns: [], truncated: false };
      if (cmd === 'repo_changes') return [];
      if (cmd === 'repo_tree') return { entries: [], truncated: false };
      return original ? original(cmd, ...rest) : null;
    });
    localStorage.removeItem('cf:pref:session.last');
    const { sessionView } = await import('./lib/prefs');
    sessionView.set('conversation');
  });

  afterEach(async () => {
    inv.mockImplementation(original!);
    const { clearSelection } = await import('./lib/selection');
    clearSelection();
  });

  async function mountAndSelect(row: { id: number }) {
    render(App);
    const { sessions } = await import('./lib/sessions');
    const { get } = await import('svelte/store');
    await waitFor(() => expect(get(sessions).length).toBe(4));
    const { selectSession } = await import('./lib/selection');
    selectSession(get(sessions).find((s) => s.id === row.id)!);
    await tick();
  }

  async function select(row: { id: number }) {
    const { sessions } = await import('./lib/sessions');
    const { get } = await import('svelte/store');
    const { selectSession } = await import('./lib/selection');
    selectSession(get(sessions).find((s) => s.id === row.id)!);
    await tick();
  }

  const tab = (id: string) => screen.getByTestId(id) as HTMLButtonElement;
  const selected = (id: string) => tab(id).getAttribute('aria-selected');
  /** The session itself shows: its Conversation or its Agent (terminal) tab. */
  const sessionTab = () =>
    selected('stab-conversation') === 'true' || selected('stab-agent') === 'true' ? 'true' : 'false';
  const hostsOpen = () => (screen.queryByTestId('hosts-overlay') ? 'true' : 'false');
  // jsdom's userAgent isn't macOS: Hosts is Ctrl+Shift+H.
  const toggleHosts = async () => {
    await fireEvent.keyDown(window, { key: 'H', ctrlKey: true, shiftKey: true });
    await tick();
  };
  const NO_PANE_TITLE_TEXT = 'Runs outside tmux — no terminal';

  it('the Conversation sub-view is disabled without a claude_session_id and enabled with one', async () => {
    await mountAndSelect(noId);
    expect(tab('stab-conversation').disabled).toBe(true);
    expect(tab('stab-conversation').title).toBe('No Claude session id yet');
    // With no transcript the row falls back to the terminal, without
    // touching the stored preference.
    expect(selected('stab-agent')).toBe('true');
    await select(work);
    expect(tab('stab-conversation').disabled).toBe(false);
  });

  it('defaults to Conversation, and the segment flips between the two views', async () => {
    await mountAndSelect(work);
    const grid = await screen.findByTestId('terminal-host');
    // Default pref is 'conversation'.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(selected('stab-conversation')).toBe('true');
    expect(sessionTab()).toBe('true');
    // The PTY stays mounted underneath.
    expect(grid.isConnected).toBe(true);

    await fireEvent.click(tab('stab-agent'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('stab-agent')).toBe('true');
    expect(sessionTab()).toBe('true');

    await fireEvent.click(tab('stab-conversation'));
    await tick();
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
  });

  it('Files and Hosts take the panel from the Session tab and give it back', async () => {
    await mountAndSelect(work);
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();

    await fireEvent.click(tab('stab-files'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('stab-files')).toBe('true');
    expect(sessionTab()).toBe('false');
    // Neither of the session's own views is the current tab while another
    // view owns the panel. (A session *is* selected here, so this is not
    // passing for the trivial reason.)
    expect(selected('stab-conversation')).toBe('false');
    expect(selected('stab-agent')).toBe('false');

    // Esc leaves Files: back to the remembered sub-view, not to the terminal.
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    await tick();
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();

    await toggleHosts();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(hostsOpen()).toBe('true');
    expect(sessionTab()).toBe('false');
  });

  it('a pane-less row shows Conversation without overwriting the stored preference', async () => {
    const { sessionView } = await import('./lib/prefs');
    const { get } = await import('svelte/store');
    await mountAndSelect(work);
    await fireEvent.click(tab('stab-agent'));
    await tick();
    expect(get(sessionView)).toBe('terminal');

    await select(bg);
    await tick();
    // No PTY on this row, so Conversation regardless of the preference.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(tab('stab-agent').disabled).toBe(true);
    expect(tab('stab-agent').title).toBe(NO_PANE_TITLE_TEXT);
    expect(get(sessionView)).toBe('terminal');

    await select(work);
    await tick();
    // Back on a tmux row: the preference survived the detour.
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('stab-agent')).toBe('true');
    // The terminal must actually be remounted and reachable after returning to the tmux row.
    expect(await screen.findByTestId('terminal-host')).toBeInTheDocument();
    expect(tab('stab-agent').disabled).toBe(false);
  });

  it('clicking the already-forced Conversation pill on a pane-less row does not overwrite the stored preference', async () => {
    const { sessionView } = await import('./lib/prefs');
    const { get } = await import('svelte/store');
    sessionView.set('terminal');
    await mountAndSelect(bg);
    // Forced to Conversation (no pane) and already checked; clicking it must
    // be a no-op on the stored preference, not a silent flip back to it.
    expect(selected('stab-conversation')).toBe('true');
    await fireEvent.click(tab('stab-conversation'));
    await tick();
    expect(get(sessionView)).toBe('terminal');
  });

  it('Esc leaves Files even with focus parked on the terminal input proxy (F9)', async () => {
    await mountAndSelect(work);
    const grid = await screen.findByTestId('terminal-host');
    // Focus lands on the terminal's hidden IME textarea, not the grid. App's
    // "is the user typing in a field?" guard must not mistake it for one, or
    // Esc stops leaving the panel that covers the terminal.
    grid.focus();
    expect(grid.contains(document.activeElement)).toBe(true);
    await fireEvent.click(tab('stab-files'));
    await tick();
    expect(selected('stab-files')).toBe('true');
    await fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    await tick();
    expect(selected('stab-files')).toBe('false');
  });

  it.each([
    ['bg', bg],
    ['external', ext],
  ])('a %s row opens Conversation by default with Terminal and Files disabled', async (_k, row) => {
    await mountAndSelect(row);
    expect(await screen.findByTestId('conversation-panel')).toBeInTheDocument();
    expect(sessionTab()).toBe('true');
    expect(selected('stab-conversation')).toBe('true');
    expect(tab('stab-agent').disabled).toBe(true);
    expect(tab('stab-agent').title).toBe(NO_PANE_TITLE_TEXT);
    expect(tab('stab-files').disabled).toBe(true);
    expect(tab('stab-files').title).toBe(NO_PANE_TITLE_TEXT);
    expect(screen.queryByTestId('terminal-host')).toBeNull();
    expect(screen.queryByTestId('bg-panel')).toBeNull();
    expect(inv.mock.calls).toContainEqual(['session_conversation', { args: { session_id: row.id, claude_session_id: row.claude_session_id } }]);
  });

  it('a bg row keeps Conversation as its view across a Hosts round trip', async () => {
    await mountAndSelect(bg);
    expect(await screen.findByTestId('conversation-panel')).toBeInTheDocument();
    await toggleHosts();
    expect(hostsOpen()).toBe('true');
    expect(sessionTab()).toBe('false');
    expect(screen.getByTestId('hosts-overlay')).toBeInTheDocument();
    await toggleHosts();
    expect(screen.queryByTestId('hosts-overlay')).toBeNull();
    expect(sessionTab()).toBe('true');
    expect(selected('stab-conversation')).toBe('true');
  });

  it('selecting a row with no claude_session_id falls back to Terminal', async () => {
    await mountAndSelect(work);
    await fireEvent.click(tab('stab-conversation'));
    await tick();
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    await select(noId);
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('stab-agent')).toBe('true');
  });

  it('the chord returns to the view you left, and only a second press flips it', async () => {
    const { sessionView } = await import('./lib/prefs');
    const { get } = await import('svelte/store');
    await mountAndSelect(work);
    await fireEvent.click(tab('stab-files'));
    await tick();
    expect(selected('stab-files')).toBe('true');

    // jsdom's userAgent isn't macOS, so the chord is Ctrl+Shift+J.
    await fireEvent.keyDown(window, { key: 'J', ctrlKey: true, shiftKey: true });
    await tick();
    // Back to the Session tab, showing the view left behind (Conversation),
    // not the Terminal — and the pref is untouched.
    expect(selected('stab-files')).toBe('false');
    expect(sessionTab()).toBe('true');
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(get(sessionView)).toBe('conversation');

    // Session tab is already active, so this press is the flip.
    await fireEvent.keyDown(window, { key: 'J', ctrlKey: true, shiftKey: true });
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('stab-agent')).toBe('true');
    expect(get(sessionView)).toBe('terminal');
  });
});

// A hub-routed MUTATION that timed out broadcasts `fleet:outcome-unknown`,
// and this listener re-fetches the whole fleet. Two things must bound it:
// a refresh that is still in flight is not started again (otherwise a hub
// that is answering slowly gets one full fleet re-fetch per timed-out call,
// each of which can time out and broadcast again), and a window whose
// configured hub is unusable does not fetch at all.
describe('App: fleet:outcome-unknown', () => {
  it('a second broadcast while the refresh is in flight does not start another list_sessions', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const inv = invoke as ReturnType<typeof vi.fn>;
    const original = inv.getMockImplementation() as
      | ((cmd: string, ...rest: unknown[]) => Promise<unknown>)
      | undefined;
    let release: (() => void) | undefined;
    let gate: Promise<void> | null = null;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'list_sessions' && gate) {
        await gate;
        return [];
      }
      return original ? original(cmd, ...rest) : null;
    });
    try {
      render(App);
      await tick();
      await tick();
      const listCalls = () => inv.mock.calls.filter((c) => c[0] === 'list_sessions').length;
      const before = listCalls();
      gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      window.dispatchEvent(new CustomEvent('fleet:outcome-unknown', { detail: { cmd: 'kill_session' } }));
      window.dispatchEvent(new CustomEvent('fleet:outcome-unknown', { detail: { cmd: 'kill_session' } }));
      await tick();
      expect(listCalls() - before).toBe(1);
      // Once it lands, a later broadcast is served again: the guard bounds
      // the burst, it does not swallow refreshes.
      release!();
      gate = null;
      await new Promise((r) => setTimeout(r, 0));
      const mark = listCalls();
      window.dispatchEvent(new CustomEvent('fleet:outcome-unknown', { detail: { cmd: 'kill_session' } }));
      await tick();
      expect(listCalls() - mark).toBe(1);
    } finally {
      release?.();
      inv.mockImplementation(original!);
    }
  });

  it('does not fetch at all while the configured hub is unusable', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const { hubStatus } = await import('./lib/hub');
    const inv = invoke as ReturnType<typeof vi.fn>;
    render(App);
    await tick();
    await tick();
    const current = (await import('svelte/store')).get(hubStatus);
    hubStatus.set({ ...current, unavailable: 'no stored token' });
    try {
      const listCalls = () => inv.mock.calls.filter((c) => c[0] === 'list_sessions').length;
      const before = listCalls();
      window.dispatchEvent(new CustomEvent('fleet:outcome-unknown', { detail: { cmd: 'kill_session' } }));
      await tick();
      expect(listCalls()).toBe(before);
    } finally {
      hubStatus.set(current);
    }
  });
});
