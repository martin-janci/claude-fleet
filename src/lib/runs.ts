// Runs (Orbit Fleet redesign 8.3): every run on the fleet's behalf —
// dispatched tasks, a mission's actions and brakes, Jev's decisions and
// fleet's own `claude -p` runs (planner, summary, triage, …), routine fires, the built-in
// agents' jobs (review, support) — newest first, each
// linked to the sessions it ran in. The Automation screen's Runs list (8.4)
// reads it. `list_runs` routes to the hub's `runs { list }` on a paired
// desktop. Mirrors `fleet_core::store::RunRow` / `service::runs`.
import { invokeCmd, type Result } from './result';

/** Who ran it (`RUN_KINDS`). */
export type RunKind =
  | 'operator'
  | 'task'
  | 'mission'
  | 'jev'
  | 'planner'
  | 'summary'
  | 'commit_message'
  | 'release_note'
  | 'morning_brief'
  | 'brief'
  | 'watch_summary'
  | 'triage'
  | 'context_help'
  | 'headless_check'
  | 'routine'
  | 'review'
  | 'support';

/** How it ended, in plain words (`RUN_OUTCOMES`). */
export type RunOutcome = 'ok' | 'failed' | 'needs_person' | 'nothing_to_do' | 'running';

/** Which table it came from (`RUN_SOURCES`). */
export type RunSource = 'task' | 'orchestration' | 'jev' | 'aux' | 'routine' | 'agent';

/** One run. Optional fields are absent (never null) on the wire. */
export interface RunRow {
  /** `<source>:<rowid>`, stable for the row's life. */
  id: string;
  source: RunSource | string;
  kind: RunKind | string;
  /** A mission's or routine's name, a session's name, `operator`, a Jev use case, `summary`. */
  owner: string;
  /** Unix seconds. */
  started_at: number;
  ended_at?: number;
  duration_ms?: number;
  outcome: RunOutcome | string;
  error?: string;
  /** Micro-USD. Absent for a task: its spend is its worker session's. */
  cost_micros?: number;
  model?: string;
  host?: string;
  org_id?: number;
  mission_id?: number;
  /** The sessions it ran in or acted on, the worker first. */
  session_ids: number[];
  summary?: string;
  /** The routine a fire belongs to (`source: 'routine'` only). */
  routine_id?: number;
}

/** `list_runs`' filters. Every field narrows. */
export interface RunsFilter {
  since?: number;
  until?: number;
  kind?: RunKind;
  outcome?: RunOutcome;
  org_id?: number;
  mission_id?: number;
  session_id?: number;
  /** One routine's fires. */
  routine_id?: number;
  /** ≤ 200; 50 when absent. */
  limit?: number;
  offset?: number;
}

export interface RunsPage {
  runs: RunRow[];
  /** How many match in all; the page is `limit` of them. */
  total: number;
}

/** One page of the runs this desktop (or, paired, this device) may see. */
export async function listRuns(filter: RunsFilter = {}): Promise<Result<RunsPage>> {
  const args: Record<string, number | string> = {};
  for (const [k, v] of Object.entries(filter)) {
    if (v !== undefined && v !== null && v !== '') args[k] = v as number | string;
  }
  const r = await invokeCmd<RunsPage>('list_runs', { args });
  if (!r.ok) return r;
  return {
    ok: true,
    value: {
      runs: (r.value?.runs ?? []).map((row) => ({ ...row, session_ids: row.session_ids ?? [] })),
      total: r.value?.total ?? 0,
    },
  };
}

const OUTCOME_WORDS: Record<RunOutcome, string> = {
  ok: 'Done',
  failed: 'Failed',
  needs_person: 'Needs you',
  nothing_to_do: 'Nothing to do',
  running: 'Working',
};

/** A run's outcome in plain words; a failure carries its error. */
export function outcomeLabel(run: Pick<RunRow, 'outcome' | 'error'>): string {
  const word = OUTCOME_WORDS[run.outcome as RunOutcome] ?? run.outcome;
  return run.outcome === 'failed' && run.error ? `${word}: ${run.error}` : word;
}

// ── Run traces (run timeline and lanes) ────────────────────────────────────
// `runs { trace }` / desktop `run_trace`: one routine or agent run's steps,
// oldest first, from migration 194's `run_trace`. Mirrors
// `fleet_core::store::RunTraceRow` / `service::run_trace`. A hub that
// predates the action answers `E_INVALID`; the timeline then shows a run
// with no steps.

