import { findByTestId, render, screen, waitFor } from '@testing-library/svelte';
import { fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { vi } from 'vitest';
import App from './App.svelte';
import { onboardingDismissed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';

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
  it('renders sidebar, center, and terminal panes', () => {
    const { getByTestId } = render(App);
    expect(getByTestId('pane-sidebar')).toBeInTheDocument();
    expect(getByTestId('pane-center')).toBeInTheDocument();
    expect(getByTestId('pane-terminal')).toBeInTheDocument();
  });

  it('contains all three panes inside the layout container', () => {
    const { container } = render(App);
    const layout = container.querySelector('.layout') as HTMLElement;
    expect(layout).not.toBeNull();
    const panes = layout.querySelectorAll('[data-testid^="pane-"]');
    expect(panes).toHaveLength(3);
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

  it('marks only the Assets tab active (not Session) when Assets is open', async () => {
    const { getByTestId } = render(App);
    await fireEvent.click(getByTestId('tab-assets'));
    expect(getByTestId('tab-session').classList.contains('active')).toBe(false);
    expect(getByTestId('tab-session').getAttribute('aria-selected')).toBe('false');
    expect(getByTestId('tab-assets').classList.contains('active')).toBe(true);
    expect(getByTestId('tab-assets').getAttribute('aria-selected')).toBe('true');
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
  const subtab = (id: string) => screen.getByTestId(id) as HTMLButtonElement;
  const checked = (id: string) => subtab(id).getAttribute('aria-checked');
  const NO_PANE_TITLE_TEXT = 'Runs outside tmux — no terminal';

  it('the Conversation sub-view is disabled without a claude_session_id and enabled with one', async () => {
    await mountAndSelect(noId);
    expect(subtab('subtab-conversation').disabled).toBe(true);
    expect(subtab('subtab-conversation').title).toBe('No Claude session id yet');
    // With no transcript the row falls back to the terminal, without
    // touching the stored preference.
    expect(checked('subtab-terminal')).toBe('true');
    await select(work);
    expect(subtab('subtab-conversation').disabled).toBe(false);
  });

  it('defaults to Conversation, and the segment flips between the two views', async () => {
    await mountAndSelect(work);
    const grid = await screen.findByTestId('terminal-host');
    // Default pref is 'conversation'.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(checked('subtab-conversation')).toBe('true');
    expect(selected('tab-session')).toBe('true');
    // The PTY stays mounted underneath.
    expect(grid.isConnected).toBe(true);
    // The center (Details) pane stays visible, unlike Files/Hosts.
    expect(screen.getByTestId('pane-center')).toBeInTheDocument();

    await fireEvent.click(subtab('subtab-terminal'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(checked('subtab-terminal')).toBe('true');
    expect(selected('tab-session')).toBe('true');

    await fireEvent.click(subtab('subtab-conversation'));
    await tick();
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
  });

  it('Files and Hosts take the panel from the Session tab and give it back', async () => {
    await mountAndSelect(work);
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();

    await fireEvent.click(tab('tab-files'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('tab-files')).toBe('true');
    expect(selected('tab-session')).toBe('false');
    // The segment belongs to the Session tab; it is gone while another view
    // owns the panel. (A session *is* selected here, so this is not passing
    // for the trivial reason.)
    expect(screen.queryByTestId('subtab-conversation')).toBeNull();
    expect(screen.queryByTestId('subtab-terminal')).toBeNull();

    await fireEvent.click(tab('tab-session'));
    await tick();
    // Back to the remembered sub-view, not to the terminal.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();

    await fireEvent.click(tab('tab-hosts'));
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(selected('tab-hosts')).toBe('true');
    expect(selected('tab-session')).toBe('false');
  });

  it('a pane-less row shows Conversation without overwriting the stored preference', async () => {
    const { sessionView } = await import('./lib/prefs');
    const { get } = await import('svelte/store');
    await mountAndSelect(work);
    await fireEvent.click(subtab('subtab-terminal'));
    await tick();
    expect(get(sessionView)).toBe('terminal');

    await select(bg);
    await tick();
    // No PTY on this row, so Conversation regardless of the preference.
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(subtab('subtab-terminal').disabled).toBe(true);
    expect(subtab('subtab-terminal').title).toBe(NO_PANE_TITLE_TEXT);
    expect(get(sessionView)).toBe('terminal');

    await select(work);
    await tick();
    // Back on a tmux row: the preference survived the detour.
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(checked('subtab-terminal')).toBe('true');
    // The terminal must actually be remounted and reachable after returning to the tmux row.
    expect(await screen.findByTestId('terminal-host')).toBeInTheDocument();
    expect(subtab('subtab-terminal').disabled).toBe(false);
  });

  it('clicking the already-forced Conversation pill on a pane-less row does not overwrite the stored preference', async () => {
    const { sessionView } = await import('./lib/prefs');
    const { get } = await import('svelte/store');
    sessionView.set('terminal');
    await mountAndSelect(bg);
    // Forced to Conversation (no pane) and already checked; clicking it must
    // be a no-op on the stored preference, not a silent flip back to it.
    expect(checked('subtab-conversation')).toBe('true');
    await fireEvent.click(subtab('subtab-conversation'));
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
    await fireEvent.click(tab('tab-files'));
    await tick();
    expect(selected('tab-files')).toBe('true');
    await fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    await tick();
    expect(selected('tab-files')).toBe('false');
  });

  it.each([
    ['bg', bg],
    ['external', ext],
  ])('a %s row opens Conversation by default with Terminal and Files disabled', async (_k, row) => {
    await mountAndSelect(row);
    expect(await screen.findByTestId('conversation-panel')).toBeInTheDocument();
    expect(selected('tab-session')).toBe('true');
    expect(checked('subtab-conversation')).toBe('true');
    expect(subtab('subtab-terminal').disabled).toBe(true);
    expect(subtab('subtab-terminal').title).toBe(NO_PANE_TITLE_TEXT);
    expect(tab('tab-files').disabled).toBe(true);
    expect(tab('tab-files').title).toBe(NO_PANE_TITLE_TEXT);
    expect(screen.queryByTestId('terminal-host')).toBeNull();
    expect(screen.queryByTestId('bg-panel')).toBeNull();
    expect(inv.mock.calls).toContainEqual(['session_conversation', { args: { session_id: row.id, claude_session_id: row.claude_session_id } }]);
  });

  it('a bg row keeps Conversation as its view across a Hosts round trip', async () => {
    await mountAndSelect(bg);
    expect(await screen.findByTestId('conversation-panel')).toBeInTheDocument();
    await fireEvent.click(tab('tab-hosts'));
    await tick();
    expect(selected('tab-hosts')).toBe('true');
    expect(selected('tab-session')).toBe('false');
    expect(screen.getByTestId('hosts-overlay')).toBeInTheDocument();
    await fireEvent.click(tab('tab-hosts'));
    await tick();
    expect(screen.queryByTestId('hosts-overlay')).toBeNull();
    expect(selected('tab-session')).toBe('true');
    expect(checked('subtab-conversation')).toBe('true');
  });

  it('selecting a row with no claude_session_id falls back to Terminal', async () => {
    await mountAndSelect(work);
    await fireEvent.click(subtab('subtab-conversation'));
    await tick();
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    await select(noId);
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(checked('subtab-terminal')).toBe('true');
  });

  it('the chord returns to the view you left, and only a second press flips it', async () => {
    const { sessionView } = await import('./lib/prefs');
    const { get } = await import('svelte/store');
    await mountAndSelect(work);
    await fireEvent.click(tab('tab-files'));
    await tick();
    expect(selected('tab-files')).toBe('true');

    // jsdom's userAgent isn't macOS, so the chord is Ctrl+Shift+J.
    await fireEvent.keyDown(window, { key: 'J', ctrlKey: true, shiftKey: true });
    await tick();
    // Back to the Session tab, showing the view left behind (Conversation),
    // not the Terminal — and the pref is untouched.
    expect(selected('tab-files')).toBe('false');
    expect(selected('tab-session')).toBe('true');
    expect(screen.getByTestId('conversation-panel')).toBeInTheDocument();
    expect(get(sessionView)).toBe('conversation');

    // Session tab is already active, so this press is the flip.
    await fireEvent.keyDown(window, { key: 'J', ctrlKey: true, shiftKey: true });
    await tick();
    expect(screen.queryByTestId('conversation-panel')).toBeNull();
    expect(checked('subtab-terminal')).toBe('true');
    expect(get(sessionView)).toBe('terminal');
  });
});
