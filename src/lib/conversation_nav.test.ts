import { describe, it, expect, beforeEach } from 'vitest';
import { findMatches, turnIndex, rowKey, scrollMemory, rememberScroll, recallScroll } from './conversation_nav';
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
  beforeEach(() => scrollMemory.clear());

  it('remembers and recalls a scroll snapshot per session', () => {
    rememberScroll(1, { rowKey: 't7', atBottom: false });
    expect(recallScroll(1)).toEqual({ rowKey: 't7', atBottom: false });
    expect(recallScroll(2)).toBeNull();
  });

  it('drops the entry when atBottom is true — recall then means "go to bottom"', () => {
    rememberScroll(3, { rowKey: 't1', atBottom: false });
    rememberScroll(3, { rowKey: 't9', atBottom: true });
    expect(recallScroll(3)).toBeNull();
  });
});
