import { describe, it, expect } from 'vitest';
import { laneColor } from './CommitGraph.svelte';
import { computeGraph } from './graph';

describe('laneColor', () => {
  it('is stable and wraps the palette', () => {
    expect(laneColor(0)).toBe(laneColor(0));
    expect(laneColor(0)).toBe(laneColor(10)); // 10-color palette wraps
    expect(laneColor(1)).not.toBe(laneColor(0));
  });
});

describe('graph + color integration', () => {
  it('assigns the merge commit and its branch parent distinct colors', () => {
    const rows = computeGraph([
      { hash: 'm', parents: ['c', 'f'] },
      { hash: 'c', parents: ['b'] },
      { hash: 'f', parents: ['b'] },
      { hash: 'b', parents: [] },
    ]);
    const m = rows.find((r) => r.hash === 'm')!;
    const f = rows.find((r) => r.hash === 'f')!;
    expect(m.color).not.toBe(f.color);
  });
});
