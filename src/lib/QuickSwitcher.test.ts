import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import { ownNewTaskChord } from './new_task';
import QuickSwitcher from './QuickSwitcher.svelte';
import { sessions, type SessionRow } from './sessions';
import { projects } from './projects';
import { selectedSession, clearSelection } from './selection';
import { newSessionRequest, clearNewSessionRequest } from './new_session_request';
import { recentSessions } from './quick_switcher';
import { assetsViewRequest } from './app_views';
import { catalog } from './assets';

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
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  };
}

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: 1, adopted: false, system: false },
  worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' }],
};

const rows = Array.from({ length: 40 }, (_, i) =>
  sess({ id: i + 1, friendly_name: i === 5 ? 'Blue sirius' : `Session ${i + 1}` }),
);

// jsdom is not a Mac (detectMac → false), so the default chord is Ctrl+Shift+K.
const LINUX_CHORD = { key: 'K', ctrlKey: true, shiftKey: true };

async function openSwitcher() {
  await fireEvent.keyDown(window, LINUX_CHORD);
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
  it('is hidden until Ctrl+Shift+K or Ctrl+Shift+P (Linux), then toggles', async () => {
    render(QuickSwitcher);
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    await openSwitcher();
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    await fireEvent.keyDown(window, { key: 'P', ctrlKey: true, shiftKey: true });
    await tick();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  // A "terminal": an element with its own keydown handler, like TerminalView.
  function fakeTerminal() {
    const seen = vi.fn();
    const term = document.createElement('div');
    term.tabIndex = 0;
    term.addEventListener('keydown', seen);
    document.body.appendChild(term);
    return { term, seen };
  }

  it('Linux: plain Ctrl+K / Ctrl+P reach the terminal untouched; Ctrl+Shift+K is captured first', async () => {
    render(QuickSwitcher);
    const { term, seen } = fakeTerminal();
    // readline kill-line / previous-history must not be eaten.
    expect(await fireEvent.keyDown(term, { key: 'k', ctrlKey: true })).toBe(true); // not defaultPrevented
    expect(await fireEvent.keyDown(term, { key: 'p', ctrlKey: true })).toBe(true);
    expect(seen).toHaveBeenCalledTimes(2);
    await tick();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    // The real chord is consumed in the capture phase — the terminal never sees it.
    await fireEvent.keyDown(term, LINUX_CHORD);
    await tick();
    expect(seen).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    term.remove();
  });

  it('macOS: Cmd+K opens, Ctrl+K still reaches the terminal', async () => {
    render(QuickSwitcher, { props: { isMac: true } });
    const { term, seen } = fakeTerminal();
    expect(await fireEvent.keyDown(term, { key: 'k', ctrlKey: true })).toBe(true);
    expect(await fireEvent.keyDown(term, LINUX_CHORD)).toBe(true);
    expect(seen).toHaveBeenCalledTimes(2);
    await tick();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    await fireEvent.keyDown(term, { key: 'k', metaKey: true });
    await tick();
    expect(seen).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    term.remove();
  });

  it('the chord is ignored while another modal is open', async () => {
    render(QuickSwitcher);
    const other = document.createElement('dialog');
    other.setAttribute('open', '');
    const field = document.createElement('input');
    other.appendChild(field);
    document.body.appendChild(other);
    field.focus();
    await fireEvent.keyDown(field, LINUX_CHORD);
    await tick();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    other.remove();
  });

  it('Escape closes the switcher and restores focus to where the user was', async () => {
    render(QuickSwitcher);
    const { term } = fakeTerminal();
    term.focus();
    expect(document.activeElement).toBe(term);
    await fireEvent.keyDown(term, LINUX_CHORD);
    await tick();
    const dialog = screen.getByTestId('quick-switcher');
    expect(document.activeElement).toBe(screen.getByTestId('switcher-input'));
    // Escape on a <dialog> surfaces as its `cancel` event.
    dialog.dispatchEvent(new Event('cancel', { cancelable: true }));
    await tick();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    expect(document.activeElement).toBe(term);
    term.remove();
  });

  it('the input is a combobox whose active descendant is the highlighted option', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    expect(input.getAttribute('role')).toBe('combobox');
    const listId = input.getAttribute('aria-controls')!;
    expect(document.getElementById(listId)?.getAttribute('role')).toBe('listbox');
    const active = () => document.querySelector('.row.active') as HTMLElement;
    expect(input.getAttribute('aria-activedescendant')).toBe(active().id);
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    await tick();
    expect(active().id).toBeTruthy();
    expect(input.getAttribute('aria-activedescendant')).toBe(active().id);
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
    // Wrapped past the top to the last row (the last command row; commands rank after projects).
    expect(document.querySelector('.row.active')?.getAttribute('data-key')).toBe('command:propose');
    await tick();
    await Promise.resolve();
    expect(scrolled).toContain('session:38');
    expect(scrolled[scrolled.length - 1]).toBe('command:propose');
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

// ---- tickets (work graph M3) -------------------------------------------------

import { invoke as __invoke } from '@tauri-apps/api/core';
import { trackers as __trackers } from './trackers';

describe('QuickSwitcher tickets', () => {
  const ticket = (key: string, over: Record<string, unknown> = {}) => ({
    id: 100 + Number(key.split('-')[1]),
    tracker_id: 1,
    source: 'jira',
    key,
    title: `${key} title`,
    status_category: 'todo',
    status_name: 'To Do',
    created_at: 1,
    updated_at: 1,
    ...over,
  });
  let calls: [string, unknown][] = [];
  beforeEach(() => {
    calls = [];
    __trackers.set([
      { id: 1, provider: 'jira', name: 'acme', site_url: 'https://acme.atlassian.net', state: 'ok', created_at: 1, config: { key_prefixes: ['ABC'] } },
    ]);
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      calls.push([cmd, args]);
      const view = (args as { args?: { view?: string } } | undefined)?.args?.view;
      if (cmd === 'work_tickets' && view === 'mine') return [ticket('ABC-1'), ticket('ABC-2', { live_session_ids: [6] })];
      if (cmd === 'work_tickets') return [];
      if (cmd === 'start_work') return { ...rows[0], id: 999 };
      return null;
    });
  });

  it('lists My work under its heading, below sessions; Enter jumps to a live one', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await vi.waitFor(() => expect(screen.getAllByTestId('switcher-ticket')).toHaveLength(2));
    expect(screen.getByText('My work')).toBeTruthy();
    await fireEvent.input(input, { target: { value: 'ABC-2' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(get(selectedSession)?.id).toBe(6);
  });

  it('Enter on a ticket with no session opens the dialog prefilled with the ticket', async () => {
    // Asked for a proposal first (3.12); a hub without the preview proposes nothing.
    const base = vi.mocked(__invoke).getMockImplementation()!;
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'preview_start_work') throw { code: 'E_INVALID', message: 'unknown work_link action preview_start' };
      return base(cmd, args as never);
    });
    render(QuickSwitcher);
    const input = await openSwitcher();
    await vi.waitFor(() => expect(screen.getAllByTestId('switcher-ticket')).toHaveLength(2));
    await fireEvent.input(input, { target: { value: 'abc-1' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await vi.waitFor(() => expect(get(newSessionRequest)).not.toBeNull());
    const req = get(newSessionRequest);
    expect(req?.initialName).toBe('ABC-1 ABC-1 title');
    expect(req?.ticket?.key).toBe('ABC-1');
  });

  it('Ctrl+Enter on a ticket starts it with the defaults, without the dialog', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await vi.waitFor(() => expect(screen.getAllByTestId('switcher-ticket')).toHaveLength(2));
    await fireEvent.input(input, { target: { value: 'abc-1' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    await vi.waitFor(() => expect(calls.some(([c]) => c === 'start_work')).toBe(true));
    const [, args] = calls.find(([c]) => c === 'start_work')!;
    expect((args as { args: unknown }).args).toEqual({ item_id: 101, with_brief: true });
    expect(get(newSessionRequest)).toBeNull();
    // The started session is opened and the switcher is gone.
    await vi.waitFor(() => expect(get(selectedSession)?.id).toBe(999));
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('Ctrl+Enter on a ticket that already has a live session jumps to it (E_EXISTS)', async () => {
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      calls.push([cmd, args]);
      const view = (args as { args?: { view?: string } } | undefined)?.args?.view;
      if (cmd === 'work_tickets' && view === 'mine') return [ticket('ABC-1')];
      if (cmd === 'work_tickets') return [];
      if (cmd === 'start_work')
        throw { code: 'E_EXISTS', message: 'ABC-1 already has a live session', details: { session_id: 7 } };
      return null;
    });
    render(QuickSwitcher);
    const input = await openSwitcher();
    await vi.waitFor(() => expect(screen.getAllByTestId('switcher-ticket')).toHaveLength(1));
    await fireEvent.input(input, { target: { value: 'abc-1' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    await vi.waitFor(() => expect(get(selectedSession)?.id).toBe(7));
    expect(get(newSessionRequest)).toBeNull();
  });

  it('a lost start race still jumps, and names the session it left unlinked', async () => {
    const { toasts, clearToasts } = await import('./toasts');
    clearToasts();
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      calls.push([cmd, args]);
      const view = (args as { args?: { view?: string } } | undefined)?.args?.view;
      if (cmd === 'work_tickets' && view === 'mine') return [ticket('ABC-1')];
      if (cmd === 'work_tickets') return [];
      if (cmd === 'start_work')
        throw {
          code: 'E_EXISTS',
          message: 'ABC-1 already has a live session (w on box); the session this start made (dev-abc-1) is not linked to it',
          details: { session_id: 7, orphan_session_id: 9 },
        };
      return null;
    });
    render(QuickSwitcher);
    const input = await openSwitcher();
    await vi.waitFor(() => expect(screen.getAllByTestId('switcher-ticket')).toHaveLength(1));
    await fireEvent.input(input, { target: { value: 'abc-1' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    await vi.waitFor(() => expect(get(selectedSession)?.id).toBe(7));
    const shown = get(toasts);
    expect(shown).toHaveLength(1);
    expect(shown[0].message).toContain('dev-abc-1');
    clearToasts();
  });

  it('a pasted ticket URL offers a lookup row', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await fireEvent.input(input, { target: { value: 'https://acme.atlassian.net/browse/ABC-77' } });
    await tick();
    expect(screen.getByTestId('switcher-lookup')).toBeTruthy();
  });
});

// Redesign 3.12 (K1): the project a ticket start lands in, proposed by a
// rule (earlier work on the key's family) or by Jev, with the shared chip.
describe('QuickSwitcher ticket starts propose a project (New layout)', () => {
  const other = {
    project: { id: 2, owner: 'acme', repo: 'papaya-pos', base_path: '/r/pp', last_session_at: 1, adopted: false, system: false },
    worktrees: [],
  };
  const ticket = { id: 101, tracker_id: 1, source: 'jira', key: 'ABC-1', title: 'ABC-1 title', status_category: 'todo', status_name: 'To Do', created_at: 1, updated_at: 1 };
  let suggested: { project_id: number; confidence_pct?: number } | null;
  let calls: [string, unknown][];
  beforeEach(async () => {
    projects.set([project, other]);
    suggested = { project_id: 2, confidence_pct: 88 };
    calls = [];
    __trackers.set([
      { id: 1, provider: 'jira', name: 'acme', site_url: 'https://acme.atlassian.net', state: 'ok', created_at: 1, config: { key_prefixes: ['ABC'] } },
    ]);
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      calls.push([cmd, args]);
      const view = (args as { args?: { view?: string } } | undefined)?.args?.view;
      if (cmd === 'work_tickets' && view === 'mine') return [ticket];
      if (cmd === 'work_tickets') return [];
      if (cmd === 'preview_start_work')
        return {
          key: 'ABC-1', title: 'ABC-1 title', item_id: 101, plan: null, missing: 'project',
          projects: [{ id: 1, owner: 'martin-janci', repo: 'claude-fleet' }, { id: 2, owner: 'acme', repo: 'papaya-pos' }],
          hosts: [], conflicts: [], suggested_project: suggested,
        };
      if (cmd === 'start_work') throw { code: 'E_AMBIGUOUS', message: 'pick a project' };
      return null;
    });
  });
  afterEach(async () => {
  });

  async function enterOnTicket(mod = false) {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await vi.waitFor(() => expect(screen.getAllByTestId('switcher-ticket')).toHaveLength(1));
    await fireEvent.input(input, { target: { value: 'abc-1' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: mod });
    await vi.waitFor(() => expect(get(newSessionRequest)).not.toBeNull());
    return get(newSessionRequest)!;
  }

  it('Jev above the floor picks the project and the dialog gets the chip', async () => {
    const req = await enterOnTicket();
    expect(req.project.project.id).toBe(2);
    expect(req.proposal).toEqual({ value: '2', source: 'jev', confidence_pct: 88 });
    expect(req.ticket?.key).toBe('ABC-1');
  });

  it('⌘↵ that needs a project lands on the same proposal', async () => {
    const req = await enterOnTicket(true);
    expect(calls.some(([c]) => c === 'start_work')).toBe(true);
    expect(req.project.project.id).toBe(2);
    expect(req.proposal?.source).toBe('jev');
  });

  it('unsure or below the floor proposes nothing: the context project, no chip', async () => {
    suggested = { project_id: 2, confidence_pct: 40 };
    const req = await enterOnTicket();
    expect(req.project.project.id).toBe(1);
    expect(req.proposal).toBeNull();
  });

  it('a rule beats Jev: earlier work on the family picks the project, no Jev call', async () => {
    sessions.set([sess({ id: 77, project_id: 2, host_alias: 'mefistos', work: { key: 'ABC-9' } } as Partial<SessionRow> & { id: number })]);
    const req = await enterOnTicket();
    expect(req.project.project.id).toBe(2);
    expect(req.initialHost).toBe('mefistos');
    expect(req.proposal).toMatchObject({ value: '2', source: 'rule', reason: 'ABC work runs here' });
    expect(calls.some(([c]) => c === 'preview_start_work')).toBe(false);
  });

  it("the dialog's Change re-opens the picker for the ticket; the pick keeps it", async () => {
    render(QuickSwitcher);
    const { openNewSessionPicker } = await import('./switcher_request');
    openNewSessionPicker(undefined, ticket as never);
    await tick();
    const input = screen.getByTestId('switcher-input') as HTMLInputElement;
    expect(input.placeholder).toBe('Repository for ABC-1…');
    await fireEvent.input(input, { target: { value: 'papaya' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    const req = get(newSessionRequest)!;
    expect(req.project.project.id).toBe(2);
    expect(req.ticket?.key).toBe('ABC-1');
    expect(req.initialName).toBe('ABC-1 ABC-1 title');
  });
});

describe('QuickSwitcher assets and commands', () => {
  const listing = {
    head: null, loaded_at: null, unmanaged: [], problems: [],
    assets: [
      { kind: 'skill', name: 'zebra-wrangler', version: '1', description: 'Tame the stripes', tags: [], hosts: [], catalog: 'acme' },
    ],
  } as never;
  beforeEach(() => {
    assetsViewRequest.set(null);
    catalog.set(listing);
  });
  afterEach(() => catalog.set(null));

  it('typing an asset name lists it under Assets, and picking it requests the Assets view with it selected', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await fireEvent.input(input, { target: { value: 'zebra' } });
    await tick();
    const row = screen.getByTestId('switcher-asset');
    expect(row.getAttribute('data-key')).toBe('asset:acme:skill/zebra-wrangler');
    expect(screen.getByText('Assets', { selector: '.group, .group *' })).toBeTruthy();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(get(assetsViewRequest)).toMatchObject({ select: 'asset:acme:skill/zebra-wrangler' });
    expect(get(assetsViewRequest)?.command).toBeUndefined();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('picking Sync fleet requests the command', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    await fireEvent.input(input, { target: { value: 'sync' } });
    await tick();
    expect(screen.getByText('Commands', { selector: '.group, .group *' })).toBeTruthy();
    await fireEvent.click(screen.getByTestId('switcher-command'));
    await tick();
    expect(get(assetsViewRequest)).toMatchObject({ command: 'sync' });
    expect(get(assetsViewRequest)?.select).toBeUndefined();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('the three commands are listed with an empty query, below sessions and projects', async () => {
    render(QuickSwitcher);
    await openSwitcher();
    expect(screen.getAllByTestId('switcher-command').map((e) => e.getAttribute('data-key'))).toEqual([
      'command:rescan', 'command:sync', 'command:propose',
    ]);
    const kinds = Array.from(document.querySelectorAll('[data-testid^="switcher-"][data-key]')).map((e) => e.getAttribute('data-key')!.split(':')[0]);
    expect(kinds.lastIndexOf('session')).toBeLessThan(kinds.indexOf('asset'));
    expect(kinds.lastIndexOf('asset')).toBeLessThan(kinds.indexOf('command'));
  });
});

// ---- New session mode (project picker spec v2) -------------------------------

import { switcherRequest, openNewSessionPicker } from './switcher_request';
import { projectPicks } from './project_picks';
import { newSessionHostRequest, addProjectRequest } from './app_views';
import { toasts } from './toasts';
import { readFrecency } from './frecency';

describe('QuickSwitcher — New session mode', () => {
  const NOW = Math.floor(Date.now() / 1000);
  const p = (id: number, owner: string, repo: string, ago: number | null) => ({
    project: { id, owner, repo, base_path: `/r/${repo}`, last_session_at: ago === null ? null : NOW - ago, adopted: false, system: false },
    worktrees: [],
  });
  const fleet = p(1, 'me', 'claude-fleet', 60);
  const om = [p(2, 'pp', 'openmarket-ai', 3600), p(3, 'pp', 'openmarket-docs', 7200), p(4, 'pp', 'openmarket-app', null)];
  const epic = p(5, 'me', 'ppt-epic-145', null);

  beforeEach(() => {
    projects.set([fleet, ...om, epic]);
    sessions.set([]);
    projectPicks.set(new Map());
    switcherRequest.set(null);
    addProjectRequest.set(null);
    toasts.set([]);
    __trackers.set([]);
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'set_project_pick' ? (args as { args: unknown }).args : null);
  });

  async function openNew(host?: string) {
    openNewSessionPicker(host);
    await tick();
    return screen.getByTestId('switcher-input') as HTMLInputElement;
  }
  const activeLabel = (input: HTMLInputElement) =>
    document.getElementById(input.getAttribute('aria-activedescendant') ?? '')?.textContent ?? '';
  /** Clear the query (Esc), so the empty-query keys (⌘Z, ⌘⌫) are the picker's. */
  async function clearQuery(input: HTMLInputElement) {
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();
    expect(input.value).toBe('');
  }
  /** Highlight the row with this data-key (the mouse moves onto it). */
  async function highlight(key: string) {
    const row = document.querySelector(`[data-key="${key}"]`);
    expect(row).not.toBeNull();
    await fireEvent.mouseMove(row!);
    await tick();
  }

  it('opens from the request with the mode chip, projects only, Hidden folded', async () => {
    sessions.set([sess({ id: 1, project_id: 1 })]);
    render(QuickSwitcher);
    await openNew();
    expect(screen.getByTestId('mode-chip').textContent).toBe('New session in');
    expect(screen.queryAllByTestId('switcher-session')).toHaveLength(0);
    expect(screen.getByText('openmarket-* · pp')).toBeTruthy(); // the cluster heading's subtitle
    expect(screen.getByText(/Show 1 in Hidden/)).toBeTruthy();
    expect(screen.queryByText('ppt-epic-145')).toBeNull();
  });

  it('Ctrl+Shift+N opens it; Backspace on an empty query leaves the mode', async () => {
    render(QuickSwitcher);
    await fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true });
    await tick();
    const input = screen.getByTestId('switcher-input');
    expect(screen.getByTestId('mode-chip')).toBeTruthy();
    await fireEvent.keyDown(input, { key: 'Backspace' });
    await tick();
    expect(screen.queryByTestId('mode-chip')).toBeNull();
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
  });

  it('stands aside while a Work view owns the chord for New task (G2.1)', async () => {
    render(QuickSwitcher);
    const release = ownNewTaskChord();
    try {
      await fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true });
      await tick();
      expect(screen.queryByTestId('mode-chip')).toBeNull();
    } finally {
      release();
    }
    await fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true });
    await tick();
    expect(screen.getByTestId('mode-chip')).toBeTruthy();
  });

  it('the Hosts view request opens it with that host as context', async () => {
    sessions.set([sess({ id: 9, project_id: 3, host_alias: 'mefistos' })]);
    render(QuickSwitcher);
    newSessionHostRequest.set('mefistos');
    await tick(); await tick();
    expect(screen.getByText('on mefistos')).toBeTruthy();
    expect(get(newSessionHostRequest)).toBeNull();
  });

  it('Enter opens the dialog and records the pick; Ctrl+Enter asks for autostart', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-docs' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(get(newSessionRequest)?.project.project.repo).toBe('openmarket-docs');
    expect(readFrecency()['pp/openmarket-docs']).toBeTruthy(); // the pref, under the app's prefix
    clearNewSessionRequest();
    const input2 = await openNew();
    await fireEvent.input(input2, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input2, { key: 'Enter', ctrlKey: true });
    await tick();
    expect(get(newSessionRequest)?.autostart).toBe(true);
  });

  it('a host request carries the host into the dialog', async () => {
    render(QuickSwitcher);
    const input = await openNew('mefistos');
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(get(newSessionRequest)?.initialHost).toBe('mefistos');
  });

  it('Ctrl+1 opens the first numbered row', async () => {
    projectPicks.set(new Map([['pp/openmarket-app', { owner: 'pp', repo: 'openmarket-app', pinned: true, vis: null, grp: null }]]));
    render(QuickSwitcher);
    const input = await openNew();
    expect(screen.getByText('Ctrl+1')).toBeTruthy();
    await fireEvent.keyDown(input, { key: '1', ctrlKey: true });
    await tick();
    expect(get(newSessionRequest)?.project.project.repo).toBe('openmarket-app');
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('Ctrl+P pins the highlighted project; Ctrl+Z undoes it', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true));
    await clearQuery(input);
    await fireEvent.keyDown(input, { key: 'z', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(false));
    // The undo says what it undid.
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Undid: Pinned openmarket-ai'));
  });

  it('Ctrl+Z with a query is the input’s own undo: the pin stays', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Pinned openmarket-ai'));
    const calls = vi.mocked(__invoke).mock.calls.length;
    expect(await fireEvent.keyDown(input, { key: 'z', ctrlKey: true })).toBe(true); // not defaultPrevented
    await tick();
    expect(vi.mocked(__invoke).mock.calls.length).toBe(calls);
    expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true);
  });

  it('Ctrl+Z after a close and reopen undoes nothing (undo is per open)', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Pinned openmarket-ai'));
    await clearQuery(input);
    await fireEvent.keyDown(input, { key: 'Escape' }); // closes
    await tick();
    const input2 = await openNew();
    const calls = vi.mocked(__invoke).mock.calls.filter((c) => c[0] === 'set_project_pick').length;
    expect(await fireEvent.keyDown(input2, { key: 'z', ctrlKey: true })).toBe(true); // not taken
    await tick();
    await Promise.resolve();
    expect(vi.mocked(__invoke).mock.calls.filter((c) => c[0] === 'set_project_pick').length).toBe(calls);
    expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true);
  });

  it('Ctrl+Backspace with a query is left to the input; with none it hides the highlighted row', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-docs' } });
    await tick();
    expect(screen.queryByText('Ctrl⌫ hide')).toBeNull(); // the hint only offers it on an empty query
    expect(await fireEvent.keyDown(input, { key: 'Backspace', ctrlKey: true })).toBe(true); // not defaultPrevented
    await tick();
    expect(get(projectPicks).get('pp/openmarket-docs')?.vis ?? null).toBeNull();
    await clearQuery(input);
    expect(screen.getByText('Ctrl⌫ hide')).toBeTruthy();
    await highlight('project:3@g:c:pp:openmarket');
    expect(await fireEvent.keyDown(input, { key: 'Backspace', ctrlKey: true })).toBe(false);
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-docs')?.vis).toBe('hide'));
  });

  it('after Hide the highlight lands on the row that followed the hidden one', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await highlight('project:3@g:c:pp:openmarket');
    expect(activeLabel(input)).toContain('openmarket-docs');
    await fireEvent.keyDown(input, { key: 'Backspace', ctrlKey: true });
    await tick();
    await tick();
    expect(activeLabel(input)).toContain('openmarket-app');
  });

  it('a project moved into an automatic group of the same name still offers Back to automatic', async () => {
    projects.set([fleet, ...om, epic, p(6, 'pp', 'zeta-tool', 60)]);
    projectPicks.set(new Map([['pp/zeta-tool', { owner: 'pp', repo: 'zeta-tool', pinned: false, vis: null, grp: 'OpenMarket' }]]));
    render(QuickSwitcher);
    const input = await openNew();
    // One openmarket section, with zeta-tool in it.
    expect(document.querySelector('[data-key="project:6@g:c:pp:openmarket"]')).not.toBeNull();
    await fireEvent.input(input, { target: { value: 'zeta' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'g', ctrlKey: true });
    await tick();
    expect(screen.getByText('Back to automatic')).toBeTruthy();
  });

  it('Ctrl+Z works at once, before the pin write answers', async () => {
    let release: () => void = () => {};
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd !== 'set_project_pick') return null;
      const a = (args as { args: { pinned: boolean } }).args;
      if (a.pinned) await new Promise<void>((r) => (release = r));
      return a;
    });
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true);
    await clearQuery(input);
    await fireEvent.keyDown(input, { key: 'z', ctrlKey: true });
    release();
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(false));
  });

  it('a pin shows on the list at once (the person acted), with an Undo toast once saved', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    expect(screen.queryByText('Pinned')).toBeNull();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await fireEvent.input(input, { target: { value: '' } });
    await tick();
    expect(screen.getByText('Pinned')).toBeTruthy();
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Pinned openmarket-ai'));
    expect(get(toasts).find((t) => t.message === 'Pinned openmarket-ai')?.action?.label).toBe('Undo');
  });

  it('a failed write leaves nothing to undo', async () => {
    vi.mocked(__invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'set_project_pick') throw { code: 'E_INTERNAL', message: 'nope' };
      return null;
    });
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(false));
    await clearQuery(input);
    const calls = vi.mocked(__invoke).mock.calls.length;
    await fireEvent.keyDown(input, { key: 'z', ctrlKey: true });
    await tick();
    expect(vi.mocked(__invoke).mock.calls.length).toBe(calls);
  });

  it('hiding a pinned project unpins it too, and says so', async () => {
    projectPicks.set(new Map([['pp/openmarket-ai', { owner: 'pp', repo: 'openmarket-ai', pinned: true, vis: null, grp: null }]]));
    render(QuickSwitcher);
    const input = await openNew();
    await highlight('project:2@pinned');
    await fireEvent.keyDown(input, { key: 'Backspace', ctrlKey: true });
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Hid openmarket-ai (unpinned)'));
    expect(get(projectPicks).get('pp/openmarket-ai')).toMatchObject({ pinned: false, vis: 'hide' });
    await fireEvent.keyDown(input, { key: 'z', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')).toMatchObject({ pinned: true, vis: null }));
  });

  it('the order is frozen while open: picks arriving later do not move the highlight', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    const before = input.getAttribute('aria-activedescendant');
    projectPicks.set(new Map([['pp/openmarket-app', { owner: 'pp', repo: 'openmarket-app', pinned: true, vis: null, grp: null }]]));
    await tick();
    expect(input.getAttribute('aria-activedescendant')).toBe(before);
    expect(screen.queryByText('Pinned')).toBeNull(); // shown on the next open
  });

  it('Esc clears a query first, then closes', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'zz' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();
    expect(input.value).toBe('');
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    // On an empty query Esc is left alone, so the dialog's cancel closes it.
    expect(await fireEvent.keyDown(input, { key: 'Escape' })).toBe(true); // not defaultPrevented
  });

  it('macOS: ⌘N opens it, ⌘P pins and the picker stays open, ⌘Z undoes', async () => {
    render(QuickSwitcher, { props: { isMac: true } });
    await fireEvent.keyDown(window, { key: 'n', metaKey: true });
    await tick();
    const input = screen.getByTestId('switcher-input') as HTMLInputElement;
    expect(screen.getByTestId('mode-chip')).toBeTruthy();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', metaKey: true });
    await tick();
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true));
    await clearQuery(input);
    await fireEvent.keyDown(input, { key: 'z', metaKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(false));
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    // ⌘K still closes it.
    await fireEvent.keyDown(input, { key: 'k', metaKey: true });
    await tick();
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('an older toast’s Undo does nothing once a newer change was made', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Pinned openmarket-ai'));
    await clearQuery(input);
    await highlight('project:3@g:c:pp:openmarket');
    await fireEvent.keyDown(input, { key: 'Backspace', ctrlKey: true });
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Hid openmarket-docs'));
    const before = get(projectPicks);
    const calls = vi.mocked(__invoke).mock.calls.length;
    get(toasts).find((t) => t.message === 'Pinned openmarket-ai')!.action!.run();
    await tick();
    await Promise.resolve();
    expect(vi.mocked(__invoke).mock.calls.length).toBe(calls);
    expect(get(projectPicks)).toBe(before);
    expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true);
    // The newest toast's Undo still works.
    get(toasts).find((t) => t.message === 'Hid openmarket-docs')!.action!.run();
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-docs')?.vis).toBeNull());
  });

  it('no match offers Add project with the query; an owner/repo prefills the clone URL', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'acme/widgets' } });
    await tick();
    expect(screen.getByText('Add project “acme/widgets”…')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('switcher-add'));
    await tick();
    expect(get(addProjectRequest)).toEqual({ cloneUrl: 'acme/widgets' });
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
    const input2 = await openNew();
    await fireEvent.input(input2, { target: { value: 'widgets thing' } });
    await tick();
    await fireEvent.click(screen.getByTestId('switcher-add'));
    expect(get(addProjectRequest)).toEqual({ cloneUrl: undefined });
  });

  it('with no projects at all the Add row is still offered, and Enter on it asks for Add project', async () => {
    projects.set([]);
    render(QuickSwitcher);
    const input = await openNew();
    expect(screen.getByTestId('switcher-add')).toBeTruthy();
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(get(addProjectRequest)).toEqual({ cloneUrl: undefined });
    expect(screen.queryByTestId('quick-switcher')).toBeNull();
  });

  it('Enter on a fold row unfolds it', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    const fold = screen.getByText(/Show 1 in Hidden/).closest('[role=option]')!;
    await fireEvent.mouseMove(fold);
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(screen.getByText('ppt-epic-145')).toBeTruthy();
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
  });

  it('a folded group is one row with its count; ArrowLeft folds an open group', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-docs' } });
    await fireEvent.input(input, { target: { value: '' } });
    await tick();
    // Highlight a member of the open openmarket group, then fold it.
    const row = Array.from(document.querySelectorAll('[data-testid=switcher-project]')).find(
      (e) => e.textContent?.includes('openmarket-app'),
    )!;
    await fireEvent.mouseMove(row);
    await fireEvent.keyDown(input, { key: 'ArrowLeft' });
    await tick();
    expect(screen.getByText('openmarket · 3 projects')).toBeTruthy();
    expect(activeLabel(input)).toContain('openmarket · 3 projects');
    await fireEvent.keyDown(input, { key: 'ArrowRight' });
    await tick();
    expect(screen.queryByText('openmarket · 3 projects')).toBeNull();
  });

  it('Shift+F10 opens the actions menu; Esc closes it and focus returns to the input', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'F10', shiftKey: true });
    await tick();
    const menu = screen.getByTestId('project-actions');
    expect(menu.getAttribute('aria-label')).toBe('pp/openmarket-ai');
    await fireEvent.keyDown(menu, { key: 'Escape' });
    await tick(); await tick();
    expect(screen.queryByTestId('project-actions')).toBeNull();
    expect(document.activeElement).toBe(input);
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
  });

  it('Ctrl+G moves the project to a group through the menu', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'claude-fleet' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'g', ctrlKey: true });
    await tick();
    const gi = screen.getByLabelText('Group name');
    await fireEvent.input(gi, { target: { value: 'Mine' } });
    await fireEvent.keyDown(gi, { key: 'Enter' });
    await vi.waitFor(() => expect(get(projectPicks).get('me/claude-fleet')?.grp).toBe('Mine'));
    expect(screen.queryByTestId('project-actions')).toBeNull();
  });

  it('My work tickets with no session head the list under Start from work', async () => {
    __trackers.set([
      { id: 1, provider: 'jira', name: 'acme', site_url: 'https://acme.atlassian.net', state: 'ok', created_at: 1, config: { key_prefixes: ['ABC'] } },
    ]);
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      const view = (args as { args?: { view?: string } } | undefined)?.args?.view;
      const t = (key: string, live: number[] = []) => ({ id: 100 + Number(key.split('-')[1]), tracker_id: 1, source: 'jira', key, title: `${key} title`, status_category: 'todo', status_name: 'To Do', created_at: 1, updated_at: 1, live_session_ids: live });
      if (cmd === 'work_tickets' && view === 'mine') return [t('ABC-1'), t('ABC-2', [6])];
      if (cmd === 'work_tickets') return [];
      return null;
    });
    render(QuickSwitcher);
    await openNew();
    await vi.waitFor(() => expect(screen.getAllByTestId('switcher-ticket')).toHaveLength(1));
    expect(screen.getByText('Start from work')).toBeTruthy();
    expect(screen.getAllByTestId('switcher-ticket')[0].getAttribute('data-key')).toBe('ticket:ABC-1');
  });

  it('normal ⌘K mode drops picker-hidden projects from the empty-query list, keeps them for a query', async () => {
    render(QuickSwitcher);
    const input = await openSwitcher();
    expect(screen.queryByTestId('mode-chip')).toBeNull();
    expect(document.querySelector('[data-key="project:4"]')).not.toBeNull(); // unknown, not hidden
    expect(document.querySelector('[data-key="project:5"]')).toBeNull();
    await fireEvent.input(input, { target: { value: 'ppt-epic' } });
    await tick();
    expect(document.querySelector('[data-key="project:5"]')).not.toBeNull();
  });
});

