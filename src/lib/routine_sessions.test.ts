import { describe, expect, it } from 'vitest';
import { isRoutineSession, routineSessionGroups, runsWord } from './routine_sessions';
import type { SessionRow } from './sessions';

const opts = { idleSecs: 0, now: 10_000 };
let next = 1;
function row(over: Partial<SessionRow>): SessionRow {
  return {
    id: next++,
    kind: 'work',
    status: 'running',
    claude_status: 'idle',
    created_at: 1_000,
    started_at: 1_000,
    friendly_name: null,
    last_activity_at: 1_000,
    ...over,
  } as unknown as SessionRow;
}

describe('routine sessions (one per run, grouped by routine)', () => {
  it('knows a routine session and leaves an external row out', () => {
    expect(isRoutineSession(row({ origin: 'routine', origin_ref: '3' }))).toBe(true);
    expect(isRoutineSession(row({ origin: 'person' }))).toBe(false);
    expect(isRoutineSession(row({ origin: 'routine', kind: 'external' } as Partial<SessionRow>))).toBe(false);
  });

  it('groups by routine, the newest run first, named by its sessions', () => {
    const a1 = row({ origin: 'routine', origin_ref: '3', friendly_name: 'Morning PR sweep', started_at: 2_000 });
    const a2 = row({ origin: 'routine', origin_ref: '3', friendly_name: 'Morning PR sweep', started_at: 5_000 });
    const b1 = row({ origin: 'routine', origin_ref: '7', started_at: 4_000, claude_status: 'blocked' });
    const mine = row({ origin: 'person', started_at: 9_000 });
    const groups = routineSessionGroups([a1, b1, mine, a2], opts);
    expect(groups.map((g) => [g.key, g.name, g.sessions.map((s) => s.id)])).toEqual([
      ['routine:3', 'Morning PR sweep', [a2.id, a1.id]],
      ['routine:7', 'Routine 7', [b1.id]],
    ]);
    expect(groups[0].routineId).toBe(3);
    expect(groups[1].needsYou).toBe(1);
    expect(groups[0].needsYou).toBe(0);
  });

  it('puts rows that name no routine in one group of their own', () => {
    const [g] = routineSessionGroups([row({ origin: 'routine', origin_ref: null })], opts);
    expect([g.key, g.routineId, g.name]).toEqual(['routine:?', null, 'Routine']);
    expect(runsWord(1)).toBe('1 run');
    expect(runsWord(3)).toBe('3 runs');
  });
});
