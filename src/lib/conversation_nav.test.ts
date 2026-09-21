import { describe, it, expect, beforeEach } from 'vitest';
import {
  findMatches,
  turnIndex,
  rowKey,
  scrollMemory,
  rememberScroll,
  recallScroll,
  forgetScroll,
  anchorAt,
  resolveScroll,
  nearestTurn,
  turnKeyNear,
  adjacentTurn,
} from './conversation_nav';
import { sessions, sessionsLoaded, type SessionRow } from './sessions';
import type { ConvItem, ConvTurn, ThreadRow, InlineEvent } from './conversation';

const tool = (summary: string, target: string | null): ConvItem => ({
  kind: 'tool',
  summary,
  error: false,
  id: null,
  name: '',
  target,
  at: null,
  ended_at: null,
  done: true,
});
const turn = (prompt: string | null, items: ConvItem[], at: string | null = null): ConvTurn => ({
  prompt,
  at,
  ended_at: null,
  items,
});
const ev = (id: number, label: string, detail: string | null = null): InlineEvent => ({
  id,
  at: 1,
  label,
  detail,
  tone: 'info',
});

describe('conversation_nav', () => {
  const rows: ThreadRow[] = [
    { kind: 'turn', turn: turn('Fix the Parser bug', []), index: 0 },
    { kind: 'turn', turn: turn('unrelated', [{ kind: 'text', text: 'the parser is fine' }]), index: 1 },
    { kind: 'event', event: ev(7, 'Turn failed: overloaded') },
    { kind: 'turn', turn: turn('nothing here', [tool('Read(/src/PARSER.rs)', '/src/PARSER.rs')]), index: 2 },
    { kind: 'event', event: ev(9, 'Parser crashed', null) },
    { kind: 'turn', turn: turn('no hit', [{ kind: 'text', text: 'nope' }]), index: 3 },
  ];

  it('findMatches finds case-insensitively across prompt, text, tool target and event label, in order', () => {
    expect(findMatches(rows, '  parser ').map((m) => m.rowKey)).toEqual(['t0', 't1', 't2', 'e9']);
    expect(rowKey(rows[2])).toBe('e7');
    expect(findMatches(rows, 'OVERLOADED').map((m) => m.rowKey)).toEqual(['e7']);
  });

  it('findMatches searches commands, subagents and compaction summaries', () => {
    const more: ThreadRow[] = [
      { kind: 'turn', turn: turn(null, [{ kind: 'command', name: '/model', args: 'opus', output: 'Set model' }]), index: 0 },
      {
        kind: 'turn',
        turn: turn('q', [
          {
            kind: 'subagent',
            id: null,
            name: 'Task',
            agent_type: 'Explore',
            description: 'Map the store',
            result: 'found zebra',
            error: false,
            at: null,
            ended_at: null,
            done: true,
          },
        ]),
        index: 1,
      },
      { kind: 'turn', turn: turn('q', [{ kind: 'compact', trigger: 'auto', pre_tokens: 1, summary: 'about zebra and opus' }]), index: 2 },
    ];
    expect(findMatches(more, 'opus').map((m) => m.rowKey)).toEqual(['t0', 't2']);
    expect(findMatches(more, 'zebra').map((m) => m.rowKey)).toEqual(['t1', 't2']);
    expect(findMatches(more, 'map the').map((m) => m.rowKey)).toEqual(['t1']);
  });

  it('an empty or whitespace query matches nothing', () => {
    expect(findMatches(rows, '')).toEqual([]);
    expect(findMatches(rows, '   ')).toEqual([]);
  });

  it('turnIndex lists prompt first lines (80 chars max), commands, and skips prompt-less turns', () => {
    const long = 'x'.repeat(100);
    const idx = turnIndex([
      { kind: 'turn', turn: turn('first line\nsecond line', [], '2026-09-18T09:00:00Z'), index: 0 },
      { kind: 'event', event: ev(1, 'Resumed conversation') },
      { kind: 'turn', turn: turn(long, []), index: 1 },
      { kind: 'turn', turn: turn(null, [{ kind: 'text', text: 'assistant only' }]), index: 2 },
      { kind: 'turn', turn: turn(null, [{ kind: 'command', name: '/model', args: 'opus', output: null }]), index: 3 },
      { kind: 'turn', turn: turn(null, [{ kind: 'command', name: '/cost', args: null, output: 'x' }]), index: 4 },
    ]);
    expect(idx).toEqual([
      { rowKey: 't0', label: 'first line', at: '2026-09-18T09:00:00Z' },
      { rowKey: 't1', label: `${'x'.repeat(79)}…`, at: null },
      { rowKey: 't3', label: '/model opus', at: null },
      { rowKey: 't4', label: '/cost', at: null },
    ]);
    expect(idx[1].label).toHaveLength(80);
  });
});

