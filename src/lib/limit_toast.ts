// The limit-hit toast (Toasts board): "tech.silvester hit the weekly limit"
// over "2 sessions paused until Fri 11:00", with "Show paused sessions". It
// says so once, when a usage reading moves an account onto its limit; the
// sessions it names are the ones the Accounts page lists as paused by it.
//
// A limit already in place when this window first reads an account's usage
// is not news (the header pill and the paused rows say it), so an account
// toasts only when a reading it already had turns into a limit.
import { get } from 'svelte/store';
import { formatResetShort } from './account_usage';
import { accountUsage, type AccountUsageSnapshot } from './account_usage_store';
import { accountLabel, accountByUuid } from './accounts';
import { pausedSessions } from './accounts_page';
import { openPausedSessions } from './account_pill';
import type { AttentionLimit } from './attention';
import { attentionFactsFrom } from './attention_facts';
import { hosts } from './hosts';
import { sessions, type SessionRow } from './sessions';
import { push } from './toasts';

/** The toast's two lines for one account that just hit `limit`. */
export function limitHitLines(
  label: string,
  limit: AttentionLimit,
  paused: number,
  now: number,
  locale?: string,
  timeZone?: string,
): { message: string; sub: string } {
  const kind = limit.window === 'five_hour' ? '5h' : 'weekly';
  const name = limit.window === 'five_hour' ? '5-hour' : 'weekly';
  const at =
    limit.resets_at != null && limit.resets_at > now
      ? formatResetShort(kind, limit.resets_at, now, locale, timeZone).replace(/^resets? (at )?/, '')
      : null;
  const until = at ? `until ${at}` : 'until it resets';
  const sub =
    paused > 0
      ? `${paused} ${paused === 1 ? 'session' : 'sessions'} paused ${until}`
      : at
        ? `Nothing paused · resets ${at}`
        : 'Nothing paused';
  return { message: `${label} hit the ${name} limit`, sub };
}

/** The accounts whose limit is new in `next`: limited now, not limited (or
 *  limited by a reset already past) before, and read at least once before. */
export function newlyLimited(
  prev: Readonly<Record<string, AttentionLimit>>,
  next: Readonly<Record<string, AttentionLimit>>,
  seenBefore: ReadonlySet<string>,
  now: number,
): string[] {
  return Object.keys(next).filter((uuid) => {
    if (!seenBefore.has(uuid)) return false;
    const was = prev[uuid];
    return !was || (was.resets_at != null && was.resets_at <= now);
  });
}

/** Push the toast for one account. Exported for the tests. */
export function announceLimitHit(
  uuid: string,
  limit: AttentionLimit,
  rows: readonly SessionRow[],
  now: number,
): number {
  const account = get(accountByUuid).get(uuid);
  const mine = rows.filter((s) => s.account_uuid === uuid);
  const paused = pausedSessions({ uuid, sessions: mine }, { [uuid]: limit }, now).length;
  const { message, sub } = limitHitLines(account ? accountLabel(account) : uuid.slice(0, 8), limit, paused, now);
  return push({
    kind: 'warning',
    message,
    sub,
    action: paused > 0 ? { label: 'Show paused sessions', run: () => openPausedSessions(uuid) } : undefined,
  });
}

/** Watch usage readings and toast each new limit; returns the unsubscribe. */
export function startLimitToasts(): () => void {
  let prev: Readonly<Record<string, AttentionLimit>> = {};
  let seen = new Set<string>();
  return accountUsage.subscribe((usage: Record<string, AccountUsageSnapshot>) => {
    const now = Math.floor(Date.now() / 1000);
    const next = attentionFactsFrom(get(hosts), usage, now).limited_accounts ?? {};
    for (const uuid of newlyLimited(prev, next, seen, now)) announceLimitHit(uuid, next[uuid], get(sessions), now);
    prev = next;
    seen = new Set(Object.keys(usage));
  });
}
