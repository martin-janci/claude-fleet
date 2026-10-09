import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const calls: { cmd: string; args: unknown }[] = [];
// What the backend holds: the view reads both lists when it opens.
const backend: { items: unknown[]; downloads: unknown[] } = { items: [], downloads: [] };
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args: unknown) => {
    calls.push({ cmd, args });
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
    await fireEvent.click(getByTestId('library-filter-repos'));
    expect(queryAllByTestId('library-row')).toHaveLength(0);
    expect(getByTestId('library-empty')).toBeTruthy();
    await fireEvent.click(getByTestId('library-filter-uploads'));
    await waitFor(() => expect(queryAllByTestId('library-row')).toHaveLength(1));
    await expectAccessible(container);
  });
});
