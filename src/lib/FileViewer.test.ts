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
