// Work keys: the ticket / workstream a session is working on, recognised
// from what the session row already carries — no tracker, no setup.
//
// This is the day-1 half of the work graph (roadmap M1): a key such as
// `ABC-123` found in the session's tags, its worktree's branch, or its
// worktree directory groups sessions by work in the sidebar. A row whose
// `SessionRow.work` carries an explicit link (M1b) uses that instead; this
// recognition stays as the fallback for rows without one — and for an older
// hub that sends none — so the grouping degrades instead of disappearing.
// A key the user rejected for the row (`work_rejected`) is never recognised.
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

/** Where a session's key was found, strongest first: an explicit link
 *  (`SessionRow.work`), then recognition. */
export type WorkKeySource = 'link' | 'tag' | 'branch' | 'worktree';

export interface WorkKey {
  /** Normalised upper-case key, e.g. `ABC-123`. */
  key: string;
  source: WorkKeySource;
  /** The text the key was found in (a tag, branch or worktree name); for a
   *  link, the work item's title (may be empty). */
  from: string;
  /** A linked tracker item's status (work graph M3), for the chip's dot. */
  status?: {
    category: string;
    name: string | null;
    url: string | null;
    unavailable: boolean;
  };
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

/** The work key in a pasted ticket URL — Jira `/browse/ABC-123` or
 *  `?selectedIssue=ABC-123`, Linear `/<ws>/issue/ENG-42/…` — or null when
 *  `text` is not a lone URL or names no key. */
export function keyFromTicketUrl(text: string): string | null {
  const t = text.trim();
  if (!/^https?:\/\/\S+$/i.test(t)) return null;
  let u: URL;
  try {
    u = new URL(t);
  } catch {
    return null;
  }
  let path = u.pathname;
  try {
    path = decodeURIComponent(path);
  } catch {
    /* keep the raw path */
  }
  return extractWorkKey(u.searchParams.get('selectedIssue')) ?? extractWorkKey(path);
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
 *  An explicit link wins (a person or the in-session agent decided it); then
 *  tags (set on purpose), then the worktree's branch, then the worktree
 *  directory name (the only hint for a remote session whose worktree row the
 *  project tree does not list). A recognised key the user rejected for this
 *  session is skipped, so "Not this" sticks. */
export function workKeyFor(
  s: SessionRow,
  branchById: ReadonlyMap<number, string>,
): WorkKey | null {
  const linked = s.work?.key || s.work?.title;
  if (s.work && linked) {
    const w = s.work;
    const status =
      w.status_category || w.unavailable
        ? {
            category: w.status_category ?? 'unknown',
            name: w.status_name ?? null,
            url: w.url ?? null,
            unavailable: !!w.unavailable,
          }
        : undefined;
    return status
      ? { key: linked, source: 'link', from: w.title, status }
      : { key: linked, source: 'link', from: w.title };
  }
  const rejected = new Set(s.work_rejected ?? []);
  const recognise = (text: string): string | null => {
    const key = extractWorkKey(text);
    return key && !rejected.has(key) ? key : null;
  };
  for (const tag of s.tags ?? []) {
    const key = recognise(tag);
    if (key) return { key, source: 'tag', from: tag };
  }
  const branch = s.worktree_id != null ? branchById.get(s.worktree_id) : undefined;
  if (branch) {
    const key = recognise(branch);
    if (key) return { key, source: 'branch', from: branch };
  }
  if (s.worktree_key && s.worktree_key !== 'main') {
    const key = recognise(s.worktree_key);
    if (key) return { key, source: 'worktree', from: s.worktree_key };
  }
  return null;
}

/** One-line explanation of why a session is in its work group, with the
 *  ticket's status when a tracker knows it. */
export function describeWorkKey(w: WorkKey): string {
  const base = describeSource(w);
  if (!w.status) return base;
  if (w.status.unavailable) return `${base} · unavailable in the tracker`;
  return w.status.name ? `${base} · ${w.status.name}` : base;
}

function describeSource(w: WorkKey): string {
  switch (w.source) {
    case 'link':
      return w.from && w.from !== w.key
        ? `${w.key} — ${w.from} (linked)`
        : `${w.key} — linked to this session`;
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

/** A work group header's ticket line (work graph M3): the title and status
 *  of the first session in the group linked to `key`, when any carries one. */
export function workGroupTicket(
  key: string,
  rows: readonly SessionRow[],
): { title: string; status: WorkKey['status'] } | null {
  for (const s of rows) {
    const w = s.work;
    if (!w || w.key !== key) continue;
    if (!w.title && !w.status_category && !w.unavailable) continue;
    return {
      title: w.title,
      status:
        w.status_category || w.unavailable
          ? {
              category: w.status_category ?? 'unknown',
              name: w.status_name ?? null,
              url: w.url ?? null,
              unavailable: !!w.unavailable,
            }
          : undefined,
    };
  }
  return null;
}
