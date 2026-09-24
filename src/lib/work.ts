// Work links (roadmap M1b.2): which work a session is doing, decided by a
// person. Thin wrappers over the `*_session_work` commands; each mutation
// answers the session's updated row (its `work` / `work_rejected`), which is
// patched into the sessions store right away — the `session:updated` event
// the backend also emits then lands as a no-op.

import { invokeCmd, type Result } from './result';
import { acceptCommandRow, type SessionRow } from './sessions';

/** One session ↔ work link (`store::WorkLinkRow`). The snapshot fields are
 *  set once the session has ended. */
export interface WorkLink {
  id: number;
  item_id?: number | null;
  ref_key?: string | null;
  participant_id?: number | null;
  /** `confirmed` | `rejected` — tolerant of values a newer hub adds. */
  state: string;
  source: string;
  is_primary?: boolean;
  created_at: number;
  decided_at?: number | null;
  ended_at?: number | null;
  snap_host?: string | null;
  snap_tmux?: string | null;
  snap_name?: string | null;
  snap_project_id?: number | null;
  snap_worktree?: string | null;
  snap_branch?: string | null;
  snap_pr_url?: string | null;
  snap_claude_ids?: string | null;
}

/** What a decision is about: a key / free-form name, or a work item. */
export type WorkRef = { key: string } | { item_id: number };

/** A session's live links, primary first. */
export function sessionWorkLinks(sessionId: number): Promise<Result<WorkLink[]>> {
  return invokeCmd<WorkLink[]>('session_work_links', { args: { session_id: sessionId } });
}

async function decide(cmd: string, args: Record<string, unknown>): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>(cmd, { args });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Say the session works on `ref`; it becomes the session's primary work. */
export function linkSessionWork(sessionId: number, ref: WorkRef): Promise<Result<SessionRow>> {
  return decide('link_session_work', { session_id: sessionId, ...ref });
}

/** "Not this": the session does not work on `ref`. Sticky. */
export function rejectSessionWork(sessionId: number, ref: WorkRef): Promise<Result<SessionRow>> {
  return decide('reject_session_work', { session_id: sessionId, ...ref });
}

/** Remove a mistaken link (not a rejection: the key may come back). */
export function unlinkSessionWork(sessionId: number, linkId: number): Promise<Result<SessionRow>> {
  return decide('unlink_session_work', { session_id: sessionId, link_id: linkId });
}
