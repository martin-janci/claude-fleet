// Runs (Orbit Fleet redesign 8.3): every run on the fleet's behalf —
// dispatched tasks, a mission's actions and brakes, Jev's decisions and
// fleet's own `claude -p` runs (planner, summary, triage, …), routine fires — newest first, each
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
  | 'routine';

/** How it ended, in plain words (`RUN_OUTCOMES`). */
export type RunOutcome = 'ok' | 'failed' | 'needs_person' | 'nothing_to_do' | 'running';

/** Which table it came from (`RUN_SOURCES`). */
export type RunSource = 'task' | 'orchestration' | 'jev' | 'aux' | 'routine';

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
