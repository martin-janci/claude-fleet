// Pull requests (redesign step 6.4): Work › Pull requests. Every PR a
// session's branch has had, as reconcile recorded it (`pull_requests`), read
// through `list_pull_requests` (the hub's `prs` tool when paired).
import { invokeCmd, type Result } from './result';

export type PrState = 'OPEN' | 'CLOSED' | 'MERGED';
export type PrFilter = 'open' | 'merged' | 'closed' | 'all';

export interface PullRequestRow {
  id: number;
  url: string;
  repo?: string | null;
  number?: number | null;
  title?: string | null;
  head_ref?: string | null;
  state: PrState | string;
  draft?: boolean;
  /** passing | failing | pending; absent without checks. */
  ci_status?: string | null;
  review_decision?: string | null;
  merge_state?: string | null;
  merged_at?: number | null;
  /** The session that opened it; it may be gone. */
  session_id?: number | null;
  session_name?: string | null;
  host_alias?: string | null;
  project_id?: number | null;
  first_seen_at: number;
  updated_at: number;
}

export interface PrList {
  items: PullRequestRow[];
  total: number;
}

export function listPullRequests(q: { state?: PrFilter; projectId?: number; limit?: number } = {}): Promise<Result<PrList>> {
  const args: Record<string, unknown> = { action: 'list', state: q.state ?? 'all' };
  if (q.projectId != null) args.project_id = q.projectId;
  if (q.limit != null) args.limit = q.limit;
  return invokeCmd<PrList>('list_pull_requests', { args });
}

/** The badge a PR's state reads as. A draft is a state of its own here. */
export function prStateLabel(pr: Pick<PullRequestRow, 'state' | 'draft'>): string {
  switch (pr.state) {
    case 'MERGED':
      return 'Merged';
    case 'CLOSED':
      return 'Closed';
    default:
      return pr.draft ? 'Draft' : 'Open';
  }
}

/** One short line for CI and review, or '' when neither is known. */
export function prChecksLabel(pr: Pick<PullRequestRow, 'ci_status' | 'review_decision'>): string {
  const ci =
    pr.ci_status === 'passing' ? 'CI passing' : pr.ci_status === 'failing' ? 'CI failing' : pr.ci_status === 'pending' ? 'CI running' : '';
  return [ci, reviewDecisionWords(pr.review_decision) ?? ''].filter(Boolean).join(' · ');
}

/** GitHub's `reviewDecision` in words, or null when it is unknown. */
export function reviewDecisionWords(decision: string | null | undefined): string | null {
  switch (decision) {
    case 'APPROVED':
      return 'approved';
    case 'CHANGES_REQUESTED':
      return 'changes requested';
    case 'REVIEW_REQUIRED':
      return 'review required';
    default:
      return null;
  }
}

/** `owner/name#42`, or the URL when the repo could not be read from it. */
export function prRef(pr: Pick<PullRequestRow, 'repo' | 'number' | 'url'>): string {
  return pr.repo && pr.number != null ? `${pr.repo}#${pr.number}` : pr.url;
}
