/**
 * The Today view (work graph M9.1): the hub's digest (`work_today`), cut to
 * the scope the sidebar shows, and the standup built from exactly that.
 *
 * Types mirror `crates/fleet-core/src/service/work/today.rs`; every field a
 * newer hub may add is optional, and an unknown bucket reads as in progress.
 * The standup is plain text, deterministic, and built here — no network, no
 * model — from what the view shows, so the copy never says more than the
 * screen (a scope chosen with ⌘⇧O applies to both).
 */

import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { SessionRow } from './sessions';
import { ALL_SCOPES, type ScopeId } from './orgs';

export type TodayBucket = 'waiting' | 'in_progress' | 'stale';

export interface TodaySession {
  id: number;
  name: string;
  host_alias: string;
  org_id?: number | null;
  /** waiting | stuck | failed | lifecycle — a person is needed. */
  attention?: string | null;
  /** idle | done — why this session is stale. */
  stale?: string | null;
  claude_status?: string | null;
  pr_url?: string | null;
  ci_status?: string | null;
  last_activity_at: number;
}

export interface TodayGroup {
  bucket: string;
  /** Absent: the sessions with no work. */
  key?: string | null;
  title?: string;
  item_id?: number | null;
  status_category?: string | null;
  status_name?: string | null;
  url?: string | null;
  org_id?: number | null;
  sessions: TodaySession[];
}

export interface TodayShipped {
  /** done (a ticket moved to done) | pr (work ended with a PR). */
  how: string;
  key?: string | null;
  title?: string;
  url?: string | null;
  pr_url?: string | null;
  at: number;
  org_id?: number | null;
}

export interface Today {
  since: number;
  now: number;
  groups: TodayGroup[];
  shipped: TodayShipped[];
}

/** What the view draws: the four sections, scoped. */
export interface TodayView {
  waiting: TodayGroup[];
  inProgress: TodayGroup[];
  shipped: TodayShipped[];
  stale: TodayGroup[];
}

/** Local midnight of `nowMs`, in unix seconds: "today" is the viewer's. */
export function localMidnight(nowMs: number = Date.now()): number {
  const d = new Date(nowMs);
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}

export function loadToday(since: number = localMidnight()): Promise<Result<Today>> {
  return invokeCmd<Today>('work_today', { args: { since } });
}

/** Whether the Today view covers Details (⌘⇧T); it is also the empty state. */
export const todayOpen = writable(false);

/** The hub's rule (`today.rs`), re-applied after scoping drops sessions. */
export function bucketOf(sessions: readonly TodaySession[]): TodayBucket {
  if (sessions.some((s) => s.attention)) return 'waiting';
  if (sessions.length > 0 && sessions.every((s) => s.stale)) return 'stale';
  return 'in_progress';
}

/**
 * Cut the digest to `scope`, the way the sidebar does: a session by the scope
 * of its live row (`scopeOf`), a shipped entry by its org (an owner scope or
 * *unassigned* keeps only entries with no org — a finished ticket has no
 * project owner to go by). Groups left empty are dropped and the rest
 * re-bucketed.
 */
export function scopeToday(
  t: Today,
  scope: ScopeId,
  rows: readonly SessionRow[],
  scopeOf: (s: SessionRow) => ScopeId,
): TodayView {
  const all = scope === ALL_SCOPES;
  const byId = new Map(rows.map((r) => [r.id, r]));
  const keepSession = (s: TodaySession) => {
    if (all) return true;
    const row = byId.get(s.id);
    return row !== undefined && scopeOf(row) === scope;
  };
  const orgScope = scope.startsWith('org:') ? Number(scope.slice(4)) : null;
  const keepShipped = (x: TodayShipped) =>
    all || (orgScope !== null ? x.org_id === orgScope : x.org_id == null);
  const out: TodayView = { waiting: [], inProgress: [], shipped: [], stale: [] };
  for (const g of t.groups ?? []) {
    const sessions = (g.sessions ?? []).filter(keepSession);
    if (sessions.length === 0) continue;
    const group = { ...g, sessions };
    const b = bucketOf(sessions);
    if (b === 'waiting') out.waiting.push(group);
    else if (b === 'stale') out.stale.push(group);
    else out.inProgress.push(group);
  }
  out.shipped = (t.shipped ?? []).filter(keepShipped);
  return out;
}

const ATTENTION_WORDS: Record<string, string> = {
  waiting: 'waiting for an answer',
  stuck: 'stuck',
  failed: 'turn failed',
  lifecycle: 'needs a look',
};

/** How a session reads in a line: its name, and why it is listed. */
export function sessionPhrase(s: TodaySession, bucket: TodayBucket): string {
  if (bucket === 'waiting' && s.attention) {
    return `${s.name} (${ATTENTION_WORDS[s.attention] ?? s.attention})`;
  }
  if (bucket === 'stale' && s.stale) {
    return `${s.name} (${s.stale === 'done' ? 'ticket done, session still running' : 'idle'})`;
  }
  return s.name;
}

/** `KEY title` for a group; the no-work group names nothing itself. */
export function groupLabel(g: Pick<TodayGroup, 'key' | 'title'>): string {
  if (!g.key) return 'No work';
  const title = (g.title ?? '').trim();
  return title ? `${g.key} ${title}` : g.key;
}

function groupLine(g: TodayGroup, bucket: TodayBucket): string[] {
  const extras: string[] = [];
  if (g.status_name) extras.push(g.status_name);
  const pr = g.sessions.find((s) => s.pr_url);
  if (pr?.pr_url) extras.push(pr.ci_status ? `PR ${pr.pr_url} (CI ${pr.ci_status})` : `PR ${pr.pr_url}`);
  const names = g.sessions.map((s) => sessionPhrase(s, bucket)).join(', ');
  if (!g.key) return g.sessions.map((s) => `- ${sessionPhrase(s, bucket)}`);
  const tail = [...extras, names].filter(Boolean).join(' · ');
  return [`- ${groupLabel(g)}${tail ? ` — ${tail}` : ''}`];
}

function shippedLine(x: TodayShipped): string {
  const label = groupLabel({ key: x.key ?? null, title: x.title ?? '' });
  const what = x.how === 'done' ? 'done' : 'PR';
  const link = x.pr_url ?? x.url ?? '';
  return `- ${x.key ? label : (x.title || 'Untitled work')} — ${what}${link ? ` ${link}` : ''}`;
}

/**
 * The standup as plain text: Shipped, In progress, Waiting on me, Stale —
 * each only when it has something. Tracker titles are copied as they are (the
 * clipboard is the person's own, not an agent's).
 */
export function standupText(v: TodayView): string {
  const parts: string[] = [];
  const section = (title: string, lines: string[]) => {
    if (lines.length > 0) parts.push([title, ...lines].join('\n'));
  };
  section('Shipped', v.shipped.map(shippedLine));
  section('In progress', v.inProgress.flatMap((g) => groupLine(g, 'in_progress')));
  section('Waiting on me', v.waiting.flatMap((g) => groupLine(g, 'waiting')));
  section('Stale', v.stale.flatMap((g) => groupLine(g, 'stale')));
  return parts.length > 0 ? parts.join('\n\n') + '\n' : 'Nothing to report.\n';
}

/** Whether the view has nothing at all to show. */
export function isEmptyView(v: TodayView): boolean {
  return v.waiting.length + v.inProgress.length + v.shipped.length + v.stale.length === 0;
}
