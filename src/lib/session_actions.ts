// The session row's ⋯ menu and right-click (Orbit Fleet redesign step 3.10):
// every action the Details pane offers on a session, from the row itself.
//
// The menu does not run an action: it opens the session and asks Details to
// run it (`requestSessionAction`), so every confirm, refusal and dialog stays
// the one Details already has. This table says which actions a row offers and
// why one is disabled, by the same rules as Details' own buttons
// (`SessionDetails.svelte`); `session_actions.test.ts` holds the two together.
//
// The move lifecycle (Move back, Finish, Undo, a pending wait) is read from
// the session's timeline, which only Details loads: the menu's "Details…"
// opens the pane where it lives.
//
// One registry (gap plan step G1.11): the row menu, both Details layouts and
// the ⌘K palette's "This session" commands all read this table, so an action
// added here is offered everywhere with the same gate. Every entry runs a
// backend command that exists today; one whose backend does not exist yet
// (a read-only link) is not listed, never shown as a dead button.
import { derived, get, writable, type Readable } from 'svelte/store';
import { hasNoPane, isInactiveAgent, type SessionRow } from './sessions';
import { hostByAlias } from './hosts';
import { canMoveSession, moveBlockedReason } from './moveEligibility';
import { hubActionBlocked, hubBlock, hubStatus, type HubStatus } from './hub';
import { hubConnection, type HubConnection } from './hub_connection';
import { sessionBlocked, type SessionAction } from './share';
import { selectSessionExplicitly } from './selection';
import { CONV_MAX_TURNS, pickerCommand, sessionConversation, transcriptMarkdown } from './conversation';
import { copyText } from './clipboard';
import { push, pushError } from './toasts';
import { announceArchive, archiveSessions } from './kill_check';
import { outbox } from './outbox';

export type SessionActionId =
  | 'label'
  | 'rename'
  | 'restart'
  | 'repair'
  | 'send_prompt'
  | 'review'
  | 'fork'
  | 'rewind'
  | 'switch_account'
  | 'change_model'
  | 'recreate'
  | 'move'
  | 'share'
  | 'copy_transcript'
  | 'archive'
  | 'remove_from_list'
  | 'safe_remove'
  | 'kill';

export interface SessionActionDef {
  id: SessionActionId;
  label: string;
  /** The Details button that runs the same action. */
  detailsTestId: string;
  danger?: boolean;
  /** Other words the ⌘K palette finds it by. */
  synonyms?: readonly string[];
}

/** In Details' order (SessionDetails board): Steer, the Reviews block's
 *  Start a review run, Place, Share, then the destructive ones last. */
export const ROW_ACTIONS: readonly SessionActionDef[] = [
  { id: 'send_prompt', label: 'Send prompt…', detailsTestId: 'send-prompt-from-details', synonyms: ['prompt', 'message', 'send'] },
  { id: 'fork', label: 'Fork…', detailsTestId: 'fork-from-details', synonyms: ['fork', 'branch', 'copy', 'duplicate'] },
  { id: 'rewind', label: 'Rewind…', detailsTestId: 'rewind-from-details', synonyms: ['rewind', 'undo', 'back', 'turn'] },
  { id: 'switch_account', label: 'Switch account…', detailsTestId: 'switch-account-from-details', synonyms: ['account', 'login', 'profile', 'limit'] },
  { id: 'change_model', label: 'Change model…', detailsTestId: 'change-model-from-details', synonyms: ['model', 'opus', 'sonnet', 'haiku'] },
  { id: 'review', label: 'Start a review run…', detailsTestId: 'open-review', synonyms: ['review', 'check'] },
  { id: 'move', label: 'Move to host…', detailsTestId: 'move-from-details', synonyms: ['move', 'host', 'transfer'] },
  { id: 'restart', label: 'Restart…', detailsTestId: 'restart-from-details', synonyms: ['restart', 'reload'] },
  { id: 'recreate', label: 'Recreate…', detailsTestId: 'recreate-from-details', synonyms: ['recreate'] },
  { id: 'repair', label: 'Repair workspace…', detailsTestId: 'repair-from-details', synonyms: ['repair', 'worktree', 'fix'] },
  { id: 'label', label: 'Rename and label…', detailsTestId: 'label-from-details', synonyms: ['label', 'name', 'rename', 'tag'] },
  { id: 'rename', label: 'Rename tmux session', detailsTestId: 'rename-from-details', synonyms: ['tmux'] },
  { id: 'share', label: 'Share…', detailsTestId: 'share-from-details', synonyms: ['share', 'grant', 'watch', 'answer', 'drive', 'steer', 'visibility', 'private'] },
  { id: 'copy_transcript', label: 'Copy transcript', detailsTestId: 'copy-transcript-from-details', synonyms: ['copy', 'transcript', 'conversation', 'export'] },
  { id: 'archive', label: 'Archive', detailsTestId: 'archive-from-details', synonyms: ['archive', 'done', 'hide'] },
  { id: 'remove_from_list', label: 'Remove from list', detailsTestId: 'remove-from-list-details', synonyms: ['remove', 'dismiss'] },
  { id: 'safe_remove', label: 'Safe remove…', detailsTestId: 'safe-kill-from-details', synonyms: ['clean up', 'remove'] },
  { id: 'kill', label: 'Kill session…', detailsTestId: 'kill-from-details', danger: true, synonyms: ['kill', 'stop', 'end'] },
];

/** A session that runs a Claude conversation in a pane fleet can restart:
 *  what Fork, Rewind and Change model need (a `bg` or `external` agent has
 *  no pane to respawn, a shell has no conversation). */
