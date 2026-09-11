import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import QuickSwitcher from './QuickSwitcher.svelte';
import { sessions, type SessionRow } from './sessions';
import { projects } from './projects';
import { selectedSession, clearSelection } from './selection';
import { newSessionRequest, clearNewSessionRequest } from './new_session_request';
import { recentSessions } from './quick_switcher';

function sess(over: Partial<SessionRow> & { id: number }): SessionRow {
  return {
    tmux_name: `dev-o-r--s${over.id}`,
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: over.id,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: null,
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
    safe_kill_requested_at: null,
    context_pct: null,
    stuck_kind: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null,
    ...over,
  };
}

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: 1 },
  worktrees: [{ id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' }],
};

const rows = Array.from({ length: 40 }, (_, i) =>
  sess({ id: i + 1, friendly_name: i === 5 ? 'Blue sirius' : `Session ${i + 1}` }),
);

async function openSwitcher() {
  await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
  await tick();
  return screen.getByTestId('switcher-input') as HTMLInputElement;
}

beforeEach(() => {
  localStorage.clear();
  recentSessions.set([]);
  clearSelection();
  clearNewSessionRequest();
  sessions.set(rows);
  projects.set([project]);
});

describe('QuickSwitcher', () => {
  it('is hidden until Ctrl/Cmd+K or Ctrl/Cmd+P, then toggles', async () => {
    render(QuickSwitcher);
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    await openSwitcher();
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    await fireEvent.keyDown(window, { key: 'p', metaKey: true });
    await tick();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('the chord is taken in the capture phase (before a focused terminal sees it)', async () => {
    render(QuickSwitcher);
    // A "terminal": an element with its own keydown handler. The chord
    // must be consumed by the switcher's capture listener before it
    // reaches this handler; a plain key must still get through.
    const seen = vi.fn();
    const term = document.createElement('div');
    term.addEventListener('keydown', seen);
    document.body.appendChild(term);
    await fireEvent.keyDown(term, { key: 'k', ctrlKey: true });
    expect(seen).not.toHaveBeenCalled();
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    await fireEvent.keyDown(term, { key: 'k' });
    expect(seen).toHaveBeenCalledOnce();
    term.remove();
  });

  it('lists every session plus a project row, first row active', async () => {
    render(QuickSwitcher);
    await openSwitcher();
    expect(screen.getAllByTestId('switcher-session')).toHaveLength(40);
    expect(screen.getAllByTestId('switcher-project')).toHaveLength(1);
    const active = document.querySelector('.row.active');
    expect(active?.getAttribute('data-key')).toBe('session:40'); // most recent activity
  });

  it('fuzzy query narrows and ranks; Enter selects (attaches) the top row', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await fireEvent.input(input, { target: { value: 'blue sir' } });
    await tick();
    expect(screen.getAllByTestId('switcher-session')).toHaveLength(1);
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(get(selectedSession)?.id).toBe(6);
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    // The pick is remembered as most recent.
    expect(get(recentSessions)[0]).toBe('local/dev-o-r--s6');
  });

  it('recent sessions come first with an empty query', async () => {
    recentSessions.set(['local/dev-o-r--s3', 'local/dev-o-r--s9']);
    render(QuickSwitcher);
    await openSwitcher();
    const keys = screen.getAllByTestId('switcher-session').map((el) => el.getAttribute('data-key'));
    expect(keys.slice(0, 2)).toEqual(['session:3', 'session:9']);
  });

  it('arrow keys move the highlight, wrap, and scroll the row into view', async () => {
    const scrolled: string[] = [];
    Element.prototype.scrollIntoView = function () {
      scrolled.push((this as HTMLElement).getAttribute('data-key') ?? '');
    };
    render(QuickSwitcher);
    const input = await openSwitcher();
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    await tick();
    expect(document.querySelector('.row.active')?.getAttribute('data-key')).toBe('session:38');
    await fireEvent.keyDown(input, { key: 'ArrowUp' });
    await fireEvent.keyDown(input, { key: 'ArrowUp' });
    await fireEvent.keyDown(input, { key: 'ArrowUp' });
    await tick();
    // Wrapped past the top to the last row (the project row).
    expect(document.querySelector('.row.active')?.getAttribute('data-key')).toBe('project:1');
    await tick();
    await Promise.resolve();
    expect(scrolled).toContain('session:38');
    expect(scrolled[scrolled.length - 1]).toBe('project:1');
    // @ts-expect-error restore jsdom default (undefined)
    delete Element.prototype.scrollIntoView;
  });

  it('Ctrl/Cmd+Enter requests a new session named after the query', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await fireEvent.input(input, { target: { value: 'red comet' } });
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    await tick();
    const req = get(newSessionRequest);
    expect(req?.project.project.id).toBe(1);
    expect(req?.initialName).toBe('red comet');
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('Enter on a project row opens the dialog for that project', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await fireEvent.input(input, { target: { value: 'new session claude' } });
    await tick();
    const first = document.querySelector('.row.active');
    expect(first?.getAttribute('data-key')).toBe('project:1');
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(get(newSessionRequest)?.project.project.id).toBe(1);
    expect(get(newSessionRequest)?.initialName).toBeUndefined();
  });

  it('clicking a row selects it', async () => {
    render(QuickSwitcher);
    await openSwitcher();
    const row = screen.getAllByTestId('switcher-session').find((el) => el.getAttribute('data-key') === 'session:2')!;
    await fireEvent.click(row);
    expect(get(selectedSession)?.id).toBe(2);
  });
});
