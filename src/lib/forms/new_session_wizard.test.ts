import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ChatForm from './ChatForm.svelte';
import WizardDialog from './WizardDialog.svelte';
import { newSessionArgs, newSessionWizard, runNewSession, NEW_WORKTREE } from './new_session_wizard';
import type { Wizard } from './wizards';
import type { SessionRow } from '../sessions';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const fieldOf = (w: Wizard, name: string) => w.spec.steps.flatMap((s) => s.fields).find((f) => f.name === name);

const CHOICES = {
  projects: [
    { id: 4, owner: 'acme', repo: 'papaya-pos' },
    { id: 7, owner: 'acme', repo: 'papaya-receipts' },
  ],
  hosts: [{ alias: 'local' }, { alias: 'mercury' }],
};

const started = (over: Partial<SessionRow> = {}) =>
  ({ id: 31, tmux_name: 'dev-acme-papaya-pos--fix-login', friendly_name: 'Fix login', host_alias: 'mercury', kind: 'work', claude_status: null, ...over }) as SessionRow;

/** Picks `v` for `name`, as chips or as a dropdown, whichever it drew. */
async function choose(name: string, v: string) {
  const chip = screen.queryByTestId(`form-field-${name}-${v}`);
  if (chip) await fireEvent.click(chip);
  else await fireEvent.change(screen.getByTestId(`form-field-${name}`), { target: { value: v } });
}

beforeEach(() => inv.mockReset());

// Redesign step 10.12: the New session wizard, the last of the six.
describe('the New session wizard', () => {
  it('opens with the fleet’s projects and hosts, and a new worktree unless it is given some', () => {
    const w = newSessionWizard(CHOICES);
    expect(fieldOf(w, 'project')?.options).toEqual([
      ['4', 'acme/papaya-pos'],
      ['7', 'acme/papaya-receipts'],
    ]);
    expect(fieldOf(w, 'host')?.options).toEqual([
      ['local', 'This machine'],
      ['mercury', 'mercury'],
    ]);
    expect(fieldOf(w, 'worktree')?.options).toEqual([[NEW_WORKTREE, 'New worktree']]);
    expect(fieldOf(w, 'agent')?.value).toBe('claude');
    expect(w.loader).toBe('pulse-sequence');

    const inOne = newSessionWizard({ ...CHOICES, worktrees: [{ id: 12, name: 'fix-login', branch: 'fix/login' }] });
    expect(fieldOf(inOne, 'worktree')?.options).toEqual([
      [NEW_WORKTREE, 'New worktree'],
      ['12', 'fix-login · fix/login'],
    ]);
  });

  it('reads the answers as the new_session call ⌘N makes', () => {
    expect(
      newSessionArgs({
        project: '4',
        host: 'mercury',
        agent: 'claude',
        worktree: NEW_WORKTREE,
        branch: 'Fix Login',
        base_branch: 'main',
        label: 'Fix login',
        model: 'opus',
        effort: 'default',
        profile: 'work',
      }),
    ).toEqual({
      args: {
        host_alias: 'mercury',
        project_id: 4,
        worktree_id: null,
        name: '',
        new_worktree: 'fix-login',
        base_branch: 'main',
        kind: 'work',
        agent: 'claude',
        start_command: null,
        friendly_name: 'Fix login',
        model: 'opus',
        effort: null,
        profile: 'work',
      },
    });
    // An existing worktree; a shell keeps its command and takes no launch options.
    expect(
      newSessionArgs({ project: '7', host: 'local', agent: 'shell', worktree: '12', start_command: 'npm run dev', model: 'opus' }),
    ).toMatchObject({
      args: { worktree_id: 12, new_worktree: null, base_branch: null, kind: 'shell', agent: 'shell', start_command: 'npm run dev', model: null },
    });
    // Codex takes no profile.
    expect(newSessionArgs({ project: '4', host: 'local', agent: 'codex', profile: 'work' })).toMatchObject({
      args: { kind: 'work', agent: 'codex', profile: null },
    });
  });

  it('refuses a profile the host could not hold, on its field', async () => {
    expect(newSessionArgs({ project: '4', host: 'local', agent: 'claude', profile: '../x' })).toEqual({
      problems: [{ field: 'profile', problem: 'Letters, digits, - and _, up to 32, starting with a letter or digit.' }],
    });
    expect(await runNewSession({ project: '4', host: 'local', agent: 'claude', profile: '../x' })).toMatchObject({ ok: false });
    expect(inv).not.toHaveBeenCalled();
  });

  it('starts nothing until the last button in the chat, then names the session with its Pulse', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'new_session' ? started() : null));
    const w = newSessionWizard(CHOICES);
    render(ChatForm, { props: { spec: w.spec, from: 'Control', sending: w.sending, onsubmit: runNewSession } });
    await choose('project', '4');
    await choose('host', 'mercury');
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-branch'), { target: { value: 'fix-login' } });
    await fireEvent.click(screen.getByTestId('form-next'));
    expect(inv).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect((await screen.findByTestId('chat-form-summary')).textContent).toBe('Fix login on mercury');
    expect(screen.getByTestId('chat-form-starting').textContent).toContain('Starting Fix login on mercury');
    expect(inv).toHaveBeenCalledWith(
      'new_session',
      expect.objectContaining({ args: expect.objectContaining({ host_alias: 'mercury', project_id: 4, new_worktree: 'fix-login' }) }),
    );
  });

  it('the same spec runs as a dialog', async () => {
    const run = vi.fn();
    render(WizardDialog, { props: { wizard: newSessionWizard(CHOICES), run, onclose: () => {} } });
    await choose('project', '7');
    await choose('host', 'local');
    await choose('agent', 'shell');
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-next'));
    // A shell asks what to run, not which model.
    expect(screen.queryByTestId('form-field-model')).toBeNull();
    expect(screen.getByTestId('form-field-start_command')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(run).toHaveBeenCalledWith(expect.objectContaining({ project: '7', host: 'local', agent: 'shell', worktree: NEW_WORKTREE }));
  });
});
