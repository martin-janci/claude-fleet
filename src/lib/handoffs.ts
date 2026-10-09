// Control's handoff receipts (Orbit Fleet redesign steps 9.3 and 9.6): what
// Control's agent sent where — a prompt or a task to a session, a new
// session, a mission, a task, a proposed tree of subtasks. The backend writes
// one receipt per successful call of the agent that handed work on
// (`service::control_handoffs`); Control draws them as chips and cards that
// follow their target's state: a session's from the live session rows, a
// mission's and a task's from the receipt, re-read on `handoff:changed` and
// on every work change.
import { derived, writable, type Readable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invokeCmd, type Result } from './result';
import { HANDOFF_CHANGED_EVENT } from './events';
import { onWorkChangedDebounced, decideWorkProposal } from './work';
import { acceptWorkProposals, createMission, setMissionItem, undoWorkAccept } from './missions';
import type { SessionRow } from './sessions';
import type { OfState } from './kit/status';

export type HandoffKind = 'session' | 'mission' | 'task' | 'tree';

/** A work item as a receipt carries it (`store::HandoffItem`). */
export interface HandoffItem {
  id: number;
  title: string;
  /** The status category: `todo`, `in_progress`, `done`, … */
  status: string;
  proposal_state?: string | null;
  /** An accepted item's last change, unix seconds: Undo's clock. */
  accepted_at?: number | null;
}

/** One receipt (`store::ControlHandoffRow`). */
export interface ControlHandoff {
  id: number;
  at: number;
  kind: HandoffKind | string;
  tool: string;
  session_id?: number | null;
  task_id?: number | null;
  mission_id?: number | null;
  mission_name?: string | null;
  mission_state?: string | null;
  /** kind task: the created item; kind tree: the parent. */
  item?: HandoffItem | null;
  /** kind tree: the proposed items as they are now. */
  items?: HandoffItem[];
  preview?: string | null;
}

/** How long after an accept Undo is offered, seconds (`ACCEPT_UNDO_SECS`). */
export const ACCEPT_UNDO_SECS = 600;

/** How many receipts Control reads. */
export const HANDOFFS_SHOWN = 20;

export const handoffs = writable<ControlHandoff[]>([]);

export function listHandoffs(limit = HANDOFFS_SHOWN): Promise<Result<ControlHandoff[]>> {
  return invokeCmd<ControlHandoff[]>('control_handoffs', { limit });
}

/** Re-read the receipts; a failed read keeps what is shown. */
export async function refreshHandoffs(): Promise<void> {
  const r = await listHandoffs();
  if (r.ok && Array.isArray(r.value)) handoffs.set(r.value);
}

let hosts = 0;
let stop: (() => void) | null = null;

/**
 * Follow the receipts for as long as a Control transcript is mounted: read
 * them now, again on `handoff:changed` (a new receipt) and on any work change
 * (a task's or mission's state moved). Returns the release.
 */
export function followHandoffs(): () => void {
  hosts += 1;
  if (hosts === 1) {
    const unlisteners: UnlistenFn[] = [];
    let disposed = false;
    void refreshHandoffs();
    void listen(HANDOFF_CHANGED_EVENT, () => void refreshHandoffs()).then((u) => {
      if (disposed) u();
      else unlisteners.push(u);
    });
    const stopWork = onWorkChangedDebounced(
      () => void refreshHandoffs(),
      () => 300,
      () => 2000,
    );
    stop = () => {
      disposed = true;
      stopWork();
      for (const u of unlisteners.splice(0)) u();
    };
  }
  return () => {
    hosts = Math.max(0, hosts - 1);
    if (hosts === 0) {
      stop?.();
      stop = null;
    }
  };
}

/** Receipts oldest first, the newest `n`: the order a transcript reads. */
export const recentHandoffs: Readable<ControlHandoff[]> = derived(handoffs, (h) =>
  h.slice(0, 8).reverse(),
);

/** A session's live state as one of the manual's five. */
export function sessionState(row: SessionRow | undefined): OfState {
  if (!row) return 'idle';
  if (row.stuck_kind) return 'failed';
  switch (row.claude_status) {
    case 'working':
      return 'working';
    case 'blocked':
      return 'waiting';
    case 'failed':
      return 'failed';
    case 'completed':
      return 'done';
    default:
      return 'idle';
  }
}

/** A session's chip words: its name and its state, or "ended" once gone. */
export function sessionChip(h: ControlHandoff, rows: readonly SessionRow[]): {
  name: string;
  state: OfState | null;
  row: SessionRow | undefined;
} {
  const row = rows.find((s) => s.id === h.session_id);
  if (!row) return { name: h.preview ?? `session ${h.session_id}`, state: null, row };
  return { name: row.friendly_name || row.tmux_name, state: sessionState(row), row };
}

/** A mission's or a task's state as one of the manual's five. */
export function workState(state: string | null | undefined): OfState {
  switch (state) {
    case 'active':
    case 'in_progress':
    case 'running':
      return 'working';
    case 'completed':
    case 'done':
      return 'done';
    case 'failed':
    case 'cancelled':
      return 'failed';
    case 'blocked':
      return 'waiting';
    default:
      return 'idle';
  }
}

/** The tree's items still waiting on a person. */
export function openProposals(h: ControlHandoff): HandoffItem[] {
  return (h.items ?? []).filter((i) => i.proposal_state === 'proposed');
}

/** Accepted items that can still be undone at `nowSec`. */
export function undoable(h: ControlHandoff, nowSec: number): HandoffItem[] {
  return (h.items ?? []).filter(
    (i) =>
      i.proposal_state === 'accepted' &&
      i.status === 'todo' &&
      i.accepted_at != null &&
      nowSec - i.accepted_at < ACCEPT_UNDO_SECS,
  );
}

/**
 * Create the ticked proposals as tasks and reject the rest. With
 * `asMission`, the parent becomes a mission's root and the new tasks its
 * members. Answers an error message, or null.
 */
export async function createFromTree(
  h: ControlHandoff,
  ticked: readonly number[],
  asMission: boolean,
): Promise<string | null> {
  const open = openProposals(h).map((i) => i.id);
  const keep = open.filter((id) => ticked.includes(id));
  const drop = open.filter((id) => !ticked.includes(id));
  if (keep.length === 0) return 'Tick at least one task to create.';
  const accepted = await acceptWorkProposals(keep);
  if (!accepted.ok) return accepted.error.message;
  for (const id of drop) {
    const r = await decideWorkProposal(id, false);
    if (!r.ok) return r.error.message;
  }
  if (asMission && h.item) {
    const m = await createMission({ name: h.item.title, goal: h.item.title }, h.item.id);
    if (!m.ok) return m.error.message;
    for (const id of keep) {
      const r = await setMissionItem(m.value.id, id, true);
      if (!r.ok) return r.error.message;
    }
  }
  await refreshHandoffs();
  return null;
}

/** Take the tree's recent accepts back. Answers an error message, or null. */
export async function undoTree(h: ControlHandoff, nowSec: number): Promise<string | null> {
  const ids = undoable(h, nowSec).map((i) => i.id);
  if (ids.length === 0) return null;
  const r = await undoWorkAccept(ids);
  if (!r.ok) return r.error.message;
  await refreshHandoffs();
  return null;
}

/** Test seam. */
export function resetHandoffsForTests(): void {
  stop?.();
  stop = null;
  hosts = 0;
  handoffs.set([]);
}

