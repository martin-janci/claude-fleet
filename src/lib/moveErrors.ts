// A failed move, in words. The backend's messages are written for an
// operator reading a log; the sheet needs one sentence on what failed and one
// on where things stand.
import type { MoveStep } from './moveProgress';
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

/**
 * The steps that touch nothing on the target. Up to and including
 * `transcript` the move has only read the source, so "nothing was copied" is
 * true; from `workspace` on, a clone, a worktree or files exist over there.
 */
const NOTHING_COPIED_YET: ReadonlySet<MoveStep> = new Set<MoveStep>(['check', 'transcript']);

/**
 * Codes the backend cannot produce before the target has been touched: the
 * carry starts at `workspace` (seeding the clone) and a dirty target is only
 * discovered by preparing its worktree.
 *
 * They override `reached`, because `reached` is how far the EVENTS got, and
 * they may never have arrived — an old hub, a dropped stream, or a result
 * that simply won the race leaves every step pending, which reads as `check`.
 * Promising "nothing was copied" on that is the one mistake here that sends
 * somebody looking for a clean host that is not clean.
 */
const ALWAYS_TOUCHED_THE_TARGET: ReadonlySet<string> = new Set([
  'E_MOVE_CARRY',
  'E_MOVE_TARGET_DIRTY',
]);

function field(details: unknown, key: string): string | null {
  if (typeof details !== 'object' || details === null) return null;
  const v = (details as Record<string, unknown>)[key];
  return typeof v === 'string' ? v : null;
}

/** A move that stopped after the target session existed. The `step` strings
 *  are the ones `partial(…)` is called with in
 *  `crates/fleet-core/src/service/move_session/mod.rs`; a step this build
 *  does not know falls back, so renaming one there is safe. */
function partialWhat(details: unknown, toHost: string): string {
  const step = field(details, 'step');
  if (step === 'confirming the target is running') {
    return `The new session on ${toHost} did not confirm that it is running.`;
  }
  if (step === 'source transcript changed after copy') {
    return (
      `The source wrote to the conversation after it was copied, so ${toHost} is missing the ` +
      `latest turn. Kill the session on ${toHost} and transfer again.`
    );
  }
  if (step !== null && step.startsWith('killing the source')) {
    return `The new session is running on ${toHost}, but the source could not be stopped. Kill the source yourself.`;
  }
  if (step === 'reconciling the target host') {
    return `The new session was started on ${toHost}, but fleet could not confirm it there.`;
  }
  return 'The new session started, but the last step of the move failed.';
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
      if (error.message.includes('already in progress')) return 'This session is already being moved.';
      // `require_source_idle`: the one refusal the user can simply wait out.
      if (error.message.includes('is not idle')) {
        return 'The source Claude is in the middle of a turn. Wait for it to finish, then transfer.';
      }
      return error.message;
    case 'E_MOVE_PARTIAL':
      return partialWhat(error.details, toHost);
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

/**
 * `reached` is the last step that is not pending — what the move had got to
 * when it stopped. It decides how much the target was left holding: the
 * cleanup removes the transfer scratch directories and nothing else.
 */
export function describeMoveError(
  error: IpcError | null,
  status: 'failed' | 'partial',
  toHost: string,
  reached: MoveStep | null,
): MoveFailure {
  const nothingCopied =
    (reached === null || NOTHING_COPIED_YET.has(reached)) &&
    !(error !== null && ALWAYS_TOUCHED_THE_TARGET.has(error.code));
  const standing =
    status === 'partial'
      ? `A new session exists on ${toHost} and the source is still there. Nothing was killed.`
      : nothingCopied
        ? `Nothing was copied to ${toHost}. The source session was not touched.`
        : 'The source session was not touched. Temporary transfer files were removed; what was ' +
          `already set up on ${toHost} — the clone, the worktree, copied files — was left there.`;
  if (error === null) {
    return {
      what: 'The move failed. It was started elsewhere, so the reason is in that window or in the session timeline.',
      standing,
    };
  }
  return { what: what(error, toHost), standing };
}
