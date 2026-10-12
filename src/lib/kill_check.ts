// Kill, Clean up and Archive for one session or a selection (redesign step
// 1.7). A plain Kill used to ask "Continue?" without looking; only Safe
// remove in Details read the worktree. Now every Kill dialog reads it first
// (`inspect_safe_kill`), lists what would be left behind, and offers Clean
// up next to Kill: commit and push through the agent, then remove. Archive
// is the quiet third way, with Undo.

import { get } from 'svelte/store';
import { hubActionBlocked, hubBlock, hubStatus } from './hub';
import { hubConnection } from './hub_connection';
import type { Result } from './result';
import { push } from './toasts';
import {
  discardKillSession,
  inspectSafeKill,
  safeKillSession,
  type DirtyFile,
  type SessionRow,
} from './sessions';
import { sessionActionBlocked, sessionIdActionBlocked } from './share';
import { applyTidy, unarchiveSession, type TidyApplyResult } from './tidy';

/** What a Kill would leave behind in one session's worktree. */
export type WorkCheck =
  | { state: 'checking' }
  | { state: 'clean' }
  | { state: 'dirty'; files: DirtyFile[]; unpushed: number; branch: string | null }
  /** Fleet could not look: on a hub client, not this person's, or the read failed. */
  | { state: 'unknown'; why: string };

/** Read one session's worktree, or say why it cannot be read. */
export async function checkWork(row: SessionRow): Promise<WorkCheck> {
  const blocked = hubBlock('inspect_safe_kill', get(hubStatus)) ?? sessionActionBlocked(row, 'inspect_safe_kill');
  if (blocked !== null) return { state: 'unknown', why: blocked };
  const r = await inspectSafeKill(row.host_alias, row.tmux_name);
  if (!r.ok) return { state: 'unknown', why: r.error.message };
  const i = r.value;
  if (!i) return { state: 'unknown', why: 'the host sent no answer' };
  if (i.error) return { state: 'unknown', why: i.error };
  if (!i.has_worktree) return { state: 'clean' };
  if ((i.dirty_files ?? []).length === 0 && (i.unpushed_commits ?? 0) === 0) return { state: 'clean' };
  return { state: 'dirty', files: i.dirty_files ?? [], unpushed: i.unpushed_commits ?? 0, branch: i.branch ?? null };
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "3 uncommitted files and 2 commits not pushed", or `null` when clean. */
export function workLine(c: WorkCheck): string | null {
  if (c.state !== 'dirty') return null;
  const parts: string[] = [];
  if (c.files.length > 0) parts.push(`${plural(c.files.length, 'uncommitted file')}`);
  if (c.unpushed > 0) parts.push(`${plural(c.unpushed, 'commit')} not pushed`);
  return parts.join(' and ');
}

/** The files to show under a session, the rest folded into "+N more". */
export const FILES_SHOWN = 5;

/** Why Clean up is not offered for this session, or `null` when it is. */
export function cleanUpBlocked(row: SessionRow): string | null {
  return (
    hubActionBlocked('safe_kill_session', get(hubStatus), get(hubConnection)) ??
    sessionActionBlocked(row, 'safe_kill_session')
  );
}

/**
 * Clean up one session: a clean, pushed worktree is removed at once
 * (`discard_kill_session` without force, which refuses if a file turned
 * dirty since) and answers `removed`; anything else, or a hub client that
 * has no direct remove, goes through the agent's Safe remove (commit, push,
 * then remove), which finishes later and answers `asked`.
 */
export async function cleanUp(row: SessionRow, check: WorkCheck): Promise<Result<'removed' | 'asked'>> {
  // Asked again at the call: a grant can be narrowed while the dialog is open.
  const refused = sessionActionBlocked(row, 'safe_kill_session');
  if (refused !== null) return { ok: false, error: { code: 'E_FORBIDDEN', message: refused } };
  const direct =
    check.state === 'clean' &&
    (hubBlock('discard_kill_session', get(hubStatus)) ?? sessionActionBlocked(row, 'discard_kill_session')) === null;
  if (direct) {
    const r = await discardKillSession(row.host_alias, row.tmux_name, false);
    return r.ok ? { ok: true, value: 'removed' } : r;
  }
  const r = await safeKillSession(row.host_alias, row.tmux_name);
  return r.ok ? { ok: true, value: 'asked' } : r;
}

/** Why Archive is not offered for these rows, or `null` when it is. */
export function archiveBlocked(rows: readonly SessionRow[]): string | null {
  const blocked = hubActionBlocked('tidy_apply', get(hubStatus), get(hubConnection));
  if (blocked) return blocked;
  if (rows.length > 0 && rows.every((r) => r.work == null)) return 'Archive puts a session in its work’s Done; none of these is linked to work.';
  return null;
}

/** What one Archive press did. */
export interface ArchiveOutcome {
  archived: number[];
  /** Rows not archived, with why: no work linked, protected, refused. */
  skipped: { row: SessionRow; why: string }[];
}

/** Archive the rows that have work linked (tidy-up's `archive`, which
 *  stamps the work link and keeps the session running). */
export async function archiveSessions(rows: readonly SessionRow[]): Promise<Result<ArchiveOutcome>> {
  const skipped: ArchiveOutcome['skipped'] = [];
  const linked: SessionRow[] = [];
  for (const r of rows) {
    if (r.work == null) skipped.push({ row: r, why: 'no work linked' });
    else if (sessionActionBlocked(r, 'tidy_apply') !== null) skipped.push({ row: r, why: 'not yours to archive' });
    else linked.push(r);
  }
  if (linked.length === 0) return { ok: true, value: { archived: [], skipped } };
  const res = await applyTidy(linked.map((r) => ({ session_id: r.id, action: 'archive' as const, link_id: r.work?.link_id })));
  if (!res.ok) return res;
  const byId = new Map(linked.map((r) => [r.id, r] as const));
  const archived: number[] = [];
  for (const x of res.value.results as TidyApplyResult[]) {
    const row = byId.get(x.session_id);
    if (x.ok) archived.push(x.session_id);
    else if (row) skipped.push({ row, why: x.error ?? 'refused' });
  }
  return { ok: true, value: { archived, skipped } };
}

/** Say what an Archive press did, with Undo when it archived anything. The
 *  one toast for the bulk bar and a single session's Archive alike. */
export function announceArchive({ archived, skipped }: ArchiveOutcome): void {
  const left = skipped.length > 0 ? ` · ${skipped.length} left as they were (${[...new Set(skipped.map((x) => x.why))].join(', ')})` : '';
  if (archived.length === 0) {
    push({ message: `Nothing archived${left}`, kind: 'info' });
    return;
  }
  push({
    message: `Archived ${archived.length} session${archived.length === 1 ? '' : 's'}${left}`,
    kind: 'success',
    action: { label: 'Undo', run: () => void undoArchive(archived) },
  });
}

/** Undo an Archive press: un-archive every session it archived. */
export async function undoArchive(ids: readonly number[]): Promise<number> {
  const mine = ids.filter((id) => sessionIdActionBlocked(id, 'unarchive_session_work') === null);
  const rs = await Promise.all(mine.map((id) => unarchiveSession(id)));
  return rs.filter((r) => r.ok).length;
}
