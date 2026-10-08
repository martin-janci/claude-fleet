// The session row's ⋯ menu and right-click (Orbit Fleet redesign step 3.10):
// every action the Details pane offers on a session, from the row itself.
//
// The menu does not run an action: it opens the session and asks Details to
// run it (`requestSessionAction`), so every confirm, refusal and dialog stays
// the one Details already has. This table says which actions a row offers and
// why one is disabled, by the same rules as Details' own buttons
// (`SessionDetails.svelte`); `session_actions.test.ts` holds the two together.
//
// The move lifecycle (Move back, Finish, Undo, a pending wait) and Switch login
// are read from the session's timeline and host, which only Details loads: the
// menu's "Details…" opens the pane where they live.
import { derived, get, writable, type Readable } from 'svelte/store';
import { hasNoPane, isInactiveAgent, type SessionRow } from './sessions';
import { canMoveSession, moveBlockedReason } from './moveEligibility';
import { hubActionBlocked, hubBlock, hubStatus, type HubStatus } from './hub';
import { hubConnection, type HubConnection } from './hub_connection';
import { sessionBlocked, type SessionAction } from './share';
import { selectSessionExplicitly } from './selection';

export type SessionActionId =
  | 'label'
  | 'rename'
  | 'restart'
  | 'repair'
  | 'send_prompt'
  | 'review'
  | 'recreate'
  | 'move'
  | 'share'
  | 'remove_from_list'
  | 'safe_remove'
  | 'kill';

export interface SessionActionDef {
  id: SessionActionId;
  label: string;
  /** The Details button that runs the same action. */
  detailsTestId: string;
  danger?: boolean;
}

/** In Details' order: destructive actions last. */
export const ROW_ACTIONS: readonly SessionActionDef[] = [
  { id: 'label', label: 'Edit label', detailsTestId: 'label-from-details' },
  { id: 'rename', label: 'Rename tmux session', detailsTestId: 'rename-from-details' },
  { id: 'restart', label: 'Restart', detailsTestId: 'restart-from-details' },
  { id: 'repair', label: 'Repair workspace', detailsTestId: 'repair-from-details' },
  { id: 'send_prompt', label: 'Send prompt', detailsTestId: 'send-prompt-from-details' },
  { id: 'review', label: 'Review', detailsTestId: 'open-review' },
  { id: 'recreate', label: 'Recreate', detailsTestId: 'recreate-from-details' },
  { id: 'move', label: 'Move to host…', detailsTestId: 'move-from-details' },
  { id: 'share', label: 'Share…', detailsTestId: 'share-from-details' },
  { id: 'remove_from_list', label: 'Remove from list', detailsTestId: 'remove-from-list-details' },
  { id: 'safe_remove', label: 'Safe remove', detailsTestId: 'safe-kill-from-details' },
  { id: 'kill', label: 'Kill session', detailsTestId: 'kill-from-details', danger: true },
];

/** Whether Details shows the action's button for this row. */
export function sessionActionShown(id: SessionActionId, s: SessionRow): boolean {
  if (id === 'label') return true;
  if (s.kind === 'external') return false;
  switch (id) {
    case 'repair':
      return !hasNoPane(s) && s.project_id !== null;
    case 'send_prompt':
      return s.kind !== 'shell';
    case 'move':
      return canMoveSession(s);
    case 'remove_from_list':
      return isInactiveAgent(s);
    case 'safe_remove':
      return !isInactiveAgent(s) && s.kind !== 'shell' && s.status === 'running' && s.safe_kill_state !== 'requested';
    case 'kill':
      return !isInactiveAgent(s);
    default:
      return true;
  }
}

type BlockedOf = (session: SessionRow, action: SessionAction) => string | null;

// The hub half of each gate, as Details composes it: the routed ones only while
// the hub link is up, the three local-SSH ones (`hubBlock`) on a hub client
// at all. The session half (`sessionBlocked`) is the same action name.
const GATES: Record<SessionActionId, { action: SessionAction; local?: boolean; move?: boolean }> = {
  label: { action: 'set_friendly_name' },
  rename: { action: 'rename_session' },
  restart: { action: 'restart_session' },
  repair: { action: 'repair_session' },
  send_prompt: { action: 'send_prompt' },
  review: { action: 'spawn_review' },
  recreate: { action: 'recreate_session' },
  move: { action: 'move_session', move: true },
  share: { action: 'session_share' },
  remove_from_list: { action: 'dismiss_agent_session', local: true },
  safe_remove: { action: 'inspect_safe_kill', local: true },
  kill: { action: 'kill_session' },
};

/** Why the action is disabled for this row, or null. */
export function sessionActionBlocked(
  id: SessionActionId,
  s: SessionRow,
  status: HubStatus,
  conn: HubConnection,
  blockedOf: BlockedOf,
): string | null {
  const g = GATES[id];
  const hub = g.move
    ? moveBlockedReason(status, conn)
    : g.local
      ? hubBlock(g.action as Parameters<typeof hubBlock>[0], status)
      : hubActionBlocked(g.action as Parameters<typeof hubActionBlocked>[0], status, conn);
  return hub ?? blockedOf(s, g.action);
}

export interface SessionMenuItem extends SessionActionDef {
  blocked: string | null;
}

/** The row menu's items for `s`, as Details would offer them. */
export const sessionMenuItems: Readable<(s: SessionRow) => SessionMenuItem[]> = derived(
  [hubStatus, hubConnection, sessionBlocked],
  ([$status, $conn, $blocked]) =>
    (s: SessionRow) =>
      ROW_ACTIONS.filter((a) => sessionActionShown(a.id, s)).map((a) => ({
        ...a,
        blocked: sessionActionBlocked(a.id, s, $status, $conn, $blocked),
      })),
);

/** A row asked Details to run an action on its session. */
export interface SessionActionRequest {
  sessionId: number;
  action: SessionActionId | 'details';
  seq: number;
}

export const sessionActionRequest = writable<SessionActionRequest | null>(null);
let seq = 0;

/** Open `s` and have Details run `action` on it ("details" only opens it). */
export function requestSessionAction(s: SessionRow, action: SessionActionId | 'details'): void {
  selectSessionExplicitly(s);
  sessionActionRequest.set({ sessionId: s.id, action, seq: ++seq });
}

/** Details takes the request meant for `sessionId`, once. */
export function takeSessionAction(sessionId: number): SessionActionRequest | null {
  const r = get(sessionActionRequest);
  if (!r || r.sessionId !== sessionId) return null;
  sessionActionRequest.set(null);
  return r;
}
