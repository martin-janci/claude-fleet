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

import { invokeCmd, type Result } from './result';
import type { SessionRow } from './sessions';
import { ALL_SCOPES, type ScopeId } from './orgs';
import type { MissionWait } from './missions';
import { waitWords } from './mission_waits';

export type TodayBucket = 'waiting' | 'in_progress' | 'stale';

export interface TodaySession {
  id: number;
  name: string;
  host_alias: string;
  org_id?: number | null;
  /** waiting | stuck | failed | lifecycle — a person is needed. */
  attention?: string | null;
  /** `probably_waiting` when Jev proposes it waits (G1.6): kept apart from
   *  `attention`, never what puts its group in Waiting on you. */
  proposed?: string | null;
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
  /** Where it came from (G3.2): a routine's name, or `mission <name>`;
   *  absent when a person started it, or from an older hub. */
  from?: string | null;
}

/** A mission waiting on a person (G1.6, `today.rs` `TodayMission`). */
export interface TodayMission {
  id: number;
  name: string;
  org_id?: number | null;
  waiting_on: MissionWait;
}

export interface Today {
  since: number;
  now: number;
  groups: TodayGroup[];
  shipped: TodayShipped[];
  /** Missions waiting on a person; absent from an older hub (contract < 15). */
  missions?: TodayMission[];
}

/** What the view draws: the four sections, scoped, and the missions that
 *  wait on you beside Waiting on you. */
export interface TodayView {
  waiting: TodayGroup[];
  inProgress: TodayGroup[];
  shipped: TodayShipped[];
  stale: TodayGroup[];
  missions?: TodayMission[];
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
  // A mission by its org, as a shipped entry.
  out.missions = (t.missions ?? []).filter((m) => all || (orgScope !== null ? m.org_id === orgScope : m.org_id == null));
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
  // G1.6: Jev's proposal says so, wherever its group stands.
  if (s.proposed && !s.attention) return `${s.name} (probably waiting)`;
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

/** What a group's status reads as: the tracker's own name when it has one,
 *  else the effective status (`status_category` — for a local item that is
 *  the live-lifted answer, including the working-session lift, which is the
 *  only status it ever has). Empty when neither is known. */
export function groupStatusLabel(g: Pick<TodayGroup, 'status_name' | 'status_category'>): string {
  if (g.status_name) return g.status_name;
  switch (g.status_category) {
    case 'in_progress':
      return 'in progress';
    case 'done':
      return 'done';
    case 'todo':
      return 'to do';
    default:
      return '';
  }
}

function groupLine(g: TodayGroup, bucket: TodayBucket): string[] {
  const extras: string[] = [];
  const status = groupStatusLabel(g);
  if (status) extras.push(status);
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
  section('Waiting on me', [
    ...v.waiting.flatMap((g) => groupLine(g, 'waiting')),
    ...(v.missions ?? []).map(missionLine),
  ]);
  section('Stale', v.stale.flatMap((g) => groupLine(g, 'stale')));
  return parts.length > 0 ? parts.join('\n\n') + '\n' : 'Nothing to report.\n';
}

/** "- Mission Hub federation v2 — sign the autonomy grant". */
export function missionLine(m: TodayMission): string {
  return `- Mission ${m.name} — ${waitWords(m.waiting_on)}`;
}

/** Whether the view has nothing at all to show. */
export function isEmptyView(v: TodayView): boolean {
  return v.waiting.length + v.inProgress.length + v.shipped.length + v.stale.length + (v.missions?.length ?? 0) === 0;
}

// ── KPI tiles, date line, truncation (gap plan G3.2, board Today) ──

export interface TodayKpis {
  needsYou: number;
  inProgress: number;
  shipped: number;
  stale: number;
}

const sessionsIn = (gs: readonly TodayGroup[]) => gs.reduce((n, g) => n + g.sessions.length, 0);

/** The four tiles: sessions (and missions) that need you, sessions in
 *  progress, what shipped today, stale sessions. */
export function todayKpis(v: TodayView): TodayKpis {
  return {
    needsYou: sessionsIn(v.waiting) + (v.missions?.length ?? 0),
    inProgress: sessionsIn(v.inProgress),
    shipped: v.shipped.length,
    stale: sessionsIn(v.stale),
  };
}

/** "Thursday 8 October · 3 hosts · 2 accounts active": the day, the hosts
 *  shown, and the accounts a session used since `since`. */
export function todayLine(
  nowMs: number,
  hostAliases: readonly string[],
  rows: readonly Pick<SessionRow, 'account_uuid' | 'last_activity_at' | 'status'>[],
  since: number,
): string {
  const day = new Date(nowMs).toLocaleDateString('en-GB', { weekday: 'long', day: 'numeric', month: 'long' }).replace(',', '');
  const accounts = new Set(
    rows.filter((r) => r.status !== 'ghost' && r.account_uuid && (r.last_activity_at ?? 0) >= since).map((r) => r.account_uuid),
  ).size;
  const parts = [day, `${hostAliases.length} ${hostAliases.length === 1 ? 'host' : 'hosts'}`];
  if (accounts > 0) parts.push(`${accounts} ${accounts === 1 ? 'account' : 'accounts'} active`);
  return parts.join(' · ');
}

/** In progress shows this many groups before "N more ›". */
export const IN_PROGRESS_SHOWN = 4;
