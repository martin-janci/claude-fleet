// Redesign step 4.5, "Verified by": every 0.5.4 option of the New session
// dialog is still present. The list below is 0.5.4's
// NewSessionDialog.svelte (v0.5.4), field by field and option by option: the
// name and its re-roll, the type (Claude / Shell), Model, Effort and Login
// profile with their choices, the shell's start command, the host chips, the
// worktree picker with + new worktree, its branch and base, the tmux name,
// the cwd preview, Cancel and Create. The redesign may add (the agent
// picker, the account picker, Run: in background); it may not take away.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

import NewSessionDialog from './NewSessionDialog.svelte';
import { hosts } from './hosts';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { MODEL_OPTIONS, LAUNCH_EFFORT_OPTIONS } from './conversation';

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null, adopted: false, system: false },
  worktrees: [
    { id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' },
    { id: 12, project_id: 1, host_alias: 'local', name: 'feat', path: '/r/cf/.claude/worktrees/feat', branch: 'feat' },
  ],
};

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ] as never);
  localStorage.clear();
});

const options = (id: string) => Array.from((screen.getByTestId(id) as HTMLSelectElement).options).map((o) => o.value);

/** 0.5.4's dialog, walked: each step names what 0.5.4 offered and finds it. */
async function walkParityList(): Promise<string[]> {
  const seen: string[] = [];
  const has = (what: string, el: Element | null) => {
    expect(el, what).not.toBeNull();
    seen.push(what);
  };
  // Name, and the re-roll beside it.
  has('Name', screen.getByTestId('friendly-name'));
  has('Roll a new name', screen.getByTestId('reroll-name'));
  // Type: Claude and Shell.
  has('Type: Claude', screen.getByTestId('kind-work'));
  has('Type: Shell', screen.getByTestId('kind-shell'));
  // Model, Effort, Login profile, each with every 0.5.4 choice.
  expect(options('launch-model')).toEqual(['', ...MODEL_OPTIONS.filter((o) => o.value !== 'default').map((o) => o.value)]);
  has('Model', screen.getByTestId('launch-model'));
  expect(options('launch-effort')).toEqual(['', ...LAUNCH_EFFORT_OPTIONS.map((o) => o.value)]);
  has('Effort', screen.getByTestId('launch-effort'));
  has('Login profile', screen.getByTestId('launch-profile'));
  // Hosts: a chip per visible host.
  for (const alias of ['local', 'mefistos']) {
    has(`Host ${alias}`, document.querySelector(`.host-pick[data-alias="${alias}"]`));
  }
  // Worktree: every existing one, and + new worktree with its branch and base.
  const wt = screen.getByTestId('wt-picker');
  const labels = Array.from(wt.querySelectorAll('[role="option"]')).map((o) => o.textContent ?? '');
  for (const name of ['main', 'feat']) {
    expect(labels.some((l) => l.includes(name)), `worktree ${name} in ${labels.join(' | ')}`).toBe(true);
    seen.push(`Worktree ${name}`);
  }
  const newOpt = Array.from(wt.querySelectorAll<HTMLElement>('[role="option"]')).find((o) => /new worktree/i.test(o.textContent ?? ''));
  has('+ new worktree', newOpt ?? null);
  await fireEvent.click(newOpt!);
  await tick();
  has('new branch / worktree name', screen.getByTestId('new-worktree-name'));
  has('base branch', screen.getByTestId('new-worktree-base'));
  // tmux name and the cwd it will start in.
  has('tmux name', screen.getByTestId('new-session-name'));
  has('cwd preview', screen.getByTestId('path-preview'));
  // Shell: its start command.
  await fireEvent.click(screen.getByTestId('kind-shell'));
  await tick();
  has('start command', screen.getByTestId('start-command'));
  // Cancel and Create.
  has('Cancel', screen.getByRole('button', { name: 'Cancel' }));
  has('Create', screen.getByTestId('create-btn'));
  return seen;
}

const PARITY_054 = [
  'Name', 'Roll a new name', 'Type: Claude', 'Type: Shell', 'Model', 'Effort', 'Login profile',
  'Host local', 'Host mefistos', 'Worktree main', 'Worktree feat', '+ new worktree',
  'new branch / worktree name', 'base branch', 'tmux name', 'cwd preview', 'start command', 'Cancel', 'Create',
];

describe('New session: every 0.5.4 option is still present (step 4.5)', () => {
  it('walks the 0.5.4 list', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect(await walkParityList()).toEqual(PARITY_054);
  });
});
