import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import * as sessionsModule from './sessions';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';
import { hosts } from './hosts';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ]);
  localStorage.clear();
});

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null, adopted: false, system: false },
  worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' }],
};

const remoteMain = { id: 501, project_id: 1, host_alias: 'mefistos', name: 'main', path: '/home/u/projects/github.com/martin-janci/claude-fleet', branch: 'main' };
const remoteFeat = { id: 502, project_id: 1, host_alias: 'mefistos', name: 'feat', path: '/home/u/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/feat', branch: 'feature/feat' };

/** Answer `list_host_worktrees` for mefistos; other commands keep `extra`. */
function mockHostWorktrees(
  reply: { cloned: boolean; worktrees: unknown[] } | Error | (() => Promise<unknown>),
  extra: (cmd: string, args?: unknown) => unknown = () => null,
) {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === 'list_host_worktrees') {
      if (reply instanceof Error) throw { code: 'E_SSH', message: reply.message };
      if (typeof reply === 'function') return reply();
      const a = (args as { args: { host_alias: string; project_id: number } }).args;
      return { host_alias: a.host_alias, project_id: a.project_id, ...reply };
    }
    return extra(cmd, args);
  });
}

function worktreeLabels(): string[] {
  return Array.from(document.querySelectorAll('[data-testid="wt-picker"] [role="option"] .label')).map(
    (e) => e.textContent?.trim() ?? '',
  );
}

describe('NewSessionDialog remote path preview (W5 G3)', () => {
  async function pickMefistos() {
    const btn = Array.from(document.querySelectorAll('.host-pick')).find(
      (p) => (p as HTMLElement).dataset.alias === 'mefistos',
    ) as HTMLButtonElement;
    await fireEvent.click(btn);
    await tick();
  }

  it('is ~/projects/github.com/<owner>/<repo> when no projects setting exists', async () => {
    fleetSettings.set({ ...SETTING_DEFAULTS });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickMefistos();
    expect(screen.getByTestId('path-preview').textContent).toContain(
      '~/projects/github.com/martin-janci/claude-fleet',
    );
  });

  it('follows the host projects root and the flat layout from settings', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'get_fleet_settings') {
        return {
          'projects.layout': 'flat',
          'projects.resolved_base': JSON.stringify({ local: '/home/u/projects', mefistos: '~/code' }),
        };
      }
      return null;
    });
    fleetSettings.set({ ...SETTING_DEFAULTS });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickMefistos();
    await vi.waitFor(() =>
      expect(screen.getByTestId('path-preview').textContent).toContain('~/code/claude-fleet'),
    );
    expect(screen.getByTestId('path-preview').textContent).not.toContain('github.com');
  });
});

