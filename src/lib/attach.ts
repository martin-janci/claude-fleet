// Attach a running session to a task (task → session spec A2, J3): the
// pure half of the attach picker — which sessions it offers and in what
// order, whether a session already on another task switches or adds by
// default, and the one write that does it, with its Undo.
import type { SessionRow } from './sessions';
import type { Result } from './result';
import { linkSessionWork, switchSessionWork, unlinkSessionWork, type WorkRef } from './work';
import { sessionIdActionBlocked } from './share';
import { hubActionBlocked, hubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { get } from 'svelte/store';

/** How a session already on another task takes this one: `switch` ends
 *  that link and makes this task the primary; `add` links this task as a
 *  secondary and the other stays primary. */
export type AttachMode = 'switch' | 'add';

/** What the picker needs to know about the task. */
export interface AttachTarget {
  ref: WorkRef;
  key?: string | null;
  /** Sessions already linked to the task: never offered. */
  linkedSessionIds: ReadonlySet<number>;
  /** Projects the task has run in before: those sessions come first. */
  projectIds: ReadonlySet<number>;
  /** The operator's own session, never offered: it gets no work links
   *  (spec §3.3), only threads. */
  operatorId?: number | null;
}

/** The task a session is on, unless it is this task. */
export function otherWork(row: SessionRow, target: AttachTarget) {
  const w = row.work;
  if (!w) return null;
  if ('item_id' in target.ref && w.item_id === target.ref.item_id) return null;
  if ('key' in target.ref && w.key && w.key.toUpperCase() === target.ref.key.toUpperCase()) return null;
  return w;
}

function rank(row: SessionRow, target: AttachTarget): number[] {
  return [
    row.project_id != null && target.projectIds.has(row.project_id) ? 0 : 1,
    row.work ? 1 : 0,
    row.claude_status === 'working' ? 1 : 0,
  ];
}

/** The sessions the picker offers: every running session not already on
 *  the task — not a lost one, not the operator — in the same repository
 *  first, then those with no task, then idle ones (spec J3), then by name.
 *  `filter` matches the name, host or task key. */
export function attachCandidates(rows: readonly SessionRow[], target: AttachTarget, filter = ''): SessionRow[] {
  const q = filter.trim().toLowerCase();
  const name = (r: SessionRow) => r.friendly_name ?? r.tmux_name;
  return rows
    .filter((r) => !target.linkedSessionIds.has(r.id) && r.lost_at == null && r.id !== target.operatorId)
    .filter(
      (r) =>
        !q ||
        name(r).toLowerCase().includes(q) ||
        r.host_alias.toLowerCase().includes(q) ||
        (r.work?.key ?? '').toLowerCase().includes(q),
    )
    .map((r) => ({ r, k: rank(r, target) }))
    .sort((a, b) => {
      for (let i = 0; i < a.k.length; i++) if (a.k[i] !== b.k[i]) return a.k[i] - b.k[i];
      return name(a.r).localeCompare(name(b.r));
    })
    .map(({ r }) => r);
}

/** Switch, unless switching would misfile work: mid-turn, the running turn's
 *  work belongs to the other task, and a checkout named after the other key
 *  would have detection suggest it again (spec J3). */
export function defaultMode(row: SessionRow): AttachMode {
  if (row.claude_status === 'working') return 'add';
  const key = row.work?.key?.toLowerCase();
  if (key && (row.worktree_key ?? '').toLowerCase().includes(key)) return 'add';
  return 'switch';
}

/** One attach. `ackLive`: the person saw the task is open elsewhere (P-3)
 *  and goes ahead; `forceCrossOrg`: across organisations, likewise. */
export interface AttachOpts {
  mode: AttachMode;
  ackLive?: boolean;
  forceCrossOrg?: boolean;
}

/** The attach's answer: the session's new row and, when there is one, the
 *  write that puts things back as they were. */
export interface Attached {
  row: SessionRow;
  undo: (() => Promise<Result<SessionRow>>) | null;
}

/** The previous primary as a ref the switch back can name. */
function refOf(w: NonNullable<SessionRow['work']>): WorkRef | null {
  if (w.item_id != null) return { item_id: w.item_id };
  if (w.key) return { key: w.key };
  return null;
}

/** Attach `row` to the task. A session with no task takes it as its primary
 *  (Undo removes the link); one on another task switches (Undo switches
 *  back, a compare-and-set on the primary the switch made) or adds a
 *  secondary link (the primary does not move, so there is nothing to undo
 *  but the link itself, which the session's Tasks list can remove). */
export async function attachSession(row: SessionRow, target: AttachTarget, opts: AttachOpts): Promise<Result<Attached>> {
  const other = otherWork(row, target);
  const ackLive = opts.ackLive ?? false;
  // Both halves, asked at the write (multi-user M1, F2b): the picker's
  // button is disabled too, but a grant can narrow while it is open. Every
  // write here — switch, link and their Undo — is `drive`.
  const refused = (): { ok: false; error: { code: string; message: string } } | null => {
    const why =
      hubActionBlocked('link_session_work', get(hubStatus), get(hubConnection)) ??
      sessionIdActionBlocked(row.id, 'switch_session_work');
    return why ? { ok: false, error: { code: 'E_FORBIDDEN', message: why } } : null;
  };
  const no = refused();
  if (no) return no;
  if (other && opts.mode === 'switch') {
    const r = await switchSessionWork(row.id, other.link_id, target.ref, {
      expectedPrimary: other.link_id,
      ackLive,
      forceCrossOrg: opts.forceCrossOrg,
    });
    if (!r.ok) return r;
    const back = refOf(other);
    const now = r.value.work?.link_id;
    return {
      ok: true,
      value: {
        row: r.value,
        undo:
          back && now != null
            ? async () => refused() ?? switchSessionWork(row.id, now, back, { expectedPrimary: now, ackLive: true })
            : null,
      },
    };
  }
  const r = await linkSessionWork(row.id, target.ref, {
    primary: !other,
    ackLive,
    forceCrossOrg: opts.forceCrossOrg,
  });
  if (!r.ok) return r;
  const made = !other ? r.value.work?.link_id : undefined;
  return {
    ok: true,
    value: { row: r.value, undo: made != null ? async () => refused() ?? unlinkSessionWork(row.id, made) : null },
  };
}
