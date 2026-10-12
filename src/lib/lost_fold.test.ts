import { describe, it, expect } from 'vitest';
import { foldedIds, foldLabel, isRestorable, lostFolds, MASS_LOSS_MIN } from './lost_fold';
import type { SessionRow } from './sessions';

let nextId = 1;
function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++,
    host_alias: 'trn',
    kind: 'work',
    lost_at: 100,
    claude_session_id: 'c-1',
    ...over,
  } as SessionRow;
}

describe('lost_fold (redesign 1.1)', () => {
  it('a lost work row with a conversation is restorable; bg, external and live rows are not', () => {
    expect(isRestorable(row())).toBe(true);
    expect(isRestorable(row({ lost_at: null }))).toBe(false);
    expect(isRestorable(row({ claude_session_id: null }))).toBe(false);
    expect(isRestorable(row({ kind: 'bg' }))).toBe(false);
    expect(isRestorable(row({ kind: 'external' }))).toBe(false);
  });

  it('folds a host only from MASS_LOSS_MIN lost rows up', () => {
    const few = Array.from({ length: MASS_LOSS_MIN - 1 }, () => row({ host_alias: 'mac' }));
    const many = Array.from({ length: 12 }, () => row({ host_alias: 'trn' }));
    const folds = lostFolds([...few, ...many, row({ host_alias: 'trn', lost_at: null })]);
    expect(folds.map((f) => f.host)).toEqual(['trn']);
    expect(folds[0].rows).toHaveLength(12);
    expect(foldLabel(folds[0])).toBe('12 stopped on trn');
    expect(foldedIds(folds)).toEqual(new Set(many.map((s) => s.id)));
  });

  it('orders folds by host and rows by id', () => {
    const b = [row({ host_alias: 'b' }), row({ host_alias: 'b' }), row({ host_alias: 'b' })];
    const a = [row({ host_alias: 'a' }), row({ host_alias: 'a' }), row({ host_alias: 'a' })];
    const folds = lostFolds([...b.reverse(), ...a]);
    expect(folds.map((f) => f.host)).toEqual(['a', 'b']);
    expect(folds[1].rows.map((s) => s.id)).toEqual(b.map((s) => s.id).sort((x, y) => x - y));
  });
});
