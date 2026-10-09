// What stands between a session's PR and a merge: the desktop's mirror of
// Rust `service::evidence` (design docs/specs/2026-09-29-result-evidence-design.md §5).
//
// The row already carries the reading (`pr_evidence`, `pr_checked_at`), so the
// card assesses it here with no round trip, the same against a hub. The rule
// is Rust's; this file is held to it by the shared fixture
// `crates/fleet-core/src/service/testdata/evidence_cases.json`, which both test
// suites run case by case.

import type { PrEvidence, SessionRow } from './sessions';
import { timeAgo } from './session_status';
import type { OfState } from './kit/status';

/** Mirrors Rust `outcome::PR_EVIDENCE_STALE_SECS` (3 × the 300 s probe TTL). */
export const PR_EVIDENCE_STALE_SECS = 900;

export type Verdict = 'ready' | 'waiting' | 'unknown' | 'blocked' | 'merged' | 'closed';

export type Reason =
  | 'stale'
  | 'no_evidence'
  | 'merged'
  | 'closed'
  | 'dirty'
  | 'unpushed'
  | 'head_mismatch'
  | 'checks_failing'
  | 'checks_pending'
  | 'no_checks'
  | 'draft'
  | 'changes_requested'
  | 'review_required'
  | 'merge_conflicts'
  | 'behind'
  | 'merge_blocked'
  | 'checks_passed';

export interface Assessment {
  verdict: Verdict;
  /** Never empty; most decisive first. */
  reasons: Reason[];
  /** The PR's head commit, the one the verdict is about. */
  commit?: string;
  checked_at?: number;
}

const REASON_VERDICT: Record<Reason, Verdict> = {
  stale: 'unknown',
  no_evidence: 'unknown',
  head_mismatch: 'unknown',
  merged: 'merged',
  closed: 'closed',
  dirty: 'blocked',
  unpushed: 'blocked',
  checks_failing: 'blocked',
  changes_requested: 'blocked',
  merge_conflicts: 'blocked',
  checks_pending: 'waiting',
  no_checks: 'waiting',
  draft: 'waiting',
  review_required: 'waiting',
  behind: 'waiting',
  merge_blocked: 'waiting',
  checks_passed: 'ready',
};

const RANK: Record<Verdict, number> = { ready: 0, waiting: 1, unknown: 2, blocked: 3, merged: 4, closed: 4 };

/** A reading stamped `checkedAt` describes the past at `nowSec`. No stamp is not stale: it is `no_evidence`. */
export function isStale(checkedAt: number | null | undefined, nowSec: number): boolean {
  return checkedAt != null && nowSec - checkedAt > PR_EVIDENCE_STALE_SECS;
}

/** Assess one PR. `null` when there is no PR: nothing to assess, which is not "unknown". */
export function assess(
  prUrl: string | null | undefined,
  ev: PrEvidence | null | undefined,
  checkedAt: number | null | undefined,
  nowSec: number,
): Assessment | null {
  if (!prUrl) return null;
  const at = checkedAt ?? undefined;
  if (!ev) return { verdict: 'unknown', reasons: ['no_evidence'], checked_at: at };
  const reasons: Reason[] = [];
  const unpushed = (ev.ahead ?? 0) > 0;
  const commit = ev.head_oid;

  const end: Reason | null = ev.state === 'MERGED' ? 'merged' : ev.state === 'CLOSED' ? 'closed' : null;
  if (end) {
    reasons.push(end);
    if (ev.dirty === true) reasons.push('dirty');
    if (unpushed) reasons.push('unpushed');
    return { verdict: REASON_VERDICT[end], reasons, commit, checked_at: at };
  }

  const stale = isStale(checkedAt, nowSec);
  if (stale) reasons.push('stale');
  if (ev.dirty === true) reasons.push('dirty');
  if (unpushed) reasons.push('unpushed');
  else if (ev.local_head && ev.head_oid && ev.local_head !== ev.head_oid) reasons.push('head_mismatch');
  const checks = ev.checks;
  if ((checks?.failing_total ?? 0) > 0) reasons.push('checks_failing');
  if ((checks?.pending ?? 0) > 0) reasons.push('checks_pending');
  if ((checks?.total ?? 0) === 0) reasons.push('no_checks');
  if (ev.draft) reasons.push('draft');
  if (ev.review_decision === 'CHANGES_REQUESTED') reasons.push('changes_requested');
  else if (ev.review_decision === 'REVIEW_REQUIRED') reasons.push('review_required');
  const onlyStale = () => reasons.every((r) => r === 'stale');
  if (ev.merge_state === 'DIRTY') reasons.push('merge_conflicts');
  else if (ev.merge_state === 'BEHIND') reasons.push('behind');
  else if (ev.merge_state === 'BLOCKED' && onlyStale()) reasons.push('merge_blocked');
  if (onlyStale()) reasons.push('checks_passed');

  let worst: Verdict = 'ready';
  for (const r of reasons) if (RANK[REASON_VERDICT[r]] > RANK[worst]) worst = REASON_VERDICT[r];
  return { verdict: stale ? 'unknown' : worst, reasons, commit, checked_at: at };
}

