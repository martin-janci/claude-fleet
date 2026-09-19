import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ImportDialog from './ImportDialog.svelte';
import { hosts } from './hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  invoke.mockReset();
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
  ]);
});

describe('ImportDialog', () => {
  it('renders a Warnings list from the report', async () => {
    invoke.mockResolvedValueOnce({
      created: [['agent', 'pm-review']],
      problems: [],
      warnings: [{ path: 'agents/PM Review.md', message: 'agent pm-review: installs under a new name; PM Review stays unmanaged' }],
      flagged_secrets: [],
      dry_run: true,
    });
    render(ImportDialog, { props: { onclose: () => {}, ondone: () => {} } });

    await fireEvent.click(screen.getByTestId('import-dry-run'));

    await waitFor(() => expect(screen.getByTestId('import-warnings')).toBeTruthy());
    expect(screen.getByTestId('import-warnings').textContent).toContain(
      'agent pm-review: installs under a new name; PM Review stays unmanaged',
    );
  });

  it('does not render a Warnings section when the report has none', async () => {
    invoke.mockResolvedValueOnce({
      created: [['skill', 'worktree']],
      problems: [],
      warnings: [],
      flagged_secrets: [],
      dry_run: true,
    });
    render(ImportDialog, { props: { onclose: () => {}, ondone: () => {} } });

    await fireEvent.click(screen.getByTestId('import-dry-run'));

    await waitFor(() => expect(screen.getByText('Would create 1')).toBeTruthy());
    expect(screen.queryByTestId('import-warnings')).toBeNull();
  });
});
