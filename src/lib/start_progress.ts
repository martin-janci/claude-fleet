// A start's progress (task → session spec P-5): the steps a start writes
// to its session's timeline, read back as one strip under the Work button.
//
//   start_spawned     the session is up (detail: `StartSpawned` JSON)
//   worktree_ready    its checkout exists (detail: the branch)
//   repl_ready        Claude's prompt has settled
//   handover_waiting  a dialog is up (`trust_prompt` | `dialog`), or the
//                     brief did not get through (any other detail)
//   handover_started  the brief was taken: the agent is working
//   start_abandoned   Cancel start ran
//
// `startSteps` is pure so the strip's states are tested without a session.

import type { SessionEvent } from './timeline';

/** `StartSpawned` in `trackers/tickets.rs`. */
export interface StartSpawned {
  key: string;
  worktree_id: number | null;
  new_worktree: boolean;
  branch: string | null;
  brief: boolean;
}

export type StepState = 'done' | 'active' | 'pending';

export interface StartStep {
  id: 'spawned' | 'worktree' | 'repl' | 'brief';
  label: string;
  state: StepState;
}

export interface StartProgress {
  steps: StartStep[];
  /** A dialog the person has to answer in the terminal. */
  waiting: 'trust_prompt' | 'dialog' | null;
  /** Why the brief did not get through, in words. */
  failed: string | null;
  /** The agent has its brief (or the start sends none). */
  done: boolean;
  /** Cancel start was run. */
  abandoned: boolean;
  /** Cancel start is worth offering: the start made a checkout of its own
   *  and nobody is working in it yet. The service re-checks it. */
  cancellable: boolean;
}

const DIALOGS = new Set(['trust_prompt', 'dialog']);

const FAILURES: Record<string, string> = {
  repl_not_ready: 'Claude did not come up in time',
  dialog_not_answered: 'the dialog was not answered in time',
  'start prompt not acknowledged': 'Claude did not take the brief',
};

function parseSpawned(detail: string | null): StartSpawned | null {
  if (!detail) return null;
  try {
    const v = JSON.parse(detail) as StartSpawned;
    return typeof v === 'object' && v !== null ? v : null;
  } catch {
    return null;
  }
}

/** The strip's state, from a session's events in any order. `null` when
 *  the session was not made by a start. */
export function startSteps(events: SessionEvent[]): StartProgress | null {
  const sorted = [...events].sort((a, b) => a.at - b.at || a.id - b.id);
  const spawnedAt = sorted.findLastIndex((e) => e.kind === 'start_spawned');
  if (spawnedAt < 0) return null;
  const spawned = parseSpawned(sorted[spawnedAt].detail);
  const after = sorted.slice(spawnedAt + 1);
  const has = (kind: string) => after.some((e) => e.kind === kind);
  const brief = spawned?.brief ?? true;
  const worktree = spawned?.worktree_id != null || has('worktree_ready');
  const repl = has('repl_ready');
  const sent = has('handover_started');

  let waiting: StartProgress['waiting'] = null;
  let failed: string | null = null;
  const lastWait = after.findLast((e) => e.kind === 'handover_waiting');
  if (lastWait && !sent) {
    const d = lastWait.detail ?? '';
    if (DIALOGS.has(d)) waiting = d as 'trust_prompt' | 'dialog';
    else failed = FAILURES[d] ?? (d.startsWith('send failed') ? 'the brief could not be typed' : d || 'the brief did not get through');
  }
  const done = brief ? sent : true;

  const steps: StartStep[] = [{ id: 'spawned', label: 'Session', state: 'done' }];
  if (worktree) steps.push({ id: 'worktree', label: spawned?.new_worktree ? 'New checkout' : 'Checkout', state: 'done' });
  if (brief) {
    steps.push({ id: 'repl', label: 'Claude ready', state: repl ? 'done' : 'active' });
    steps.push({ id: 'brief', label: 'Brief sent', state: sent ? 'done' : repl ? 'active' : 'pending' });
  }
  if (failed) for (const s of steps) if (s.state === 'active') s.state = 'pending';

  const abandoned = has('start_abandoned');
  return {
    steps,
    waiting,
    failed,
    done,
    abandoned,
    cancellable: !abandoned && !sent && spawned?.new_worktree === true,
  };
}

/** One line for a screen reader and the strip's caption. */
export function progressText(p: StartProgress): string {
  if (p.abandoned) return 'Start cancelled';
  if (p.failed) return `Start stopped: ${p.failed}`;
  if (p.waiting === 'trust_prompt') return 'Waiting for you: trust this folder in the terminal';
  if (p.waiting === 'dialog') return 'Waiting for you: answer the dialog in the terminal';
  if (p.done) return 'Started: the agent has its brief';
  const next = p.steps.find((s) => s.state === 'active');
  return next ? `Starting: ${next.label.toLowerCase()}…` : 'Starting…';
}
