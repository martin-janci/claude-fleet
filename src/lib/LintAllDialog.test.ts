import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import LintAllDialog from './LintAllDialog.svelte';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => invoke.mockReset());

describe('LintAllDialog', () => {
  it('loads catalog_lint_all on mount and shows counts, problems and per-asset findings', async () => {
    invoke.mockResolvedValueOnce({
      errors: 1,
      warnings: 1,
      problems: [{ path: 'hooks/bad.yaml', message: 'name invalid' }],
      assets: [
        { kind: 'skill', name: 'worktree', report: { errors: [{ field: 'body', message: 'body.md must not be empty' }], warnings: [] } },
        { kind: 'agent', name: 'reviewer', report: { errors: [], warnings: [{ field: 'tools', message: 'no tools are allowed' }] } },
        { kind: 'hook', name: 'clean', report: { errors: [], warnings: [] } },
      ],
    });
    render(LintAllDialog, { onclose: () => {}, onselect: () => {} });

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_lint_all', undefined));
    expect(await screen.findByTestId('lint-all-counts')).toBeTruthy();
    expect(screen.getByTestId('lint-all-counts').textContent).toContain('1 errors');
    expect(screen.getByTestId('lint-all-counts').textContent).toContain('1 warnings');
    expect(screen.getByTestId('lint-all-asset-skill-worktree').textContent).toContain('body.md must not be empty');
    expect(screen.getByTestId('lint-all-asset-agent-reviewer').textContent).toContain('no tools are allowed');
    expect(screen.queryByTestId('lint-all-asset-hook-clean')).toBeNull();
  });

  it('clicking an asset link calls onselect with kind and name', async () => {
    invoke.mockResolvedValueOnce({
      errors: 1, warnings: 0, problems: [],
      assets: [{ kind: 'skill', name: 'worktree', report: { errors: [{ field: 'body', message: 'empty' }], warnings: [] } }],
    });
    const onselect = vi.fn();
    render(LintAllDialog, { onclose: () => {}, onselect });

    await fireEvent.click(await screen.findByTestId('lint-all-select-skill-worktree'));
    expect(onselect).toHaveBeenCalledWith('skill', 'worktree');
  });

  it('shows the backend error on failure', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_CATALOG_NOT_CONFIGURED', message: 'configure the catalog repo first' });
    render(LintAllDialog, { onclose: () => {}, onselect: () => {} });
    expect(await screen.findByTestId('lint-all-error')).toBeTruthy();
  });
});