describe('scroll memory', () => {
  beforeEach(() => {
    scrollMemory.clear();
    sessionsLoaded.set(false);
    sessions.set([]);
  });

  const AT7 = '2026-09-18T09:07:00Z';

  it('remembers and recalls a scroll snapshot per session', () => {
    rememberScroll(1, { turnAt: AT7, rowKey: 't7', atBottom: false });
    expect(recallScroll(1)).toEqual({ turnAt: AT7, rowKey: 't7', atBottom: false });
    expect(recallScroll(2)).toBeNull();
  });

  it('drops the entry when atBottom is true — recall then means "go to bottom"', () => {
    rememberScroll(3, { turnAt: AT7, rowKey: 't1', atBottom: false });
    rememberScroll(3, { turnAt: null, rowKey: 't9', atBottom: true });
    expect(recallScroll(3)).toBeNull();
  });

  it('forgetScroll drops one session\'s entry and leaves the others', () => {
    rememberScroll(3, { turnAt: AT7, rowKey: 't1', atBottom: false });
    rememberScroll(4, { turnAt: AT7, rowKey: 't1', atBottom: false });
    forgetScroll(3);
    expect(recallScroll(3)).toBeNull();
    expect(recallScroll(4)).not.toBeNull();
  });

  it('drops the entry of a session that leaves the sessions store', () => {
    const row = (id: number) => ({ id, tmux_name: `s${id}` }) as unknown as SessionRow;
    sessionsLoaded.set(true);
    sessions.set([row(3), row(4)]);
    rememberScroll(3, { turnAt: AT7, rowKey: 't1', atBottom: false });
    rememberScroll(4, { turnAt: AT7, rowKey: 't2', atBottom: false });

    // Session 3 is killed: the store loses the row.
    sessions.set([row(4)]);
    expect(recallScroll(3)).toBeNull();
    expect(recallScroll(4)).not.toBeNull();
  });

  it('an empty store before the first list has landed is "not loaded", not "all gone"', () => {
    rememberScroll(3, { turnAt: AT7, rowKey: 't1', atBottom: false });
    sessions.set([]); // sessionsLoaded is still false
    expect(recallScroll(3)).not.toBeNull();
  });
});

describe('scroll memory anchors on the turn, not the window position', () => {
  // The SAME two turns, first as the tail of a 4-turn window (t2/t3) and
  // then after two more turns arrived and the window slid (t0/t1).
  const A = '2026-09-18T09:00:00Z';
  const B = '2026-09-18T09:05:00Z';
  const before = turnIndex([
    { kind: 'turn', turn: turn('older one', [], '2026-09-18T08:50:00Z'), index: 0 },
    { kind: 'turn', turn: turn('older two', [], '2026-09-18T08:55:00Z'), index: 1 },
    { kind: 'turn', turn: turn('the one being read', [], A), index: 2 },
    { kind: 'turn', turn: turn('and the next', [], B), index: 3 },
  ]);
  const after = turnIndex([
    { kind: 'turn', turn: turn('the one being read', [], A), index: 0 },
    { kind: 'turn', turn: turn('and the next', [], B), index: 1 },
    { kind: 'turn', turn: turn('newer one', [], '2026-09-18T09:10:00Z'), index: 2 },
    { kind: 'turn', turn: turn('newer two', [], '2026-09-18T09:15:00Z'), index: 3 },
  ]);

  it('anchorAt reads the timestamp of the turn at-or-before the key', () => {
    expect(anchorAt(before, 't2')).toBe(A);
    expect(anchorAt(before, 't3')).toBe(B);
    expect(anchorAt(before, null)).toBeNull();
    expect(anchorAt([], 't2')).toBeNull();
  });

  it('a remembered turnAt resolves to the row key that turn wears NOW', () => {
    const snap = { turnAt: anchorAt(before, 't2'), rowKey: 't2', atBottom: false };
    expect(snap.turnAt).toBe(A);
    // The window slid by two turns: the same content is `t0` now. The raw
    // key would have restored the reader two turns further down.
    expect(resolveScroll(after, snap)).toBe('t0');
    expect(resolveScroll(before, snap)).toBe('t2');
  });

  it('an unknown turnAt (or none) resolves to null, so the view stays pinned', () => {
    expect(resolveScroll(after, { turnAt: '2026-01-01T00:00:00Z', rowKey: 't2', atBottom: false })).toBeNull();
    expect(resolveScroll(after, { turnAt: null, rowKey: 't2', atBottom: false })).toBeNull();
  });

  it('an inline event is restored by its own key — `e<id>` is the backend id, not a window position', () => {
    expect(resolveScroll(after, { turnAt: A, rowKey: 'e42', atBottom: false })).toBe('e42');
    // Even with no anchor at all: the event id alone is enough.
    expect(resolveScroll(after, { turnAt: null, rowKey: 'e42', atBottom: false })).toBe('e42');
  });
});

