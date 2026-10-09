// The Accounts page (Orbit Fleet redesign step 4.1, Accounts board): every
// Claude account fleet knows, with its plan, its 5-hour and weekly usage and
// their history, the hosts and login profiles signed in to it, and the
// sessions running on it. Accounts stay derived from host logins and
// profiles; nothing here stores a credential.
import type { AccountRow } from './accounts';
import { accountLabel } from './accounts';
import type { HostRow } from './hosts';
import type { SessionRow } from './sessions';
import type { AccountUsage, AccountUsageSnapshot } from './account_usage_store';
import type { UsageWindowKind } from './account_usage';
import { invokeCmd, type Result } from './result';

/** Mirrors `store::UsageSnapshotRow`. */
export interface UsageSnapshotRow {
  account_uuid: string;
  fetched_at: number;
  usage: AccountUsage;
  subscription: string | null;
  source_host: string | null;
}

/** One login on a host: the host's own (`profile: null`) or a profile's. */
export interface AccountLogin {
  host: string;
  profile: string | null;
}

export interface AccountSummary {
  uuid: string;
  account: AccountRow;
  label: string;
  /** `Max`, `Pro`, … from the last usage answer, else the seat tier. */
  plan: string | null;
  usage: AccountUsageSnapshot | null;
  logins: AccountLogin[];
  sessions: SessionRow[];
}

/** `max` → `Max`; an unknown plan keeps its own spelling, capitalised. */
export function planLabel(subscription: string | null | undefined, seatTier?: string | null): string | null {
  const raw = (subscription ?? '').trim() || (seatTier ?? '').trim();
  if (!raw) return null;
  return raw.charAt(0).toUpperCase() + raw.slice(1);
}

/** Hosts and profiles logged in to `uuid`, host order then the host's own
 *  login before its profiles. */
export function loginsFor(uuid: string, hosts: readonly HostRow[]): AccountLogin[] {
  const out: AccountLogin[] = [];
  const sorted = [...hosts].sort((a, b) => a.alias.localeCompare(b.alias));
  for (const h of sorted) {
    if (h.account_uuid === uuid) out.push({ host: h.alias, profile: null });
    for (const p of h.claude_profiles ?? []) {
      if (p.account_uuid === uuid) out.push({ host: h.alias, profile: p.name });
    }
  }
  return out;
}

/** Every known account, sorted by label, with what the page shows for it. */
export function accountSummaries(
  accounts: readonly AccountRow[],
  hosts: readonly HostRow[],
  sessions: readonly SessionRow[],
  usage: Record<string, AccountUsageSnapshot>,
): AccountSummary[] {
  return accounts
    .map((a) => {
      const u = usage[a.uuid] ?? null;
      return {
        uuid: a.uuid,
        account: a,
        label: accountLabel(a),
        plan: planLabel(u?.subscription, a.seat_tier),
        usage: u,
        logins: loginsFor(a.uuid, hosts),
        sessions: sessions.filter((s) => s.account_uuid === a.uuid),
      };
    })
    .sort((x, y) => x.label.localeCompare(y.label) || x.uuid.localeCompare(y.uuid));
}

/** How far back each window's history reaches: a day of 5-hour windows, and
 *  a week and a day of the weekly one so last week's reset shows. */
export const HISTORY_SPAN_SECS: Record<UsageWindowKind, number> = {
  '5h': 24 * 3600,
  weekly: 8 * 86400,
};

export interface HistoryPoint {
  at: number;
  /** Percent used, 0..100. */
  used: number;
}

/** The points of one window's history, oldest first, from `now - span`. */
export function historyPoints(
  rows: readonly UsageSnapshotRow[],
  window: UsageWindowKind,
  now: number,
): HistoryPoint[] {
  const from = now - HISTORY_SPAN_SECS[window];
  const out: HistoryPoint[] = [];
  for (const r of rows) {
    if (r.fetched_at < from) continue;
    const w = window === '5h' ? r.usage.five_hour : r.usage.seven_day;
    if (!w || !Number.isFinite(w.utilization)) continue;
    out.push({ at: r.fetched_at, used: Math.min(100, Math.max(0, w.utilization)) });
  }
  return out;
}

/**
 * An SVG path for `points` across `[now - span, now]` × `[0, 100]` used, in a
 * `width` × `height` box (y grows down, so 100% used is the top). Empty with
 * fewer than two points: one reading is not a line.
 */
