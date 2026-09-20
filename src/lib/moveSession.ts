import { invokeCmd, type Result } from './result';
import { mergeSession, type SessionRow } from './sessions';

/** What a move carried besides the transcript (mirrors `carry::CarryReport`). */
export interface CarryReport {
  /** Commits the target lacked (unpushed work included). */
  commits: number;
  bundle_bytes: number;
  /** `git status --porcelain` rows restored on the target. */
  dirty_entries: { status: string; path: string }[];
  ignored_carried: { path: string; bytes: number }[];
  /** `bytes` is null for a deny-listed entry: it is never sized. */
  ignored_left_behind: {
    path: string;
    bytes: number | null;
    reason: 'denylisted' | 'over_cap' | 'unsupported_name';
  }[];
  target_seeded: 'existing' | 'cloned' | 'initialized';
  /** The per-session directory (subagent transcripts, tool results, title). */
  session_state: {
    carried: { path: string; bytes: number }[];
    /** The target already had an equal or larger copy. */
    kept_target: string[];
    left_behind: { path: string; bytes: number | null; reason: 'denylisted' | 'over_cap' | 'unsupported_name' }[];
  };
  /** The project's Claude memory; the target's own files are never replaced. */
  memory: {
    carried: { path: string; bytes: number }[];
    kept_target: string[];
    identical: number;
    index_lines_added: number;
    left_behind: { path: string; bytes: number | null; reason: 'denylisted' | 'over_cap' | 'unsupported_name' }[];
  };
}

/** What a completed `move_session` did (mirrors `service::move_session::MoveReport`). */
export interface MoveReport {
  source_session_id: number;
  target_session_id: number;
  from_host: string;
  to_host: string;
  /** tmux name on the target (the source name unless it was taken there). */
  tmux_name: string;
  claude_session_id: string;
  branch: string;
  target_cwd: string;
  transcript_bytes: number;
  source_killed: boolean;
  warnings: string[];
  carried: CarryReport;
  /** The new row; its `parent_session_id` is the source. */
  target: SessionRow;
}

/** Move a work session to another host with its work as it is: transcript,
 *  unpushed commits, uncommitted and untracked files, small git-ignored files.
 *  The source is killed once the target is confirmed (unless `keepSource`).
 *  `strict` refuses a dirty or unpushed source (`E_MOVE_DIRTY` /
 *  `E_MOVE_UNPUSHED`) instead of carrying it. Other refusals: `E_MOVE_MIDOP`,
 *  `E_MOVE_TARGET_DIRTY`, `E_MOVE_TOO_LARGE`, `E_MOVE_CARRY`; `E_MOVE_PARTIAL`
 *  leaves both sessions. `cleanTarget` replaces a stale attempt's leftovers on
 *  the target instead of refusing with `E_MOVE_TARGET_DIRTY` again. */
export async function moveSession(
  sessionId: number,
  targetHostAlias: string,
  opts: { keepSource?: boolean; strict?: boolean; cleanTarget?: boolean } = {},
): Promise<Result<MoveReport>> {
  const r = await invokeCmd<MoveReport>('move_session', {
    args: {
      session_id: sessionId,
      target_host_alias: targetHostAlias,
      keep_source: opts.keepSource ?? false,
      strict: opts.strict ?? false,
      clean_target: opts.cleanTarget ?? false,
    },
  });
  if (r.ok) mergeSession(r.value.target);
  return r;
}

export type ResolveAction = 'finish' | 'undo';

/** What a `resolve_move` did (mirrors `service::move_session::ResolveMoveReport`). */
export interface ResolveMoveReport {
  action: ResolveAction;
  source_session_id: number;
  target_session_id: number;
  from_host: string;
  to_host: string;
  source_killed: boolean;
  target_killed: boolean;
  warnings: string[];
}

/** Finish or undo a partial move (`E_MOVE_PARTIAL`). `sessionId` is the
 *  TARGET session's id. Refusals: `E_INVALID_STATE` (not a partial; the source
 *  moved on; the target took a turn or is not idle), `E_NOTFOUND`. */
export async function resolveMove(
  sessionId: number,
  action: ResolveAction,
): Promise<Result<ResolveMoveReport>> {
  return invokeCmd<ResolveMoveReport>('resolve_move', {
    args: { session_id: sessionId, action },
  });
}
