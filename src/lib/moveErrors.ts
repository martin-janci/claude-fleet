// A failed move, in words. The backend's messages are written for an
// operator reading a log; the sheet needs one sentence on what failed and one
// on where things stand.
import type { IpcError } from './result';

export interface MoveFailure {
  what: string;
  standing: string;
}

const CARRY_STEP: Record<string, string> = {
  seed: 'The target could not be given a clone of the repository to receive the work into.',
  haves: 'The target could not say which commits it already has.',
  snapshot: 'The source could not take a snapshot of its uncommitted work.',
  download: 'The git work could not be read from the source.',
  upload: 'The git work could not be written to the target.',
  fetch: 'The target could not take in the carried commits.',
  apply: 'The uncommitted work could not be replayed on the target.',
  verify: 'The target did not end up with the same uncommitted work as the source.',
  target: 'The target worktree could not be prepared.',
};

/** Both transports report a blown wall clock as `E_SSH_TIMEOUT`; `E_TIMEOUT`
 *  is the older name and costs nothing to keep. */
const TIMEOUTS = new Set(['E_SSH_TIMEOUT', 'E_TIMEOUT']);

function field(details: unknown, key: string): string | null {
  if (typeof details !== 'object' || details === null) return null;
  const v = (details as Record<string, unknown>)[key];
  return typeof v === 'string' ? v : null;
}

function what(error: IpcError, toHost: string): string {
  switch (error.code) {
    case 'E_MOVE_DIRTY':
      return 'The source has uncommitted work, and this move was asked to refuse that.';
    case 'E_MOVE_UNPUSHED':
      return 'The source has commits that are not pushed, and this move was asked to refuse that.';
    case 'E_MOVE_MIDOP':
      return 'The source is in the middle of a merge, rebase or similar. Finish or abort it, then move.';
    case 'E_MOVE_TARGET_DIRTY':
      return `${toHost} already has uncommitted work in this worktree. Clean it up there first.`;
    case 'E_MOVE_TOO_LARGE':
      return 'The work to carry is too large for a move.';
    case 'E_LOCAL_ONLY':
      return 'This desktop is a window onto a hub, and the hub refused the move.';
    case 'E_INVALID_STATE':
      return error.message.includes('already in progress')
        ? 'This session is already being moved.'
        : error.message;
    case 'E_MOVE_PARTIAL':
      return 'The new session started, but the last step of the move failed.';
    case 'E_MOVE_CARRY': {
      const step = field(error.details, 'step');
      const sentence = step === null ? undefined : CARRY_STEP[step];
      if (sentence === undefined) return error.message;
      return TIMEOUTS.has(field(error.details, 'cause_code') ?? '')
        ? `${sentence} The host timed out.`
        : sentence;
    }
    default:
      return error.message;
  }
}

export function describeMoveError(
  error: IpcError | null,
  status: 'failed' | 'partial',
  toHost: string,
): MoveFailure {
  const standing =
    status === 'partial'
      ? `The session is running on ${toHost}, but the source could not be retired. Both sessions were left as they are.`
      : `The source session was not touched. Anything copied to ${toHost} was cleaned up.`;
  if (error === null) {
    return {
      what: 'The move failed. It was started elsewhere, so the reason is in that window or in the session timeline.',
      standing,
    };
  }
  return { what: what(error, toHost), standing };
}