/** The phases a run passes through, in order (`RUN_PHASES`). */
export const RUN_PHASES = ['trigger', 'checks', 'placement', 'spawn', 'prompt', 'working', 'settle', 'outcome'] as const;
export type RunPhase = (typeof RUN_PHASES)[number];

export const PHASE_LABEL: Record<RunPhase, string> = {
  trigger: 'Trigger',
  checks: 'Checks',
  placement: 'Placement',
  spawn: 'Session started',
  prompt: 'Prompt delivered',
  working: 'Working',
  settle: 'Settle',
  outcome: 'Outcome',
};

/** One step. Optional fields are absent (never null) on the wire. */
export interface RunTraceStep {
  seq: number;
  attempt: number;
  /** The run this one retries. */
  parent_ref?: string;
  /** Unix seconds. */
  at: number;
  phase: RunPhase | string;
  /** trigger | check | placement | spawn | prompt | tool_call | thinking |
   *  waiting | decision | settle | outcome | retry | error | note */
  kind: string;
  /** scheduler | hub | claude | jev | rule | person */
  actor: string;
  summary: string;
  data?: Record<string, unknown>;
  duration_ms?: number;
  cost_micros?: number;
  /** The Jev decision the step records. */
  decision_id?: number;
}

export interface RunTrace {
  run_id: string;
  steps: RunTraceStep[];
  /** More steps follow the last one: read on from its `seq`. */
  more: boolean;
}

/** Whether a run keeps a trace: routine fires and agent jobs do. */
export function hasTrace(run: Pick<RunRow, 'source'>): boolean {
  return run.source === 'routine' || run.source === 'agent';
}

/** One page of run `runId`'s steps after `afterSeq`. */
export async function runTrace(runId: string, afterSeq?: number, limit?: number): Promise<Result<RunTrace>> {
  const args: Record<string, string | number> = { run_id: runId };
  if (afterSeq !== undefined) args.after_seq = afterSeq;
  if (limit !== undefined) args.limit = limit;
  const r = await invokeCmd<RunTrace>('run_trace', { args });
  if (!r.ok) return r;
  return { ok: true, value: { run_id: r.value?.run_id ?? runId, steps: r.value?.steps ?? [], more: !!r.value?.more } };
}

/** The pages a read on takes at most: 10 × 500 steps covers a capped run. */
const TRACE_PAGES = 10;

/** `have` and every step after its last, page by page while `more`. */
export async function readTraceOn(runId: string, have: RunTraceStep[] = []): Promise<Result<RunTraceStep[]>> {
  let steps = have;
  for (let i = 0; i < TRACE_PAGES; i++) {
    const after = steps.length > 0 ? steps[steps.length - 1].seq : undefined;
    const r = await runTrace(runId, after);
    if (!r.ok) return r;
    steps = steps.concat(r.value.steps.filter((s) => after === undefined || s.seq > after));
    if (!r.value.more || r.value.steps.length === 0) break;
  }
  return { ok: true, value: steps };
}

/** A phase's state on the rail: reached and passed, the one live now, an
 *  error in it, a fire that stopped there, or not reached. */
export type PhaseState = 'done' | 'live' | 'failed' | 'skipped' | 'todo';

export interface PhaseCell {
  phase: RunPhase;
  label: string;
  state: PhaseState;
  /** The phase's last step, in words; empty when not reached. */
  meta: string;
  /** How long its steps took, summed. */
  ms?: number;
  count: number;
}

/**
 * PURE: the phase rail for `steps` of a run that ended `outcome`. A phase
 * with steps is done (failed when one is an error); while the run is
 * `running` the phase after the newest reached one is live (`working` once
 * the prompt is in); a skipped fire stops at its checks.
 */
export function phaseRail(steps: RunTraceStep[], outcome: string): PhaseCell[] {
  const by = new Map<string, RunTraceStep[]>();
  for (const s of steps) by.set(s.phase, [...(by.get(s.phase) ?? []), s]);
  const skipped = (by.get('checks') ?? []).some((s) => s.data && s.data.skipped);
  let last = -1;
  RUN_PHASES.forEach((p, i) => {
    if (by.has(p)) last = i;
  });
  const running = outcome === 'running';
  return RUN_PHASES.map((phase, i) => {
    const mine = by.get(phase) ?? [];
    const tail = mine[mine.length - 1];
    let state: PhaseState = 'todo';
    if (mine.some((s) => s.kind === 'error')) state = 'failed';
    else if (phase === 'checks' && skipped) state = 'skipped';
    else if (mine.length > 0) state = 'done';
    if (running && !skipped) {
      // The newest reached phase is still going when it is `working`, or
      // the next one is the run's live edge.
      const live = RUN_PHASES[last] === 'working' || RUN_PHASES[last] === 'prompt' ? RUN_PHASES.indexOf('working') : last + 1;
      if (i === live && state !== 'failed') state = 'live';
    }
    const ms = mine.reduce((a, s) => a + (s.duration_ms ?? 0), 0);
    const meta =
      phase === 'working' && mine.length > 0 ? `${mine.length} step${mine.length === 1 ? '' : 's'}${running ? ' so far' : ''}` : (tail?.summary ?? '');
    return { phase, label: PHASE_LABEL[phase], state, meta, ms: ms > 0 ? ms : undefined, count: mine.length };
  });
}

