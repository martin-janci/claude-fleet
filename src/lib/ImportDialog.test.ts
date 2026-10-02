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

  /** `host` defaults to the literal `local`, which is not always an option: a
   *  hub with `hub.local_host=false`, and every Windows desktop, has no `local`
   *  host. Svelte leaves a bound value matching no `<option>` alone, so the
   *  select drew blank and Import sent `local` for the backend to refuse. */
  it('snaps the host to a real option when local is not one', async () => {
    hosts.set([
      { alias: 'pine', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
      { alias: 'hidden-one', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: true, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
    ]);
    invoke.mockResolvedValue({ created: [], problems: [], warnings: [], flagged_secrets: [], dry_run: true });
    render(ImportDialog, { props: { onclose: () => {}, ondone: () => {} } });

    await waitFor(() => expect((screen.getByTestId('import-host') as HTMLSelectElement).value).toBe('pine'));
    await fireEvent.click(screen.getByTestId('import-dry-run'));
    await waitFor(() => expect(invoke).toHaveBeenCalled());
    const args = invoke.mock.calls[0][1] as { args: { host_alias?: string; hostAlias?: string } };
    expect(JSON.stringify(args)).toContain('pine');
    expect(JSON.stringify(args)).not.toContain('local');
  });

  /** A remote import is an SSH round trip of up to 64 MiB, and its report
   *  carries "Secrets to replace" — the one thing to read before committing.
   *  Dismissing it mid-flight threw that away. */
  it('cannot be dismissed while a read is in flight', async () => {
    let release: (v: unknown) => void = () => {};
    invoke.mockImplementation(() => new Promise((res) => (release = res)));
    const onclose = vi.fn();
    render(ImportDialog, { props: { onclose, ondone: () => {} } });

    await fireEvent.click(screen.getByTestId('import-dry-run'));
    await waitFor(() => expect(screen.getByTestId('import-busy')).toBeTruthy());
    // Close is disabled, so a real click never reaches the handler. (jsdom
    // dispatches to a disabled button's listener regardless, so `disabled` is
    // the thing to assert, not a simulated click.)
    expect((screen.getByTestId('import-close') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('import-dry-run') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('import-host') as HTMLSelectElement).disabled).toBe(true);
    // And Escape / a backdrop click cannot close it either: the Modal is given
    // no `onclose` while a read is out.
    await fireEvent.keyDown(screen.getByTestId('import-dialog'), { key: 'Escape' });
    expect(onclose).not.toHaveBeenCalled();

    release({ created: [], problems: [], warnings: [], flagged_secrets: [], dry_run: true });
    await waitFor(() => expect((screen.getByTestId('import-close') as HTMLButtonElement).disabled).toBe(false));
  });
});
