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

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
  ]);
  localStorage.clear();
});

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
  worktrees: [{ id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' }],
};

describe('NewSessionDialog', () => {
  it('renders one host-pick button per non-hidden host', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const picks = document.querySelectorAll('.host-pick');
    expect(picks).toHaveLength(2);
    expect(Array.from(picks).map((p) => p.textContent?.trim())).toEqual(['local', 'mefistos']);
  });

  it('defaults to last-host pref (local on first run)', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const active = document.querySelector('.host-pick.active');
    expect(active?.textContent?.trim()).toBe('local');
  });

  it('clicking a host pick + Create sends host_alias to new_session', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'new_session') {
        return { id: 99, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 1, worktree_id: null, created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null, claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null, friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null };
      }
      if (cmd === 'list_sessions') return [];
      return null;
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => p.textContent?.trim() === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
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
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
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
        safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
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
        safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
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
        safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
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
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
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
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
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
        friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
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
    ci_status: null,
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
        { id: 12, project_id: 1, name: 'blue-sirius', path: '/r/cf/.worktrees/blue-sirius', branch: 'blue-sirius' },
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
    sessions.set([okRow({ id: 7, tmux_name: 'dev-martin-janci-claude-fleet', worktree_id: 11 })]);
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const friendly = (screen.getByTestId('friendly-name') as HTMLInputElement).value;
    expect((screen.getByTestId('new-session-name') as HTMLInputElement).value)
      .toBe(`dev-martin-janci-claude-fleet--${slugOf(friendly)}`);
    // …but only on the same host.
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => p.textContent?.trim() === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
    await tick();
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

  it('initialName pre-fills the friendly name (quick switcher hand-off)', async () => {
    render(NewSessionDialog, { props: { project, initialName: 'Fix the login bug', onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect((screen.getByTestId('friendly-name') as HTMLInputElement).value).toBe('Fix the login bug');
  });

  it('remembers host, worktree mode and kind per project after a successful create', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: okRow() });
    const { unmount } = render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => p.textContent?.trim() === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
    await fireEvent.click(screen.getByTestId('kind-shell'));
    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(JSON.parse(localStorage.getItem('cf:pref:newsession.project.1')!)).toEqual({
      host: 'mefistos', worktree: 'new', kind: 'shell',
    });
    unmount();
    // Re-open: the remembered choices are applied.
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect(document.querySelector('.host-pick.active')?.textContent?.trim()).toBe('mefistos');
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
        id: 100 + i, project_id: 1, name: `wt-${i}`, path: `/r/cf/.worktrees/wt-${i}`, branch: `wt-${i}`,
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
