// Per-host fleet-hook health for Settings → Hosts (Q8 / R10).
//
// "Installed" is inferred from the host's control-API token: both the
// Settings button / enable-time auto-install (local) and `provision_hosts`
// (remote) mint the token and write the hook block together. "Last event"
// is the newest `last_stop_at` among the host's sessions; that column is
// stamped only by the Stop hook, so it is a real hook delivery, not a
// reconcile guess. (UserPromptSubmit writes `last_hook_at`, which is not on
// the wire yet.)

import type { SessionRow } from './sessions';

export type HookHealth =
  | { state: 'not_installed' }
  | { state: 'never_seen' }
  | { state: 'seen'; lastAt: number };

/** Newest Stop-hook timestamp across `rows` for `alias`, or null. */
export function lastHookEventAt(
  alias: string,
  rows: readonly Pick<SessionRow, 'host_alias' | 'last_stop_at'>[],
): number | null {
  let best: number | null = null;
  for (const r of rows) {
    if (r.host_alias !== alias || r.last_stop_at == null) continue;
    if (best === null || r.last_stop_at > best) best = r.last_stop_at;
  }
  return best;
}

/** A host that delivered a hook is "seen" even without a token row (the
 *  token may have been revoked since); otherwise the token decides. */
export function hookHealth(
  alias: string,
  hasToken: boolean,
  rows: readonly Pick<SessionRow, 'host_alias' | 'last_stop_at'>[],
): HookHealth {
  const lastAt = lastHookEventAt(alias, rows);
  if (lastAt !== null) return { state: 'seen', lastAt };
  return hasToken ? { state: 'never_seen' } : { state: 'not_installed' };
}

/** Compact age: 12s, 5m, 3h, 2d. Clock skew (future) reads as 0s. */
export function formatAge(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

export function hookHealthLabel(h: HookHealth, nowSec: number): string {
  switch (h.state) {
    case 'not_installed':
      return 'not installed';
    case 'never_seen':
      return 'installed · never seen';
    case 'seen':
      return `last event ${formatAge(nowSec - h.lastAt)} ago`;
  }
}