/** The row's assessment, or `null` without a PR. */
export function assessRow(row: SessionRow, nowSec: number): Assessment | null {
  return assess(row.pr_url, row.pr_evidence, row.pr_checked_at, nowSec);
}

/**
 * Whether the UI should draw an assessment. One with nothing behind it (no
 * evidence AND no probe stamp) comes from a hub older than migration 082,
 * or a PR not yet probed since the update: the card would say "unknown" on
 * every PR for no reason a person can act on, so the CI chip stays alone.
 * A PR the probe DID observe without evidence (an old `gh`) still shows.
 */
export function hasReading(a: Assessment | null): a is Assessment {
  return a !== null && !(a.checked_at === undefined && a.reasons[0] === 'no_evidence');
}

/** The first seven characters of a commit id. */
export function shortSha(oid: string | null | undefined): string {
  return oid ? oid.slice(0, 7) : '?';
}

/** A verdict as one of the manual's status words, with the reason after
 *  " · " (content rules: no seventh word, so Blocked reads Failed). */
export function verdictLabel(v: Verdict): string {
  switch (v) {
    case 'ready':
      return 'Done · ready to merge';
    case 'waiting':
      return 'Needs you · not ready to merge';
    case 'unknown':
      return 'Idle · not checked';
    case 'blocked':
      return 'Failed · cannot merge';
    case 'merged':
      return 'Done · merged';
    case 'closed':
      return 'Idle · closed';
  }
}

/** The kit state a verdict's chip and dot take: the same palette as the CI
 *  badge (`ciStatusColor`), so the two read alike. */
export function verdictState(v: Verdict): OfState {
  switch (v) {
    case 'ready':
    case 'merged':
      return 'done';
    case 'blocked':
      return 'failed';
    case 'waiting':
      return 'waiting';
    case 'unknown':
    case 'closed':
      return 'idle';
  }
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** One reason as the card says it, with the numbers and names from the reading. */
export function describeReason(r: Reason, ev: PrEvidence | null | undefined, checkedAt: number | null | undefined, nowMs: number = Date.now()): string {
  const head = shortSha(ev?.head_oid);
  switch (r) {
    case 'stale':
      return checkedAt != null
        ? `Not checked since ${timeAgo(checkedAt, nowMs)}: what follows is the last reading`
        : 'Not checked recently: what follows is the last reading';
    case 'no_evidence':
      return 'No evidence yet (not probed since the update, or the host’s gh is too old)';
    case 'merged':
      return 'Merged';
    case 'closed':
      return 'Closed without merging';
    case 'dirty':
      return 'Uncommitted changes in the worktree';
    case 'unpushed':
      return `${plural(ev?.ahead ?? 0, 'commit', 'commits')} not pushed; CI and review describe ${head}`;
    case 'head_mismatch':
      return `Worktree is on ${shortSha(ev?.local_head)}, the PR on ${head}`;
    case 'checks_failing': {
      const names = (ev?.checks.failing ?? []).map((f) => f.name);
      const more = (ev?.checks.failing_total ?? names.length) - names.length;
      return `Failing: ${names.join(', ') || 'checks'}${more > 0 ? ` (+${more} more)` : ''}`;
    }
    case 'checks_pending':
      return `${plural(ev?.checks.pending ?? 0, 'check', 'checks')} still running`;
    case 'no_checks':
      return 'No checks configured (not the same as passing)';
    case 'draft':
      return 'Draft PR';
    case 'changes_requested':
      return 'Changes requested';
    case 'review_required':
      return 'Review required';
    case 'merge_conflicts':
      return 'Merge conflicts with the base branch';
    case 'behind':
      return 'Branch is behind its base';
    case 'merge_blocked':
      return 'GitHub reports the merge as blocked by branch rules';
    case 'checks_passed':
      return `Checks passed for ${head}`;
  }
}
