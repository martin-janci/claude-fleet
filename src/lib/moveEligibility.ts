// Who can move, and where to. One copy, shared by the terminal-header chip,
// the details-panel button and the Transfer sheet.
import type { HostRow } from './hosts';
import { hubActionBlocked, type HubStatus } from './hub';
import type { HubConnection } from './hub_connection';
import type { SessionRow } from './sessions';
import type { AccountUsageSnapshot } from './account_usage_store';
import { sizeText } from './hosts_table';

/** Only a worktree-backed work session with a Claude id can move: there is a
 *  branch to recreate and a conversation to resume. */
export function canMoveSession(s: SessionRow): boolean {
  return s.kind === 'work' && s.worktree_id !== null && s.claude_session_id !== null;
}

/** Other visible, reachable hosts that can run a session. */
export function moveTargetsFor(s: SessionRow, hosts: HostRow[]): HostRow[] {
  return hosts.filter(
    (h) =>
      h.alias !== s.host_alias && !h.hidden && h.reachable && (h.provisioned || h.alias === 'local'),
  );
}

/** Why a move cannot be started right now (a hub client that is offline), or null. */
export function moveBlockedReason(status: HubStatus, conn: HubConnection): string | null {
  return hubActionBlocked('move_session', status, conn);
}

/** One row of Move to host's radio list (Dialogs board): every visible host
 *  but hidden ones, the current one included and marked, each either
 *  pickable or disabled with the reason in plain words. */
export interface MoveChoice {
  alias: string;
  current: boolean;
  /** Why it cannot be picked, or null when it can. */
  disabled: string | null;
  /** `18 ms · 1.2 TB free`, from the last probe; null before one. */
  health: string | null;
}

/** Whether `uuid`'s 5-hour or weekly window is used up, from the usage cache. */
function accountAtLimit(uuid: string | null, usage: Readonly<Record<string, AccountUsageSnapshot>>): boolean {
  const u = uuid ? usage[uuid]?.usage : null;
  if (!u) return false;
  return [u.five_hour, u.seven_day].some((w) => w != null && Number.isFinite(w.utilization) && w.utilization >= 100);
}

/** Move to host's choices, in the order the hosts list holds them with the
 *  current host first. Pickable is exactly {@link moveTargetsFor} less the
 *  hosts whose login has no room left. */
export function moveChoicesFor(
  s: SessionRow,
  hosts: HostRow[],
  usage: Readonly<Record<string, AccountUsageSnapshot>> = {},
): MoveChoice[] {
  const eligible = new Set(moveTargetsFor(s, hosts).map((h) => h.alias));
  const rows = hosts.filter((h) => !h.hidden || h.alias === s.host_alias);
  rows.sort((a, b) => Number(b.alias === s.host_alias) - Number(a.alias === s.host_alias));
  return rows.map((h) => {
    const current = h.alias === s.host_alias;
    const parts: string[] = [];
    if (h.reachable && h.latency_ms != null) parts.push(`${h.latency_ms} ms`);
    if (h.reachable && h.disk_home_free_kb != null) parts.push(`${sizeText(h.disk_home_free_kb)} free`);
    let disabled: string | null = null;
    if (current) disabled = 'current';
    else if (!h.reachable) disabled = 'offline';
    else if (!eligible.has(h.alias)) disabled = 'not set up';
    else if (accountAtLimit(h.account_uuid, usage)) disabled = 'account at limit';
    return { alias: h.alias, current, disabled, health: parts.length > 0 ? parts.join(' · ') : null };
  });
}
