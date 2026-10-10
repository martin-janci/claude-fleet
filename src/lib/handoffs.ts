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
  /** Its done-when lines (`ci:<check>`, `review`, `test:<cmd>`, `person`);
   *  absent when none (or from an older hub). G3.11. */
  done_when?: string[];
  /** The items it waits for: the plan card's waves. G3.11. */
  depends_on?: number[];
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

// ── tasks in chat (gap plan G3.11, board MCTasks) ──

/** The plan card's waves: an item with nothing to wait for inside the
 *  tree is wave 1, one waiting on wave n items is wave n+1. An edge to an
 *  item outside the tree does not hold it back here. Each wave keeps the
 *  tree's order. */
export function treeWaves(items: readonly HandoffItem[]): HandoffItem[][] {
  const ids = new Set(items.map((i) => i.id));
  const byId = new Map(items.map((i) => [i.id, i]));
  const memo = new Map<number, number>();
  const wave = (i: HandoffItem, seen: Set<number>): number => {
    const known = memo.get(i.id);
    if (known !== undefined) return known;
    if (seen.has(i.id)) return 1; // a cycle the store refuses; never loop
    seen.add(i.id);
    let w = 1;
    for (const d of i.depends_on ?? []) {
      const dep = ids.has(d) ? byId.get(d) : undefined;
      if (dep) w = Math.max(w, wave(dep, seen) + 1);
    }
    memo.set(i.id, w);
    return w;
  };
  const out: HandoffItem[][] = [];
  for (const i of items) {
    const w = wave(i, new Set());
    (out[w - 1] ??= []).push(i);
  }
  return out.filter((w) => w && w.length > 0);
}

/** One done-when line in words: "CI test passes", "a review approves",
 *  "`pnpm test` passes", "a person checks it". */
export function doneWhenWords(line: string): string {
  const [kind, ...rest] = line.split(':');
  const arg = rest.join(':').trim();
  switch (kind.trim()) {
    case 'ci':
      return arg ? `CI ${arg} passes` : 'CI passes';
    case 'review':
      return 'a review approves';
    case 'test':
      return arg ? `\`${arg}\` passes` : 'the tests pass';
    case 'person':
      return 'a person checks it';
    default:
      return line;
  }
}

/** "finishes when CI test passes and a review approves", or '' with no lines. */
export function finishesWhen(lines: readonly string[] | undefined): string {
  const w = (lines ?? []).map(doneWhenWords);
  if (w.length === 0) return '';
  const head = w.length > 1 ? `${w.slice(0, -1).join(', ')} and ${w[w.length - 1]}` : w[0];
  return `finishes when ${head}`;
}

/** The live sessions working on a task, the status card's session line. */
export function liveSessionsOf(itemId: number, rows: readonly SessionRow[]): SessionRow[] {
  return rows.filter((s) => s.work?.item_id === itemId && s.status !== 'ghost');
}

/** A created task's Undo: the backend takes back an accept within
 *  `ACCEPT_UNDO_SECS`; a task written directly has no undo (Move to Done). */
export function taskUndoable(item: HandoffItem, nowSec: number): boolean {
  return item.proposal_state === 'accepted' && item.status === 'todo' && item.accepted_at != null && nowSec - item.accepted_at < ACCEPT_UNDO_SECS;
}

/** Test seam. */
export function resetHandoffsForTests(): void {
  stop?.();
  stop = null;
  hosts = 0;
  handoffs.set([]);
}

