import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FileViewer from './FileViewer.svelte';
import type { SessionRow } from './sessions';

const session = { id: 1, tmux_name: 'ctl', host_alias: 'local', kind: 'work' } as unknown as SessionRow;

async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
  await Promise.resolve();
  await tick();
}

beforeEach(() => {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd === 'repo_file') {
      return { path: 'a.ts', content: 'one\ntwo\nthree\nfour', truncated: false, binary: false, is_dir: false, size: 18 };
    }
    if (cmd === 'repo_diff') return { path: 'a.ts', diff: '', binary: false, truncated: false };
    if (cmd === 'repo_blame') {
      return {
        path: 'a.ts',
        truncated: false,
        hunks: [
          { start: 1, lines: 2, hash: 'a41c9e2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', author: 'Ada', time: 1, summary: 'first', uncommitted: false },
          { start: 3, lines: 1, hash: '0'.repeat(40), author: 'Not Committed Yet', time: 2, summary: '', uncommitted: true },
          { start: 4, lines: 1, hash: '7d02b11bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', author: 'Bob', time: 3, summary: 'later', uncommitted: false },
        ],
      };
    }
    return null;
  });
});

describe('FileViewer focusLine', () => {
  it('opens the File view, highlights the line and scrolls it into view', async () => {
    const scrolled: HTMLElement[] = [];
    (HTMLElement.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = function (this: HTMLElement) {
      scrolled.push(this);
    };
    render(FileViewer, { session, path: 'a.ts', status: 'M', reloadKey: 0, focusLine: 3 });
    await settle();
    const row = screen.getByTestId('file-focus-row');
    expect(row.textContent).toContain('3');
    expect(row.textContent).toContain('three');
    expect(scrolled).toContain(row);
  });

  it('no highlight without a focus line', async () => {
    render(FileViewer, { session, path: 'a.ts', status: undefined, reloadKey: 0 });
    await settle();
    expect(screen.queryByTestId('file-focus-row')).toBeNull();
  });

  it('a focus line never applies inside a commit view', async () => {
    render(FileViewer, { session, path: 'a.ts', status: 'M', reloadKey: 0, commit: 'abc123', focusLine: 3 });
    await settle();
    expect(screen.queryByTestId('file-focus-row')).toBeNull();
  });
});

// Step 5.7 (FilesTree board): Blame beside the File view.
describe('FileViewer blame', () => {
  it('labels the first line of each run and marks uncommitted lines', async () => {
    render(FileViewer, { props: { session, path: 'a.ts', status: 'modified', reloadKey: 0 } });
    await settle();
    await screen.getByTestId('blame-toggle').click();
    await settle();
    const labels = screen.getAllByTestId('blame-label').map((l) => l.textContent ?? '');
    expect(labels).toHaveLength(3);
    expect(labels[0]).toMatch(/^a41c9e2 Ada · /);
    expect(labels[1]).toBe('Not committed');
    expect(labels[2]).toMatch(/^7d02b11 Bob · /);
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    expect(inv.mock.calls.filter((c) => c[0] === 'repo_blame')).toHaveLength(1);
  });

  it('is off until asked for, and unavailable for an untracked file', async () => {
    render(FileViewer, { props: { session, path: 'a.ts', status: 'untracked', reloadKey: 0 } });
    await settle();
    expect(screen.getByTestId('blame-toggle')).toBeDisabled();
    expect(screen.queryByTestId('blame-label')).toBeNull();
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    expect(inv.mock.calls.some((c) => c[0] === 'repo_blame')).toBe(false);
  });

  it('is not offered inside a commit view', async () => {
    render(FileViewer, { props: { session, path: 'a.ts', status: 'modified', reloadKey: 0, commit: 'abc123' } });
    await settle();
    expect(screen.queryByTestId('blame-toggle')).toBeNull();
  });
});
