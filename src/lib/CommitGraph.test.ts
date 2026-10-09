import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import CommitGraph from './CommitGraph.svelte';
import type { Commit } from './history';

const commit: Commit = { hash: 'abc1234', shortHash: 'abc1234', parents: [], refs: [], author: 'm', date: '2026-10-09', subject: 'first' };

describe('CommitGraph', () => {
  it('names its glyph buttons by their action, even while blocked (review r11)', () => {
    const noop = () => {};
    render(CommitGraph, {
      props: { commits: [commit], selected: null, onSelect: noop, onCreateBranch: noop, onCheckoutCommit: noop, writeBlocked: 'Read-only on this host' },
    });
    expect(screen.getByRole('button', { name: 'Create branch from here' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Checkout this commit (detached)' })).toBeDisabled();
  });
});
