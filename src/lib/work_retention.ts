/**
 * Work graph retention (M12.3): `work_admin { status | sweep_now }` on a
 * standalone desktop. Types mirror `service/work/retention.rs`. Both
 * commands are local-only: on a desktop paired with a hub the Limits
 * section, and this with it, is replaced by the remote note.
 */

import { invokeCmd, type Result } from './result';

export interface RetentionTableStatus {
  table: string;
  setting: string;
  /** 0 = keep forever. */
  days: number;
  rows: number;
  /** Dry run: what sweeps would delete now. */
  would_delete: number;
}

export interface RetentionSweep {
  at: number;
  journal: number;
  tracker_items: number;
  timeline_work_events: number;
}

export interface RetentionStatus {
  tables: RetentionTableStatus[];
  last_sweep?: RetentionSweep | null;
  tick_cap: number;
}

export function retentionStatus(): Promise<Result<RetentionStatus>> {
  return invokeCmd<RetentionStatus>('work_retention_status');
}

export function retentionSweepNow(): Promise<Result<RetentionSweep>> {
  return invokeCmd<RetentionSweep>('work_retention_sweep');
}

/** The UI's name for a swept table. */
export const RETENTION_TABLE_LABELS: Readonly<Record<string, string>> = {
  work_journal: 'journal',
  work_items: 'done tickets',
  session_events: 'work timeline',
};

export function retentionTableLabel(table: string): string {
  return RETENTION_TABLE_LABELS[table] ?? table;
}

/** `12 of 300 (keep forever)` style line for one table. */
export function retentionLine(t: RetentionTableStatus): string {
  const label = retentionTableLabel(t.table);
  if (t.days === 0) return `${label}: ${t.rows} rows, kept forever`;
  return `${label}: ${t.rows} rows, ${t.would_delete} older than ${t.days} d with nothing live on them`;
}

/** The last sweep, one line; `now` in unix seconds. */
export function lastSweepLine(s: RetentionSweep | null | undefined, now: number): string {
  if (!s) return 'no sweep yet';
  const mins = Math.max(0, Math.round((now - s.at) / 60));
  const ago = mins < 60 ? `${mins} min ago` : mins < 2880 ? `${Math.round(mins / 60)} h ago` : `${Math.round(mins / 1440)} d ago`;
  return `last sweep ${ago}: ${s.journal} journal, ${s.tracker_items} tickets, ${s.timeline_work_events} events deleted`;
}
