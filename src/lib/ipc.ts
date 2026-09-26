import { invokeCmd, type Result } from './result';

export interface Health {
  version: string;
  db_ready: boolean;
  schema_version: number;
  /** Work graph M12.4; absent from an older hub. */
  trackers?: TrackersHealth;
}

/** One tracker in `fleet_health.trackers` (`service/health.rs`). */
export interface TrackerHealth {
  tracker_id: number;
  provider?: string;
  name?: string;
  org_id?: number | null;
  /** ok | degraded | failing; an unknown value reads as degraded. */
  status: string;
  state?: string;
  consecutive_failures?: number;
  last_error?: string | null;
  last_success_at?: number | null;
  last_pass_at?: number | null;
}

/** The trackers' sync health and the detection backlog. */
export interface TrackersHealth {
  trackers?: TrackerHealth[];
  failing?: number;
  degraded?: number;
  /** Suggestions older than `backlog_days` still waiting on a person. */
  detection_backlog?: number;
  backlog_days?: number;
}

/** The footer's tracker line ("trackers: 1 failing, 2 degraded · 4
 *  suggestions waiting > 7 d"), or null when there is nothing to say. */
export function trackersHealthLine(t: TrackersHealth | null | undefined): string | null {
  if (!t) return null;
  const parts: string[] = [];
  const bad = [
    t.failing ? `${t.failing} failing` : null,
    t.degraded ? `${t.degraded} degraded` : null,
  ].filter(Boolean);
  if (bad.length > 0) parts.push(`trackers: ${bad.join(', ')}`);
  const n = t.detection_backlog ?? 0;
  if (n > 0) parts.push(`${n} suggestion${n === 1 ? '' : 's'} waiting > ${t.backlog_days ?? 7} d`);
  return parts.length > 0 ? parts.join(' · ') : null;
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