describe('QuickSwitcher — the header command field (3.17)', () => {
  it('opens the plain switcher, not New session mode', async () => {
    switcherRequest.set(null);
    render(QuickSwitcher);
    const { openSwitcher } = await import('./switcher_request');
    openSwitcher();
    await tick();
    expect(screen.getByTestId('switcher-input')).toBeTruthy();
    expect(screen.queryByTestId('mode-chip')).toBeNull();
    expect(get(switcherRequest)).toBeNull();
  });
});

// ---- Pin, hide and groups under the New layout (parity H1–H3) ----------------
// The same keys as the session mode tests above: pin, hide and groups.

describe('QuickSwitcher in the New layout', () => {
  const NOW = Math.floor(Date.now() / 1000);
  const p = (id: number, owner: string, repo: string, ago: number | null) => ({
    project: { id, owner, repo, base_path: `/r/${repo}`, last_session_at: ago === null ? null : NOW - ago, adopted: false, system: false },
    worktrees: [],
  });
  const fleet = p(1, 'me', 'claude-fleet', 60);
  const om = [p(2, 'pp', 'openmarket-ai', 3600), p(3, 'pp', 'openmarket-docs', 7200), p(4, 'pp', 'openmarket-app', null)];
  const epic = p(5, 'me', 'ppt-epic-145', null);

  beforeEach(async () => {
    projects.set([fleet, ...om, epic]);
    sessions.set([]);
    projectPicks.set(new Map());
    switcherRequest.set(null);
    addProjectRequest.set(null);
    toasts.set([]);
    __trackers.set([]);
    vi.mocked(__invoke).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'set_project_pick' ? (args as { args: unknown }).args : null);
  });

  afterEach(async () => {
  });

  async function openNew() {
    openNewSessionPicker();
    await tick();
    return screen.getByTestId('switcher-input') as HTMLInputElement;
  }
  const activeLabel = (input: HTMLInputElement) =>
    document.getElementById(input.getAttribute('aria-activedescendant') ?? '')?.textContent ?? '';
  async function clearQuery(input: HTMLInputElement) {
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();
    expect(input.value).toBe('');
  }
  async function highlight(key: string) {
    const row = document.querySelector(`[data-key="${key}"]`);
    expect(row).not.toBeNull();
    await fireEvent.mouseMove(row!);
    await tick();
  }

  it('Ctrl+P pins the highlighted project; Ctrl+Z undoes it', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true));
    await clearQuery(input);
    await fireEvent.keyDown(input, { key: 'z', ctrlKey: true });
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(false));
    await vi.waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Undid: Pinned openmarket-ai'));
  });

  it('macOS: ⌘P pins the highlighted project and the picker stays open', async () => {
    render(QuickSwitcher, { props: { isMac: true } });
    await fireEvent.keyDown(window, { key: 'n', metaKey: true });
    await tick();
    const input = screen.getByTestId('switcher-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'openmarket-ai' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'p', metaKey: true });
    await tick();
    expect(screen.getByTestId('quick-switcher')).toBeTruthy();
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-ai')?.pinned).toBe(true));
  });

  it('Ctrl+Backspace with a query is left to the input; with none it hides the highlighted row', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'openmarket-docs' } });
    await tick();
    expect(screen.queryByText('Ctrl⌫ hide')).toBeNull();
    expect(await fireEvent.keyDown(input, { key: 'Backspace', ctrlKey: true })).toBe(true); // not defaultPrevented
    await tick();
    expect(get(projectPicks).get('pp/openmarket-docs')?.vis ?? null).toBeNull();
    await clearQuery(input);
    expect(screen.getByText('Ctrl⌫ hide')).toBeTruthy();
    await highlight('project:3@g:c:pp:openmarket');
    expect(await fireEvent.keyDown(input, { key: 'Backspace', ctrlKey: true })).toBe(false);
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-docs')?.vis).toBe('hide'));
  });

  it('macOS: ⌘⌫ on an empty query hides the highlighted row', async () => {
    render(QuickSwitcher, { props: { isMac: true } });
    await fireEvent.keyDown(window, { key: 'n', metaKey: true });
    await tick();
    const input = screen.getByTestId('switcher-input') as HTMLInputElement;
    await highlight('project:3@g:c:pp:openmarket');
    expect(activeLabel(input)).toContain('openmarket-docs');
    expect(await fireEvent.keyDown(input, { key: 'Backspace', metaKey: true })).toBe(false);
    await vi.waitFor(() => expect(get(projectPicks).get('pp/openmarket-docs')?.vis).toBe('hide'));
  });

  it('Ctrl+G moves the project to a group through the menu', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    await fireEvent.input(input, { target: { value: 'claude-fleet' } });
    await tick();
    await fireEvent.keyDown(input, { key: 'g', ctrlKey: true });
    await tick();
    const gi = screen.getByLabelText('Group name');
    await fireEvent.input(gi, { target: { value: 'Mine' } });
    await fireEvent.keyDown(gi, { key: 'Enter' });
    await vi.waitFor(() => expect(get(projectPicks).get('me/claude-fleet')?.grp).toBe('Mine'));
    expect(screen.queryByTestId('project-actions')).toBeNull();
  });

  it('a folded group is one row with its count; ArrowLeft folds an open group, ArrowRight opens it', async () => {
    render(QuickSwitcher);
    const input = await openNew();
    const row = Array.from(document.querySelectorAll('[data-testid=switcher-project]')).find(
      (e) => e.textContent?.includes('openmarket-app'),
    )!;
    await fireEvent.mouseMove(row);
    await fireEvent.keyDown(input, { key: 'ArrowLeft' });
    await tick();
    expect(screen.getByText('openmarket · 3 projects')).toBeTruthy();
    expect(activeLabel(input)).toContain('openmarket · 3 projects');
    await fireEvent.keyDown(input, { key: 'ArrowRight' });
    await tick();
    expect(screen.queryByText('openmarket · 3 projects')).toBeNull();
  });
});
