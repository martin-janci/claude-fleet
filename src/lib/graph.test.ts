import { describe, it, expect } from 'vitest';
import { computeGraph, type GraphInput } from './graph';

const lin: GraphInput[] = [
  { hash: 'c', parents: ['b'] },
  { hash: 'b', parents: ['a'] },
  { hash: 'a', parents: [] },
];

const branchMerge: GraphInput[] = [
  { hash: 'm', parents: ['c', 'f'] }, // merge of main(c) and feature(f)
  { hash: 'c', parents: ['b'] },
  { hash: 'f', parents: ['b'] },
  { hash: 'b', parents: ['a'] },
  { hash: 'a', parents: [] },
];

describe('computeGraph', () => {
  it('keeps a linear history in one column', () => {
    const rows = computeGraph(lin);
    expect(rows.map((r) => r.column)).toEqual([0, 0, 0]);
    expect(rows[0].color).toBe(rows[2].color); // first-parent inherits color
  });

  it('opens a second lane for a branch and closes it at the fork point', () => {
    const rows = computeGraph(branchMerge);
    const byHash = Object.fromEntries(rows.map((r) => [r.hash, r]));
    // merge sits in lane 0; its second parent f occupies a new lane.
    expect(byHash['m'].column).toBe(0);
    expect(byHash['f'].column).toBeGreaterThan(0);
    // after the merge row, two lanes are live (c and f).
    expect(byHash['m'].lanesOut.filter((x) => x !== null).length).toBe(2);
    // b is the common ancestor — both lanes converge, so at/after b only one lane remains.
    expect(byHash['b'].lanesOut.filter((x) => x !== null).length).toBe(1);
  });

  it('handles an empty input', () => {
    expect(computeGraph([])).toEqual([]);
  });

  it('handles disjoint roots without crashing', () => {
    const rows = computeGraph([
      { hash: 'x', parents: [] },
      { hash: 'y', parents: [] },
    ]);
    expect(rows).toHaveLength(2);
    expect(rows[0].column).toBe(0);
  });
});
