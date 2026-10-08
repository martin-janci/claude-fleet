// Step 10.9: what Jev's quick answer may move, and what it never may.
import { describe, it, expect } from 'vitest';
import { proposedN, quickOrder, quickProposal, risky } from './quick_answer';

const opts = [
  { n: 1, label: 'Jest' },
  { n: 2, label: 'Vitest' },
  { n: 3, label: "Yes, and don't ask again" },
];
const jev = (value: string, confidence_pct: number | null = 80) => ({ value, source: 'jev' as const, confidence_pct });

describe('quick answer', () => {
  it('moves the proposed option first and keeps the rest in order', () => {
    const r = quickOrder(opts, jev('o2'));
    expect(r.shown.map((o) => o.n)).toEqual([2, 1, 3]);
    expect(r.proposed?.n).toBe(2);
  });

  it('never moves a risky option, a permission, unsure, or a weak answer', () => {
    for (const [p, kind] of [[jev('o3'), 'input'], [jev('o2'), 'permission'], [jev('unsure'), 'input'], [jev('o2', 40), 'input'], [jev('o2', null), 'input']] as const) {
      const r = quickOrder(opts, p, kind);
      expect(r.proposed).toBeNull();
      expect(r.shown.map((o) => o.n)).toEqual([1, 2, 3]);
    }
    for (const l of ['Push to main', 'Force-push', 'rm -rf build', 'Deploy to prod', 'Always allow']) expect(risky(l), l).toBe(true);
    expect(risky('Pushover')).toBe(false);
  });

  it('reads the row’s quick-answer proposal and an option word', () => {
    expect(quickProposal({ proposals: [{ feature: 'start_project', value: 'p1', source: 'jev' }] })).toBeNull();
    expect(quickProposal({})).toBeNull();
    expect(quickProposal({ proposals: [{ feature: 'quick_answer', value: 'o2', source: 'jev' }] })?.value).toBe('o2');
    expect(quickProposal(null)).toBeNull();
    expect([proposedN('o2'), proposedN('o10'), proposedN('unsure')]).toEqual([2, null, null]);
  });
});