describe('NewSessionDialog', () => {
  it('renders one host-pick button per non-hidden host', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const picks = document.querySelectorAll('.host-pick');
    expect(picks).toHaveLength(2);
    expect(Array.from(picks).map((p) => (p as HTMLElement).dataset.alias)).toEqual(['local', 'mefistos']);
  });

  it('defaults to last-host pref (local on first run)', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const active = document.querySelector(".host-pick[aria-pressed='true']");
    expect(active?.getAttribute('data-alias')).toBe('local');
  });

  it('clicking a host pick + Create sends host_alias to new_session', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_host_worktrees') return { host_alias: 'mefistos', project_id: 1, cloned: true, worktrees: [remoteMain] };
      if (cmd === 'new_session') {
        return { id: 99, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 1, worktree_id: null, created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null, claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null, friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null };
      }
      if (cmd === 'list_sessions') return [];
      return null;
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => (p as HTMLElement).dataset.alias === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
    await vi.waitFor(() => expect(worktreeLabels()).toContain('main'));
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const newSessionCall = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'new_session');
    expect((newSessionCall![1] as any).args.host_alias).toBe('mefistos');
  });

  it('clicking + new chip, typing a name, and clicking Create passes new_worktree and worktree_id=null', async () => {
    const newSessionAbortableSpy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 42,
        tmux_name: 'dev-martin-janci-claude-fleet--feat-test',
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
        worktree_key: null,
        lost_at: null,
        claude_session_id: null,
        claude_status: null,
        effort_level: null,
        pr_url: null,
        current_activity: null,
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
      },
    });

    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();

    // Click the "+ new" chip
    const newChip = screen.getByTestId('new-worktree-chip');
    await fireEvent.click(newChip);
    await tick();

    // Type a worktree name
    const nameInput = screen.getByTestId('new-worktree-name');
    await fireEvent.input(nameInput, { target: { value: 'feat-test' } });
    await tick();

    // Click Create
    await fireEvent.click(screen.getByText('Create'));
    await tick();

    expect(newSessionAbortableSpy).toHaveBeenCalledOnce();
    const callArgs = newSessionAbortableSpy.mock.calls[0][0];
    expect(callArgs.new_worktree).toBe('feat-test');
    expect(callArgs.worktree_id).toBeNull();
    // No base branch typed → defaults to the repo default (null).
    expect(callArgs.base_branch).toBeNull();

    newSessionAbortableSpy.mockRestore();
  });

  it('auto-slugifies a free-form sentence in the worktree-name input', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 44, tmux_name: 'dev-martin-janci-claude-fleet--fix-the-login-bug',
        host_alias: 'local', project_id: 1, worktree_id: null,
        created_at: 1, last_activity_at: 1, status: 'running', notes: null,
        account_uuid: null, kind: 'work', reviews_session_id: null,
        worktree_key: null, lost_at: null, claude_session_id: null,
        claude_status: null, effort_level: null, pr_url: null, current_activity: null,
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null,
        safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
      },
    });

    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();

    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();

    const input = screen.getByTestId('new-worktree-name') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Fix the login bug!' } });
    await tick();

    // The input itself reflects the slug, not the raw sentence.
    expect(input.value).toBe('fix-the-login-bug');
    // The derived tmux name picks up the slug too.
    expect((screen.getByTestId('new-session-name') as HTMLInputElement).value)
      .toBe('dev-martin-janci-claude-fleet--fix-the-login-bug');

    await fireEvent.click(screen.getByText('Create'));
    await tick();

    expect(spy.mock.calls[0][0].new_worktree).toBe('fix-the-login-bug');
    spy.mockRestore();
  });

  it('strips a trailing dash on submit even if the user did not blur the input', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 45, tmux_name: 'x', host_alias: 'local', project_id: 1, worktree_id: null,
        created_at: 1, last_activity_at: 1, status: 'running', notes: null,
        account_uuid: null, kind: 'work', reviews_session_id: null,
        worktree_key: null, lost_at: null, claude_session_id: null,
        claude_status: null, effort_level: null, pr_url: null, current_activity: null,
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null,
        safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
      },
    });

    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();
    // Trailing space → live slugifier yields a trailing dash.
    await fireEvent.input(screen.getByTestId('new-worktree-name'), {
      target: { value: 'fix login ' },
    });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();

    expect(spy.mock.calls[0][0].new_worktree).toBe('fix-login');
    spy.mockRestore();
  });

  it('typing a base branch in new-worktree mode passes base_branch', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 43,
        tmux_name: 'dev-martin-janci-claude-fleet--feat-test',
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
        safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
      },
    });

    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();

    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();
    await fireEvent.input(screen.getByTestId('new-worktree-name'), {
      target: { value: 'feat-test' },
    });
    await tick();

    // The base-branch input is only present in new-worktree mode.
    await fireEvent.input(screen.getByTestId('new-worktree-base'), {
      target: { value: 'dev' },
    });
    await tick();

    await fireEvent.click(screen.getByText('Create'));
    await tick();

    expect(spy).toHaveBeenCalledOnce();
    expect(spy.mock.calls[0][0].base_branch).toBe('dev');

    spy.mockRestore();
  });

  it('defaults to a "work" session', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 1, tmux_name: 'dev-martin-janci-claude-fleet', host_alias: 'local',
        project_id: 1, worktree_id: 11, created_at: 1, last_activity_at: 1,
        status: 'running', notes: null, account_uuid: null, kind: 'work',
        reviews_session_id: null, worktree_key: null, lost_at: null,
        claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null,
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
      },
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(spy.mock.calls[0][0].kind).toBe('work');
    spy.mockRestore();
  });

  it('picking Shell passes kind="shell" and suffixes the tmux name with -term', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 2, tmux_name: 'dev-martin-janci-claude-fleet-term', host_alias: 'local',
        project_id: 1, worktree_id: 11, created_at: 1, last_activity_at: 1,
        status: 'running', notes: null, account_uuid: null, kind: 'shell',
        reviews_session_id: null, worktree_key: null, lost_at: null,
        claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null,
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
      },
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('kind-shell'));
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(spy.mock.calls[0][0].kind).toBe('shell');
    expect(spy.mock.calls[0][0].name).toBe('dev-martin-janci-claude-fleet-term');
    expect(spy.mock.calls[0][0].start_command).toBeNull();
    spy.mockRestore();
  });

  it('a start command typed in Shell mode is passed as start_command', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 3, tmux_name: 'dev-martin-janci-claude-fleet-term', host_alias: 'local',
        project_id: 1, worktree_id: 11, created_at: 1, last_activity_at: 1,
        status: 'running', notes: null, account_uuid: null, kind: 'shell',
        reviews_session_id: null, worktree_key: null, lost_at: null,
        claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null,
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
      },
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('kind-shell'));
    await tick();
    await fireEvent.input(screen.getByTestId('start-command'), { target: { value: 'pnpm test' } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(spy.mock.calls[0][0].start_command).toBe('pnpm test');
    spy.mockRestore();
  });
});

// ── generated names, re-roll, Enter, memory, overflow ────────────────────

import { sessions } from './sessions';
import { isGeneratedName } from './names';

