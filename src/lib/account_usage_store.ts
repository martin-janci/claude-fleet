import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

/**
 * Mirrors `service::account_usage::UsageOutcomeKind` (`#[serde(rename_all =
 * "snake_case")]`). Every variant that enum can serialize must appear here —
 * see `account_usage_store.test.ts` for a runtime fixture that checks this.
 */
export type UsageStatus =
  | 'ok'
  | 'no_credentials'
  | 'access_token_expired'
  | 'login_expired'
  | 'token_rejected'
  | 'rate_limited'
  | 'unavailable'
  | 'no_online_host'
  | 'never_fetched'
  | 'host_unsupported';

/** Mirrors `service::account_usage::Window`. */
export interface UsageWindow {
  utilization: number;
  /** Unix seconds; `null` when absent or not RFC 3339. */
  resets_at: number | null;
}

/** Mirrors `service::account_usage::AccountUsage`. Any bucket may be absent. */
export interface AccountUsage {
  five_hour: UsageWindow | null;
  seven_day: UsageWindow | null;
  seven_day_opus: UsageWindow | null;
  seven_day_sonnet: UsageWindow | null;
}

/**
 * Mirrors `service::account_usage::AccountUsageSnapshot`.
 *
 * `next_try_at` is a plain `number`, NOT `number | null`: the Rust field is
 * `i64` (not `Option<i64>`) — `0` when no attempt has ever been scheduled,
 * else the unix second the floor next allows one.
 */
export interface AccountUsageSnapshot {
  account_uuid: string;
  /** Last-known usage (from the last `ok`), even when `status` is a failure. */
  usage: AccountUsage | null;
  subscription: string | null;
  /** When `usage` was fetched (unix seconds). */
  fetched_at: number | null;
  source_host: string | null;
  status: UsageStatus;
  detail: string | null;
  next_try_at: number;
}

/** Keyed by `account_uuid`. */
export const accountUsage = writable<Record<string, AccountUsageSnapshot>>({});

function mergeInto(
  map: Record<string, AccountUsageSnapshot>,
  row: AccountUsageSnapshot,
): Record<string, AccountUsageSnapshot> {
  if (!row) return map;
  return { ...map, [row.account_uuid]: row };
}

/** Fetch every known account's cached usage snapshot. Never triggers a fetch. */
export async function loadAccountUsage(): Promise<Result<AccountUsageSnapshot[]>> {
  const r = await invokeCmd<AccountUsageSnapshot[]>('list_account_usage');
  // Defensive, like `loadTasks`: a mocked / older backend may answer with
  // nothing.
  if (r.ok && Array.isArray(r.value)) {
    const map: Record<string, AccountUsageSnapshot> = {};
    for (const row of r.value) map[row.account_uuid] = row;
    accountUsage.set(map);
  }
  return r;
}

/**
 * Fetch `accountUuid`'s usage now if the floor allows, else the current
 * snapshot is returned unchanged (its `next_try_at` says when a refresh
 * becomes possible). Either way the result is merged into the store.
 */
export async function refreshAccountUsage(
  accountUuid: string,
): Promise<Result<AccountUsageSnapshot>> {
  const r = await invokeCmd<AccountUsageSnapshot>('refresh_account_usage', {
    args: { account_uuid: accountUuid },
  });
  if (r.ok) accountUsage.update((m) => mergeInto(m, r.value));
  return r;
}

/** Apply a burst of `account_usage:updated` rows in ONE store update. */
export function applyAccountUsageEvents(rows: readonly AccountUsageSnapshot[]): void {
  if (rows.length === 0) return;
  accountUsage.update((m) => rows.reduce(mergeInto, m));
}