/** `ms` as "410 ms", "41 s", "2m 48s". */
export function tookWords(ms: number | undefined): string {
  if (ms === undefined) return '';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

/** Where a step sits on its run's clock, in percent of `[from, to]`. */
export function stepBar(step: RunTraceStep, from: number, to: number): { left: number; width: number } {
  const span = Math.max(to - from, 1);
  const end = step.at;
  const start = step.duration_ms !== undefined ? end - step.duration_ms / 1000 : end;
  const left = Math.min(Math.max(((start - from) / span) * 100, 0), 100);
  const right = Math.min(Math.max(((end - from) / span) * 100, 0), 100);
  return { left, width: Math.max(right - left, 0.8) };
}

/** The lane groups, in the order the lanes draw them. */
export const LANE_GROUPS = ['Routines', 'Agents', 'Missions', 'Jev', 'Fleet'] as const;
export type LaneGroup = (typeof LANE_GROUPS)[number];

/** Which group a run's lane sits in. */
export function laneGroup(run: Pick<RunRow, 'source'>): LaneGroup {
  switch (run.source) {
    case 'routine':
      return 'Routines';
    case 'agent':
      return 'Agents';
    case 'task':
    case 'orchestration':
      return 'Missions';
    case 'jev':
      return 'Jev';
    default:
      return 'Fleet';
  }
}

export interface Lane {
  key: string;
  group: LaneGroup;
  name: string;
  host?: string;
  runs: RunRow[];
}

/**
 * PURE: `runs` laid out one lane per owner (a routine, an agent, a mission,
 * a Jev use case), grouped as [`LANE_GROUPS`], lanes with a live run first
 * and then by name. Only runs that overlap `[from, to]` are kept.
 */
export function buildLanes(runs: RunRow[], from: number, to: number): { group: LaneGroup; lanes: Lane[] }[] {
  const lanes = new Map<string, Lane>();
  for (const run of runs) {
    const end = run.ended_at ?? (run.outcome === 'running' ? to : run.started_at);
    if (end < from || run.started_at > to) continue;
    const group = laneGroup(run);
    const name = run.source === 'agent' ? run.kind : run.owner;
    const key = `${group}:${name}`;
    const lane = lanes.get(key) ?? { key, group, name, runs: [] };
    lane.host ??= run.host;
    lane.runs.push(run);
    lanes.set(key, lane);
  }
  const live = (l: Lane) => l.runs.some((r) => r.outcome === 'running');
  return LANE_GROUPS.map((group) => ({
    group,
    lanes: [...lanes.values()]
      .filter((l) => l.group === group)
      .sort((a, b) => Number(live(b)) - Number(live(a)) || a.name.localeCompare(b.name)),
  })).filter((g) => g.lanes.length > 0);
}

/** How a run draws in its lane: a bar with a width, or a mark at a point
 *  (a fire that never started, one Jev decision). */
export type RunMark = 'bar' | 'skip' | 'jev';

export function runMark(run: RunRow): RunMark {
  if (run.source === 'jev') return 'jev';
  // A routine fire that started no session was skipped (overlap, budget…).
  if (run.source === 'routine' && run.outcome === 'nothing_to_do' && run.session_ids.length === 0) return 'skip';
  return 'bar';
}

/** A run's span on `[from, to]` in percent; a live run reaches `now`. */
export function runSpan(run: RunRow, from: number, to: number, now: number): { left: number; width: number } {
  const span = Math.max(to - from, 1);
  const end =
    run.ended_at ?? (run.duration_ms !== undefined ? run.started_at + run.duration_ms / 1000 : run.outcome === 'running' ? now : run.started_at);
  const pct = (t: number) => Math.min(Math.max(((t - from) / span) * 100, 0), 100);
  const left = pct(run.started_at);
  return { left, width: Math.max(pct(end) - left, 0.6) };
}