function okRow(over: Partial<sessionsModule.SessionRow> = {}): sessionsModule.SessionRow {
  return {
    id: 50, tmux_name: 'x', host_alias: 'local', project_id: 1, worktree_id: 11,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null,
    account_uuid: null, kind: 'work', reviews_session_id: null,
    worktree_key: null, lost_at: null, claude_session_id: null,
    claude_status: null, effort_level: null, pr_url: null, current_activity: null,
    friendly_name: null, safe_kill_state: null, safe_kill_nonce: null,
    safe_kill_detail: null, safe_kill_requested_at: null,
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

const slugOf = (words: string) => words.trim().toLowerCase().replace(/\s+/g, '-');

describe('NewSessionDialog — generated names', () => {
  beforeEach(() => {
    sessions.set([]);
  });

  it('prefills the name with a generated adjective-noun pair on open', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name') as HTMLInputElement;
    expect(input.value).toMatch(/^[a-z]+ [a-z]+$/);
    expect(isGeneratedName(slugOf(input.value))).toBe(true);
  });

  it('the dice button and Ctrl/Cmd+R re-roll the name', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name') as HTMLInputElement;
    const seen = new Set<string>([input.value]);
    for (let i = 0; i < 6; i++) {
      await fireEvent.click(screen.getByTestId('reroll-name'));
      await tick();
      expect(isGeneratedName(slugOf(input.value))).toBe(true);
      seen.add(input.value);
    }
    expect(seen.size).toBeGreaterThan(1);
    const before = input.value;
    let changed = false;
    for (let i = 0; i < 6 && !changed; i++) {
      await fireEvent.keyDown(input, { key: 'r', ctrlKey: true });
      await tick();
      changed = input.value !== before;
    }
    expect(changed).toBe(true);
  });

  it('a generated name never collides with a worktree or session slug on the project', async () => {
    // Make (almost) every pair unavailable is impractical; instead assert the
    // generator is fed the taken set: a worktree named after the first pair
    // the dialog would otherwise offer must never be offered.
    const busyProject = {
      ...project,
      worktrees: [
        ...project.worktrees,
        { id: 12, project_id: 1, host_alias: 'local', name: 'blue-sirius', path: '/r/cf/.worktrees/blue-sirius', branch: 'blue-sirius' },
      ],
    };
    sessions.set([okRow({ id: 7, tmux_name: 'dev-martin-janci-claude-fleet--amber-vega', friendly_name: 'Amber vega' })]);
    render(NewSessionDialog, { props: { project: busyProject, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name') as HTMLInputElement;
    for (let i = 0; i < 40; i++) {
      expect(input.value).not.toBe('blue sirius');
      expect(input.value).not.toBe('amber vega');
      await fireEvent.click(screen.getByTestId('reroll-name'));
      await tick();
    }
  });

  it('in new-worktree mode the slug, tmux name and cwd preview follow the generated name', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();
    const friendly = (screen.getByTestId('friendly-name') as HTMLInputElement).value;
    const slug = slugOf(friendly);
    expect((screen.getByTestId('new-worktree-name') as HTMLInputElement).value).toBe(slug);
    expect((screen.getByTestId('new-session-name') as HTMLInputElement).value)
      .toBe(`dev-martin-janci-claude-fleet--${slug}`);
    expect(screen.getByTestId('path-preview').textContent).toContain(`/r/cf/.worktrees/${slug}`);
  });

  it('a second session on the same worktree gets a name-suffixed tmux name instead of a collision', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_host_worktrees') return { host_alias: 'mefistos', project_id: 1, cloned: true, worktrees: [remoteMain] };
      return null;
    });
    sessions.set([okRow({ id: 7, tmux_name: 'dev-martin-janci-claude-fleet', worktree_id: 11 })]);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const friendly = (screen.getByTestId('friendly-name') as HTMLInputElement).value;
    expect((screen.getByTestId('new-session-name') as HTMLInputElement).value)
      .toBe(`dev-martin-janci-claude-fleet--${slugOf(friendly)}`);
    // …but only on the same host.
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => (p as HTMLElement).dataset.alias === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
    await vi.waitFor(() => expect(worktreeLabels()).toContain('main'));
    expect((screen.getByTestId('new-session-name') as HTMLInputElement).value).toBe('dev-martin-janci-claude-fleet');
  });

  it('Enter in a field submits; the friendly name is sent as typed', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: okRow() });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name');
    await fireEvent.input(input, { target: { value: 'red comet' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(spy).toHaveBeenCalledOnce();
    expect(spy.mock.calls[0][0].friendly_name).toBe('red comet');
    spy.mockRestore();
  });

  it('an emptied tmux name is sent as "" so the backend generates one', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: okRow() });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.input(screen.getByTestId('new-session-name'), { target: { value: '' } });
    await tick();
    const create = screen.getByText('Create') as HTMLButtonElement;
    expect(create.disabled).toBe(false);
    await fireEvent.click(create);
    await tick();
    expect(spy.mock.calls[0][0].name).toBe('');
    spy.mockRestore();
  });

  const twoWt = {
    ...project,
    worktrees: [
      ...project.worktrees,
      { id: 12, project_id: 1, host_alias: 'local', name: 'feat-x', path: '/r/cf/.worktrees/feat-x', branch: 'feat-x' },
    ],
  };
  const pickWorktree = async (name: string) => {
    const row = screen.getAllByTestId('worktree-row').find((r) => r.textContent?.includes(name))!;
    await fireEvent.click(row);
    await tick();
  };

  it('a typed name survives a worktree switch and entering new-worktree mode', async () => {
    render(NewSessionDialog, { props: { project: twoWt, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'red comet' } });
    await pickWorktree('feat-x');
    expect(input.value).toBe('red comet');
    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();
    expect(input.value).toBe('red comet');
    expect((screen.getByTestId('new-worktree-name') as HTMLInputElement).value).toBe('red-comet');
  });

  it('the quick-switcher initialName survives a worktree switch', async () => {
    render(NewSessionDialog, { props: { project: twoWt, initialName: 'Fix login', onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickWorktree('feat-x');
    expect((screen.getByTestId('friendly-name') as HTMLInputElement).value).toBe('Fix login');
  });

  it('after a re-roll (or clearing the field) worktree switches regenerate again', async () => {
    render(NewSessionDialog, { props: { project: twoWt, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'red comet' } });
    await fireEvent.click(screen.getByTestId('reroll-name'));
    await tick();
    await pickWorktree('feat-x');
    // Not dirty → the worktree's humanised branch is offered.
    expect(input.value).toBe('Feat x');
    await fireEvent.input(input, { target: { value: '' } });
    await pickWorktree('main');
    expect(isGeneratedName(slugOf(input.value))).toBe(true);
  });

  it('remembered host is used only while it is visible and reachable, else last-host, else local', async () => {
    hosts.update((h) => [
      ...h,
      { alias: 'hetzner', ssh_alias: 'hetzner', reachable: false, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
      { alias: 'hidden-box', ssh_alias: 'hidden-box', reachable: true, claude_version: null, tmux_version: null, hidden: true, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ] as typeof h);
    const active = () => document.querySelector(".host-pick[aria-pressed='true']")?.getAttribute('data-alias');
    const open = async () => {
      const r = render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
      await tick();
      const a = active();
      r.unmount();
      return a;
    };
    const remember = (host: string) =>
      localStorage.setItem('cf:pref:newsession.project.1', JSON.stringify({ host, worktree: 11, kind: 'work' }));

    remember('mefistos');
    expect(await open()).toBe('mefistos'); // usable → honoured

    remember('hetzner'); // unreachable
    localStorage.setItem('cf:pref:last-host', JSON.stringify('mefistos'));
    expect(await open()).toBe('mefistos'); // → last-host

    remember('hidden-box'); // hidden
    localStorage.setItem('cf:pref:last-host', JSON.stringify('gone'));
    expect(await open()).toBe('local'); // last-host unknown too → local
  });

  it('a second-session name that is itself taken gets a numeric suffix', async () => {
    sessions.set([
      okRow({ id: 7, tmux_name: 'dev-martin-janci-claude-fleet', worktree_id: 11 }),
      okRow({ id: 8, tmux_name: 'dev-martin-janci-claude-fleet--red-comet', worktree_id: 11 }),
      okRow({ id: 9, tmux_name: 'dev-martin-janci-claude-fleet--red-comet-2', worktree_id: 11 }),
    ]);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.input(screen.getByTestId('friendly-name'), { target: { value: 'red comet' } });
    await tick();
    expect((screen.getByTestId('new-session-name') as HTMLInputElement).value)
      .toBe('dev-martin-janci-claude-fleet--red-comet-3');
  });

  it('dots in a worktree name never reach the tmux name', async () => {
    const dotted = {
      ...project,
      worktrees: [{ id: 13, project_id: 1, host_alias: 'local', name: 'v1.2', path: '/r/cf/.worktrees/v1.2', branch: 'v1.2' }],
    };
    render(NewSessionDialog, { props: { project: dotted, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect((screen.getByTestId('new-session-name') as HTMLInputElement).value)
      .toBe('dev-martin-janci-claude-fleet--v1-2');
  });

  it('Ctrl/Cmd+R re-rolls even with focus on the dialog element itself (never a reload)', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name') as HTMLInputElement;
    const dialog = document.querySelector('dialog') as HTMLDialogElement;
    const before = input.value;
    let changed = false;
    for (let i = 0; i < 6 && !changed; i++) {
      const notPrevented = await fireEvent.keyDown(dialog, { key: 'r', ctrlKey: true });
      expect(notPrevented).toBe(false); // preventDefault → no webview reload
      await tick();
      changed = input.value !== before;
    }
    expect(changed).toBe(true);
  });

  it('initialName pre-fills the friendly name (quick switcher hand-off)', async () => {
    render(NewSessionDialog, { props: { project, initialName: 'Fix the login bug', onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect((screen.getByTestId('friendly-name') as HTMLInputElement).value).toBe('Fix the login bug');
  });

  it('remembers host, worktree mode and kind per project after a successful create', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: okRow() });
    const { unmount } = render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => (p as HTMLElement).dataset.alias === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
    await fireEvent.click(screen.getByTestId('kind-shell'));
    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(JSON.parse(localStorage.getItem('cf:pref:newsession.project.1')!)).toEqual({
      host: 'mefistos', kind: 'shell', worktrees: { mefistos: 'new' },
    });
    unmount();
    // Re-open: the remembered choices are applied.
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect(document.querySelector(".host-pick[aria-pressed='true']")?.getAttribute('data-alias')).toBe('mefistos');
    expect((document.querySelector('.kind-pick.active') as HTMLElement).textContent?.trim()).toBe('Shell');
    expect(screen.getByTestId('new-worktree-name')).toBeTruthy();
    spy.mockRestore();
  });

  it('the worktree list is a bounded scrollable picker that keeps the active row visible', async () => {
    const scrolled: string[] = [];
    Element.prototype.scrollIntoView = function () {
      scrolled.push((this as HTMLElement).getAttribute('data-key') ?? '');
    };
    const many = {
      ...project,
      worktrees: Array.from({ length: 30 }, (_, i) => ({
        id: 100 + i, project_id: 1, host_alias: 'local', name: `wt-${i}`, path: `/r/cf/.worktrees/wt-${i}`, branch: `wt-${i}`,
      })),
    };
    render(NewSessionDialog, { props: { project: many, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const list = screen.getByTestId('wt-picker') as HTMLElement;
    expect(list.style.maxHeight).toBe('9rem');
    expect(screen.getAllByTestId('worktree-row')).toHaveLength(30);
    const last = screen.getAllByTestId('worktree-row')[29];
    await fireEvent.click(last);
    await tick();
    await Promise.resolve();
    expect(scrolled[scrolled.length - 1]).toBe('129');
    // Create stays reachable: the actions row is outside the scroll region.
    expect(screen.getByText('Create').closest('.fields')).toBeNull();
    // @ts-expect-error restore jsdom default (undefined)
    delete Element.prototype.scrollIntoView;
  });
});

describe('NewSessionDialog host-scoped worktrees', () => {
  async function pickHost(alias: string) {
    const btn = Array.from(document.querySelectorAll('.host-pick')).find(
      (p) => (p as HTMLElement).dataset.alias === alias,
    ) as HTMLButtonElement;
    await fireEvent.click(btn);
    await tick();
  }

  it('local never calls list_host_worktrees and lists the project tree rows', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect(worktreeLabels()).toEqual(['main', '+ new worktree']);
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_host_worktrees');
    expect(calls).toHaveLength(0);
  });

  it('switching to a remote host scans it once and swaps the list', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toEqual(['main', 'feat', '+ new worktree']));
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_host_worktrees');
    expect(calls).toHaveLength(1);
    expect((calls[0][1] as any).args).toEqual({ host_alias: 'mefistos', project_id: 1 });
    // The remote main row is selected, not the local one.
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('501');
  });

  it('shows a scanning status while the scan is in flight', async () => {
    let resolve!: (v: unknown) => void;
    mockHostWorktrees(() => new Promise((r) => (resolve = r)));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    expect(screen.getByTestId('wt-status')).toHaveTextContent('Scanning mefistos');
    expect(worktreeLabels()).toEqual(['+ new worktree']);
    resolve({ host_alias: 'mefistos', project_id: 1, cloned: true, worktrees: [remoteMain] });
    await vi.waitFor(() => expect(screen.queryByTestId('wt-status')).toBeNull());
    expect(worktreeLabels()).toEqual(['main', '+ new worktree']);
  });

  it('a host without the clone offers only + new worktree and says so', async () => {
    mockHostWorktrees({ cloned: false, worktrees: [] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(screen.getByTestId('wt-status')).toHaveTextContent('Not cloned on mefistos yet'));
    expect(worktreeLabels()).toEqual(['+ new worktree']);
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('new');
    expect((screen.getByTestId('new-worktree-name') as HTMLInputElement).value).not.toBe('');
  });

  it('a failed scan shows the error and keeps + new worktree usable', async () => {
    mockHostWorktrees(new Error('ssh: connect to host mefistos: timed out'));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(screen.getByTestId('wt-status')).toHaveTextContent('timed out'));
    expect(worktreeLabels()).toEqual(['+ new worktree']);
    expect(screen.getByText('Create')).not.toBeDisabled();
  });

  it('a slow earlier scan cannot overwrite a later host', async () => {
    hosts.update((h) => [...h, { alias: 'vps', ssh_alias: 'vps', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' }]);
    let resolveMef!: (v: unknown) => void;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd !== 'list_host_worktrees') return null;
      const host = (args as { args: { host_alias: string } }).args.host_alias;
      if (host === 'mefistos') return new Promise((r) => (resolveMef = r));
      return { host_alias: 'vps', project_id: 1, cloned: true, worktrees: [{ ...remoteMain, id: 601, host_alias: 'vps' }] };
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await pickHost('vps');
    await vi.waitFor(() => expect(worktreeLabels()).toEqual(['main', '+ new worktree']));
    resolveMef({ host_alias: 'mefistos', project_id: 1, cloned: true, worktrees: [remoteMain, remoteFeat] });
    await tick(); await tick();
    expect(worktreeLabels()).toEqual(['main', '+ new worktree']);
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('601');
  });

  it('remembers the worktree per host', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: { id: 1 } as any });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toContain('feat'));
    await fireEvent.click(document.querySelector('[data-testid="wt-picker"] [data-key="502"]')!);
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const mem = JSON.parse(localStorage.getItem('cf:pref:newsession.project.1')!);
    expect(mem.host).toBe('mefistos');
    expect(mem.worktrees).toEqual({ mefistos: 502 });
    spy.mockRestore();
    // Re-open: mefistos remembers feat, local still defaults to its first row.
    document.body.innerHTML = '';
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('502'),
    );
    await pickHost('local');
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('11');
  });

  it('switching host mid-scan never leaves a foreign worktree row selected/submittable', async () => {
    // The scan never resolves within this test — regression test for the
    // stale-foreign-worktree-id bug: during the (possibly long) SSH scan,
    // the picker must already show "+ new worktree" as active, not the
    // previous host's row.
    mockHostWorktrees(() => new Promise(() => {}));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('new');
  });

  it('a scan error also leaves + new worktree selected, not a foreign row', async () => {
    mockHostWorktrees(new Error('ssh: connect to host mefistos: timed out'));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(screen.getByTestId('wt-status')).toHaveTextContent('timed out'));
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('new');
  });

  it('a legacy flat local memory value still selects that worktree on open', async () => {
    localStorage.setItem('cf:pref:newsession.project.1', JSON.stringify({ host: 'local', worktree: 11, kind: 'work' }));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('11');
  });

  it('a remembered worktree id that no longer exists on that host falls back to its main', async () => {
    localStorage.setItem(
      'cf:pref:newsession.project.1',
      JSON.stringify({ host: 'mefistos', kind: 'work', worktrees: { mefistos: 999 } }),
    );
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('501'),
    );
  });

  it('switching host while already in new-worktree mode preserves the typed branch name', async () => {
    // The scan never resolves — this isolates the scan-start reset (the
    // thing under test) from the separate "rows arrived" repair effect.
    mockHostWorktrees(() => new Promise(() => {}));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();
    await fireEvent.input(screen.getByTestId('new-worktree-name'), { target: { value: 'my-typed-branch' } });
    await tick();
    await pickHost('mefistos');
    expect((screen.getByTestId('new-worktree-name') as HTMLInputElement).value).toBe('my-typed-branch');
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('new');
  });

  it('remember() folds a legacy flat local value into the per-host map', async () => {
    localStorage.setItem('cf:pref:newsession.project.1', JSON.stringify({ host: 'local', worktree: 11, kind: 'work' }));
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain] });
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: { id: 1 } as any });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toContain('main'));
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const mem = JSON.parse(localStorage.getItem('cf:pref:newsession.project.1')!);
    expect(mem.worktrees).toEqual({ local: 11, mefistos: 501 });
    spy.mockRestore();
  });

  it('remembers what was submitted, not a later selection made while busy', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    let resolveCreate!: (v: unknown) => void;
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockImplementation(() => new Promise((r) => (resolveCreate = r as (v: unknown) => void)) as any);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    // Submit while on local + main (worktree id 11); the request hangs.
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    // The host chips stay clickable while `busy` — switch to mefistos and
    // let its scan settle onto a real (different) selection before the
    // original request finally resolves.
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toContain('main'));
    resolveCreate({ ok: true, value: { id: 1 } as any });
    await tick();
    const mem = JSON.parse(localStorage.getItem('cf:pref:newsession.project.1')!);
    expect(mem.host).toBe('local');
    expect(mem.worktrees).toEqual({ local: 11 });
    spy.mockRestore();
  });
});

