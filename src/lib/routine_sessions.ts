// Routine sessions in the Sessions list: a routine starts one session per
// run (`origin: 'routine'`, `origin_ref` its id), so a morning sweep alone
// adds a row a day. They leave the project tree and the work groups for one
// Routines section, a group per routine, newest run first, so the list
// keeps the person's own work and a routine's runs stay one line until
// opened. A run that needs a person still says so on its group's line, and
// still lands in the Inbox.
import { attentionState, countsTowardBadge, type AttentionOptions } from './attention';
import type { SessionRow } from './sessions';

/** A session a routine started (an `external` row is never one). */
export function isRoutineSession(s: Pick<SessionRow, 'origin' | 'kind'>): boolean {
  return s.origin === 'routine' && s.kind !== 'external';
}

export interface RoutineSessionGroup {
  /** `routine:<id>`, or `routine:?` for rows that name none. */
  key: string;
  routineId: number | null;
  /** The routine's name, as its newest session carries it. */
  name: string;
  /** Newest run first. */
  sessions: SessionRow[];
  /** How many of them raise the Needs you badge. */
  needsYou: number;
}

const startedAt = (s: SessionRow) => s.started_at ?? s.created_at ?? 0;

/** The routine sessions of `rows`, a group per routine: the group with the
 *  newest run first, and inside one the newest run first. */
export function routineSessionGroups(rows: readonly SessionRow[], opts: AttentionOptions): RoutineSessionGroup[] {
  const byKey = new Map<string, RoutineSessionGroup>();
  for (const s of rows) {
    if (!isRoutineSession(s)) continue;
    const ref = s.origin_ref?.trim() ?? '';
    const id = /^\d+$/.test(ref) ? Number(ref) : null;
    const key = `routine:${id ?? '?'}`;
    let g = byKey.get(key);
    if (!g) {
      g = { key, routineId: id, name: '', sessions: [], needsYou: 0 };
      byKey.set(key, g);
    }
    g.sessions.push(s);
    if (countsTowardBadge(attentionState(s, opts))) g.needsYou++;
  }
  const groups = [...byKey.values()];
  for (const g of groups) {
    g.sessions.sort((a, b) => startedAt(b) - startedAt(a) || b.id - a.id);
    const named = g.sessions.find((s) => s.friendly_name?.trim())?.friendly_name?.trim();
    g.name = named ?? (g.routineId !== null ? `Routine ${g.routineId}` : 'Routine');
  }
  const newest = (g: RoutineSessionGroup) => startedAt(g.sessions[0]);
  return groups.sort((a, b) => newest(b) - newest(a) || a.key.localeCompare(b.key));
}

/** "3 runs", "1 run". */
export function runsWord(n: number): string {
  return n === 1 ? '1 run' : `${n} runs`;
}