describe('turn stepper', () => {
  // t0, t1, (t2 has no prompt/command — skipped by turnIndex), t3, t4.
  const idx = turnIndex([
    { kind: 'turn', turn: turn('first line\nsecond line', [], '2026-09-18T09:00:00Z'), index: 0 },
    { kind: 'event', event: ev(1, 'Resumed conversation') },
    { kind: 'turn', turn: turn('second turn', []), index: 1 },
    { kind: 'turn', turn: turn(null, [{ kind: 'text', text: 'assistant only' }]), index: 2 },
    { kind: 'turn', turn: turn(null, [{ kind: 'command', name: '/model', args: 'opus', output: null }]), index: 3 },
    { kind: 'turn', turn: turn(null, [{ kind: 'command', name: '/cost', args: null, output: 'x' }]), index: 4 },
  ]);

  it('nearestTurn finds the turn at or before the top visible key', () => {
    expect(idx.map((e) => e.rowKey)).toEqual(['t0', 't1', 't3', 't4']);
    expect(nearestTurn(idx, 't0')).toBe(0);
    expect(nearestTurn(idx, 't4')).toBe(3);
    // t2 is not itself a turn-index entry (no label) — nearest at-or-before is t1.
    expect(nearestTurn(idx, 't2')).toBe(1);
  });

  it('nearestTurn falls back to the first turn (0) when the key is unknown', () => {
    expect(nearestTurn(idx, null)).toBe(0);
    expect(nearestTurn(idx, 'e1')).toBe(0);
    expect(nearestTurn(idx, 'nonsense')).toBe(0);
    expect(nearestTurn([], 't0')).toBe(0);
  });

  it('turnKeyNear resolves an inline event FORWARD to the turn below it', () => {
    const keys = ['t0', 'e1', 't1', 't2', 'e9'];
    // A turn row is already its own answer, whichever direction is asked.
    expect(turnKeyNear(keys, 't1', 1)).toBe('t1');
    expect(turnKeyNear(keys, 't1', -1)).toBe('t1');
    // The event between t0 and t1: forward is t1 (so `]` steps to t2, not to
    // the top of the conversation), backward is t0.
    expect(turnKeyNear(keys, 'e1', 1)).toBe('t1');
    expect(turnKeyNear(keys, 'e1', -1)).toBe('t0');
    // A trailing event has no turn below it: fall back to the one above.
    expect(turnKeyNear(keys, 'e9', 1)).toBe('t2');
    // Nothing to resolve against.
    expect(turnKeyNear(keys, null, 1)).toBeNull();
    expect(turnKeyNear(keys, 'e404', 1)).toBeNull();
    expect(turnKeyNear(['e1', 'e2'], 'e1', 1)).toBeNull();
  });

  it('adjacentTurn steps to the neighboring turn and clamps to null at either end', () => {
    expect(adjacentTurn(idx, 0, 1)).toEqual(idx[1]);
    expect(adjacentTurn(idx, 3, -1)).toEqual(idx[2]);
    expect(adjacentTurn(idx, 0, -1)).toBeNull();
    expect(adjacentTurn(idx, 3, 1)).toBeNull();
  });
});
