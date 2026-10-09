import { invokeCmd, type Result } from './result';
import type { DecideHealth } from './decide_health';
import type { TrackersHealth } from './tracker_health';
import type { OrgBudgetAlert } from './org_budget';

export interface Health {
  version: string;
  db_ready: boolean;
  schema_version: number;
  /** Work graph M12.4: the tracker roll-up; absent from an older hub. */
  trackers?: TrackersHealth;
  /** The hub's context threshold (percent); absent from an older hub. */
  context_red_pct?: number;
  /** The Jev decision envelope's last hour; absent when it is off, from an
   *  older hub, and for a scoped caller. */
  decide?: DecideHealth;
  /** Org administration phase C: the orgs at or over a budget; absent when
   *  none are, from an older hub, and for a caller that does not see every
   *  session. */
  org_budgets?: OrgBudgetAlert[];
  /** Every background loop's last and next run (redesign 8.1); absent from
   *  an older hub. */
  loops?: LoopHealth[];
  /** `automation.paused`: the pausable loops stand still. */
  automation_paused?: boolean;
}

/** One background loop (`service::loops::LoopHealth`). */
export interface LoopHealth {
  name: string;
  label: string;
  /** Stops while `automation.paused` is on. */
  pausable: boolean;
  /** Why a loop that is not pausable keeps running on Pause all. */
  keeps_running?: string | null;
  last_run_at?: number | null;
  next_run_at?: number | null;
  /** `ok`, `error` or `paused`; absent before its first run. */
  result?: 'ok' | 'error' | 'paused' | string | null;
  last_error?: string | null;
  runs: number;
  failures: number;
}

/**
 * The fleet in front of you, not the process you launched.
 *
 * Standalone those are the same thing. Pointed at a hub this is the HUB's
 * version, database and schema, because `health_check` routes to the hub's
 * `fleet_health` tool — which is why it answers a `Result` now rather than a
 * bare `Health`. A hub can be unreachable, and the only thing the old
 * signature could do about that was fall back to the local database, which a
 * hub client never fills: a perfectly zeroed fleet (no stuck sessions, no
 * ghosts, nothing in the red) is the most reassuring thing this app can say,
 * and it was saying it about a fleet it was not looking at.
 */
export async function healthCheck(): Promise<Result<Health>> {
  return invokeCmd<Health>('health_check');
}
