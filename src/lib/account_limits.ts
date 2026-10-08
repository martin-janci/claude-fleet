// Limit handling (Orbit Fleet redesign step 4.4): whether a login on a host
// is past `accounts.pause_at`, and which other login there has headroom.
// Mirrors `service::account_limits`; the cache it reads is local-only, so a
// hub client gets `E_HUB_LOCAL_ONLY` and simply starts as before.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

/** Mirrors `account_limits::HostLogin`. */
export interface HostLogin {
  /** `null` = the host's own login. */
  profile: string | null;
  account_uuid: string;
  used_pct: number | null;
}

/** Mirrors `account_limits::Headroom`. */
export interface Headroom {
  pause_at_pct: number;
  chosen: HostLogin | null;
  over: boolean;
  suggestion: HostLogin | null;
  logins: HostLogin[];
}

export function checkAccountHeadroom(hostAlias: string, profile: string | null): Promise<Result<Headroom>> {
  return invokeCmd<Headroom>('check_account_headroom', {
    args: { host_alias: hostAlias, profile },
  });
}

/** How a login reads in a choice: "work (m.janci@…)" or the account alone. */
export function loginLabel(l: HostLogin, accountName: (uuid: string) => string): string {
  const who = accountName(l.account_uuid);
  return l.profile ? `${l.profile} (${who})` : who;
}

/** "95% used" or "no reading". */
export function usedText(l: HostLogin): string {
  return l.used_pct == null ? 'no reading' : `${Math.round(l.used_pct)}% used`;
}

/** "Fri 11:00": when a paused row's limit resets, for its Wait button. */
export function resetText(resetsAt: number): string {
  return new Intl.DateTimeFormat(undefined, {
    weekday: 'short',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).format(new Date(resetsAt * 1000));
}

/** Rows a person chose to wait out, by session id, with the reset they wait
 *  for. In memory only: a paused row asks again after a restart of the app,
 *  and once its limit has reset it is no longer paused at all. */
export const waitingOut = writable<ReadonlyMap<number, number>>(new Map());

export function waitOut(sessionId: number, resetsAt: number): void {
  waitingOut.update((m) => new Map(m).set(sessionId, resetsAt));
}

/** Where a paused session would go: the login on its host with the most
 *  headroom, on another account. `null` when no login is under the line or
 *  the usage cache cannot be read (a hub client). */
export async function switchTarget(sess: {
  host_alias: string;
  claude_profile?: string | null;
}): Promise<HostLogin | null> {
  const h = await checkAccountHeadroom(sess.host_alias, sess.claude_profile ?? null);
  return h.ok ? (h.value?.suggestion ?? null) : null;
}

/** Bulk move (step 4.4): resume each session under the login with the most
 *  headroom on its host. Sessions already under the line stay put; the
 *  answer counts what moved, what had nowhere to go, and what failed. */
export async function moveToHeadroom(
  rows: readonly { host_alias: string; tmux_name: string; claude_profile?: string | null }[],
  restart: (host: string, name: string, profile: string) => Promise<Result<unknown>>,
): Promise<{ moved: number; stayed: number; nowhere: number; failed: number }> {
  const out = { moved: 0, stayed: 0, nowhere: 0, failed: 0 };
  for (const s of rows) {
    const h = await checkAccountHeadroom(s.host_alias, s.claude_profile ?? null);
    if (!h.ok) {
      out.failed += 1;
      continue;
    }
    if (!h.value.over) {
      out.stayed += 1;
      continue;
    }
    const to = h.value.suggestion;
    if (!to) {
      out.nowhere += 1;
      continue;
    }
    const r = await restart(s.host_alias, s.tmux_name, to.profile ?? '');
    if (r.ok) out.moved += 1;
    else out.failed += 1;
  }
  return out;
}

/** The New session dialog's default (step 4.5): the login with the most
 *  headroom among those with a reading; `null` when none has one. */
export function freestLogin(logins: readonly HostLogin[]): HostLogin | null {
  let best: HostLogin | null = null;
  for (const l of logins) {
    if (l.used_pct == null) continue;
    if (best == null || l.used_pct < (best.used_pct as number)) best = l;
  }
  return best;
}
