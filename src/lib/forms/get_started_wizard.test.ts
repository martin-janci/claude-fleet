// Redesign 10.5 and 10.12: Get started runs on its own form spec, the same
// in Get started's dialog and in the chat. Its last button adds the project
// (unless one in the fleet was picked) and starts the first session.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ChatForm from './ChatForm.svelte';
import WizardDialog from './WizardDialog.svelte';
import { buildFirstFleet, getStartedSource, getStartedWizard, runGetStarted } from './get_started_wizard';
import { WIZARDS, type Wizard } from './wizards';
import { readOption } from './forms';
import { loadSaved } from './saved_answers';
import type { SessionRow } from '../sessions';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const fieldOf = (w: Wizard, name: string) => w.spec.steps.flatMap((s) => s.fields).find((f) => f.name === name);
const started = { id: 9, tmux_name: 'dev-acme-pos--first', friendly_name: null, host_alias: 'mercury', kind: 'work' } as unknown as SessionRow;
const project = (id: number) => ({ project: { id, owner: 'acme', repo: 'pos', base_path: '/p', last_session_at: null, adopted: false, system: false }, worktrees: [] });

beforeEach(() => {
  inv.mockReset();
  localStorage.clear();
});

describe('the Get started wizard', () => {
  it('is a fleet.form/1 spec with the Galaxy while it builds', () => {
    expect(WIZARDS.get_started.spec.spec).toBe('fleet.form/1');
    expect(WIZARDS.get_started.loader).toBe('galaxy');
    expect(WIZARDS.get_started.spec.submit).toBe('Build my fleet');
  });

  it('offers the fleet’s hosts, and a project in the fleet only when there is one', () => {
    const none = getStartedWizard({ projects: [], hosts: [{ alias: 'local' }, { alias: 'mercury' }] });
    expect(fieldOf(none, 'host')?.options).toEqual([
      ['local', 'This machine'],
      ['mercury', 'mercury'],
    ]);
    expect(fieldOf(none, 'source')?.options?.map((o) => readOption(o).value)).toEqual(['clone', 'folder']);
    expect(none.spec.steps.map((s) => s.title)).toEqual(['Host', 'Project', 'Agent', 'Check it']);
    const some = getStartedWizard({ projects: [{ id: 4, owner: 'acme', repo: 'pos' }], hosts: [] });
    expect(fieldOf(some, 'source')?.options?.map((o) => readOption(o).value)).toEqual(['existing', 'clone', 'folder']);
    expect(fieldOf(some, 'project')?.options).toEqual([['4', 'acme/pos']]);
    expect(fieldOf(some, 'host')?.options).toEqual([['local', 'This machine']]);
  });

  it('reads the project source from the answers', () => {
    expect(getStartedSource({ source: 'clone', url: ' acme/pos ' })).toEqual({ kind: 'clone', url: 'acme/pos' });
    expect(getStartedSource({ source: 'folder', path: '~/x' })).toEqual({ kind: 'folder', path: '~/x' });
    expect(getStartedSource({ source: 'existing', project: '4' })).toBeNull();
  });

  it('adds the project, then starts the first session in a new worktree', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'add_project' ? project(12) : cmd === 'new_session' ? started : null));
    const r = await buildFirstFleet({ host: 'mercury', source: 'clone', url: 'acme/pos', agent: 'codex', label: '' });
    expect(r.ok).toBe(true);
    const calls = inv.mock.calls.map(([c]) => c);
    expect(calls.indexOf('add_project')).toBeLessThan(calls.indexOf('new_session'));
    const ns = inv.mock.calls.find(([c]) => c === 'new_session')![1] as { args: Record<string, unknown> };
    expect(ns.args).toMatchObject({ host_alias: 'mercury', project_id: 12, worktree_id: null, agent: 'codex', kind: 'work' });
  });

  it('starts in a project already in the fleet without adding one', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'new_session' ? started : null));
    const r = await runGetStarted({ host: 'mercury', source: 'existing', project: '4', agent: 'claude' });
    expect(r).toEqual({ ok: true, summary: 'dev-acme-pos--first on mercury', starting: 'Starting dev-acme-pos--first on mercury' });
    expect(inv.mock.calls.map(([c]) => c)).not.toContain('add_project');
  });

  it('says the project was added when the session then fails', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'add_project') return project(12);
      if (cmd === 'new_session') throw { code: 'E_SSH', message: 'mercury did not answer' };
      return null;
    });
    const r = await buildFirstFleet({ host: 'mercury', source: 'clone', url: 'acme/pos', agent: 'claude' });
    expect(r.ok).toBe(false);
    expect(!r.ok && r.error).toMatch(/^The project was added; /);
  });

  it('the same spec runs as a dialog and as a chat form', async () => {
    const w = getStartedWizard({ projects: [], hosts: [{ alias: 'local' }] });
    const d = render(WizardDialog, { props: { wizard: w, run: vi.fn(), onclose: vi.fn() } });
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('Host');
    d.unmount();
    render(ChatForm, { props: { spec: w.spec, from: 'Control', onsubmit: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('form-step-title')).toHaveTextContent('Host'));
    expect(screen.queryByRole('dialog')).toBeNull();
    await fireEvent.click(screen.getByTestId('form-next'));
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('Project');
  });

  // G1.3: the wizard spec that uses the review step and Save and finish later.
  it('ends on a review with Edit links, and saves to finish later from the dialog', async () => {
    const w = getStartedWizard({ projects: [], hosts: [{ alias: 'local' }, { alias: 'mercury' }] });
    const run = vi.fn();
    const onclose = vi.fn();
    const d = render(WizardDialog, { props: { wizard: w, run, onclose } });
    expect(screen.getByTestId('form-step-chips')).toHaveTextContent('Review');
    await fireEvent.click(screen.getByTestId('form-field-host-mercury'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-url'), { target: { value: 'acme/pos' } });
    await fireEvent.click(screen.getByTestId('form-save-later'));
    // Closed without "Discard changes?": the answers are kept.
    expect(onclose).toHaveBeenCalled();
    expect(screen.queryByTestId('form-discard-ask')).toBeNull();
    expect(loadSaved('wizard:get_started')).toMatchObject({ host: 'mercury', source: 'clone', url: 'acme/pos' });
    d.unmount();

    render(WizardDialog, { props: { wizard: w, run, onclose: vi.fn() } });
    expect(screen.getByTestId('form-field-host-mercury')).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(screen.getByTestId('form-next'));
    expect(screen.getByTestId('form-field-url')).toHaveValue('acme/pos');
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-next'));
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('Check it');
    expect(screen.getByTestId('form-review-step-1')).toHaveTextContent('acme/pos');
    await fireEvent.click(screen.getByTestId('form-review-edit-0'));
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('Host');
    for (let i = 0; i < 3; i++) await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(run).toHaveBeenCalledWith(expect.objectContaining({ host: 'mercury', url: 'acme/pos' }));
    expect(loadSaved('wizard:get_started')).toBeNull();
  });
});
