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
  account_uuid?: string | null;
}): Promise<HostLogin | null> {
  const h = await checkAccountHeadroom(sess.host_alias, sess.claude_profile ?? null);
  return h.ok && h.value ? headroomForAccount(h.value, sess.account_uuid).suggestion : null;
}

/** The headroom answer for the account a session actually bills. The
 *  check reads the login by profile, but a row's `account_uuid` is what it
 *  runs on (a profile can be re-logged into another account since the
 *  start), so when the row names an account the host has a login for, `over`
 *  and the suggestion are read for that account, as `account_limits::
 *  headroom` reads them for a profile (review r05). */
export function headroomForAccount(h: Headroom, accountUuid: string | null | undefined): Headroom {
  const chosen = accountUuid ? h.logins.find((l) => l.account_uuid === accountUuid) : undefined;
  if (!chosen) return h;
  const over = chosen.used_pct != null && chosen.used_pct >= h.pause_at_pct;
  let suggestion: HostLogin | null = null;
  if (over) {
    for (const l of h.logins) {
      if (l.account_uuid === chosen.account_uuid || l.used_pct == null || l.used_pct >= h.pause_at_pct) continue;
      if (suggestion == null || l.used_pct < (suggestion.used_pct as number)) suggestion = l;
    }
  }
  return { ...h, chosen, over, suggestion };
}

/** Bulk move (step 4.4): resume each session under the login with the most
 *  headroom on its host. Sessions already under the line stay put; the
 *  answer counts what moved, what had nowhere to go, and what failed. */
export async function moveToHeadroom(
  rows: readonly {
    host_alias: string;
    tmux_name: string;
    claude_profile?: string | null;
    account_uuid?: string | null;
  }[],
  restart: (host: string, name: string, profile: string) => Promise<Result<unknown>>,
): Promise<{ moved: number; stayed: number; nowhere: number; failed: number }> {
  const out = { moved: 0, stayed: 0, nowhere: 0, failed: 0 };
  for (const s of rows) {
    const r0 = await checkAccountHeadroom(s.host_alias, s.claude_profile ?? null);
    if (!r0.ok) {
      out.failed += 1;
      continue;
    }
    const h = headroomForAccount(r0.value, s.account_uuid);
    if (!h.over) {
      out.stayed += 1;
      continue;
    }
    const to = h.suggestion;
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

/** Bulk move to an account the person picked (step 4.4): resume each
 *  session under the login on its host that is logged in to `accountUuid`.
 *  `null` is the default, the login with the most headroom
 *  ({@link moveToHeadroom}). A session already on that account stays; one
 *  whose host has no login on it has nowhere to go. A picked account moves
 *  a session whether or not it is past the line: the person chose it. */
export async function moveToAccount(
  rows: Parameters<typeof moveToHeadroom>[0],
  accountUuid: string | null,
  restart: (host: string, name: string, profile: string) => Promise<Result<unknown>>,
): Promise<{ moved: number; stayed: number; nowhere: number; failed: number }> {
  if (accountUuid === null) return moveToHeadroom(rows, restart);
  const out = { moved: 0, stayed: 0, nowhere: 0, failed: 0 };
  for (const s of rows) {
    const r0 = await checkAccountHeadroom(s.host_alias, s.claude_profile ?? null);
    if (!r0.ok) {
      out.failed += 1;
      continue;
    }
    const h = headroomForAccount(r0.value, s.account_uuid);
    if ((s.account_uuid ?? h.chosen?.account_uuid) === accountUuid) {
      out.stayed += 1;
      continue;
    }
    const to = h.logins.find((l) => l.account_uuid === accountUuid);
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