// #147 finding 1: `newSessionBlocked` (the same derived the Create button's
// `disabled` reads) was checked only by the button — Enter in the Name field
// went straight to `submit()` and still routed `new_session` while the hub
// connection was down. The handler itself must refuse, not just the button.
describe('NewSessionDialog: Enter is gated the same as the Create button', () => {
  const remote: HubStatus = {
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    client_mode: null,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  };

  it('Enter in the Name field calls no ipc while a hub client is reconnecting', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable');
    hubStatus.set(remote);
    hubConnection.set({ state: 'reconnecting', attempt: 1, retry_in_secs: 3, reason: 'closed' });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name');
    await fireEvent.input(input, { target: { value: 'red comet' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(spy).not.toHaveBeenCalled();
    spy.mockRestore();
  });

  it('standalone is untouched: Enter still creates', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: okRow() });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const input = screen.getByTestId('friendly-name');
    await fireEvent.input(input, { target: { value: 'red comet' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(spy).toHaveBeenCalledOnce();
    spy.mockRestore();
  });
});

// #168: `list_host_worktrees` now routes to a hub tool, so a hub client
// scans a remote host through the hub and gets real rows. What it must NOT
// do is invent them: never read `project.worktrees` as a substitute (those
// are only ever the store's own local rows — `list_projects_joined` joins
// `host_alias = 'local'` only, so filtering them by a remote alias always
// came back empty, which read as "no worktrees" rather than "unknown"), and
// never show an empty list when the hub could not answer. A hub that cannot
// answer — too old for the tool, wire-contract skew, or simply not reachable
// — keeps #146's neutral note and "+ new worktree", which still works.
describe('NewSessionDialog host-scoped worktrees on a hub client', () => {
  const remote: HubStatus = {
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    client_mode: null,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  };
  async function pickHost(alias: string) {
    const btn = Array.from(document.querySelectorAll('.host-pick')).find(
      (p) => (p as HTMLElement).dataset.alias === alias,
    ) as HTMLButtonElement;
    await fireEvent.click(btn);
    await tick();
  }

  /** Answer `list_host_worktrees` with one `IpcError` code. */
  function refuseWith(code: string) {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_host_worktrees') throw { code, message: `refused: ${code}` };
      return null;
    });
  }

  it('lists the remote hosts worktrees through the hub, with no note', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    hubStatus.set(remote);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toEqual(['main', 'feat', '+ new worktree']));
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_host_worktrees');
    expect(calls).toHaveLength(1);
    expect((calls[0][1] as { args: unknown }).args).toEqual({ host_alias: 'mefistos', project_id: 1 });
    expect(screen.queryByTestId('wt-remote-unknown')).toBeNull();
    expect(screen.queryByTestId('wt-status')).toBeNull();
  });

  // A hub that cannot answer the call at all is #146's state again, by code
  // and not by message text.
  //
  // `E_FORBIDDEN` is the one an older hub really sends: this desktop pairs as
  // an ordinary client, and the hub's tool gates run before its router
  // dispatches and fail closed on the tool NAME — so a hub with no policy row
  // for `list_host_worktrees` refuses it as "not a client-callable tool"
  // rather than as "no such tool"
  // (`a_tool_name_an_old_hub_does_not_know_refuses_a_client_with_e_forbidden`
  // pins that on the hub side). `E_HUB_PROTOCOL` covers the hubs that predate
  // that fail-closed gate.
  for (const code of ['E_FORBIDDEN', 'E_HUB_PROTOCOL', 'E_HUB_CONTRACT', 'E_HUB_UNREACHABLE', 'E_HUB_UNAVAILABLE']) {
    it(`falls back to the neutral note when the hub answers ${code}`, async () => {
      refuseWith(code);
      hubStatus.set(remote);
      render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
      await tick();
      await pickHost('mefistos');
      await vi.waitFor(() => expect(screen.getByTestId('wt-remote-unknown')).toBeTruthy());
      expect(worktreeLabels()).toEqual(['+ new worktree']);
      expect(screen.getByTestId('wt-remote-unknown').textContent).toContain('mefistos');
      // The neutral note is not the (red, error-only) `wt-status` line.
      expect(screen.queryByTestId('wt-status')).toBeNull();
    });
  }

  // The scan itself failing on the host is a real error and still reads as
  // one: it says what went wrong instead of blaming the hub.
  it('a scan that fails on the host still shows the error line, not the note', async () => {
    refuseWith('E_GIT_SETUP');
    hubStatus.set(remote);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(screen.getByTestId('wt-status')).toHaveTextContent('E_GIT_SETUP'));
    expect(screen.queryByTestId('wt-remote-unknown')).toBeNull();
  });

  it('"+ new worktree" is still selectable and Create still submits worktree_id: null / new_worktree', async () => {
    refuseWith('E_HUB_PROTOCOL');
    hubStatus.set(remote);
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: okRow() });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(screen.getByTestId('wt-remote-unknown')).toBeTruthy());
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('new');
    await fireEvent.input(screen.getByTestId('new-worktree-name'), { target: { value: 'fix-login-bug' } });
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(spy).toHaveBeenCalledOnce();
    expect(spy.mock.calls[0][0].host_alias).toBe('mefistos');
    expect(spy.mock.calls[0][0].worktree_id).toBeNull();
    expect(spy.mock.calls[0][0].new_worktree).toBe('fix-login-bug');
    spy.mockRestore();
  });

  it('standalone is untouched: the ipc is still called and the neutral note never shows', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toEqual(['main', 'feat', '+ new worktree']));
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_host_worktrees');
    expect(calls).toHaveLength(1);
    expect(screen.queryByTestId('wt-remote-unknown')).toBeNull();
  });
});

