// The header's account pills (redesign step 3.17, Main board): one quiet
// button per Claude login with a health dot, its name and both windows in
// one short line ("5h 75% · wk 85%", % left), or what stops it ("weekly
// limit · Fri 11:00"). The worst account comes first; the header shows a few
// and a "+N" opens the rest on the Accounts page.
import {
  formatResetShort,
  freshness,
  leftPct,
  severityLeft,
  severity,
  windowOf,
  type UsageSeverity,
  type UsageWindowKind,
} from './account_usage';
import type { AccountUsageSnapshot } from './account_usage_store';
import { accountLabel, type AccountRow } from './accounts';

/** How many pills the header shows before "+N". */
export const HEADER_ACCOUNTS_MAX = 3;

export type HeaderAccountHealth = 'ok' | 'caution' | 'limit' | 'unknown';

export interface HeaderAccount {
  uuid: string;
  label: string;
  health: HeaderAccountHealth;
  /** "5h 75% · wk 85%", "weekly limit · Fri 11:00", or "" with no reading. */
  meta: string;
  /** The meta says the account is stopped (amber), not a level. */
  limited: boolean;
  /** The pill's accessible name, in words. */
  aria: string;
}

const SHORT: Record<UsageWindowKind, string> = { '5h': '5h', weekly: 'wk' };
const NAME: Record<UsageWindowKind, string> = { '5h': '5-hour', weekly: 'weekly' };
const RANK: Record<HeaderAccountHealth, number> = { limit: 0, caution: 1, ok: 2, unknown: 3 };

function health(levels: UsageSeverity[]): HeaderAccountHealth {
  if (levels.length === 0) return 'unknown';
  if (levels.includes('limit')) return 'limit';
  if (levels.includes('low') || levels.includes('caution')) return 'caution';
  return 'ok';
}

export function headerAccount(
  account: AccountRow,
  snap: AccountUsageSnapshot | undefined,
  now: number,
  locale?: string,
  timeZone?: string,
): HeaderAccount {
  const label = accountLabel(account);
  const usage = snap?.usage ?? null;
  const shown: { kind: UsageWindowKind; left: number; resetsAt: number | null; level: UsageSeverity }[] = [];
  for (const kind of ['5h', 'weekly'] as const) {
    const w = windowOf(usage, kind);
    if (!w || freshness(kind, snap?.fetched_at ?? null, w.resets_at, now) === 'expired') continue;
    const left = leftPct(w);
    // At the limit only on the raw figure (as `attention_facts.ts`): 99.6%
    // used shows 0% left but is low, not stopped.
    shown.push({ kind, left, resetsAt: w.resets_at, level: severity(kind, severityLeft(w), w.resets_at, now) });
  }
  const h = health(shown.map((s) => s.level));
  const stop = shown.find((s) => s.level === 'limit');
  if (stop) {
    const reset = formatResetShort(stop.kind, stop.resetsAt, now, locale, timeZone).replace(/^resets? (at )?/, '');
    return {
      uuid: account.uuid,
      label,
      health: h,
      meta: `${NAME[stop.kind]} limit · ${reset}`,
      limited: true,
      aria: `Account ${label}, ${NAME[stop.kind]} limit reached, resets ${reset}`,
    };
  }
  return {
    uuid: account.uuid,
    label,
    health: h,
    meta: shown.map((s) => `${SHORT[s.kind]} ${s.left}%`).join(' · '),
    limited: false,
    aria:
      shown.length === 0
        ? `Account ${label}, no recent usage reading`
        : `Account ${label}, ${shown.map((s) => `${s.left} percent of the ${NAME[s.kind]} window left`).join(', ')}`,
  };
}

/** Every account as a pill, worst first, then by name. */
export function headerAccounts(
  accounts: readonly AccountRow[],
  snaps: Record<string, AccountUsageSnapshot>,
  now: number,
  locale?: string,
  timeZone?: string,
): HeaderAccount[] {
  return accounts
    .map((a) => headerAccount(a, snaps[a.uuid], now, locale, timeZone))
    .sort((a, b) => RANK[a.health] - RANK[b.health] || a.label.localeCompare(b.label));
}
