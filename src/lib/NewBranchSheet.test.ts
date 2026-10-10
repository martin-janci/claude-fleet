// Gap plan G2.7, the FormsSession board's New branch (replacing the bare
// PromptDialog): where it starts, the name checked with a corrected one to
// take, Check it out now, and a failure that keeps the name.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewBranchSheet from './NewBranchSheet.svelte';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const br = (name: string, isCurrent = false, isRemote = false) => ({
  name,
  isCurrent,
  isRemote,
  upstream: null,
  ahead: 0,
  behind: 0,
  tipHash: '4e1a9c2f00',
  merged: false,
});
let createFails = false;

beforeEach(() => {
  createFails = false;
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'repo_branches') return [br('main'), br('fix/hub-e2e-windows', true), br('origin/main', false, true)];
    if (cmd === 'repo_create_branch') {
      if (createFails) throw { code: 'E_GIT', message: 'cannot lock ref' };
      return null;
    }
    return null;
  });
});

function mount(startPoint: string | null = null) {
  const ondone = vi.fn();
  const onclose = vi.fn();
  render(NewBranchSheet, { props: { sessionId: 7, startPoint, ondone, onclose } });
  return { ondone, onclose };
}
const nameInput = () => screen.getByTestId('new-branch-name') as HTMLInputElement;
const create = () => screen.getByTestId('new-branch-create') as HTMLButtonElement;

describe('NewBranchSheet', () => {
  it('starts from the checked-out branch, or from the picked commit', async () => {
    mount();
    await waitFor(() => expect(screen.getByText('From fix/hub-e2e-windows at 4e1a9c2.')).toBeTruthy());
  });

  it('names a picked commit as its start', () => {
    mount('abcdef1234');
    expect(screen.getByText('From abcdef1.')).toBeTruthy();
  });

  it('says what is wrong with a name on blur and offers the corrected one', async () => {
    mount();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('repo_branches', { args: { session_id: 7 } }));
    await fireEvent.input(nameInput(), { target: { value: 'fix/hub e2e windows' } });
    await fireEvent.blur(nameInput());
    expect(screen.getByTestId('new-branch-problem').textContent).toContain('cannot contain whitespace');
    expect(create().disabled).toBe(true);
    expect(screen.getByTestId('sheet-why').textContent).toBe('Fix the name first.');
    await fireEvent.click(screen.getByTestId('new-branch-suggestion'));
    expect(nameInput().value).toBe('fix/hub-e2e-windows-2');
    expect(create().disabled).toBe(false);
  });

  it('a name a branch already has is a problem too', async () => {
    mount();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('repo_branches', expect.anything()));
    await fireEvent.input(nameInput(), { target: { value: 'main' } });
    await fireEvent.blur(nameInput());
    await waitFor(() => expect(screen.getByTestId('new-branch-problem').textContent).toContain('already exists'));
    expect(screen.getByTestId('new-branch-suggestion').textContent).toBe('Use main-2?');
  });

  it('creates it, checked out unless unticked, and Enter in the name creates', async () => {
    const { ondone } = mount('abcdef1234');
    await fireEvent.input(nameInput(), { target: { value: 'feat/x' } });
    await fireEvent.click(screen.getByTestId('new-branch-checkout'));
    await fireEvent.keyDown(nameInput(), { key: 'Enter' });
    await waitFor(() => expect(ondone).toHaveBeenCalledWith('feat/x', false));
    expect(invoke).toHaveBeenCalledWith('repo_create_branch', {
      args: { session_id: 7, name: 'feat/x', start_point: 'abcdef1234', checkout: false },
    });
  });

  it('a failed create keeps the name and says why at the top', async () => {
    createFails = true;
    const { ondone } = mount();
    await fireEvent.input(nameInput(), { target: { value: 'feat/y' } });
    await fireEvent.click(create());
    await waitFor(() => expect(screen.getByTestId('new-branch-error').textContent).toContain('cannot lock ref'));
    expect(ondone).not.toHaveBeenCalled();
    expect(nameInput().value).toBe('feat/y');
  });
});
