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
