// A failed move, in words. The backend's messages are written for an
// operator reading a log; the sheet needs one sentence on what failed and one
// on where things stand.
import type { MoveStep } from './moveProgress';
import type { IpcError } from './result';
import { crossOrgOf } from './work';

export interface MoveFailure {
  what: string;
  standing: string;
  /** What the sheet may offer. `clean` carries the paths a cleanup would
   *  replace, so the confirmation can name them, and `more` — how many the
   *  backend's cap (`carry::LEFTOVER_CAP`) left out of `paths` — so a
   *  truncated confirmation can say so instead of understating what it is
   *  about to delete; `force_cross_org` is the org-boundary refusal (work
   *  graph M5), which a retry with `forceCrossOrg` carries through with a
   *  warning; `null` means there is nothing the app can do — only the user,
   *  on that host. */
  action:
    | { kind: 'retry' }
    | { kind: 'clean'; paths: string[]; more: number }
    | { kind: 'force_cross_org' }
    | null;
}

/** A frontend-only marker meaning "this run ended because you undid the
 *  partial move". It never comes from the backend and never crosses the
 *  wire, so it deliberately does NOT wear the `E_*` prefix: those codes are
 *  the backend's namespace (`ipc_error.rs`), and a frontend copy of one
 *  would collide the day the backend mints its own. */
export const UNDONE = 'MOVE_UNDONE';

/** `begin_wait`'s refusal of a second wait for a session
 *  (`crates/fleet-core/src/service/move_session/wait.rs`: "session {id} is
 *  already waiting to move"). `moves.ts` follows the recorded wait instead of
 *  failing on it when it can. */
export const ALREADY_WAITING = 'already waiting to move';

/** `require_confirmed_contract`'s refusal before the hub's first `ready`
 *  frame (`src-tauri/src/backend/remote.rs`: "this app has not yet confirmed
 *  {url}'s version"). The other `E_HUB_CONTRACT` refusals are a version
 *  skew, which only an update fixes. */
const CONTRACT_UNCONFIRMED = 'not yet confirmed';

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

