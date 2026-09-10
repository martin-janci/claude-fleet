import { findByTestId, render, screen } from '@testing-library/svelte';
import { fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
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
      safe_kill_detail: null, safe_kill_requested_at: null,
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
});
