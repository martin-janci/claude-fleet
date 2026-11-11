// New worktree setup (docs/work-graph.md#new-worktree-setup): the project
// menu's dialog reads the setup, checks globs before Save and writes both.
import { render, fireEvent, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as realInvoke } from '@tauri-apps/api/core';
import WorktreeSetupDialog from './WorktreeSetupDialog.svelte';
import ProjectActionsMenu from './ProjectActionsMenu.svelte';
import { globError, parseGlobs } from './worktree_setup';

const invoke = realInvoke as unknown as ReturnType<typeof vi.fn>;

beforeEach(() => invoke.mockReset());

describe('worktree_setup', () => {
  it('reads one glob per line, trimmed, without blanks or repeats', () => {
    expect(parseGlobs(' .env \n\nconfig/*.local\n.env\n')).toEqual(['.env', 'config/*.local']);
  });
  it('refuses a glob that leaves the project folder, as the backend does', () => {
    for (const bad of ['/etc/passwd', '~/.ssh/id_rsa', '../x', 'a/../../b', '.git/config', 'c:\\x']) {
      expect(globError(bad), bad).not.toBeNull();
    }
    for (const ok of ['.env', '**/.env.local', 'a..b']) expect(globError(ok)).toBeNull();
  });
});

describe('WorktreeSetupDialog', () => {
  it('shows the saved setup and saves both fields', async () => {
    invoke.mockImplementation(async (cmd: string, a?: { args: Record<string, unknown> }) => {
      if (cmd !== 'worktree_setup' || !a) return null;
      return {
        project_id: 7,
        project: 'acme/api',
        copy: (a.args.copy as string[] | undefined) ?? ['.env'],
        command: (a.args.command as string | undefined) ?? 'pnpm install',
      };
    });
    const onclose = vi.fn();
    render(WorktreeSetupDialog, { props: { projectId: 7, title: 'acme/api', onclose } });
    const globs = screen.getByTestId('worktree-setup-globs') as HTMLTextAreaElement;
    await waitFor(() => expect(globs.value).toBe('.env'));
    expect((screen.getByTestId('worktree-setup-command') as HTMLInputElement).value).toBe('pnpm install');
    expect(invoke).toHaveBeenCalledWith('worktree_setup', { args: { project_id: 7 } });

    await fireEvent.input(globs, { target: { value: '.env\n../secrets' } });
    expect(screen.getByTestId('worktree-setup-glob-error').textContent).toContain('inside the project folder');
    expect((screen.getByTestId('worktree-setup-save') as HTMLButtonElement).disabled).toBe(true);

    await fireEvent.input(globs, { target: { value: '.env\nconfig/*.local' } });
    await fireEvent.input(screen.getByTestId('worktree-setup-command'), { target: { value: ' pnpm i ' } });
    await fireEvent.click(screen.getByTestId('worktree-setup-save'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(invoke).toHaveBeenLastCalledWith('worktree_setup', {
      args: { project_id: 7, copy: ['.env', 'config/*.local'], command: 'pnpm i' },
    });
  });

  it('asks before closing over typed globs, and closes at once when nothing changed', async () => {
    invoke.mockImplementation(async () => ({ project_id: 7, project: 'acme/api', copy: ['.env'], command: null }));
    const onclose = vi.fn();
    render(WorktreeSetupDialog, { props: { projectId: 7, title: 'acme/api', onclose } });
    const globs = screen.getByTestId('worktree-setup-globs') as HTMLTextAreaElement;
    await waitFor(() => expect(globs.value).toBe('.env'));
    await fireEvent.input(globs, { target: { value: '.env\nconfig/*.local' } });
    await fireEvent.click(screen.getByTestId('sheet-cancel'));
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByTestId('form-discard-ask')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('form-keep-editing'));
    await fireEvent.input(globs, { target: { value: '.env' } });
    await fireEvent.click(screen.getByTestId('sheet-cancel'));
    expect(onclose).toHaveBeenCalled();
  });

  it('says why when the setup cannot be read', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'worktree_setup') throw { code: 'E_FORBIDDEN', message: 'a person sets a project’s worktree setup' };
      return null;
    });
    render(WorktreeSetupDialog, { props: { projectId: 7, title: 'acme/api', onclose: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('worktree-setup-error').textContent).toContain('a person sets'));
    expect((screen.getByTestId('worktree-setup-save') as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('ProjectActionsMenu', () => {
  const base = {
    title: 'acme/api', pinned: false, hidden: false, groups: [], currentGroup: null, manualGroup: false,
    startIn: 'main' as const, onpin: vi.fn(), onhide: vi.fn(), ongroup: vi.fn(), onclose: vi.fn(),
  };
  it('offers New worktree setup… only when the picker passes it', async () => {
    const onsetup = vi.fn();
    const { unmount } = render(ProjectActionsMenu, { props: { ...base, onsetup } });
    await fireEvent.click(screen.getByTestId('project-worktree-setup'));
    expect(onsetup).toHaveBeenCalled();
    unmount();
    render(ProjectActionsMenu, { props: base });
    expect(screen.queryByTestId('project-worktree-setup')).toBeNull();
  });
});
