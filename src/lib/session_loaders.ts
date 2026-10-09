// Session loaders (Orbit Fleet redesign step 5.13): the pure halves of the
// Pulse sequence a session start shows and the line the Atom sits beside
// while the agent thinks. Both advance on what the app is told (a command
// in flight, a row arriving, a status flipping), never on a timer.
import { sessionAgent, type SessionRow } from './sessions';
import { AGENT_LABELS } from './row_groups';
import type { StartProgress } from './start_progress';
import { START_STEPS, type StartStepId, type StartSteps } from './start_steps';

export type PulseState = 'done' | 'active' | 'pending';

export interface PulseStep {
  id: 'worktree' | 'tmux' | 'agent';
  label: string;
  state: PulseState;
  /** What the step left behind ("created", "pane 3"); null while it runs. */
  note: string | null;
}

export interface SessionPulse {
  steps: PulseStep[];
  /** 1-based step the start is on; `steps.length` once done. */
  at: number;
  done: boolean;
  /** "Setting up worktree · 1 of 3", or "Started" once done. */
  caption: string;
}

const ACTIVE: Record<PulseStep['id'], string> = {
  worktree: 'Setting up worktree',
  tmux: 'Starting tmux',
  agent: 'Starting the agent',
};

function pulse(steps: PulseStep[]): SessionPulse {
  const i = steps.findIndex((s) => s.state !== 'done');
  const done = i < 0;
  const at = done ? steps.length : i + 1;
  const caption = done ? 'Started' : `${steps[i].id === 'agent' ? steps[i].label + ' starting' : ACTIVE[steps[i].id]} · ${at} of ${steps.length}`;
  return { steps, at, done, caption };
}

/** The agent's name on the step, from `sessions.agent` (5.1's name). */
export function agentName(row: Pick<SessionRow, 'kind' | 'agent'>): string {
  return AGENT_LABELS[sessionAgent(row)] ?? 'Claude Code';
}

/**
 * A plain start (⌘N): `null` while the create command is in flight — the
 * worktree and tmux steps run inside it, so the worktree is what it is on —
 * then the row it made. Once the row is here the worktree and tmux are
 * done; the agent is done when the row carries an agent status (a shell
 * needs nothing more than its pane).
 */
export function sessionPulse(
  row: Pick<SessionRow, 'kind' | 'agent' | 'claude_status'> | null,
  kind: 'work' | 'shell' = 'work',
): SessionPulse {
  const name = row ? agentName(row) : kind === 'shell' ? 'Shell' : 'Claude Code';
  if (!row) {
    return pulse([
      { id: 'worktree', label: 'Worktree', state: 'active', note: null },
      { id: 'tmux', label: 'tmux', state: 'pending', note: null },
      { id: 'agent', label: name, state: 'pending', note: null },
    ]);
  }
  const agentUp = row.kind === 'shell' || row.claude_status !== null;
  return pulse([
    { id: 'worktree', label: 'Worktree', state: 'done', note: 'ready' },
    { id: 'tmux', label: 'tmux', state: 'done', note: 'up' },
    { id: 'agent', label: name, state: agentUp ? 'done' : 'active', note: agentUp ? 'ready' : null },
  ]);
}

// ─── the start's own steps (`start:progress`) ────────────────────────────────

export {
  START_STEPS,
  NO_START_STEPS,
  foldStartProgress,
  newStartToken,
  type StartStepId,
  type StartStepState,
  type StartSteps,
  type StartProgressFrame,
} from './start_steps';

const DONE_NOTE: Record<StartStepId, string> = { worktree: 'ready', tmux: 'up', agent: 'launched' };

/**
 * The Pulse for a start in flight, from the steps the backend reported
 * (`start:progress`): a satellite lights when its step says `done`, pulses
 * while it says `started`, and nothing moves until a frame arrives.
 */
export function startPulse(steps: StartSteps, kind: 'work' | 'shell' = 'work', agent = kind === 'shell' ? 'Shell' : 'Claude Code'): SessionPulse {
  const label: Record<StartStepId, string> = { worktree: 'Worktree', tmux: 'tmux', agent };
  return pulse(
    START_STEPS.map((id) => {
      const st = steps[id];
      return {
        id,
        label: label[id],
        state: st === 'done' ? 'done' : st === 'started' ? 'active' : 'pending',
        note: st === 'done' ? DONE_NOTE[id] : st === 'failed' ? 'failed' : null,
      } satisfies PulseStep;
    }),
  );
}

/**
 * A ticket start (task → session P-5): its own timeline steps, read by
 * `startSteps`, folded onto the same three. The start writes the session
 * (tmux) and its checkout, then waits for the agent's prompt.
 */
export function ticketPulse(p: StartProgress): SessionPulse {
  const step = (id: string) => p.steps.find((s) => s.id === id);
  const repl = step('repl');
  const agentDone = repl ? repl.state === 'done' : true;
  return pulse([
    { id: 'worktree', label: 'Worktree', state: 'done', note: step('worktree')?.label.toLowerCase() ?? 'ready' },
    { id: 'tmux', label: 'tmux', state: 'done', note: 'up' },
    { id: 'agent', label: 'Claude Code', state: agentDone ? 'done' : p.failed ? 'pending' : 'active', note: agentDone ? 'ready' : null },
  ]);
}

const PROGRESSIVE: Record<string, string> = {
  Read: 'reading',
  Edit: 'editing',
  Write: 'writing',
  Run: 'running',
  Search: 'searching',
  Find: 'finding',
  Fetch: 'fetching',
  'Search web': 'searching the web',
  'Update todos': 'updating todos',
};

/**
 * The Atom's line while the agent works: "Thinking · reading hub/pair.rs ·
 * 14 s" when a tool call is running (its label is "Read hub/pair.rs"),
 * "Thinking · <label>" for anything else it is doing, and "Thinking" alone
 * between calls.
 */
export function thinkingLabel(doing: { label: string; since: string | null } | null): string {
  if (!doing) return 'Thinking';
  const verb = Object.keys(PROGRESSIVE)
    .sort((a, b) => b.length - a.length)
    .find((v) => doing.label === v || doing.label.startsWith(`${v} `));
  const what = verb ? `${PROGRESSIVE[verb]}${doing.label.slice(verb.length)}` : doing.label;
  return doing.since ? `Thinking · ${what} · ${doing.since}` : `Thinking · ${what}`;
}
