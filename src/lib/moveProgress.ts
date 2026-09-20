// The wire shape of `move:progress` (`fleet_core::events::MoveProgress`) and
// the nine steps of a move. The Rust test
// `frontend_declares_the_move_steps_in_order` reads the list between the two
// markers below: keep nothing but the step names in single quotes there.

// move-steps:begin
export const MOVE_STEPS = [
  'check',
  'transcript',
  'workspace',
  'git',
  'replay',
  'ignored',
  'claude_state',
  'start',
  'handoff',
] as const;
// move-steps:end

export type MoveStep = (typeof MOVE_STEPS)[number];
export type MoveStepState = 'started' | 'done' | 'warned' | 'failed';

export interface MoveProgress {
  /** The SOURCE session's row id. */
  session_id: number;
  to_host: string;
  step: MoveStep;
  /** 1-based position of `step` in `MOVE_STEPS`. */
  index: number;
  total: number;
  state: MoveStepState;
  /** A short count ("2 commits"); null on `failed`. */
  detail: string | null;
}

/** What the sheet calls a step. */
export function stepLabel(step: MoveStep, toHost: string): string {
  switch (step) {
    case 'check':
      return 'Check the source';
    case 'transcript':
      return 'Read the conversation';
    case 'workspace':
      return 'Prepare the target';
    case 'git':
      return 'Carry the git work';
    case 'replay':
      // The worktree is created and fast-forwarded here, after the fetch —
      // see `MoveStep` in `crates/fleet-core/src/events.rs`.
      return 'Set up the worktree, replay uncommitted work';
    case 'ignored':
      return 'Ignored files';
    case 'claude_state':
      return 'Subagents and memory';
    case 'start':
      return `Start on ${toHost}`;
    case 'handoff':
      return 'Hand over';
  }
}
