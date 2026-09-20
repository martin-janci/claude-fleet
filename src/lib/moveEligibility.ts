// Who can move, and where to. One copy, shared by the terminal-header chip,
// the details-panel button and the Transfer sheet.
import type { HostRow } from './hosts';
import { hubActionBlocked, type HubStatus } from './hub';
import type { HubConnection } from './hub_connection';
import type { SessionRow } from './sessions';

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
