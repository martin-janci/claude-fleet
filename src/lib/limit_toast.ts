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
import type { AttentionFacts, AttentionLimit } from './attention';
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
  const name =
    limit.window === 'five_hour'
      ? '5-hour'
      : limit.window === 'weekly_opus'
        ? 'weekly Opus'
        : limit.window === 'weekly_sonnet'
          ? 'weekly Sonnet'
          : 'weekly';
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
  // A model's own weekly window (r4 w07-5) pauses only the sessions launched
  // on that model: count those, not every session on the account.
  const family = limit.window === 'weekly_opus' ? 'opus' : limit.window === 'weekly_sonnet' ? 'sonnet' : null;
  const paused = (
    family
      ? pausedSessions({ uuid, sessions: mine }, {}, now, { [uuid]: { [family]: limit } })
      : pausedSessions({ uuid, sessions: mine }, { [uuid]: limit }, now)
  ).length;
  const { message, sub } = limitHitLines(account ? accountLabel(account) : uuid.slice(0, 8), limit, paused, now);
  return push({
    kind: 'warning',
    message,
    sub,
    action: paused > 0 ? { label: 'Show paused sessions', run: () => openPausedSessions(uuid) } : undefined,
  });
}

/** `limited_models` as one flat record, "<uuid> <family>" → its limit. */
function flatModels(models: AttentionFacts['limited_models']): Record<string, AttentionLimit> {
  const out: Record<string, AttentionLimit> = {};
  for (const [uuid, byFamily] of Object.entries(models ?? {})) {
    for (const [family, limit] of Object.entries(byFamily)) if (limit) out[`${uuid} ${family}`] = limit;
  }
  return out;
}

/** Watch usage readings and toast each new limit; returns the unsubscribe. */
export function startLimitToasts(): () => void {
  let prev: Readonly<Record<string, AttentionLimit>> = {};
  let prevModels: Readonly<Record<string, AttentionLimit>> = {};
  let seen = new Set<string>();
  return accountUsage.subscribe((usage: Record<string, AccountUsageSnapshot>) => {
    const now = Math.floor(Date.now() / 1000);
    const facts = attentionFactsFrom(get(hosts), usage, now);
    const next = facts.limited_accounts ?? {};
    const hit = newlyLimited(prev, next, seen, now);
    for (const uuid of hit) announceLimitHit(uuid, next[uuid], get(sessions), now);
    // A model's own weekly window filling pauses that model's sessions too
    // (`limited_models`), and said nothing (review r4 w07-5). One toast per
    // account at a time: the account's own limit, when it is new, says more.
    const nextModels = flatModels(facts.limited_models);
    const seenModels = new Set(Object.keys(nextModels).filter((k) => seen.has(k.split(' ')[0])));
    for (const key of newlyLimited(prevModels, nextModels, seenModels, now)) {
      const uuid = key.split(' ')[0];
      if (!next[uuid]) announceLimitHit(uuid, nextModels[key], get(sessions), now);
    }
    prev = next;
    prevModels = nextModels;
    seen = new Set(Object.keys(usage));
  });
}
