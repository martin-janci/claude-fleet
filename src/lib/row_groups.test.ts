import { describe, it, expect } from 'vitest';
import { groupRows, isFlatGroupBy, STATE_LABELS } from './row_groups';
import { ATTENTION_STATES } from './attention';
import { session } from './hosts_fixture';
import type { SessionRow } from './sessions';

const NOW = 1_000_000;
const opts = { idleSecs: 3600, now: NOW };

const working = session('mac', 'a', { claude_status: 'working', last_activity_at: NOW - 60 });
const failed = session('nas', 'b', { claude_status: 'failed', kind: 'bg', last_activity_at: NOW - 60 });
const stuck = session('mac', 'c', { stuck_kind: 'auth_menu', last_activity_at: NOW - 60 });
const shell = session('alpha', 'd', { kind: 'shell', agent: 'shell', last_activity_at: NOW - 60 });
const codex = session('nas', 'e', { agent: 'codex', claude_status: 'working', last_activity_at: NOW - 60 });
const rows = [working, failed, stuck, shell, codex];

const ids = (g: { rows: SessionRow[] }) => g.rows.map((s) => s.tmux_name);

describe('groupRows (redesign step 3.6)', () => {
  it('groups by state in urgency order, with the manual status words', () => {
    const groups = groupRows(rows, 'state', opts);
    expect(groups.map((g) => g.label)).toEqual(['Needs you', 'Failed', 'Working', 'Idle']);
    expect(groups.map(ids)).toEqual([['c'], ['b'], ['a', 'e'], ['d']]);
    expect(groups[0].key).toBe('state:action_required');
  });

  it('puts a Blocked row under Needs you: six words, one header each (plan: status words)', () => {
    const down = session('down', 'g', { claude_status: 'idle', last_activity_at: NOW - 60 });
    const groups = groupRows([working, down, stuck], 'state', { ...opts, facts: { down_hosts: ['down'] } });
    expect(groups.map((g) => g.label)).toEqual(['Needs you', 'Working']);
    expect(groups[0].rows.map((s) => s.tmux_name).sort()).toEqual(['c', 'g']);
    expect(STATE_LABELS.blocked).toBe('Needs you');
  });

  it('has a word for every attention state', () => {
    for (const st of ATTENTION_STATES) expect(STATE_LABELS[st]).toBeTruthy();
  });

  it('groups by host, sorted by name, keeping each row in its arrival order', () => {
    const groups = groupRows(rows, 'host', opts);
    expect(groups.map((g) => g.label)).toEqual(['alpha', 'mac', 'nas']);
    expect(groups.map(ids)).toEqual([['d'], ['a', 'c'], ['b', 'e']]);
  });

  it("groups by agent in the manual's order; a missing agent reads as Claude Code", () => {
    const groups = groupRows(rows, 'agent', opts);
    expect(groups.map((g) => g.label)).toEqual(['Claude Code', 'Codex', 'Shell']);
    expect(groups.map(ids)).toEqual([['a', 'b', 'c'], ['e'], ['d']]);
  });

  it('never drops a row: an agent this build does not know gets its own group, last', () => {
    const future = session('mac', 'f', { agent: 'gemini' as SessionRow['agent'] });
    const groups = groupRows([future, working], 'agent', opts);
    expect(groups.map((g) => g.label)).toEqual(['Claude Code', 'gemini']);
    for (const by of ['state', 'host', 'agent'] as const) {
      const all = groupRows(rows, by, opts).flatMap((g) => g.rows);
      expect(all.length, by).toBe(rows.length);
    }
  });

  it('knows its modes', () => {
    expect(['state', 'host', 'agent'].every(isFlatGroupBy)).toBe(true);
    expect(isFlatGroupBy('project')).toBe(false);
    expect(isFlatGroupBy('work')).toBe(false);
  });
});

describe('a session ⌘N just started (step 5.14)', () => {
  it('lands under Working until its agent reports, then goes where its state says', () => {
    const fresh = session('mac', 'new', { id: 77, claude_status: null, last_activity_at: NOW - 5 });
    const starting = new Set([77]);
    let groups = groupRows([working, fresh], 'state', opts, starting);
    expect(groups.map((g) => g.label)).toEqual(['Working']);
    expect(ids(groups[0])).toEqual(['a', 'new']);
    groups = groupRows([working, fresh], 'state', opts, new Set());
    expect(groups.map((g) => g.label)).toEqual(['Working', 'Idle']);
  });
});
