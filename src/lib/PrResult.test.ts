// The Result card (result evidence phase 2): the row's own reading, assessed
// by `evidence.ts`, drawn as a verdict, the commit, the age and the reasons.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import PrResult from './PrResult.svelte';
import { session, NOW } from './hosts_fixture';
import type { PrEvidence } from './sessions';

const HEAD = '1490bc3a9275fba9c26757c531b7702f5a68df8c';
const ev = (over: Partial<PrEvidence> = {}): PrEvidence => ({
  head_oid: HEAD,
  local_head: HEAD,
  ahead: 0,
  dirty: false,
  draft: false,
  state: 'OPEN',
  checks: { total: 3, pending: 0, skipped: 0, failing_total: 0 },
  ...over,
});
const row = (over = {}) =>
  session('mefistos', 'pay', { pr_url: 'https://github.com/o/r/pull/1', pr_checked_at: NOW - 120, ...over });

describe('PrResult', () => {
  it('draws nothing without a PR', () => {
    render(PrResult, { session: session('mefistos', 'pay'), nowSec: NOW });
    expect(screen.queryByTestId('pr-result')).toBeNull();
  });

  it('a green, pushed PR is Ready for its head commit', () => {
    render(PrResult, { session: row({ pr_evidence: ev() }), nowSec: NOW });
    expect(screen.getByTestId('pr-result')).toHaveAttribute('data-verdict', 'ready');
    expect(screen.getByTestId('pr-result-verdict')).toHaveTextContent('Done · ready to merge');
    expect(screen.getByTestId('pr-result')).toHaveTextContent('1490bc3');
    expect(screen.getByTestId('pr-result-checked')).toHaveTextContent('checked 2m ago');
    expect(screen.getByTestId('pr-result-reason')).toHaveTextContent('Checks passed for 1490bc3');
  });

  it('lists every reason and links the failing checks', () => {
    const failing = { total: 3, pending: 0, skipped: 0, failing_total: 1, failing: [{ name: 'rust', url: 'https://github.com/o/r/actions/runs/9' }] };
    render(PrResult, { session: row({ pr_evidence: ev({ dirty: true, checks: failing }) }), nowSec: NOW });
    expect(screen.getByTestId('pr-result')).toHaveAttribute('data-verdict', 'blocked');
    const reasons = screen.getAllByTestId('pr-result-reason').map((li) => li.getAttribute('data-reason'));
    expect(reasons).toEqual(['dirty', 'checks_failing']);
    const link = screen.getByRole('link', { name: 'rust' });
    expect(link).toHaveAttribute('href', 'https://github.com/o/r/actions/runs/9');
  });

  it('an old reading is Unknown and says so first, keeping the last-known reasons', () => {
    render(PrResult, { session: row({ pr_evidence: ev(), pr_checked_at: NOW - 3600 }), nowSec: NOW });
    expect(screen.getByTestId('pr-result')).toHaveAttribute('data-verdict', 'unknown');
    const items = screen.getAllByTestId('pr-result-reason');
    expect(items[0]).toHaveTextContent('Not checked since 1h ago');
    expect(items[1]).toHaveTextContent('Checks passed');
  });

  it('a PR the probe observed without evidence (an old gh) is Unknown, not passing', () => {
    render(PrResult, { session: row({ pr_evidence: null, ci_status: 'passing' }), nowSec: NOW });
    expect(screen.getByTestId('pr-result')).toHaveAttribute('data-verdict', 'unknown');
    expect(screen.getByTestId('pr-result-reason')).toHaveTextContent('No evidence yet');
  });

  it('draws nothing when there is no reading at all (a hub older than 082)', () => {
    render(PrResult, { session: row({ pr_evidence: undefined, pr_checked_at: undefined, ci_status: 'passing' }), nowSec: NOW });
    expect(screen.queryByTestId('pr-result')).toBeNull();
  });
});
