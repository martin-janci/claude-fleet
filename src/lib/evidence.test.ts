import { describe, it, expect } from 'vitest';
import cases from '../../crates/fleet-core/src/service/testdata/evidence_cases.json';
import { assess, describeReason, isStale, PR_EVIDENCE_STALE_SECS, type Reason } from './evidence';
import type { PrEvidence } from './sessions';

interface Case {
  name: string;
  now: number;
  pr_url: string | null;
  evidence: PrEvidence | null;
  checked_at: number | null;
  want: { verdict: string; reasons: Reason[] } | null;
}

describe('evidence assessment', () => {
  // The shared fixture: Rust's `service::evidence` runs the same cases.
  it.each((cases as Case[]).map((c) => [c.name, c] as const))('%s', (_name, c) => {
    const got = assess(c.pr_url, c.evidence, c.checked_at, c.now);
    if (c.want === null) {
      expect(got).toBeNull();
      return;
    }
    expect(got).not.toBeNull();
    expect(got!.verdict).toBe(c.want.verdict);
    expect(got!.reasons).toEqual(c.want.reasons);
    expect(got!.commit).toBe(c.evidence?.head_oid);
  });

  it('staleness matches the Rust threshold, strictly older', () => {
    expect(isStale(null, 10_000)).toBe(false);
    expect(isStale(10_000 - PR_EVIDENCE_STALE_SECS, 10_000)).toBe(false);
    expect(isStale(10_000 - PR_EVIDENCE_STALE_SECS - 1, 10_000)).toBe(true);
  });

  it('describes every reason in the fixture with the reading’s numbers', () => {
    const seen = new Set<Reason>();
    for (const c of cases as Case[]) {
      for (const r of c.want?.reasons ?? []) {
        seen.add(r);
        const text = describeReason(r, c.evidence, c.checked_at, c.now * 1000);
        expect(text.length).toBeGreaterThan(0);
        expect(text).not.toContain('undefined');
      }
    }
    expect(seen.size).toBe(17);
    const ev = (cases as Case[]).find((c) => c.name.startsWith('dirty worktree and failing'))!.evidence;
    expect(describeReason('checks_failing', ev, null)).toBe('Failing: rust (ubuntu-24.04), hub-headless');
    const unpushed = (cases as Case[]).find((c) => c.name.startsWith('unpushed'))!.evidence;
    expect(describeReason('unpushed', unpushed, null)).toBe('2 commits not pushed; CI and review describe 1490bc3');
  });
});
