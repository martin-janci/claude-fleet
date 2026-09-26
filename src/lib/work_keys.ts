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
  /** Detection linked it without a person (work graph M4): the chip shows
   *  a small dot and the tooltip says why. */
  auto?: boolean;
  /** The link's "why" (`from the branch · rule R3`). */
  why?: string;
  /** A linked tracker item's status (work graph M3), for the chip's dot. */
  status?: {
    category: string;
    name: string | null;
    url: string | null;
    unavailable: boolean;
  };
}

// The recogniser is shared with the hub (work graph M4.1):
// `crates/fleet-core/src/service/work/recognize.rs` is its Rust twin, and both
// run `crates/fleet-core/src/service/work/testdata/recognize_cases.json`, so
// the desktop's fallback and the hub's detection agree on what text names.
//
// `[A-Za-z][A-Za-z0-9_]{1,9}` is a Jira-style project key (2-10 chars); the
// look-behind / look-ahead keep it from matching inside a longer token
// (`XABC-12`, `ABC-12abc`); a `.` and a digit after the number is a version
// (`lodash-4.17`), a sentence's full stop is not.
const KEY_RE = /(?<![A-Za-z0-9])([A-Za-z][A-Za-z0-9_]{1,9})-(\d{1,7})(?![A-Za-z0-9]|\.\d)/g;
const URL_RE = /https?:\/\/[^\s<>"'`()[\]]+/gi;
const ISSUE_RE = /#(\d{1,7})(?![A-Za-z0-9_])/g;

/** Lower-case prefixes that name something other than a ticket. Checked only
 *  for keys that were not written in upper case. Keep in sync with `DENY` in
 *  `recognize.rs` (the shared fixture exercises both). */
const DENY = new Set([
  'utf', 'sha', 'md', 'iso', 'rfc', 'cve', 'ipv', 'http', 'python', 'node', 'java',
  'release', 'hotfix', 'fix', 'bugfix', 'bug', 'feature', 'feat', 'chore', 'patch',
  'wip', 'tmp', 'temp', 'test', 'tests', 'build', 'version', 'rc', 'beta', 'alpha',
  'step', 'phase', 'part', 'round', 'try', 'attempt', 'day', 'week', 'sprint',
  'iter', 'iteration', 'draft', 'backup', 'copy', 'old', 'new', 'dev', 'main',
  'master', 'revert', 'dependabot', 'renovate', 'spike', 'poc', 'demo',
]);

/** What recognition knows besides the text (mirrors `RecognizeCtx`). */
export interface RecognizeCtx {
  /** Every tracker's key prefixes; non-empty restricts keys to them. */
  prefixes?: readonly string[];
  /** Configured trackers, so a URL's host names its tracker. */
  trackers?: readonly { id: number; host: string; provider?: string }[];
  /** The session's GitHub `owner/repo`, for a bare `#123`. */
  repo?: string | null;
}

/** One recognised reference (mirrors the Rust `Match`, minus its span). */
export interface TicketRef {
  /** `repo_issue`: a GitHub issue of a named repository — a bare `#123`
   *  against the session's repo, or an enterprise `host/owner/repo#123`. */
  kind: 'key' | 'url' | 'repo_issue';
  /** `ABC-123`, `owner/repo#42`, `asana:<task>`. */
  key: string;
  tracker_id: number | null;
  provider: string | null;
  /** The matched text as written. */
  text: string;
  upper_written: boolean;
}

function keyAccepted(prefix: string, ctx: RecognizeCtx): boolean | null {
  const upper = /[A-Z]/.test(prefix) && !/[a-z]/.test(prefix);
  if (!upper) {
    if (!/^[A-Za-z]+$/.test(prefix)) return null;
    if (DENY.has(prefix.toLowerCase())) return null;
  }
  const known = ctx.prefixes ?? [];
  if (known.length && !known.some((p) => p.toUpperCase() === prefix.toUpperCase())) return null;
  return upper;
}

/** The key a whole URL segment / query value is, or null. */
function wholeKey(v: string): string | null {
  const m = /^([A-Za-z][A-Za-z0-9_]{1,9})-(\d{1,7})$/.exec(v);
  return m ? `${m[1].toUpperCase()}-${m[2]}` : null;
}

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** A GitHub owner or repository name (mirrors `github_name` in
 *  `recognize.rs`). */
function githubName(s: string): boolean {
  return s.length > 0 && s.length <= 100 && /^[A-Za-z0-9_][A-Za-z0-9_.-]*$/.test(s);
}

/** The configured GitHub Enterprise hosts (work graph M11.4): every GitHub
 *  tracker's host that is not github.com, lower case. Only these hosts'
 *  URLs and `host/owner/repo#n` refs are recognised — never an arbitrary
 *  host's. */
function ghesHosts(ctx: RecognizeCtx): string[] {
  const out: string[] = [];
  for (const t of ctx.trackers ?? []) {
    if (t.provider !== 'github') continue;
    const h = t.host.toLowerCase();
    if (h && h !== 'github.com' && h !== 'www.github.com' && !out.includes(h)) out.push(h);
  }
  return out;
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function ticketUrl(
  raw: string,
  ctx: RecognizeCtx,
): { key: string; provider: string; host: string } | null {
  const rest = raw.slice(raw.indexOf('://') + 3);
  const cut = rest.search(/[/?#]/);
  const authority = cut < 0 ? rest : rest.slice(0, cut);
  const tail = cut < 0 ? '' : rest.slice(cut);
  const host = (authority.split('@').pop() ?? '').split(':')[0].toLowerCase();
  const noFrag = tail.split('#')[0];
  const q = noFrag.indexOf('?');
  const path = decode(q < 0 ? noFrag : noFrag.slice(0, q));
  const query = q < 0 ? '' : noFrag.slice(q + 1);
  const segs = path.split('/').filter((x) => x);
  if (host === 'github.com' || host === 'www.github.com') {
    const [o, r, kind, n] = segs;
    if (kind === 'issues' && n && /^\d{1,9}$/.test(n)) {
      return { key: `${o.toLowerCase()}/${r.toLowerCase()}#${n}`, provider: 'github', host };
    }
    return null;
  }
  if (host === 'app.asana.com') {
    let task: string | undefined;
    if (segs[0] === '0' && segs.length >= 3) task = segs[2];
    else if (segs[0] === '1' && segs[2] === 'project' && segs[4] === 'task') task = segs[5];
    return task && /^\d{1,24}$/.test(task) ? { key: `asana:${task}`, provider: 'asana', host } : null;
  }
  if (host === 'linear.app') {
    const k = segs[1] === 'issue' && segs[2] ? wholeKey(segs[2]) : null;
    return k ? { key: k, provider: 'linear', host } : null;
  }
  if (ghesHosts(ctx).includes(host)) {
    const [o, r, kind, n] = segs;
    if (o && r && githubName(o) && githubName(r) && kind === 'issues' && n && /^\d{1,9}$/.test(n)) {
      return { key: `${host}/${o.toLowerCase()}/${r.toLowerCase()}#${n}`, provider: 'github', host };
    }
    return null;
  }
  for (const pair of query.split('&')) {
    if (pair.startsWith('selectedIssue=')) {
      const k = wholeKey(decode(pair.slice('selectedIssue='.length)));
      if (k) return { key: k, provider: 'jira', host };
    }
  }
  const i = segs.indexOf('browse');
  const k = i >= 0 && segs[i + 1] ? wholeKey(segs[i + 1]) : null;
  return k ? { key: k, provider: 'jira', host } : null;
}

/** Every ticket reference in `text` — keys, ticket URLs (Jira, Linear, Asana,
 *  GitHub) and, with `ctx.repo`, a bare `#123` — in order of appearance. The
 *  same answer the hub's `recognize` gives (see the shared fixture). */
export function extractTicketRefs(text: string | null | undefined, ctx: RecognizeCtx = {}): TicketRef[] {
  if (!text) return [];
  const found: { at: number; ref: TicketRef }[] = [];
  const urls: [number, number][] = [];
  for (const m of text.matchAll(URL_RE)) {
    const raw = m[0].replace(/[.,;:!?]+$/, '');
    const at = m.index ?? 0;
    urls.push([at, at + raw.length]);
    const t = ticketUrl(raw, ctx);
    if (!t) continue;
    const tracker = (ctx.trackers ?? []).find((x) => x.host.toLowerCase() === t.host);
    found.push({
      at,
      ref: {
        kind: 'url',
        key: t.key,
        tracker_id: tracker ? tracker.id : null,
        provider: t.provider,
        text: raw,
        upper_written: true,
      },
    });
  }
  const inUrl = (i: number) => urls.some(([a, b]) => i >= a && i < b);
  for (const m of text.matchAll(KEY_RE)) {
    const at = m.index ?? 0;
    if (inUrl(at)) continue;
    const [whole, prefix, num] = m;
    const upper = keyAccepted(prefix, ctx);
    if (upper === null) continue;
    found.push({
      at,
      ref: {
        kind: 'key',
        key: `${prefix.toUpperCase()}-${num}`,
        tracker_id: null,
        provider: null,
        text: whole,
        upper_written: upper,
      },
    });
  }
  // `host/owner/repo#n` of a configured enterprise host (M11.4).
  for (const host of ghesHosts(ctx)) {
    const re = new RegExp(
      `(?<![A-Za-z0-9._\\-/:@])${escapeRe(host)}\\/([A-Za-z0-9_.-]+)\\/([A-Za-z0-9_.-]+)#(\\d{1,9})(?![A-Za-z0-9_])`,
      'gi',
    );
    for (const m of text.matchAll(re)) {
      const at = m.index ?? 0;
      const [whole, o, r, n] = m;
      if (inUrl(at) || !githubName(o) || !githubName(r)) continue;
      const tracker = (ctx.trackers ?? []).find(
        (x) => x.provider === 'github' && x.host.toLowerCase() === host,
      );
      found.push({
        at,
        ref: {
          kind: 'repo_issue',
          key: `${host}/${o.toLowerCase()}/${r.toLowerCase()}#${n}`,
          tracker_id: tracker ? tracker.id : null,
          provider: 'github',
          text: whole,
          upper_written: false,
        },
      });
    }
  }
  const repo = ctx.repo;
  if (repo && repo.includes('/')) {
    for (const m of text.matchAll(ISSUE_RE)) {
      const at = m.index ?? 0;
      const before = at > 0 ? text[at - 1] : '';
      if (inUrl(at) || /[A-Za-z0-9&#/]/.test(before)) continue;
      found.push({
        at,
        ref: {
          kind: 'repo_issue',
          key: `${repo.toLowerCase()}#${m[1]}`,
          tracker_id: null,
          provider: 'github',
          text: m[0],
          upper_written: false,
        },
      });
    }
  }
  return found.sort((a, b) => a.at - b.at).map((f) => f.ref);
}

/** The first work key in `text`, normalised to upper case, or null. */
export function extractWorkKey(text: string | null | undefined): string | null {
  return extractTicketRefs(text).find((r) => r.kind === 'key')?.key ?? null;
}

/** The work key in a pasted ticket URL — Jira `/browse/ABC-123` or
 *  `?selectedIssue=ABC-123`, Linear `/<ws>/issue/ENG-42/…` — or null when
 *  `text` is not a lone URL or names no key. */
export function keyFromTicketUrl(text: string): string | null {
  const t = text.trim();
  if (!/^https?:\/\/\S+$/i.test(t)) return null;
  const ref = extractTicketRefs(t).find((r) => r.kind === 'url');
  if (ref && /^[A-Z][A-Z0-9_]{1,9}-\d{1,7}$/.test(ref.key)) return ref.key;
  // Not a known ticket URL shape: a key anywhere in its path still names it.
  let u: URL;
  try {
    u = new URL(t);
  } catch {
    return null;
  }
  return extractWorkKey(decode(u.pathname).replace(/\//g, ' '));
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
    const out: WorkKey = { key: linked, source: 'link', from: w.title };
    if (status) out.status = status;
    if (w.strength && AUTO_LINK_SOURCES.includes(w.source)) out.auto = true;
    if (w.rule || AUTO_LINK_SOURCES.includes(w.source)) out.why = linkWhy(w.source, w.rule);
    return out;
  }
  // A hub with detection (work graph M4) already recognised this row and
  // proposes its key as a suggestion: a guess never moves a session into a
  // work group, so the fallback stays out of its way.
  if (s.work_suggested) return null;
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

/** Link sources detection writes (mirrors `AUTO_SOURCES` in `work.ts`, kept
 *  here so this module stays free of command wrappers). */
const AUTO_LINK_SOURCES: readonly string[] = ['branch', 'pr', 'trailer', 'url', 'prompt', 'agent_inferred'];

function linkWhy(source: string, rule: string | null | undefined): string {
  const label: Record<string, string> = {
    branch: 'the branch',
    pr: 'the pull request',
    trailer: 'a commit trailer',
    url: 'a ticket URL in a prompt',
    prompt: 'a prompt',
    agent_inferred: "Claude's guess when asked",
  };
  const from = label[source] ? `linked from ${label[source]}` : `linked (${source})`;
  return rule ? `${from} · rule ${rule}` : from;
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
    case 'link': {
      const why = w.why ? ` (${w.why})` : ' (linked)';
      return w.from && w.from !== w.key
        ? `${w.key} — ${w.from}${why}`
        : w.why
          ? `${w.key} — ${w.why}`
          : `${w.key} — linked to this session`;
    }
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
