import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const calls: { cmd: string; args: unknown }[] = [];
// What the backend holds: the view reads both lists when it opens.
const backend: { items: unknown[]; downloads: unknown[]; fail: boolean } = { items: [], downloads: [], fail: false };
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args: unknown) => {
    calls.push({ cmd, args });
    if (cmd === 'list_library' && backend.fail) throw { code: 'E_HUB_UNREACHABLE', message: 'connection refused' };
    if (cmd === 'list_library') return { items: backend.items };
    if (cmd === 'list_downloads') return { downloads: backend.downloads, total_bytes: 0, max_total_bytes: 1, max_file_bytes: 1 };
    if (cmd === 'pick_attachments') return [{ path: '/Users/m/spec.pdf', name: 'spec.pdf', size: 12, kind: 'binary' }];
    if (cmd === 'upload_attachments') return ['/w/.claude-fleet-attachments/spec.pdf'];
    if (cmd === 'add_library_items')
      return {
        items: [
          { id: 3, at: 200, kind: 'upload', host_alias: 'mac', session_id: 1, session_name: 'fix-login', path: '/w/.claude-fleet-attachments/spec.pdf', name: 'spec.pdf', size: 12 },
        ],
      };
    throw { code: 'E_TEST', message: `no ${cmd}` };
  }),
}));
import LibraryView from './LibraryView.svelte';
import { libraryItems } from './library';
import { downloads } from './downloads';
import { sessions } from './sessions';
import { selectSession } from './selection';
import { session } from './hosts_fixture';
import { expectAccessible } from './a11y_check';

// Redesign step 9.7: Control's Library.

const focus = session('mac', 'fix-login');

beforeEach(() => {
  calls.length = 0;
  backend.items = [];
  backend.downloads = [];
  backend.fail = false;
  sessions.set([focus]);
});
afterEach(() => {
  sessions.set([]);
  libraryItems.set([]);
  downloads.set([]);
});

describe('LibraryView (step 9.7)', () => {
  it('lists a download from a session, under its host', async () => {
    backend.downloads = [
      { id: 7, at: 100, host_alias: 'mac', session_id: focus.id, session_name: 'fix-login', path: '/w/out/report.pdf', name: 'report.pdf', size: 2048, state: 'ready', source: 'agent' },
    ];
    const { getAllByTestId, getByText } = render(LibraryView);
    await waitFor(() => expect(getAllByTestId('library-row')).toHaveLength(1));
    const row = getAllByTestId('library-row')[0];
    expect(row.getAttribute('data-kind')).toBe('output');
    expect(row.textContent).toContain('Session output · fix-login');
    expect(getByText('mac')).toBeTruthy();
  });

  it('Upload… waits for a session in focus', () => {
    const { getByTestId } = render(LibraryView);
    expect((getByTestId('library-upload') as HTMLButtonElement).disabled).toBe(true);
  });

  it('Upload… puts the picked files beside the session in focus and lists them', async () => {
    selectSession(focus);
    const { getByTestId, getAllByTestId } = render(LibraryView);
    await fireEvent.click(getByTestId('library-upload'));
    await waitFor(() => expect(getAllByTestId('library-row')).toHaveLength(1));
    const up = calls.find((c) => c.cmd === 'upload_attachments')!.args as { args: { host_alias: string; session_name: string } };
    expect(up.args).toMatchObject({ host_alias: 'mac', session_name: 'fix-login' });
    const add = calls.find((c) => c.cmd === 'add_library_items')!.args as { args: unknown };
    expect(add.args).toEqual({
      kind: 'upload',
      session_id: focus.id,
      files: [{ path: '/w/.claude-fleet-attachments/spec.pdf', name: 'spec.pdf', size: 12 }],
    });
    expect(getAllByTestId('library-row')[0].getAttribute('data-kind')).toBe('upload');
  });

  it('filters, and passes the axe and audit checks', async () => {
    backend.items = [
      { id: 3, at: 200, kind: 'attachment', host_alias: 'mac', session_name: 'fix-login', path: '/w/a.md', name: 'a.md' },
    ];
    const { getByTestId, queryAllByTestId, container } = render(LibraryView);
    await fireEvent.change(getByTestId('library-type'), { target: { value: 'repos' } });
    expect(queryAllByTestId('library-row')).toHaveLength(0);
    expect(getByTestId('library-empty')).toBeTruthy();
    await fireEvent.change(getByTestId('library-type'), { target: { value: 'uploads' } });
    await waitFor(() => expect(queryAllByTestId('library-row')).toHaveLength(1));
    await expectAccessible(container);
  });
});

