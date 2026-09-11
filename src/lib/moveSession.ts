import { invokeCmd, type Result } from './result';
import { mergeSession, type SessionRow } from './sessions';

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
  /** The new row; its `parent_session_id` is the source. */
  target: SessionRow;
}

/** Move a work session to another host: copy its transcript, start it there
 *  with `--resume` in the same branch, and kill the source once the target is
 *  confirmed (unless `keepSource`). Refused with `E_MOVE_DIRTY` /
 *  `E_MOVE_UNPUSHED` / `E_MOVE_TOO_LARGE`; `E_MOVE_PARTIAL` leaves both. */
export async function moveSession(
  sessionId: number,
  targetHostAlias: string,
  opts: { keepSource?: boolean } = {},
): Promise<Result<MoveReport>> {
  const r = await invokeCmd<MoveReport>('move_session', {
    args: {
      session_id: sessionId,
      target_host_alias: targetHostAlias,
      keep_source: opts.keepSource ?? false,
    },
  });
  if (r.ok) mergeSession(r.value.target);
  return r;
}
