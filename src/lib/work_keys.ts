// Work keys: the ticket / workstream a session is working on, recognised
// from what the session row already carries — no tracker, no setup.
//
// This is the day-1 half of the work graph (roadmap M1): a key such as
// `ABC-123` found in the session's tags, its worktree's branch, or its
// worktree directory groups sessions by work in the sidebar. The backend
// will later send a resolved `SessionRow.work` (explicit links, trackers,
// the live branch from the transcript); this module stays as the fallback
// for rows that do not carry it — an older hub, a phone-less build — so the
// grouping degrades instead of disappearing.
//
// Recognition is deliberately conservative, because a wrong group is worse
// than no group:
// - an UPPER-case key (`ABC-123`, `ENG2-7`) is taken as written;
// - a lower-case one (`abc-123-fix`, Linear's `user/eng-123-…`) only when its
//   prefix is letters only and not a word branch names use for other things
//   (`release-2024`, `hotfix-2`, `python-3`… see DENY);
// - a key directly followed by a `.` is a version (`foo-1.2.3`), not work.

import type { CiStatus, SessionRow } from './sessions';
import type { ProjectTreeRow } from './projects';

/** Where a session's key was found, strongest first. */
export type WorkKeySource = 'tag' | 'branch' | 'worktree';

export interface WorkKey {
  /** Normalised upper-case key, e.g. `ABC-123`. */
  key: string;
  source: WorkKeySource;
  /** The text the key was found in (a tag, branch or worktree name). */
  from: string;
}

// `[A-Za-z][A-Za-z0-9_]{1,9}` is a Jira-style project key (2-10 chars);
// the look-behind / look-ahead keep it from matching inside a longer token
// (`XABC-12`, `ABC-12abc`) or a version (`ABC-1.2`).
const KEY_RE = /(?<![A-Za-z0-9])([A-Za-z][A-Za-z0-9_]{1,9})-(\d{1,7})(?![A-Za-z0-9.])/g;

/** Lower-case prefixes that name something other than a ticket. Checked only
 *  for keys that were not written in upper case. */
const DENY = new Set([
  'utf', 'sha', 'md', 'iso', 'rfc', 'cve', 'ipv', 'http', 'python', 'node', 'java',
  'release', 'hotfix', 'fix', 'bugfix', 'bug', 'feature', 'feat', 'chore', 'patch',
  'wip', 'tmp', 'temp', 'test', 'tests', 'build', 'version', 'rc', 'beta', 'alpha',
  'step', 'phase', 'part', 'round', 'try', 'attempt', 'day', 'week', 'sprint',
  'iter', 'iteration', 'draft', 'backup', 'copy', 'old', 'new', 'dev', 'main',
  'master', 'revert', 'dependabot', 'renovate', 'spike', 'poc', 'demo',
]);

/** The first work key in `text`, normalised to upper case, or null. */
export function extractWorkKey(text: string | null | undefined): string | null {
  if (!text) return null;
  for (const m of text.matchAll(KEY_RE)) {
    const [, prefix, num] = m;
    const upperWritten = prefix === prefix.toUpperCase() && /[A-Z]/.test(prefix);
    if (!upperWritten) {
      if (!/^[A-Za-z]+$/.test(prefix)) continue;
      if (DENY.has(prefix.toLowerCase())) continue;
    }
    return `${prefix.toUpperCase()}-${num}`;
  }
  return null;
}

/** worktree id → branch, over every worktree the projects store holds. */
export function worktreeBranchById(projects: readonly ProjectTreeRow[]): Map<number, string> {
  const out = new Map<number, string>();
  for (const p of projects) {
    for (const w of p.worktrees) {
      if (w.branch) out.set(w.id, w.branch);
    }
  }
  return out;
}

/** The session's work key, or null when nothing it carries names one.
 *  Tags win (a person or the in-session agent set them on purpose), then the
 *  worktree's branch, then the worktree directory name (the only hint for a
 *  remote session whose worktree row the project tree does not list). */
export function workKeyFor(
  s: SessionRow,
  branchById: ReadonlyMap<number, string>,
): WorkKey | null {
  for (const tag of s.tags ?? []) {
    const key = extractWorkKey(tag);
    if (key) return { key, source: 'tag', from: tag };
  }
  const branch = s.worktree_id != null ? branchById.get(s.worktree_id) : undefined;
  if (branch) {
    const key = extractWorkKey(branch);
    if (key) return { key, source: 'branch', from: branch };
  }
  if (s.worktree_key && s.worktree_key !== 'main') {
    const key = extractWorkKey(s.worktree_key);
    if (key) return { key, source: 'worktree', from: s.worktree_key };
  }
  return null;
}

/** One-line explanation of why a session is in its work group. */
export function describeWorkKey(w: WorkKey): string {
  switch (w.source) {
    case 'tag':
      return `${w.key} — from the session tag "${w.from}"`;
    case 'branch':
      return `${w.key} — from the branch ${w.from}`;
    case 'worktree':
      return `${w.key} — from the worktree ${w.from}`;
  }
}

/** Distinct pull requests across a work group and their worst CI state
 *  (failing > pending > passing), for the group header. */
export function workGroupPrSummary(rows: readonly SessionRow[]): {
  prCount: number;
  ci: CiStatus | null;
} {
  const order: readonly CiStatus[] = ['passing', 'pending', 'failing'];
  const prs = new Set<string>();
  let worst = -1;
  for (const s of rows) {
    if (!s.pr_url) continue;
    prs.add(s.pr_url);
    const i = s.ci_status ? order.indexOf(s.ci_status) : -1;
    if (i > worst) worst = i;
  }
  return { prCount: prs.size, ci: worst >= 0 ? order[worst] : null };
}
