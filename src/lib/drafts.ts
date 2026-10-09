// LLM drafts in Control (redesign step 9.11): a completed mission's release
// note and Today's morning brief. Both are on demand: opening Today reads the
// brief drafted last and runs nothing; only Refresh (or Draft, the first
// time) asks the hub to write a new one. Each run is booked on the hub with
// its origin (`release_note`, `brief`).
import { invokeCmd, type Result } from './result';
import { localMidnight } from './today';

/** A draft and where it came from, as `DraftField` shows it. */
export interface Draft {
  text: string;
  model: string;
  host_alias: string;
  /** What it was drafted from: "12 tasks and 2 PRs". */
  from: string;
  /** Unix seconds. */
  at: number;
  truncated?: boolean;
}

export interface Brief {
  draft?: Draft | null;
  org_id?: number | null;
}

export function draftReleaseNote(missionId: number): Promise<Result<Draft>> {
  return invokeCmd<Draft>('mission_release_note', { args: { mission_id: missionId } });
}

/** The brief drafted last (`refresh: false`, runs nothing) or a new one. */
export function todayBrief(refresh: boolean, since: number = localMidnight()): Promise<Result<Brief>> {
  return invokeCmd<Brief>('today_brief', { args: { refresh, since } });
}

/**
 * A hub older than 9.11 answers one of these for the call: no drafts there,
 * not a failure. Matched by code, never by message text (as today.ts).
 */
export const HUB_HAS_NO_DRAFTS = ['E_INVALID', 'E_FORBIDDEN', 'E_HUB_PROTOCOL'];

/** "Drafted 08:02": the local time a draft was written. */
export function draftedAt(at: number): string {
  const d = new Date(at * 1000);
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  return `Drafted ${hh}:${mm}`;
}
