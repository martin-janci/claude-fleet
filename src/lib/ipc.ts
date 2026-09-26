import { invokeCmd, type Result } from './result';
import type { TrackersHealth } from './tracker_health';

export interface Health {
  version: string;
  db_ready: boolean;
  schema_version: number;
  /** Work graph M12.4: the tracker roll-up; absent from an older hub. */
  trackers?: TrackersHealth;
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
