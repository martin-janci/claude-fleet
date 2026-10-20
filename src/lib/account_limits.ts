// Limit handling (Orbit Fleet redesign step 4.4): whether a login on a host
// is past `accounts.pause_at`, and which other login there has headroom.
// Mirrors `service::account_limits`. A hub client asks the hub, which
// answers from the usage it followed (hub contract 14).
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { formatResetShort } from './account_usage';

/** Mirrors `account_limits::HostLogin`. */
export interface HostLogin {
  /** `null` = the host's own login. */
  profile: string | null;
  account_uuid: string;
  used_pct: number | null;
  /** Fleet knows this login is signed out: a start or switch onto it opens
   *  `/login`, whatever its last reading says. Absent = signed in (or an
   *  older backend). */
  signed_out?: boolean;
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

/** "95% used", "no reading", or "signed out". */
export function usedText(l: HostLogin): string {
  if (l.signed_out) return 'signed out';
  return l.used_pct == null ? 'no reading' : `${Math.round(l.used_pct)}% used`;
}

/** When a paused row's limit resets, for its Wait button, the Wait toast
 *  and the routine hold line, worded as the Accounts card words it
 *  (`formatResetShort`, review w03-12): "15:10" for a reset within five
 *  hours (a 5-hour window's), "Fri 11:00" within the week, and "Oct 26
 *  11:00" six days or more ahead, where a weekday alone would read as
 *  today's. */
export function resetText(
  resetsAt: number,
  opts: { now?: number; locale?: string; timeZone?: string } = {},
): string {
  const now = opts.now ?? Math.floor(Date.now() / 1000);
  const ahead = resetsAt - now;
  if (ahead >= 6 * 86_400 && Number.isFinite(resetsAt)) {
    const parts = new Intl.DateTimeFormat(opts.locale, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      hourCycle: 'h23',
      timeZone: opts.timeZone,
    }).formatToParts(new Date(resetsAt * 1000));
    const get = (k: string) => parts.find((p) => p.type === k)?.value ?? '';
    return `${get('month')} ${get('day')} ${get('hour')}:${get('minute')}`;
  }
  return formatResetShort(ahead <= 5 * 3600 ? '5h' : 'weekly', resetsAt, now, opts.locale, opts.timeZone).replace(
    /^resets? (at )?/,
    '',
  );
}

/** Rows a person chose to wait out, by session id, with the reset they wait
 *  for. In memory only: a paused row asks again after a restart of the app,
 *  and once its limit has reset it is no longer paused at all. */
export const waitingOut = writable<ReadonlyMap<number, number>>(new Map());

export function waitOut(sessionId: number, resetsAt: number): void {
  waitingOut.update((m) => new Map(m).set(sessionId, resetsAt));
}

type SwitchSession = {
  host_alias: string;
  claude_profile?: string | null;
  account_uuid?: string | null;
};

/** Where a paused session would go: the login on its host with the most
 *  headroom, on another account. `null` when no login is under the line or
 *  the check failed; {@link switchCheck} tells the two apart. */
export async function switchTarget(sess: SwitchSession): Promise<HostLogin | null> {
  const r = await switchCheck(sess);
  return r.ok ? r.value.target : null;
}

/** {@link switchTarget} with the whole answer (review w03-9): the target,
 *  the host's logins, and an error kept as an error, so "no room" is never
 *  said for a check that did not run. */
export async function switchCheck(
  sess: SwitchSession,
  opts: { signedOut?: boolean } = {},
): Promise<Result<{ target: HostLogin | null; headroom: Headroom | null }>> {
  const h = await checkAccountHeadroom(sess.host_alias, sess.claude_profile ?? null);
  if (!h.ok) return h;
  // An empty answer (an older backend): nothing to propose.
  if (!h.value) return { ok: true, value: { target: null, headroom: null } };
  const headroom = headroomForAccount(h.value, sess.account_uuid);
  if (opts.signedOut) {
    // A session whose login is signed out (review w03-3) goes to any other
    // signed-in login with room, whether or not its own account is over.
    const own = sess.claude_profile ?? null;
    const others = headroom.logins.filter(
      (l) => (l.profile ?? null) !== own && !l.signed_out && (l.used_pct == null || l.used_pct < headroom.pause_at_pct),
    );
    return { ok: true, value: { target: freestLogin(others) ?? others[0] ?? null, headroom } };
  }
  return { ok: true, value: { target: headroom.suggestion, headroom } };
}

/** The soonest a login on the host frees up, for the "no room" line (review
 *  w03-9): per signed-in login, the latest reset among its windows at or
 *  past the line (the login is over until all of them reset), then the
 *  soonest of those that is still ahead. `null` without one. */
export function nextReset(
  h: Headroom,
  usage: Readonly<Record<string, { usage: { five_hour: Win; seven_day: Win } | null } | undefined>>,
  now: number,
): { login: HostLogin; at: number } | null {
  let best: { login: HostLogin; at: number } | null = null;
  const seen = new Set<string>();
  for (const l of h.logins) {
    if (l.signed_out || seen.has(l.account_uuid)) continue;
    seen.add(l.account_uuid);
    const u = usage[l.account_uuid]?.usage;
    if (!u) continue;
    const over = [u.five_hour, u.seven_day].filter(
      (w): w is NonNullable<Win> => w != null && w.utilization >= h.pause_at_pct && w.resets_at != null,
    );
    if (over.length === 0) continue;
    const at = Math.max(...over.map((w) => w.resets_at as number));
    if (at > now && (best == null || at < best.at)) best = { login: l, at };
  }
  return best;
}
type Win = { utilization: number; resets_at: number | null } | null;

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
      if (l.account_uuid === chosen.account_uuid || l.signed_out || l.used_pct == null || l.used_pct >= h.pause_at_pct) continue;
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
    const to = h.logins.find((l) => l.account_uuid === accountUuid && !l.signed_out);
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
 *  headroom among the signed-in ones with a reading; `null` when none has
 *  one. */
export function freestLogin(logins: readonly HostLogin[]): HostLogin | null {
  let best: HostLogin | null = null;
  for (const l of logins) {
    if (l.used_pct == null || l.signed_out) continue;
    if (best == null || l.used_pct < (best.used_pct as number)) best = l;
  }
  return best;
}
