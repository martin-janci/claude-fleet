// The account pill (Orbit Fleet redesign step 4.3): the account a session
// runs on, on its row and its palette row, with the % left of the tighter
// window. Amber from 80% used, red at the limit; a click opens that account
// on the Accounts page (4.1).
import { writable } from 'svelte/store';
import type { AccountRow } from './accounts';
import { accountLabel } from './accounts';
import type { AccountUsageSnapshot } from './account_usage_store';
import {
  bindingWindow,
  formatResetShort,
  freshness,
  leftPct,
  limitWording,
  usedPct,
  windowOf,
  type UsageWindowKind,
} from './account_usage';
import { goTo } from './destination';

export type PillLevel = 'ok' | 'warn' | 'limit';

/** % used from which a pill turns amber. */
export const PILL_WARN_USED_PCT = 80;

export function pillLevel(used: number): PillLevel {
  if (used >= 100) return 'limit';
  if (used >= PILL_WARN_USED_PCT) return 'warn';
  return 'ok';
}

export interface AccountPillView {
  uuid: string;
  label: string;
  /** % left of the tighter window; `null` with no usable reading. */
  left: number | null;
  window: UsageWindowKind | null;
  level: PillLevel;
  /** The pill's text: `label 25%`, or `label LIMIT`, or just `label`. */
  text: string;
  title: string;
}

const WINDOW_NAME: Record<UsageWindowKind, string> = { '5h': '5-hour', weekly: 'weekly' };

/**
 * What a pill shows for `uuid`. A reading past its freshness limit, or past
 * its window's reset, is not shown: the pill says the account and no more
 * rather than a number that no longer holds.
 */
export function accountPill(
  uuid: string,
  account: AccountRow | undefined,
  snap: AccountUsageSnapshot | undefined,
  now: number,
  locale?: string,
  timeZone?: string,
): AccountPillView {
  const label = account ? accountLabel(account) : uuid.slice(0, 8);
  const usage = snap?.usage ?? null;
  const kind = bindingWindow(usage, now);
  const w = kind ? windowOf(usage, kind) : null;
  const shown =
    kind !== null && w !== null && freshness(kind, snap?.fetched_at ?? null, w.resets_at, now) !== 'expired';
  if (!shown || !kind || !w) {
    return { uuid, label, left: null, window: null, level: 'ok', text: label, title: `Account ${label}` };
  }
  const left = leftPct(w);
  // LIMIT on the raw figure, as the Blocked classifier (`attention_facts.ts`)
  // decides it: 99.6% used rounds to 0% left but does not stop the account.
  const level = w.utilization >= 100 ? 'limit' : pillLevel(Math.min(usedPct(w), 99));
  const reset = formatResetShort(kind, w.resets_at, now, locale, timeZone);
  const text = level === 'limit' ? `${label} ${limitWording(account?.has_extra_usage ?? false)}` : `${label} ${left}%`;
  return {
    uuid,
    label,
    left,
    window: kind,
    level,
    text,
    title: `Account ${label}: ${left}% of the ${WINDOW_NAME[kind]} window left, ${reset}. Open the account`,
  };
}

/** The account the Accounts page should show next; it takes it and clears it. */
export const accountsPageRequest = writable<string | null>(null);

/** With `accountsPageRequest`: show only the sessions its limit paused
 *  ("Show paused sessions" on the limit-hit toast). The page clears it. */
export const accountsPausedRequest = writable(false);

/** Open `uuid` on the Accounts page. */
export function openAccount(uuid: string): void {
  accountsPausedRequest.set(false);
  accountsPageRequest.set(uuid);
  goTo('accounts');
}

/** Open `uuid` on the Accounts page, its Sessions narrowed to the paused ones. */
export function openPausedSessions(uuid: string): void {
  accountsPausedRequest.set(true);
  accountsPageRequest.set(uuid);
  goTo('accounts');
}
