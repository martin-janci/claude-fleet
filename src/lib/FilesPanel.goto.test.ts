import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FilesPanel from './FilesPanel.svelte';
import type { SessionRow } from './sessions';
import { goToFileMatches } from './files';
import { sessionView } from './prefs';
import { destination } from './destination';
import { composerInsert } from './conversation';
import { expectAccessible } from './a11y_check';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const TREE = ['README.md', 'src/lib/FilesPanel.svelte', 'src/lib/files.ts', 'scripts/hub-e2e.sh'];

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, payload: { args: Record<string, unknown> }) => {
    switch (cmd) {
      case 'repo_changes':
        return [{ path: 'README.md', status: 'modified', staged: false, orig_path: null }];
      case 'repo_branch_diff':
        throw { code: 'E_HUB_PROTOCOL', message: 'tool not found' };
      case 'repo_tree':
        return { entries: TREE, truncated: false };
      case 'repo_diff':
        return { path: payload.args.path, diff: '', binary: false, truncated: false };
      case 'repo_file':
        return { path: payload.args.path, content: 'x', truncated: false, binary: false, is_dir: false, size: 1 };
      default:
        return null;
    }
  });
});

// ⌥⌘P off a Mac is Ctrl+Alt+P; jsdom's navigator is not a Mac.
const chord = () =>
  fireEvent.keyDown(window, { key: 'p', code: 'KeyP', ctrlKey: true, altKey: true });

describe('goToFileMatches', () => {
  it('ranks a match in the file name first, then by path length', () => {
    expect(goToFileMatches(TREE, 'files')).toEqual(['src/lib/files.ts', 'src/lib/FilesPanel.svelte']);
  });
  it('matches a subsequence across folders, and drops non-matches', () => {
    expect(goToFileMatches(TREE, 'slfp')).toEqual(['src/lib/FilesPanel.svelte']);
    expect(goToFileMatches(TREE, 'zzz')).toEqual([]);
  });
  it('an empty query lists the first paths', () => {
    expect(goToFileMatches(TREE, '  ', 2)).toEqual(['README.md', 'src/lib/FilesPanel.svelte']);
  });
});

// Step 5.6 (Files board): Go to file on the Files tab.
describe('FilesPanel Go to file', () => {
  it('the chord opens the picker; Enter opens the best match in the tree', async () => {
    render(FilesPanel, { props: { session: { id: 1 } as SessionRow } });
    await screen.findByText('README.md');
    await chord();
    const input = await screen.findByTestId('go-to-file-input');
    await waitFor(() => expect(screen.getAllByTestId('go-to-file-row').length).toBe(4));
    await expectAccessible(screen.getByTestId('go-to-file'));
    await fireEvent.input(input, { target: { value: 'hube2e' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.queryByTestId('go-to-file')).toBeNull();
    await waitFor(() =>
      expect(invoke.mock.calls.some((c) => c[0] === 'repo_file' && c[1].args.path === 'scripts/hub-e2e.sh')).toBe(true),
    );
  });
});

describe('FileViewer actions (new layout)', () => {
  it('Mention in chat puts @path in the composer and goes to the conversation', async () => {
    sessionView.set('terminal');
    destination.set('files');
    render(FilesPanel, { props: { session: { id: 7, host_alias: 'local', tmux_name: 'x' } as SessionRow } });
    await fireEvent.click(await screen.findByText('README.md'));
    await fireEvent.click(await screen.findByTestId('viewer-mention'));
    expect(get(composerInsert)?.draft).toContain('@README.md');
    expect(get(sessionView)).toBe('conversation');
    expect(get(destination)).toBe('session');
    expect(screen.getByTestId('viewer-copy-path')).toBeTruthy();
    expect(screen.getByTestId('viewer-open-editor')).toBeTruthy();
    // An empty diff has nothing to ask about.
    expect(screen.queryByTestId('viewer-ask-diff')).toBeNull();
  });

  it('Ask Claude about this drafts a message about the shown diff (G7.10)', async () => {
    const base = invoke.getMockImplementation() as (cmd: string, payload: unknown) => Promise<unknown>;
    invoke.mockImplementation(async (cmd: string, payload: { args: Record<string, unknown> }) =>
      cmd === 'repo_diff'
        ? { path: payload.args.path, diff: '@@ -1 +1 @@\n-a\n+b\n', binary: false, truncated: false }
        : base(cmd, payload),
    );
    sessionView.set('terminal');
    destination.set('files');
    render(FilesPanel, { props: { session: { id: 8, host_alias: 'local', tmux_name: 'x' } as SessionRow } });
    await fireEvent.click(await screen.findByText('README.md'));
    const ask = await screen.findByTestId('viewer-ask-diff');
    expect(ask.textContent).toBe('Ask Claude about this');
    await fireEvent.click(ask);
    expect(get(composerInsert)).toMatchObject({ sessionId: 8 });
    expect(get(composerInsert)?.draft).toContain('About the diff of @README.md in the working tree: ');
    expect(get(sessionView)).toBe('conversation');
    expect(get(destination)).toBe('session');
  });

});
