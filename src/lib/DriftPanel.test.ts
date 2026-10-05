import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import DriftPanel from './DriftPanel.svelte';
import type { ChangesetView } from './assets_workspace';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const view: ChangesetView = {
  id: 12, kind: 'drift', summary: 'skill/w was edited on oci (catalog personal)', state: 'proposed', created_at: 1, commits: {}, undoable: false, catalogs: ['personal'],
  items: [
    { position: 0, grp: 'drift', catalog: 'personal', kind: 'skill', name: 'w', action: 'take_host', params: { host: 'oci', harness: 'claude' }, decider: 'rule', state: 'pending' },
    { position: 1, grp: 'drift', catalog: 'personal', kind: 'skill', name: 'w', action: 'restore', params: { host: 'oci', harness: 'claude' }, decider: 'rule', state: 'pending' },
  ],
};
const props = (o = {}) => ({ view, readOnly: false, busy: false, onapply: vi.fn(), ...o });
const P = '~/.claude/skills/w/SKILL.md';
const answer = (files: object[]) => invoke.mockResolvedValue({ host_alias: 'oci', harness: 'claude', files });

beforeEach(() => {
  invoke.mockReset();
  answer([{ path: P, catalog: 'a\nb\n', host: 'a\nX\n' }]);
});

describe('DriftPanel', () => {
  it('reads the two texts for the card host and shows their diff', async () => {
    render(DriftPanel, props());
    await screen.findByTestId(`drift-file-${P}`);
    expect(invoke).toHaveBeenCalledWith('catalog_drift_diff', { args: { host_alias: 'oci', kind: 'skill', name: 'w', harness: 'claude', catalog: null } });
    expect(screen.getByTestId(`drift-file-${P}`)).toHaveTextContent('X');
    // The path is shown as the planner gave it.
    expect(screen.getByTestId(`drift-file-${P}`)).toHaveTextContent(P);
  });
  it('takes the host version with the take position', async () => {
    const p = props();
    render(DriftPanel, p);
    await fireEvent.click(await screen.findByTestId('drift-take'));
    expect(p.onapply).toHaveBeenCalledWith([0]);
    expect(screen.getByTestId('drift-take')).toHaveTextContent("Take oci's version into personal");
  });
  it('restores only after a confirm that names the backup', async () => {
    const p = props();
    render(DriftPanel, p);
    await fireEvent.click(await screen.findByTestId('drift-restore'));
    expect(p.onapply).not.toHaveBeenCalled();
    expect(screen.getByTestId('confirm-dialog')).toHaveTextContent('a .fleet-bak copy is kept');
    await fireEvent.click(screen.getByTestId('confirm-restore'));
    expect(p.onapply).toHaveBeenCalledWith([1]);
  });
  it('cancelling the confirm applies nothing', async () => {
    const p = props();
    render(DriftPanel, p);
    await fireEvent.click(await screen.findByTestId('drift-restore'));
    await fireEvent.click(screen.getByRole('button', { name: /cancel/i }));
    expect(screen.queryByTestId('confirm-dialog')).toBeNull();
    expect(p.onapply).not.toHaveBeenCalled();
  });
  it('an MCP entry says its diff is not shown', async () => {
    invoke.mockResolvedValue({ host_alias: 'oci', harness: 'claude', files: [], merges_only: true });
    render(DriftPanel, props());
    expect(await screen.findByTestId('drift-note')).toHaveTextContent('lives in a config file');
  });
  it('an older hub is named', async () => {
    invoke.mockRejectedValue({ code: 'E_INVALID', message: 'unknown variant `drift_diff`' });
    render(DriftPanel, props());
    expect(await screen.findByTestId('drift-note')).toHaveTextContent('The hub is older than this desktop and cannot show this diff yet');
  });
  it('read-only: the diff without the buttons', async () => {
    render(DriftPanel, props({ readOnly: true }));
    await screen.findByTestId(`drift-file-${P}`);
    expect(screen.queryByTestId('drift-take')).toBeNull();
    expect(screen.queryByTestId('drift-restore')).toBeNull();
  });
  it('a rejected take leaves Restore only', async () => {
    const v = { ...view, items: [{ ...view.items[0], state: 'rejected' as const }, view.items[1]] };
    render(DriftPanel, props({ view: v }));
    await waitFor(() => expect(screen.queryByTestId('drift-take')).toBeNull());
    expect(screen.getByTestId('drift-restore')).toBeInTheDocument();
  });
  it('a file with a secret placeholder says so and is never "Identical."', async () => {
    answer([{ path: P, secret: true }]);
    render(DriftPanel, props());
    const f = await screen.findByTestId(`drift-file-${P}`);
    expect(f).toHaveTextContent('Contains a secret — its text is not shown.');
    expect(f).not.toHaveTextContent('Identical.');
  });
  it('copies that differ only in a final newline are not called identical', async () => {
    answer([{ path: P, catalog: 'a\nb\n', host: 'a\nb' }]);
    render(DriftPanel, props());
    const f = await screen.findByTestId(`drift-file-${P}`);
    expect(f).toHaveTextContent('Differs only in line endings or a final newline.');
    expect(f).not.toHaveTextContent('Identical.');
  });
  it('CRLF against LF is a difference too', async () => {
    answer([{ path: P, catalog: 'a\r\nb\r\n', host: 'a\nb\n' }]);
    render(DriftPanel, props());
    // Whatever the line diff makes of the \r, it is never "Identical.".
    const f = await screen.findByTestId(`drift-file-${P}`);
    expect(f).not.toHaveTextContent('Identical.');
  });
  it('equal texts are identical', async () => {
    answer([{ path: P, catalog: 'a\n', host: 'a\n' }]);
    render(DriftPanel, props());
    expect(await screen.findByTestId(`drift-file-${P}`)).toHaveTextContent('Identical.');
  });
  it('a binary file says the copies differ', async () => {
    answer([{ path: P, binary: true }]);
    render(DriftPanel, props());
    expect(await screen.findByTestId(`drift-file-${P}`)).toHaveTextContent('Binary file: the copies differ.');
  });
  it('a refreshed card does not read the files again', async () => {
    const r = render(DriftPanel, props());
    await screen.findByTestId(`drift-file-${P}`);
    await r.rerender(props({ view: { ...view, items: view.items.map((i) => ({ ...i })) } }));
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});
