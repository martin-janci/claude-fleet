// Work → Missions → Compare (lanes design 2026-10-23, L6): the run
// metrics of the missions that have run, side by side, and a summary per
// execution mode (`item`: each run its own session; `lane`: a lane's tasks
// in one lead session), so the owner can tell which mode makes more
// progress for its cost before switching the default.
//
// Pure apart from `loadCompare`, which reads each mission's detail (the
// metrics ride `work { mission }`'s `stats`; the list does not carry them).

import { getMission, type Mission, type MissionDetail, type MissionPolicy } from './missions';

export type Execution = NonNullable<MissionPolicy['execution']>;

/** One mission's metrics, normalised per done task where that is fair. */
export interface CompareRow {
  id: number;
  name: string;
  execution: Execution;
  state: string;
  done: number;
  total: number;
  /** Attempts per done task; null before the first is done. */
  runsPerDone: number | null;
  retries: number;
  /** Sessions started (cold starts) per done task. */
  coldStartsPerDone: number | null;
  /** A done task's median time, seconds. */
  taskSecs: number | null;
  /** The share of the working span with no run open, 0–100. */
  idlePct: number;
  /** What it spent per done task, micro-USD; null without a cost. */
  costPerDoneMicros: number | null;
}

/** The median of the rows that have a value, per mode. */
export interface ModeSummary {
  execution: Execution;
  missions: number;
  doneTasks: number;
  runsPerDone: number | null;
  coldStartsPerDone: number | null;
  taskSecs: number | null;
  idlePct: number | null;
  costPerDoneMicros: number | null;
}

/** The states of a mission that has run (a draft has no metrics). */
const RAN = ['active', 'paused', 'completed', 'failed', 'cancelled'];

/** PURE: a mission's row, or null when it has no working span yet. */
export function compareRow(d: MissionDetail): CompareRow | null {
  const st = d.stats;
  if (!st || st.span_secs <= 0) return null;
  const done = st.items_done;
  const per = (n: number | null | undefined) => (done > 0 && n != null ? n / done : null);
  return {
    id: d.mission.id,
    name: d.mission.name,
    execution: d.mission.policy?.execution === 'lane' ? 'lane' : 'item',
    state: d.mission.state,
    done,
    total: st.items,
    runsPerDone: st.runs_per_done_item_x100 != null ? st.runs_per_done_item_x100 / 100 : null,
    retries: st.retries,
    coldStartsPerDone: per(st.sessions),
    taskSecs: st.item_secs_p50 ?? null,
    idlePct: Math.round((st.idle_secs * 100) / st.span_secs),
    costPerDoneMicros: per(d.mission.cost_micros),
  };
}

function median(xs: (number | null)[]): number | null {
  const v = xs.filter((x): x is number => x != null).sort((a, b) => a - b);
  if (v.length === 0) return null;
  const m = Math.floor(v.length / 2);
  return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2;
}

/** PURE: the median of each metric per execution mode, `item` first; a
 *  mode with no mission is left out. */
export function summarize(rows: CompareRow[]): ModeSummary[] {
  return (['item', 'lane'] as const)
    .map((execution) => {
      const r = rows.filter((x) => x.execution === execution);
      return {
        execution,
        missions: r.length,
        doneTasks: r.reduce((a, x) => a + x.done, 0),
        runsPerDone: median(r.map((x) => x.runsPerDone)),
        coldStartsPerDone: median(r.map((x) => x.coldStartsPerDone)),
        taskSecs: median(r.map((x) => x.taskSecs)),
        idlePct: median(r.map((x) => x.idlePct)),
        costPerDoneMicros: median(r.map((x) => x.costPerDoneMicros)),
      };
    })
    .filter((s) => s.missions > 0);
}

/** PURE: which missions to compare: those that have run, newest first, at
 *  most `limit` (each one costs a detail read). */
export function missionsToCompare(missions: Mission[], limit = 12): Mission[] {
  return missions
    .filter((m) => RAN.includes(m.state) && m.mode !== 'plan')
    .sort((a, b) => b.updated_at - a.updated_at)
    .slice(0, limit);
}

/** Read the metrics of the missions to compare. A detail that fails to
 *  read is counted in `failed`, not shown. */
export async function loadCompare(missions: Mission[]): Promise<{ rows: CompareRow[]; failed: number }> {
  const picked = missionsToCompare(missions);
  const read = await Promise.all(picked.map((m) => getMission(m.id)));
  let failed = 0;
  const rows: CompareRow[] = [];
  for (const r of read) {
    if (!r.ok) {
      failed += 1;
      continue;
    }
    const row = compareRow(r.value);
    if (row) rows.push(row);
  }
  return { rows, failed };
}
