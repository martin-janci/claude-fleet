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

export type TargetState =
  | { state: 'absent' }
  | { state: 'unknown' }
  | { state: 'clean'; head: string }
  | { state: 'dirty'; head: string; entries: { status: string; path: string }[] };

/** Mirrors `service::move_session::preview::MovePreview`. */
export interface MovePreview {
  session_id: number;
  from_host: string;
  to_host: string;
  branch: string;
  source_cwd: string;
  unpushed_commits: number | null;
  commits_ahead: number | null;
  dirty: { status: string; path: string }[];
  ignored_carried: { path: string; bytes: number }[];
  ignored_left_behind: CarryReport['ignored_left_behind'];
  transcript_bytes: number;
  session_state_files: number;
  session_state_bytes: number;
  memory_files: number;
  memory_bytes: number;
  target_path: string;
  target: TargetState;
  unknowns: string[];
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

/** `when` argument to `move_session` (mirrors `service::move_session::When`):
 *  `now` moves at once (a busy source is refused), `idle` waits for the
 *  source to go idle before moving, `cancel` ends a pending wait. */
export type When = 'now' | 'idle' | 'cancel';

/** What a pending wait reports back to its caller (mirrors
 *  `service::move_session::wait::MoveWaiting`). */
export interface MoveWaiting {
  session_id: number;
  to_host: string;
  deadline_unix: number;
}

/** What `when: cancel` answers with (mirrors
 *  `service::move_session::MoveWaitCancelled`). `was_waiting` is false when
 *  there was nothing pending to cancel. */
export interface MoveWaitCancelled {
  session_id: number;
  was_waiting: boolean;
}

/** What `move_session` answers. Internally tagged: a `moved` outcome is the
 *  `MoveReport` with one extra key, and so on for the other three. */
export type MoveOutcome =
  | ({ kind: 'moved' } & MoveReport)
  | ({ kind: 'preview' } & MovePreview)
  | ({ kind: 'waiting' } & MoveWaiting)
  | ({ kind: 'wait_cancelled' } & MoveWaitCancelled);

/** Move a work session to another host with its work as it is: transcript,
 *  unpushed commits, uncommitted and untracked files, small git-ignored files.
 *  The source is killed once the target is confirmed (unless `keepSource`).
 *  `strict` refuses a dirty or unpushed source (`E_MOVE_DIRTY` /
 *  `E_MOVE_UNPUSHED`) instead of carrying it. Other refusals: `E_MOVE_MIDOP`,
 *  `E_MOVE_TARGET_DIRTY`, `E_MOVE_TOO_LARGE`, `E_MOVE_CARRY`; `E_MOVE_PARTIAL`
 *  leaves both sessions. `cleanTarget` replaces a stale attempt's leftovers on
 *  the target instead of refusing with `E_MOVE_TARGET_DIRTY` again.
 *  `forceCrossOrg` carries a live work link across the org boundary the
 *  target host would put the session on, with a warning, instead of the
 *  refusal (`E_FORBIDDEN`, details `cross_org: true` — `crossOrgOf` in
 *  `work.ts` reads it); sent only when set, so an older hub sees the same
 *  arguments as before.
 *  `when` defaults to `'idle'` — an idle source moves at once, a busy one
 *  yields a pending wait. That default is this wrapper's own; the wire's
 *  own default (an omitted `when`) is `'now'` (`MoveSessionArgs::when`,
 *  `#[serde(default)]` on `When::Now`) — a caller that talks to
 *  `move_session` directly, bypassing this wrapper, still gets the old flat
 *  refusal on a busy source unless it asks for `idle` itself. Narrows to
 *  `moved | waiting`. */
export async function moveSession(
  sessionId: number,
  targetHostAlias: string,
  opts: {
    keepSource?: boolean;
    strict?: boolean;
    cleanTarget?: boolean;
    forceCrossOrg?: boolean;
    when?: 'now' | 'idle';
  } = {},
): Promise<Result<({ kind: 'moved' } & MoveReport) | ({ kind: 'waiting' } & MoveWaiting)>> {
  const r = await invokeCmd<MoveOutcome>('move_session', {
    args: {
      session_id: sessionId,
      target_host_alias: targetHostAlias,
      keep_source: opts.keepSource ?? false,
      strict: opts.strict ?? false,
      clean_target: opts.cleanTarget ?? false,
      ...(opts.forceCrossOrg ? { force_cross_org: true } : {}),
      dry_run: false,
      when: opts.when ?? 'idle',
    },
  });
  if (!r.ok) return r;
  if (r.value.kind === 'moved') {
    mergeSession(r.value.target);
    return { ok: true, value: r.value };
  }
  if (r.value.kind === 'waiting') {
    return { ok: true, value: r.value };
  }
  return {
    ok: false,
    error: { code: 'E_PARSE', message: 'move_session answered a preview or a cancelled wait, not a move' },
  };
}

/** A dry run: what this move would do, changing nothing. `Err` is the
 *  refusal the real move would return — same code, same message. The
 *  Transfer sheet never offers `strict`, so this always sends `strict: false`.
 *  Sends `when: 'idle'` too, so the preview matches what a real transfer will
 *  do (spec §4: a busy source previews with a "will wait" line instead of
 *  stopping at the refusal). */
export async function previewMove(
  sessionId: number,
  targetHostAlias: string,
): Promise<Result<MovePreview>> {
  const r = await invokeCmd<MoveOutcome>('move_session', {
    args: {
      session_id: sessionId,
      target_host_alias: targetHostAlias,
      keep_source: false,
      strict: false,
      clean_target: false,
      dry_run: true,
      when: 'idle',
    },
  });
  if (!r.ok) return r;
  if (r.value.kind !== 'preview') {
    return {
      ok: false,
      error: { code: 'E_PARSE', message: 'move_session answered a move, not a preview' },
    };
  }
  const { kind: _kind, ...preview } = r.value;
  return { ok: true, value: preview };
}

/** End a pending wait. Carries the wait's target host: the backend's access
 *  checks name both hosts even for a cancel. */
export async function cancelMoveWait(
  sessionId: number,
  targetHostAlias: string,
): Promise<Result<MoveWaitCancelled>> {
  const r = await invokeCmd<MoveOutcome>('move_session', {
    args: {
      session_id: sessionId,
      target_host_alias: targetHostAlias,
      keep_source: false,
      strict: false,
      clean_target: false,
      dry_run: false,
      when: 'cancel',
    },
  });
  if (!r.ok) return r;
  if (r.value.kind !== 'wait_cancelled') {
    return {
      ok: false,
      error: { code: 'E_PARSE', message: 'move_session answered something other than a cancelled wait' },
    };
  }
  const { kind: _kind, ...cancelled } = r.value;
  return { ok: true, value: cancelled };
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
