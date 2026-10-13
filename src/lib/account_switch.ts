// Automatic account switching (docs/accounts.md, *Switch automatically*).
// When the fleet's `accounts.auto_switch` is on, the backend resumes a
// session whose account ran out under the login on its host with the most
// headroom, unless the session's owner turned it off for their own sessions
// (`auto_switch_account`, the Accounts page's toggle). Each attempt lands on
// the session's timeline as `account_switched` / `account_switch_failed`;
// this module reads those for the toast and the conversation thread.
import { get } from 'svelte/store';
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
  /** The fleet's `accounts.auto_switch`: without it nothing switches. */
  fleet_enabled: boolean;
}

/** Read the caller's answer, or set it with `enabled`. */
export function autoSwitchAccount(enabled?: boolean): Promise<Result<AutoSwitchView>> {
  return invokeCmd<AutoSwitchView>('auto_switch_account', {
    args: enabled === undefined ? {} : { enabled },
  });
}

/** An attempt's timeline detail (`service::account_switch::detail`). */
export interface SwitchDetail {
  from_account: string;
  to_account: string;
  profile: string | null;
  used_pct: number | null;
  error: string | null;
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
  return { message: `${name} resumed under ${to}`, sub: `${from} ran out${used}` };
}

/** Toast every automatic switch among `events` (the timeline push). */
export function announceAccountSwitches(events: readonly SessionEvent[]): void {
  for (const e of events) {
    if (e.kind !== EVENT_SWITCHED && e.kind !== EVENT_SWITCH_FAILED) continue;
    const d = parseSwitchDetail(e.detail);
    if (!d) continue;
    const row = get(sessions).find((s) => s.id === e.session_id);
    const name = row ? displayName(row, get(showFriendlyNames)) : `session ${e.session_id}`;
    const byUuid = get(accountByUuid);
    const accountName = (uuid: string) => {
      const a = byUuid.get(uuid);
      return a ? accountLabel(a) : uuid.slice(0, 8);
    };
    const { message, sub } = switchLines(e.kind, name, d, accountName);
    push({
      kind: e.kind === EVENT_SWITCHED ? 'info' : 'warning',
      message,
      sub,
      action: row ? { label: 'Open', run: () => selectSessionExplicitly(row) } : undefined,
    });
  }
}