describe('NewSessionDialog: account headroom on the host chips', () => {
  const USAGE = { locale: 'en-GB', timeZone: 'UTC' };
  const M = 60;

  async function setup(over: { hosts?: unknown[] } = {}) {
    const { NOW, RESET_5H, ADMIN, WORK, GMAIL, host, snapshot } = await import('./hosts_fixture');
    const { accounts } = await import('./accounts');
    const { accountUsage } = await import('./account_usage_store');
    const five = (uuid: string, left: number, fetchedAgo = 2 * M) =>
      snapshot(uuid, {
        fetched_at: NOW - fetchedAgo,
        usage: {
          five_hour: { utilization: 100 - left, resets_at: RESET_5H },
          seven_day: { utilization: 2, resets_at: NOW + 3 * 86400 },
          seven_day_opus: null,
          seven_day_sonnet: null,
        },
      });
    hosts.set(
      (over.hosts as never) ?? [
        host('local', { account_uuid: GMAIL.uuid }),
        host('mefistos', { account_uuid: ADMIN.uuid }),
        host('claude-fleet-oci', { account_uuid: ADMIN.uuid }),
        host('claude-fleet-htz', { account_uuid: WORK.uuid }),
        host('nas', { account_uuid: null }),
        host('claude-fleet-trn', { account_uuid: GMAIL.uuid, reachable: false }),
        host('hidden-box', { account_uuid: 'acc-hidden', hidden: true }),
      ],
    );
    accounts.set([ADMIN, WORK, GMAIL]);
    accountUsage.set({
      [GMAIL.uuid]: five(GMAIL.uuid, 91), // fresh
      [ADMIN.uuid]: five(ADMIN.uuid, 8), // low
      [WORK.uuid]: five(WORK.uuid, 62, 14 * M), // stale
    });
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'refresh_account_usage') throw { code: 'E_RATE_LIMITED', message: 'floor' };
      return null;
    });
    return { NOW, five, accountUsage, ADMIN, WORK, GMAIL, inv };
  }

  const chip = (alias: string) => document.querySelector<HTMLButtonElement>(`.host-pick[data-alias="${alias}"]`)!;
  const chipUsage = (alias: string) => chip(alias).querySelector('[data-testid="chip-usage"]')?.textContent?.trim();
  const active = () => document.querySelector(".host-pick[aria-pressed='true']")?.getAttribute('data-alias');

  it('each chip shows % left and the reset; stale gets ~ and ◷; expired ? left; no account; offline', async () => {
    const { NOW, five, accountUsage, WORK } = await setup();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, ...USAGE } });
    await tick();
    expect(chipUsage('local')).toBe('91% left · resets 15:10');
    expect(chipUsage('mefistos')).toBe('▲ 8% left · resets 15:10');
    expect(chipUsage('claude-fleet-htz')).toBe('~62% left ◷ · resets 15:10');
    expect(chipUsage('nas')).toBe('no account');
    expect(chipUsage('claude-fleet-trn')).toBe('offline');
    expect(chip('hidden-box')).toBeNull();
    accountUsage.update((m) => ({ ...m, [WORK.uuid]: five(WORK.uuid, 62, 45 * M) }));
    await tick();
    expect(chipUsage('claude-fleet-htz')).toBe('? left');
    // The alias itself stays the chip's first line.
    expect(chip('local').querySelector('.alias')?.textContent).toBe('local');
  });

  it('the selected chip gets one full line below the row', async () => {
    const { NOW } = await setup();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, ...USAGE } });
    await tick();
    expect(active()).toBe('local');
    expect(screen.getByTestId('host-usage-line').textContent).toBe(
      'mj-janci@users.noreply.github.com · 5h 91% left, resets 15:10 · weekly 98% left · 2 min ago',
    );
    expect(screen.queryByTestId('host-usage-warning')).toBeNull();
    await fireEvent.click(chip('claude-fleet-htz'));
    await tick();
    expect(screen.getByTestId('host-usage-line').textContent).toBe(
      'm-janci@users.noreply.github.com · 5h ~62% left, resets 15:10 · weekly ~98% left · 14 min ago',
    );
  });

  it('a low account warns, naming the account and the other hosts sharing it — and never switches the host', async () => {
    const { NOW } = await setup();
    render(NewSessionDialog, {
      props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, initialHost: 'mefistos', ...USAGE },
    });
    await tick();
    expect(active()).toBe('mefistos');
    expect(screen.getByTestId('host-usage-warning').textContent).toBe(
      '▲ admin-janci@users.noreply.github.com has 8% of its 5-hour window left (resets 15:10). Also used by claude-fleet-oci.',
    );
    // Choosing the other low host on the same account: still no auto-switch.
    await fireEvent.click(chip('claude-fleet-oci'));
    await tick();
    await tick();
    expect(active()).toBe('claude-fleet-oci');
    expect(screen.getByTestId('host-usage-warning').textContent).toContain('Also used by mefistos.');
  });

  it('a host whose account drops to low while the dialog is open stays chosen', async () => {
    const { NOW, five, accountUsage, GMAIL } = await setup();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, ...USAGE } });
    await tick();
    expect(active()).toBe('local');
    accountUsage.update((m) => ({ ...m, [GMAIL.uuid]: five(GMAIL.uuid, 3) }));
    await tick();
    expect(active()).toBe('local');
    expect(screen.getByTestId('host-usage-warning').textContent).toContain('mj-janci@users.noreply.github.com has 3%');
  });

  it('opening refreshes usage for the visible hosts’ accounts, once each, and shows no error when refused', async () => {
    const { NOW, inv, ADMIN, WORK, GMAIL } = await setup();
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, ...USAGE } });
    await tick();
    await tick();
    const refreshed = inv.mock.calls
      .filter((c) => c[0] === 'refresh_account_usage')
      .map((c) => (c[1] as { args: { account_uuid: string } }).args.account_uuid)
      .sort();
    expect(refreshed).toEqual([ADMIN.uuid, GMAIL.uuid, WORK.uuid].sort());
    expect(get(toasts)).toHaveLength(0);
    expect(document.body.textContent).not.toContain('floor');
  });

  // #147: `refresh_account_usage` is local-only in
  // remote mode, and this onMount fetch-trigger is the same shape as
  // HostsView's — gated on `ownsTheFleet(...)`.
  const remote: HubStatus = {
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    client_mode: null,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  };

  it('does not fire refresh_account_usage on open on a hub client', async () => {
    const { NOW, inv } = await setup();
    hubStatus.set(remote);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, ...USAGE } });
    await tick();
    await tick();
    expect(inv.mock.calls.some((c) => c[0] === 'refresh_account_usage')).toBe(false);
    hubStatus.set({ ...STANDALONE });
  });

  it('standalone is untouched: opening still refreshes linked accounts', async () => {
    const { NOW, inv, ADMIN, WORK, GMAIL } = await setup();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, ...USAGE } });
    await tick();
    await tick();
    const refreshed = inv.mock.calls
      .filter((c) => c[0] === 'refresh_account_usage')
      .map((c) => (c[1] as { args: { account_uuid: string } }).args.account_uuid)
      .sort();
    expect(refreshed).toEqual([ADMIN.uuid, GMAIL.uuid, WORK.uuid].sort());
  });

  it('is about 520px wide', async () => {
    const { NOW } = await setup();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {}, clock: () => NOW, ...USAGE } });
    await tick();
    expect((screen.getByRole('dialog', { name: 'New session' }) as HTMLElement).style.width).toBe('520px');
  });
});

