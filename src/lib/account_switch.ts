// Automatic account switching (docs/accounts.md, *Switch automatically*).
// When the fleet's `accounts.auto_switch` is on, the backend resumes a
// session whose account ran out under the login on its host with the most
// headroom, unless the session's owner turned it off for their own sessions
// (`auto_switch_account`, the Accounts page's toggle). Each attempt lands on
// the session's timeline as `account_switched` / `account_switch_failed`;
// this module reads those for the toast and the conversation thread.
import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { SessionEvent } from './timeline';
import { sessions, showFriendlyNames } from './sessions';
import { accountByUuid, accountLabel } from './accounts';
import { displayName } from './attention';
import { selectSessionExplicitly } from './selection';
import { push } from './toasts';

export const EVENT_SWITCHED = 'account_switched';
export const EVENT_SWITCH_FAILED = 'account_switch_failed';

export interface AutoSwitchView {
  /** The signed-in person's own answer; on unless they turned it off. */
  enabled: boolean;
  /** After a switch, type "Continue where you left off." into a session
   *  whose turn the limit cut off; off unless turned on. */
  continue_after: boolean;
  /** The fleet's `accounts.auto_switch`: without it nothing switches. */
  fleet_enabled: boolean;
}

export interface AutoSwitchPatch {
  enabled?: boolean;
  continue_after?: boolean;
}

/** Read the caller's answer, or change part of it. */
export function autoSwitchAccount(patch: AutoSwitchPatch = {}): Promise<Result<AutoSwitchView>> {
  return invokeCmd<AutoSwitchView>('auto_switch_account', { args: patch });
}

/** The signed-in person's answer as last read; `null` until read (or on a
 *  hub too old to answer). Shared by the Accounts page and every paused row. */
export const autoSwitchState = writable<AutoSwitchView | null>(null);
let loading: Promise<void> | null = null;

/** Read the answer once per window (again with `force`). */
export function loadAutoSwitch(force = false): Promise<void> {
  if (loading && !force) return loading;
  loading = autoSwitchAccount().then((r) => {
    if (r.ok && r.value) autoSwitchState.set(r.value);
  });
  return loading;
}

/** Change the answer and keep the shared copy in step. */
export async function setAutoSwitch(patch: AutoSwitchPatch): Promise<Result<AutoSwitchView>> {
  const r = await autoSwitchAccount(patch);
  if (r.ok && r.value) autoSwitchState.set(r.value);
  return r;
}

/** Whether a paused session will be moved without anyone pressing Switch:
 *  the fleet has it on and so does the person reading. (The row's owner
 *  decides on the hub; a person sees their own sessions, so their answer is
 *  the one that applies to what they see.) */
export function switchesAutomatically(v: AutoSwitchView | null): boolean {
  return !!v && v.fleet_enabled && v.enabled;
}

/** An attempt's timeline detail (`service::account_switch::detail`). */
export interface SwitchDetail {
  from_account: string;
  to_account: string;
  profile: string | null;
  used_pct: number | null;
  error: string | null;
  /** "Continue where you left off." was queued after the switch. */
  continued: boolean;
}

export function parseSwitchDetail(detail: string | null): SwitchDetail | null {
  if (!detail) return null;
  try {
    const d = JSON.parse(detail) as Partial<SwitchDetail>;
    if (typeof d.from_account !== 'string' || typeof d.to_account !== 'string') return null;
    return {
      from_account: d.from_account,
      to_account: d.to_account,
      profile: typeof d.profile === 'string' ? d.profile : null,
      used_pct: typeof d.used_pct === 'number' ? d.used_pct : null,
      error: typeof d.error === 'string' ? d.error : null,
      continued: (d as { continue?: unknown }).continue === true,
    };
  } catch {
    return null;
  }
}

/** "work (ada@…)" for a profile, the account alone for the host's login. */
export function targetLabel(d: SwitchDetail, accountName: (uuid: string) => string): string {
  const who = accountName(d.to_account);
  return d.profile ? `${d.profile} (${who})` : who;
}

/** The toast's two lines for one attempt on session `name`. */
export function switchLines(
  kind: string,
  name: string,
  d: SwitchDetail,
  accountName: (uuid: string) => string,
): { message: string; sub: string } {
  const from = accountName(d.from_account);
  const to = targetLabel(d, accountName);
  if (kind === EVENT_SWITCH_FAILED) {
    return {
      message: `Could not move ${name} off ${from}`,
      sub: `Switching to ${to} failed${d.error ? ` (${d.error})` : ''}; it stays paused`,
    };
  }
  const used = d.used_pct == null ? '' : ` · ${Math.round(d.used_pct)}% used`;
  const carry = d.continued ? ' · told to continue' : '';
  return { message: `${name} resumed under ${to}`, sub: `${from} ran out${used}${carry}` };
}

/** Toast the automatic switches among `events` (the timeline push): one
 *  toast per attempt, or one for all of them when several sessions moved in
 *  the same push (a whole account running out). */
export function announceAccountSwitches(events: readonly SessionEvent[]): void {
  const rows = get(sessions);
  const friendly = get(showFriendlyNames);
  const byUuid = get(accountByUuid);
  const accountName = (uuid: string) => {
    const a = byUuid.get(uuid);
    return a ? accountLabel(a) : uuid.slice(0, 8);
  };
  const attempts = events.flatMap((e) => {
    if (e.kind !== EVENT_SWITCHED && e.kind !== EVENT_SWITCH_FAILED) return [];
    const d = parseSwitchDetail(e.detail);
    if (!d) return [];
    const row = rows.find((s) => s.id === e.session_id);
    return [{ kind: e.kind, d, row, name: row ? displayName(row, friendly) : `session ${e.session_id}` }];
  });
  const switched = attempts.filter((a) => a.kind === EVENT_SWITCHED);
  if (switched.length > 1) {
    const from = [...new Set(switched.map((a) => accountName(a.d.from_account)))].join(', ');
    push({
      kind: 'info',
      message: `${switched.length} sessions resumed under other logins`,
      sub: `${from} ran out · ${switched.map((a) => a.name).join(', ')}`,
    });
  }
  for (const a of attempts) {
    if (a.kind === EVENT_SWITCHED && switched.length > 1) continue;
    const { message, sub } = switchLines(a.kind, a.name, a.d, accountName);
    const row = a.row;
    push({
      kind: a.kind === EVENT_SWITCHED ? 'info' : 'warning',
      message,
      sub,
      action: row ? { label: 'Open', run: () => selectSessionExplicitly(row) } : undefined,
    });
  }
}
