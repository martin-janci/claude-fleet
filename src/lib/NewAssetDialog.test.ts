import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewAssetDialog from './NewAssetDialog.svelte';
import { catalog } from './assets';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  invoke.mockReset();
  catalog.set({
    head: 'h', loaded_at: 1, problems: [], unmanaged: [],
    assets: [
      { kind: 'skill', name: 'worktree', version: '1', description: 'd', tags: [], hosts: [] },
      { kind: 'skill', name: 'other-skill', version: '1', description: 'd', tags: [], hosts: [] },
      { kind: 'agent', name: 'reviewer', version: '1', description: 'd', tags: [], hosts: [] },
    ],
  });
});

describe('NewAssetDialog', () => {
  it('lists duplicate-from candidates scoped to the selected kind', async () => {
    render(NewAssetDialog, { onclose: () => {}, onsaved: () => {} });
    const options = Array.from((screen.getByTestId('new-asset-duplicate-from') as HTMLSelectElement).options).map((o) => o.value);
    expect(options).toEqual(['', 'worktree', 'other-skill']);

    await fireEvent.change(screen.getByTestId('new-asset-kind'), { target: { value: 'agent' } });
    const optionsAfter = Array.from((screen.getByTestId('new-asset-duplicate-from') as HTMLSelectElement).options).map((o) => o.value);
    expect(optionsAfter).toEqual(['', 'reviewer']);
  });

  it('Create is disabled until a valid kebab-case name is entered', async () => {
    render(NewAssetDialog, { onclose: () => {}, onsaved: () => {} });
    expect(screen.getByTestId('new-asset-create')).toBeDisabled();

    await fireEvent.input(screen.getByTestId('new-asset-name'), { target: { value: 'Bad Name' } });
    expect(screen.getByTestId('new-asset-create')).toBeDisabled();
    expect(screen.getByTestId('new-asset-name-error')).toBeTruthy();

    await fireEvent.input(screen.getByTestId('new-asset-name'), { target: { value: 'my-new-skill' } });
    expect(screen.getByTestId('new-asset-create')).not.toBeDisabled();
  });

  it('Create calls catalog_create_asset with duplicate_from null when none chosen, then onsaved(kind, name)', async () => {
    invoke.mockResolvedValueOnce({ commit: 'sha1', lint: { errors: [], warnings: [] } });
    const onsaved = vi.fn();
    render(NewAssetDialog, { onclose: () => {}, onsaved });

    await fireEvent.input(screen.getByTestId('new-asset-name'), { target: { value: 'my-new-skill' } });
    await fireEvent.click(screen.getByTestId('new-asset-create'));

    await waitFor(() => expect(onsaved).toHaveBeenCalledWith('skill', 'my-new-skill'));
    expect(invoke).toHaveBeenCalledWith('catalog_create_asset', { args: { kind: 'skill', name: 'my-new-skill', duplicate_from: null } });
  });

  it('Create passes duplicate_from when chosen', async () => {
    invoke.mockResolvedValueOnce({ commit: 'sha1', lint: { errors: [], warnings: [] } });
    const onsaved = vi.fn();
    render(NewAssetDialog, { onclose: () => {}, onsaved });

    await fireEvent.input(screen.getByTestId('new-asset-name'), { target: { value: 'copy-of-worktree' } });
    await fireEvent.change(screen.getByTestId('new-asset-duplicate-from'), { target: { value: 'worktree' } });
    await fireEvent.click(screen.getByTestId('new-asset-create'));

    await waitFor(() => expect(onsaved).toHaveBeenCalledWith('skill', 'copy-of-worktree'));
    expect(invoke).toHaveBeenCalledWith('catalog_create_asset', { args: { kind: 'skill', name: 'copy-of-worktree', duplicate_from: 'worktree' } });
  });

  it('shows the backend error and does not call onsaved on failure', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_ASSET_EXISTS', message: 'skill worktree already exists' });
    const onsaved = vi.fn();
    render(NewAssetDialog, { onclose: () => {}, onsaved });

    await fireEvent.input(screen.getByTestId('new-asset-name'), { target: { value: 'worktree' } });
    await fireEvent.click(screen.getByTestId('new-asset-create'));

    await waitFor(() => expect(screen.getByTestId('new-asset-error')).toBeTruthy());
    expect(screen.getByTestId('new-asset-error').textContent).toContain('already exists');
    expect(onsaved).not.toHaveBeenCalled();
  });
});