function hasPaneConversation(s: SessionRow): boolean {
  return !hasNoPane(s) && s.kind !== 'shell' && s.claude_session_id != null;
}

/** Whether Details shows the action's button for this row. */
export function sessionActionShown(id: SessionActionId, s: SessionRow): boolean {
  if (id === 'label') return true;
  if (s.kind === 'external') return false;
  switch (id) {
    case 'repair':
      return !hasNoPane(s) && s.project_id !== null;
    case 'send_prompt':
      return s.kind !== 'shell';
    case 'fork':
    case 'rewind':
      return hasPaneConversation(s);
    case 'switch_account':
    case 'change_model':
      return !hasNoPane(s) && s.kind !== 'shell';
    case 'copy_transcript':
      return s.kind !== 'shell' && s.claude_session_id != null;
    case 'archive':
      // Archive puts a session in its work's Done (tidy-up's `archive`): a
      // session with no work linked has nowhere to go, and an archived one
      // is un-archived from its work group.
      return s.work != null && s.work.archived_at == null;
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
  fork: { action: 'rewind_conversation' },
  rewind: { action: 'rewind_conversation' },
  // Switching the account is a restart under another login.
  switch_account: { action: 'restart_session' },
  // `/model <alias>` typed into the pane, as the composer's picker sends it.
  change_model: { action: 'send_prompt' },
  recreate: { action: 'recreate_session' },
  move: { action: 'move_session', move: true },
  share: { action: 'session_share' },
  // A read: a watcher may copy what they may read.
  copy_transcript: { action: 'session_conversation' },
  archive: { action: 'tidy_apply' },
  remove_from_list: { action: 'dismiss_agent_session', local: true },
  safe_remove: { action: 'inspect_safe_kill', local: true },
  kill: { action: 'kill_session' },
};

/** The logins a session's host lists (its `claude_profiles`). */
type HostLogins = { claude_profiles?: readonly { name: string }[] | null } | null | undefined;

/** Why Switch account has nothing to switch to, or null: a session on the
 *  host's own login, on a host with no login profile, has no other login. */
export function noOtherLogin(s: SessionRow, host: HostLogins): string | null {
  if (s.claude_profile) return null;
  if ((host?.claude_profiles ?? []).length > 0) return null;
  return `${s.host_alias} has no other login. Add a login profile under Accounts first.`;
}

/** Why the action is disabled for this row, or null. */
export function sessionActionBlocked(
  id: SessionActionId,
  s: SessionRow,
  status: HubStatus,
  conn: HubConnection,
  blockedOf: BlockedOf,
  host?: HostLogins,
): string | null {
  const g = GATES[id];
  const hub = g.move
    ? moveBlockedReason(status, conn)
    : g.local
      ? hubBlock(g.action as Parameters<typeof hubBlock>[0], status)
      : hubActionBlocked(g.action as Parameters<typeof hubActionBlocked>[0], status, conn);
  return hub ?? blockedOf(s, g.action) ?? (id === 'switch_account' ? noOtherLogin(s, host) : null);
}

export interface SessionMenuItem extends SessionActionDef {
  blocked: string | null;
}

/** The row menu's items for `s`, as Details would offer them. */
export const sessionMenuItems: Readable<(s: SessionRow) => SessionMenuItem[]> = derived(
  [hubStatus, hubConnection, sessionBlocked, hostByAlias],
  ([$status, $conn, $blocked, $hosts]) =>
    (s: SessionRow) =>
      ROW_ACTIONS.filter((a) => sessionActionShown(a.id, s)).map((a) => ({
        ...a,
        blocked: sessionActionBlocked(a.id, s, $status, $conn, $blocked, $hosts.get(s.host_alias)),
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

/** Copy transcript: the session's current conversation as Markdown, read
 *  through `session_conversation` (the newest `CONV_MAX_TURNS` turns, the
 *  most the backend serves) and put on the clipboard. */
export async function copySessionTranscript(s: SessionRow): Promise<boolean> {
  const r = await sessionConversation(s.id, CONV_MAX_TURNS);
  if (!r.ok) {
    pushError(r.error, 'Copying the transcript failed');
    return false;
  }
  const turns = r.value.turns.length;
  if (turns === 0) {
    push({ kind: 'info', message: 'This conversation has no turns to copy yet' });
    return false;
  }
  const ok = await copyText(transcriptMarkdown(r.value, s.friendly_name || s.tmux_name));
  if (!ok) {
    push({ kind: 'error', message: 'Could not write to the clipboard' });
    return false;
  }
  push({
    kind: 'success',
    message: `Copied ${turns} turn${turns === 1 ? '' : 's'}${r.value.truncated ? ' (the newest; older turns are not included)' : ''}`,
  });
  return true;
}

/** Archive one session: its work goes to Done and the session keeps
 *  running, with Undo (the bulk bar's Archive, for one row). */
export async function archiveOneSession(s: SessionRow): Promise<boolean> {
  const r = await archiveSessions([s]);
  if (!r.ok) {
    pushError(r.error, 'Archive failed');
    return false;
  }
  announceArchive(r.value);
  return r.value.archived.length > 0;
}

/** `/model <alias>` into the session's pane through the outbox, the same
 *  send the composer's model picker makes. */
export function sendModelChange(s: SessionRow, model: string): boolean {
  const text = pickerCommand('model', model);
  if (!text) return false;
  outbox.enqueue({ id: s.id, host_alias: s.host_alias, tmux_name: s.tmux_name }, { kind: 'command', text, prefix: null });
  return true;
}
