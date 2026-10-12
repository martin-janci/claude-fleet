// Redesign step 1.1: a mass loss folds into one row. When a host reboots or
// its tmux server restarts, every session on it is marked lost at once; a
// dozen identical rows scattered over the project tree (and, for the ones
// that were waiting on a prompt, a dozen badge counts nobody can answer until
// the panes come back) bury the sessions that do need a person. The sidebar
// shows them instead as one "12 stopped on trn · Restore" row per host, which
// expands to the rows themselves, so nothing is lost, only moved.
import type { SessionRow } from './sessions';

/** Fewer lost rows than this on one host stay where they are: one or two
 *  stopped panes are ordinary rows, not a mass loss. */
export const MASS_LOSS_MIN = 3;

/** A row `restore_host_sessions` can bring back: marked lost by the backend,
 *  with a Claude conversation to resume, and a fleet-managed tmux pane to
 *  restore into (`bg` and `external` rows have none). HostDetail's "Restore
 *  n lost sessions" counts the same rows. */
export function isRestorable(s: SessionRow): boolean {
  return s.lost_at !== null && !!s.claude_session_id && s.kind !== 'bg' && s.kind !== 'external';
}

export interface LostFold {
  host: string;
  rows: SessionRow[];
}

/** The hosts with a mass loss among `rows`, by host name, each with its
 *  restorable rows in id order. */
export function lostFolds(rows: readonly SessionRow[], min = MASS_LOSS_MIN): LostFold[] {
  const byHost = new Map<string, SessionRow[]>();
  for (const s of rows) {
    if (!isRestorable(s)) continue;
    const list = byHost.get(s.host_alias);
    if (list) list.push(s);
    else byHost.set(s.host_alias, [s]);
  }
  return [...byHost.entries()]
    .filter(([, list]) => list.length >= min)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([host, list]) => ({ host, rows: [...list].sort((a, b) => a.id - b.id) }));
}

/** Every row id a fold holds. */
export function foldedIds(folds: readonly LostFold[]): Set<number> {
  const ids = new Set<number>();
  for (const f of folds) for (const s of f.rows) ids.add(s.id);
  return ids;
}

/** The fold row's label: "12 stopped on trn". */
export function foldLabel(f: LostFold): string {
  return `${f.rows.length} stopped on ${f.host}`;
}
