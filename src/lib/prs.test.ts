import { describe, it, expect } from 'vitest';
import { prEvidenceLine } from './prs';

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
