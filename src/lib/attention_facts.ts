// What the fleet knows beyond a session's own row (Orbit Fleet redesign step
// 2.4), built from the host and usage stores: which hosts are down, and which
// accounts are at a usage limit or have no usable login. `classify` reads it
// through `AttentionOptions.facts` to put a live session in one of the three
// Blocked buckets. Mirrors `attention::Facts::from_fleet` in fleet-core; the
// shared fixture (`attention_states.json`) checks both classifiers agree.
//
// The New layout only: Classic keeps its row-only classification, so its
// Needs you count does not move until the layout switch.
import { derived, type Readable } from 'svelte/store';
import type { HostRow } from './hosts';
import { hosts } from './hosts';
import type { AccountUsageSnapshot, UsageStatus } from './account_usage_store';
import { accountUsage } from './account_usage_store';
import type { AttentionFacts, AttentionLimit } from './attention';
import { uiLayout } from './prefs';

/** The usage windows' lengths, seconds. */
const FIVE_HOUR_SECS = 5 * 3600;
const WEEK_SECS = 7 * 86400;

/** Login states that need a person to sign in again. An expired access
 *  token refreshes by itself, and `no_credentials` is the usage script
 *  finding no token file, which on a macOS host (the token lives in the
 *  Keychain) says nothing about the login. */
const LOGIN_GONE: ReadonlySet<UsageStatus> = new Set(['login_expired', 'token_rejected']);

export function attentionFactsFrom(
  hostRows: readonly HostRow[],
  usage: Readonly<Record<string, AccountUsageSnapshot>>,
  now: number,
): AttentionFacts {
  const down_hosts = hostRows.filter((h) => !h.reachable && h.last_pinged_at != null).map((h) => h.alias);
  const limited_accounts: Record<string, AttentionLimit> = {};
  const uncredentialed_accounts: string[] = [];
  for (const snap of Object.values(usage)) {
    if (LOGIN_GONE.has(snap.status)) uncredentialed_accounts.push(snap.account_uuid);
    const u = snap.usage;
    if (!u) continue;
    // A window with no reset time counts only while the reading is younger
    // than the window itself (`Window::live_at` in fleet-core).
    const atLimit = (w: { utilization: number; resets_at: number | null } | null, len: number) =>
      !!w &&
      w.utilization >= 100 &&
      (w.resets_at != null ? w.resets_at > now : snap.fetched_at != null && now - snap.fetched_at < len);
    if (atLimit(u.seven_day, WEEK_SECS)) {
      limited_accounts[snap.account_uuid] = { window: 'weekly', resets_at: u.seven_day!.resets_at };
    } else if (atLimit(u.five_hour, FIVE_HOUR_SECS)) {
      limited_accounts[snap.account_uuid] = { window: 'five_hour', resets_at: u.five_hour!.resets_at };
    }
  }
  return { down_hosts, limited_accounts, uncredentialed_accounts };
}

/** The facts the stores hold now, or `undefined` in the Classic layout. The
 *  classifier re-checks a limit's reset against its own clock, so a limit
 *  that resets between two usage reads stops blocking on time. */
export const attentionFacts: Readable<AttentionFacts | undefined> = derived(
  [hosts, accountUsage, uiLayout],
  ([$hosts, $usage, $layout]) =>
    $layout === 'new' ? attentionFactsFrom($hosts, $usage, Math.floor(Date.now() / 1000)) : undefined,
);

/** The reason line of a Blocked row (step 2.4), as the Main board words it:
 *  "Paused · weekly limit on tech.silvester". `null` for any other bucket. */
export function blockedLine(
  bucket: string,
  sess: { host_alias: string; account_uuid: string | null },
  facts: AttentionFacts | undefined,
  accountName: (uuid: string) => string,
): string | null {
  switch (bucket) {
    case 'host_down':
      return `Blocked · ${sess.host_alias} is down`;
    case 'account_limit': {
      const uuid = sess.account_uuid ?? '';
      const w = facts?.limited_accounts?.[uuid]?.window === 'five_hour' ? '5-hour' : 'weekly';
      return `Paused · ${w} limit on ${accountName(uuid)}`;
    }
    case 'no_credentials':
      return `Blocked · ${accountName(sess.account_uuid ?? '')} is signed out`;
    default:
      return null;
  }
}