function stringArrayField(details: unknown, key: string): string[] {
  if (typeof details !== 'object' || details === null) return [];
  const v = (details as Record<string, unknown>)[key];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

function numberField(details: unknown, key: string): number {
  if (typeof details !== 'object' || details === null) return 0;
  const v = (details as Record<string, unknown>)[key];
  return typeof v === 'number' ? v : 0;
}

/** `a, b and 7 more` — an older backend without `more_*` in `details` simply
 *  never adds the tail. */
function withMore(paths: string[], more: number): string {
  const s = paths.join(', ');
  return more > 0 ? `${s}${s ? ' and ' : ''}${more} more` : s;
}

type Action = MoveFailure['action'];

/** `E_MOVE_TARGET_DIRTY`: `details.leftovers` says whose work the target is
 *  holding. `ours` is a stale attempt's leftovers, safe to replace; `theirs`
 *  is the target's own uncommitted work, which only a person on that host can
 *  resolve; anything else means the check itself could not tell. */
function targetDirtyWhat(details: unknown, toHost: string): { what: string; action: Action } {
  const leftovers = field(details, 'leftovers');
  if (leftovers === 'ours') {
    const paths = stringArrayField(details, 'ours');
    const more = numberField(details, 'more_ours');
    return {
      what:
        `${toHost} still holds work an earlier transfer attempt left behind, and it differs from ` +
        `what is being carried now: ${withMore(paths, more)}.`,
      action: { kind: 'clean', paths, more },
    };
  }
  if (leftovers === 'theirs') {
    const paths = stringArrayField(details, 'theirs');
    const more = numberField(details, 'more_theirs');
    return {
      what:
        `${toHost} has uncommitted work of its own in this worktree: ${withMore(paths, more)}. Commit or ` +
        'discard it there first.',
      action: null,
    };
  }
  return {
    what: `${toHost} already has uncommitted work in this worktree. Clean it up there first.`,
    action: { kind: 'retry' },
  };
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

/** `E_FORBIDDEN` with `cross_org: true` (`orgs::check_cross_org`'s shape, as
 *  `crossOrgOf` reads it): a live work link of the source belongs to one
 *  org and the target host would put the session in another. Any other
 *  `E_FORBIDDEN` is a plain refusal. */
function forbiddenWhat(
  error: IpcError,
  toHost: string,
  orgName: (id: number) => string | undefined,
): { what: string; action: Action } {
  const c = crossOrgOf(error);
  if (!c) return { what: error.message, action: { kind: 'retry' } };
  const w = orgName(c.workOrgId) ?? `organisation ${c.workOrgId}`;
  const s = orgName(c.sessionOrgId) ?? `organisation ${c.sessionOrgId}`;
  return {
    what:
      `This session's work belongs to ${w}, and on ${toHost} the session would belong to ${s}. ` +
      "Fleet does not carry one company's work into another's session by mistake.",
    action: { kind: 'force_cross_org' },
  };
}

function what(
  error: IpcError,
  toHost: string,
  orgName: (id: number) => string | undefined,
): { what: string; action: Action } {
  switch (error.code) {
    case 'E_FORBIDDEN':
      return forbiddenWhat(error, toHost, orgName);
    case 'E_MOVE_DIRTY':
      return {
        what: 'The source has uncommitted work, and this move was asked to refuse that.',
        action: { kind: 'retry' },
      };
    case 'E_MOVE_UNPUSHED':
      return {
        what: 'The source has commits that are not pushed, and this move was asked to refuse that.',
        action: { kind: 'retry' },
      };
    case 'E_MOVE_MIDOP':
      return {
        what: 'The source is in the middle of a merge, rebase or similar. Finish or abort it, then move.',
        action: { kind: 'retry' },
      };
    case 'E_MOVE_TARGET_DIRTY':
      return targetDirtyWhat(error.details, toHost);
    case 'E_MOVE_TOO_LARGE':
      return { what: 'The work to carry is too large for a move.', action: { kind: 'retry' } };
    case 'E_LOCAL_ONLY':
      return {
        what: 'This desktop is a window onto a hub, and the hub refused the move.',
        action: null,
      };
    case 'E_HUB_CONTRACT':
      if (error.message.includes(CONTRACT_UNCONFIRMED)) {
        return {
          what: "The hub hasn't confirmed its version yet — try again in a moment, or update the hub.",
          action: { kind: 'retry' },
        };
      }
      // A skew: the message already says which side to update.
      return { what: error.message, action: null };
    case 'E_INVALID_STATE':
      if (error.message.includes(ALREADY_WAITING)) {
        return {
          what:
            'This session is already waiting to finish its turn before a transfer. Cancel that wait before starting another.',
          action: null,
        };
      }
      if (error.message.includes('already in progress')) {
        return { what: 'This session is already being moved.', action: { kind: 'retry' } };
      }
      // `require_source_idle`: only reached with `when: 'now'` — Transfer
      // itself now waits out a busy source, so this states the fact rather
      // than telling the user to do what the button already does.
      if (error.message.includes('is not idle')) {
        return {
          what: 'The source Claude is in the middle of a turn.',
          action: { kind: 'retry' },
        };
      }
      return { what: error.message, action: { kind: 'retry' } };
    case 'E_MOVE_PARTIAL':
      return { what: partialWhat(error.details, toHost), action: null };
    case 'E_MOVE_CARRY': {
      const step = field(error.details, 'step');
      const sentence = step === null ? undefined : CARRY_STEP[step];
      if (sentence === undefined) return { what: error.message, action: { kind: 'retry' } };
      const text = TIMEOUTS.has(field(error.details, 'cause_code') ?? '')
        ? `${sentence} The host timed out.`
        : sentence;
      return { what: text, action: { kind: 'retry' } };
    }
    case UNDONE:
      return {
        what: `You undid the transfer: the new session on ${toHost} was killed and the source is still running.`,
        action: null,
      };
    default:
      return { what: error.message, action: { kind: 'retry' } };
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
  /** Org names for the cross-org sentence; an id it does not know reads as
   *  `organisation <id>`. */
  orgName: (id: number) => string | undefined = () => undefined,
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
      action: { kind: 'retry' },
    };
  }
  const { what: text, action } = what(error, toHost, orgName);
  return { what: text, standing, action };
}