export function sparkPath(
  points: readonly HistoryPoint[],
  window: UsageWindowKind,
  now: number,
  width: number,
  height: number,
): string {
  if (points.length < 2) return '';
  const span = HISTORY_SPAN_SECS[window];
  const from = now - span;
  const x = (at: number) => (((at - from) / span) * width).toFixed(1);
  const y = (used: number) => (height - (used / 100) * height).toFixed(1);
  return points.map((p, i) => `${i === 0 ? 'M' : 'L'}${x(p.at)} ${y(p.used)}`).join(' ');
}

/** The highest use the history saw, for "peaked at 92%". `null` with none. */
export function peakUsed(points: readonly HistoryPoint[]): number | null {
  if (points.length === 0) return null;
  return Math.round(Math.max(...points.map((p) => p.used)));
}

/** One account's stored usage since `since`, oldest first. Never fetches. */
export async function loadUsageHistory(
  accountUuid: string,
  since: number,
): Promise<Result<UsageSnapshotRow[]>> {
  return invokeCmd<UsageSnapshotRow[]>('account_usage_history', {
    args: { account_uuid: accountUuid, since },
  });
}

// ── Spend, routines and paused sessions per account (steps 4.1 / 4.2, the
// Accounts board's "7 sessions · 2 routines · $18.40 today" and "2 paused
// sessions → Show · Switch to admin@…").

/** Mirrors `service::account_spend::AccountSpend` (the fields the page reads). */
export interface AccountSpend {
  /** `""` gathers sessions whose login fleet has not read yet. */
  account_uuid: string;
  cost_micros: number;
  models?: { model: string; totals: { cost_micros: number } }[];
  by_day?: { day: string; cost_micros: number }[];
}

/** Each account's live spend since `since` (unix seconds, from the start of
 *  its UTC day), from the `usage_daily_account` roll-up. Local only: a
 *  paired desktop collects no usage (`E_HUB_LOCAL_ONLY`). */
export function loadAccountSpend(since: number): Promise<Result<AccountSpend[]>> {
  return invokeCmd<AccountSpend[]>('account_spend', { args: { since } });
}

/** account uuid → micro-USD, from one `account_spend` answer. */
export function spendByAccount(rows: readonly AccountSpend[] | null | undefined): Map<string, number> {
  const m = new Map<string, number>();
  for (const r of rows ?? []) if (r.account_uuid) m.set(r.account_uuid, (m.get(r.account_uuid) ?? 0) + (r.cost_micros ?? 0));
  return m;
}

/** The account a login (a host's own, or one of its profiles) bills, as the
 *  routines scheduler resolves it; null when the host has not reported one. */
export function loginAccount(
  hostAlias: string,
  profile: string | null | undefined,
  hosts: readonly HostRow[],
): string | null {
  const h = hosts.find((x) => x.alias === hostAlias);
  if (!h) return null;
  const p = profile?.trim();
  if (p) return h.claude_profiles?.find((x) => x.name === p)?.account_uuid ?? null;
  return h.account_uuid ?? null;
}

/** How many switched-on routines run as `uuid`. */
export function routinesOn(
  uuid: string,
  routines: readonly { host_alias: string; profile?: string | null; enabled: boolean }[],
  hosts: readonly HostRow[],
): number {
  return routines.filter((r) => r.enabled && loginAccount(r.host_alias, r.profile, hosts) === uuid).length;
}

/** The limit a usage reading puts an account at (`attentionFacts`). */
export interface AccountLimitFact {
  resets_at: number | null;
}

/** The live sessions on an account that its limit has paused: not working,
 *  while the limit has not reset (`attention.ts::blockedBy`, "Paused · limit"). */
export function pausedSessions(
  a: Pick<AccountSummary, 'uuid' | 'sessions'>,
  limited: Readonly<Record<string, AccountLimitFact>> | undefined,
  now: number,
): SessionRow[] {
  const limit = limited?.[a.uuid];
  if (!limit || (limit.resets_at != null && limit.resets_at <= now)) return [];
  return a.sessions.filter((s) => s.status !== 'ghost' && s.lost_at === null && s.claude_status !== 'working');
}

/** "7 sessions · 2 routines · $18.40 today": the card's count line. */
export function countLine(sessions: number, routines: number, spend: string | null): string {
  const parts = [`${sessions} ${sessions === 1 ? 'session' : 'sessions'}`];
  if (routines > 0) parts.push(`${routines} ${routines === 1 ? 'routine' : 'routines'}`);
  if (spend !== null) parts.push(`${spend} today`);
  return parts.join(' · ');
}