// Gap plan G3.10 (board MCViews): a sortable table, a grid, Link a repo.
describe('LibraryView table (G3.10)', () => {
  const item = (id: number, name: string, at: number, size?: number) => ({
    id,
    at,
    kind: 'upload',
    host_alias: 'mac',
    session_name: 'fix-login',
    path: `/w/${name}`,
    name,
    size,
  });
  const names = (rows: HTMLElement[]) => rows.map((r) => r.querySelector('.name')!.textContent);

  it('sorts by a header press: newest first, then Name A to Z and back, Size largest first', async () => {
    backend.items = [item(1, 'b.md', 100, 5), item(2, 'a.md', 300, 50), item(3, 'c.md', 200)];
    const { getAllByTestId, getByTestId, container } = render(LibraryView);
    await waitFor(() => expect(getAllByTestId('library-row')).toHaveLength(3));
    expect(names(getAllByTestId('library-row'))).toEqual(['a.md', 'c.md', 'b.md']);
    await fireEvent.click(getByTestId('library-sort-name'));
    expect(names(getAllByTestId('library-row'))).toEqual(['a.md', 'b.md', 'c.md']);
    expect(getByTestId('library-sort-name').closest('th')!.getAttribute('aria-sort')).toBe('ascending');
    await fireEvent.click(getByTestId('library-sort-name'));
    expect(names(getAllByTestId('library-row'))).toEqual(['c.md', 'b.md', 'a.md']);
    await fireEvent.click(getByTestId('library-sort-size'));
    // No size sorts last.
    expect(names(getAllByTestId('library-row'))).toEqual(['a.md', 'b.md', 'c.md']);
    await expectAccessible(container);
  });

  it('the grid shows the same rows as tiles, and the choice is kept', async () => {
    backend.items = [item(1, 'b.md', 100, 5)];
    const first = render(LibraryView);
    await fireEvent.click(first.getByTestId('library-layout-grid'));
    await waitFor(() => expect(first.getByTestId('library-grid')).toBeTruthy());
    expect(first.queryByTestId('library-table')).toBeNull();
    first.unmount();
    const again = render(LibraryView);
    await waitFor(() => expect(again.getByTestId('library-grid')).toBeTruthy());
    await fireEvent.click(again.getByTestId('library-layout-list'));
  });

  it('Link a repo… asks for Add project', async () => {
    const { addProjectRequest } = await import('./app_views');
    const { get } = await import('svelte/store');
    const { getByTestId } = render(LibraryView);
    await fireEvent.click(getByTestId('library-link-repo'));
    expect(get(addProjectRequest)).toEqual({ cloneUrl: undefined });
    addProjectRequest.set(null);
  });
});

// Gap plan G7.8 (board MCViews): folders, each row naming its host.
describe('LibraryView folders (G7.8)', () => {
  it('puts outputs and uploads in their own folders, counts today, and folds one shut', async () => {
    const now = Math.floor(Date.now() / 1000);
    backend.downloads = [
      { id: 7, at: now, host_alias: 'mac', session_id: focus.id, session_name: 'fix-login', path: '/w/out/r.pdf', name: 'r.pdf', size: 2, state: 'ready', source: 'agent' },
      { id: 8, at: 100, host_alias: 'nas', session_id: focus.id, session_name: 'fix-login', path: '/w/out/old.pdf', name: 'old.pdf', size: 2, state: 'ready', source: 'agent' },
    ];
    backend.items = [{ id: 3, at: 200, kind: 'upload', host_alias: 'mac', session_name: 'fix-login', path: '/w/a.md', name: 'a.md' }];
    const { getAllByTestId } = render(LibraryView);
    await waitFor(() => expect(getAllByTestId('library-row')).toHaveLength(3));
    const folders = getAllByTestId('library-folder');
    expect(folders.map((f) => f.getAttribute('data-folder'))).toEqual(['outputs', 'uploads']);
    expect(getAllByTestId('library-folder-count').map((c) => c.textContent)).toEqual(['today 1 of 2', '1']);
    expect(folders[0].querySelectorAll('.host-of')[1]?.textContent).toBe('nas');
    const toggle = getAllByTestId('library-folder-toggle')[0];
    await fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(getAllByTestId('library-row')).toHaveLength(1);
  });
});

// Review round 13: a failed read is never shown as an empty Library.
describe('LibraryView when the read fails (review r13)', () => {
  it('says it could not load, not "Nothing here yet", and Retry reads again', async () => {
    backend.fail = true;
    const { findByTestId, queryByTestId, getByTestId } = render(LibraryView);
    await findByTestId('library-load-error');
    expect(queryByTestId('library-empty')).toBeNull();
    expect(getByTestId('library-load-error-text').textContent).toBe("Couldn't reach the hub.");
    backend.fail = false;
    await fireEvent.click(getByTestId('library-load-error-retry'));
    await waitFor(() => expect(queryByTestId('library-load-error')).toBeNull());
    expect(getByTestId('library-empty')).toBeTruthy();
  });
});
