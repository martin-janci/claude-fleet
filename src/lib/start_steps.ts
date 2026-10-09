// The steps a session start reports (redesign step 5.13): the wire shape of
// `start:progress` (`fleet_core::events::StartProgress`) and the fold that
// turns its frames into a start's state. Imports nothing, so `sessions.ts`
// and `session_loaders.ts` can both use it without a cycle.

/** The three steps a start reports, in order — `fleet_core::events::StartStep`
 *  (`frontend_declares_the_start_steps_in_order` reads this line). */
export const START_STEPS = ['worktree', 'tmux', 'agent'] as const;
export type StartStepId = (typeof START_STEPS)[number];
export type StartStepState = 'pending' | 'started' | 'done' | 'failed';
export type StartSteps = Readonly<Record<StartStepId, StartStepState>>;

/** The wire shape of `start:progress` (`fleet_core::events::StartProgress`). */
export interface StartProgressFrame {
  token: string;
  step: StartStepId;
  index: number;
  total: number;
  state: 'started' | 'done' | 'warned' | 'failed';
}

/** Nothing reported yet: the command was sent, no step has answered. */
export const NO_START_STEPS: StartSteps = { worktree: 'pending', tmux: 'pending', agent: 'pending' };

const RANK: Record<StartStepState, number> = { pending: 0, started: 1, done: 2, failed: 2 };

/**
 * Fold one `start:progress` frame into a start's steps. A step only moves
 * forward (a late `started` never undoes its `done`), a step that starts
 * closes the ones before it, and an unknown step or state changes nothing.
 */
export function foldStartProgress(steps: StartSteps, f: Pick<StartProgressFrame, 'step' | 'state'>): StartSteps {
  const i = START_STEPS.indexOf(f.step);
  if (i < 0) return steps;
  const next: StartStepState | null =
    f.state === 'started' ? 'started' : f.state === 'done' || f.state === 'warned' ? 'done' : f.state === 'failed' ? 'failed' : null;
  if (!next || RANK[next] < RANK[steps[f.step]] || steps[f.step] === 'failed') return steps;
  const out: Record<StartStepId, StartStepState> = { ...steps, [f.step]: next };
  for (const before of START_STEPS.slice(0, i)) if (out[before] !== 'failed') out[before] = 'done';
  return out;
}

/** An opaque id for one start (`NewSessionArgs.start_token`): letters,
 *  digits and `-`, well under the backend's 64. */
export function newStartToken(): string {
  const rnd = Math.random().toString(36).slice(2, 10).replace(/[^a-z0-9]/g, '');
  return `st-${Date.now().toString(36)}-${rnd || '0'}`;
}
