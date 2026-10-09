// The FilesTree board: the Files tab's tree marks a changed file with its
// change letter, and every folder above it, so a change inside a closed
// folder is still seen.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import FileList from './FileList.svelte';

const tree = { entries: ['CLAUDE.md', 'scripts/hub-e2e.sh', 'scripts/repro-pair.sh', 'scripts/verify.sh', 'crates/fleet-core/src/lib.rs'], truncated: false };
const changes = [
  { path: 'scripts/hub-e2e.sh', status: 'modified', staged: false, orig_path: null },
  { path: 'scripts/repro-pair.sh', status: 'added', staged: false, orig_path: null },
];

describe('the files tree (FilesTree board)', () => {
  it('marks the folders that hold a change, and each changed file with its letter', async () => {
    render(FileList, {
      props: { mode: 'tree', changes, tree, loading: false, error: null, selectedPath: null, onSelect: () => {} },
    });
    // Closed: only the folder says something changed inside it.
    expect(screen.getAllByTestId('tree-dir-changed')).toHaveLength(1);
    expect(screen.queryByTestId('tree-badge')).toBeNull();
    await fireEvent.click(screen.getByText('scripts'));
    expect(screen.getAllByTestId('tree-badge').map((b) => b.textContent)).toEqual(['M', 'A']);
    // An unchanged folder carries no mark.
    expect(screen.getByText('crates').closest('button')!.querySelector('[data-testid="tree-dir-changed"]')).toBeNull();
  });
});
