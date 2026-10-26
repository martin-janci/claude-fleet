import { describe, it, expect } from 'vitest';
import { otherSessionPrs, prEvidenceLine, type PullRequestRow } from './prs';

describe('prEvidenceLine (G7.9, Main board: "15/15 checks · no reviews")', () => {
  const checks = (total: number, pending = 0, failing_total = 0, skipped = 0) => ({ total, pending, failing_total, skipped });

  it('counts passing checks of those that ran, then the review decision', () => {
    expect(prEvidenceLine({ checks: checks(15) })).toBe('15/15 checks · no reviews');
    expect(prEvidenceLine({ checks: checks(10, 2, 1, 1), review_decision: 'APPROVED' })).toBe('6/9 checks · approved');
  });

  it('leaves out checks when none ran, and says nothing without a reading', () => {
    expect(prEvidenceLine({ checks: checks(0), review_decision: 'CHANGES_REQUESTED' })).toBe('changes requested');
    expect(prEvidenceLine(null)).toBe('');
    expect(prEvidenceLine(undefined)).toBe('');
  });
});

describe('otherSessionPrs (the inspector lists every PR a session opened)', () => {
  const pr = (n: number, session_id: number | null, first_seen_at: number): PullRequestRow => ({
    id: n,
    url: `https://github.com/o/r/pull/${n}`,
    number: n,
    state: 'OPEN',
    session_id,
    first_seen_at,
    updated_at: first_seen_at,
  });

  it('keeps the session\'s own PRs, newest first, without the one its branch is on now', () => {
    const items = [pr(1, 7, 100), pr(2, 8, 150), pr(3, 7, 300), pr(4, 7, 200), pr(5, null, 400)];
    const got = otherSessionPrs(items, 7, 'https://github.com/o/r/pull/3').map((p) => p.number);
    expect(got).toEqual([4, 1]);
  });

  it('keeps them all when the session points at no PR now', () => {
    expect(otherSessionPrs([pr(1, 7, 100), pr(2, 7, 200)], 7, null).map((p) => p.number)).toEqual([2, 1]);
  });
});