// #191: a hub-routed `new_session` sends no `call_id` (`HubBackend::new_session`
// in `src-tauri/src/backend/remote.rs`, deliberately — the hub has no tool to
// cancel by one), so aborting the local wait does not stop the hub from
// finishing the create; the session it was building appears anyway. In
// hub-client mode the dialog must not offer a cancel that lies.
describe('NewSessionDialog: Cancel creation while a hub-routed create is in flight (#191)', () => {
  const remote: HubStatus = {
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    client_mode: null,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  };

  it('local mode: Cancel creation still aborts the in-flight request (unchanged)', async () => {
    let capturedSignal: AbortSignal | undefined;
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockImplementation((_args, signal) => {
        capturedSignal = signal;
        return new Promise(() => {}) as any;
      });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const cancelBtn = screen.getByTestId('cancel-create');
    expect(cancelBtn.textContent).toBe('Cancel creation');
    expect(screen.queryByTestId('hub-create-note')).toBeNull();
    await fireEvent.click(cancelBtn);
    expect(capturedSignal?.aborted).toBe(true);
    spy.mockRestore();
  });

  it('hub-client mode: no cancel is offered, and the dialog says why', async () => {
    hubStatus.set(remote);
    let capturedSignal: AbortSignal | undefined;
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockImplementation((_args, signal) => {
        capturedSignal = signal;
        return new Promise(() => {}) as any;
      });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(screen.queryByTestId('cancel-create')).toBeNull();
    const note = screen.getByTestId('hub-create-note');
    expect(note.textContent).toContain('fleet.example.com');
    expect(note.textContent?.toLowerCase()).toContain("can't be cancelled");
    expect(note.textContent?.toLowerCase()).toContain('appear');
    // No hidden cancel either — nothing aborts the wait from this state.
    expect(capturedSignal?.aborted).toBe(false);
    spy.mockRestore();
  });

  it('hub-client mode: the dialog is still closable during creation, without pretending to cancel it', async () => {
    hubStatus.set(remote);
    let capturedSignal: AbortSignal | undefined;
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockImplementation((_args, signal) => {
        capturedSignal = signal;
        return new Promise(() => {}) as any;
      });
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const dlg = screen.getByRole('dialog') as HTMLDialogElement;
    dlg.dispatchEvent(new Event('cancel', { cancelable: true }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    // Closing the dialog is not a cancel: the in-flight request is untouched.
    expect(capturedSignal?.aborted).toBe(false);
    spy.mockRestore();
  });
});

// Closing the dialog (Escape / backdrop) during a creation never aborts the
// request in either mode — a late success already merges into the session
// store on its own. A late FAILURE used to have nowhere to go: the
// paragraph that would show `error` is gone with the dialog. That gap
// predates #191, but #191's new copy ("…will appear when it's ready")
// makes it worth closing: the promise should hold for failure too, not just
// success.
describe('NewSessionDialog: a creation that fails after the dialog is closed', () => {
  const remote: HubStatus = {
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    client_mode: null,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  };

  it('hub mode: a late failure after closing toasts once', async () => {
    hubStatus.set(remote);
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    let resolveCreate!: (v: unknown) => void;
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockImplementation(() => new Promise((r) => (resolveCreate = r as (v: unknown) => void)) as any);
    const { unmount } = render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    unmount();
    resolveCreate({ ok: false, error: { code: 'E_HUB_UNREACHABLE', message: 'connection reset' } });
    await tick();
    expect(get(toasts)).toHaveLength(1);
    expect(get(toasts)[0].code).toBe('E_HUB_UNREACHABLE');
    spy.mockRestore();
  });

  it('local mode: a late failure after closing toasts once (the same gap, not hub-specific)', async () => {
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    let resolveCreate!: (v: unknown) => void;
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockImplementation(() => new Promise((r) => (resolveCreate = r as (v: unknown) => void)) as any);
    const { unmount } = render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    unmount();
    resolveCreate({ ok: false, error: { code: 'E_SSH', message: 'connection reset' } });
    await tick();
    expect(get(toasts)).toHaveLength(1);
    expect(get(toasts)[0].code).toBe('E_SSH');
    spy.mockRestore();
  });

  it('a failure while the dialog is still open stays inline, no toast', async () => {
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockResolvedValue({ ok: false, error: { code: 'E_SSH', message: 'connection reset' } });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(document.body.textContent).toContain('connection reset');
    expect(get(toasts)).toHaveLength(0);
    spy.mockRestore();
  });

  it('local mode: clicking Cancel creation never toasts, closed or not', async () => {
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    let resolveCreate!: (v: unknown) => void;
    const spy = vi
      .spyOn(sessionsModule, 'newSessionAbortable')
      .mockImplementation(() => new Promise((r) => (resolveCreate = r as (v: unknown) => void)) as any);
    const { unmount } = render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    await fireEvent.click(screen.getByTestId('cancel-create'));
    unmount();
    resolveCreate({ ok: false, error: { code: 'E_CANCELLED', message: 'aborted' } });
    await tick();
    expect(get(toasts)).toHaveLength(0);
    spy.mockRestore();
  });
});
