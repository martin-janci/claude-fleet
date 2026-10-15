import { describe, it, expect } from 'vitest';
import { buildThreads, filterThreads, groupThreads, siblingsLabel, threadCounts, threadSiblings } from './control_threads';
import type { ControlHandoff } from './handoffs';
import type { Mission } from './missions';
import { session } from './hosts_fixture';

// Thread room (2026-10-14): Control's left column, Main chat and the
// threads Control handed out.

const now = 1_800_000_000;
const since = now - 3600 * 5;
const opts = { idleSecs: 30 * 60, now };

function mission(id: number, over: Partial<Mission> = {}): Mission {
  return { id, name: `M${id}`, goal: '', mode: 'finite', state: 'active', level: 2, plan_version: 1, created_at: 1, updated_at: now - 60, version: 1, ...over };
}

describe('control threads', () => {
  const operator = session('mac', 'fleet-operator', { origin: 'person' });
  const started = session('mac', 'fix-flake', { origin: 'operator', origin_ref: String(operator.id), claude_status: 'working', last_activity_at: now - 60 });
  const asking = session('htz', 'release', { origin: 'person', claude_status: 'blocked', last_activity_at: now - 120 });
  const done = session('htz', 'archive', { origin: 'operator', claude_status: 'idle', last_stop_at: now - 600, last_activity_at: now - 600 });
  const old = session('htz', 'old', { origin: 'operator', claude_status: 'idle', last_stop_at: since - 10, last_activity_at: since - 10 });
  const stranger = session('mac', 'mine', { origin: 'person', claude_status: 'working' });
  const worker = session('mercury', 'step-6', { origin: 'mission', origin_ref: '5', claude_status: 'working' });
  const rows = [operator, started, asking, done, old, stranger, worker];
  const receipts: ControlHandoff[] = [
    { id: 1, at: now - 100, kind: 'session', tool: 'send_prompt', session_id: asking.id },
    { id: 2, at: now - 50, kind: 'mission', tool: 'work_link', mission_id: 5, mission_name: 'Hub federation v2', mission_state: 'active' },
    { id: 3, at: now - 40, kind: 'session', tool: 'send_prompt', session_id: operator.id },
    { id: 4, at: now - 30, kind: 'session', tool: 'send_prompt', session_id: 999 },
  ];

  const threads = buildThreads({ rows, handoffs: receipts, missions: [mission(5, { name: 'Hub federation v2', total: 7, done: 4 })], operatorId: operator.id, opts, since });

  it("lists the sessions Control started and the sessions and missions its receipts name, never itself or someone else's", () => {
    const keys = threads.map((t) => t.key).sort();
    expect(keys).toEqual([`s:${started.id}`, `s:${asking.id}`, `s:${done.id}`, `s:${old.id}`, 'm:5'].sort());
    // A mission's worker belongs to the mission's thread.
    expect(keys).not.toContain(`s:${worker.id}`);
  });

  it('groups them Needs you, Working, Done today and Earlier', () => {
    const g = Object.fromEntries(threads.map((t) => [t.title, t.group]));
    expect(g).toMatchObject({ release: 'needs', 'fix-flake': 'working', archive: 'done', old: 'earlier', 'Hub federation v2': 'working' });
    expect(groupThreads(threads).map((x) => x.id)).toEqual(['needs', 'working', 'done', 'earlier']);
  });

  it('words a mission by its progress, and by what it waits on', () => {
    const m = threads.find((t) => t.kind === 'mission')!;
    expect(m.line).toBe('Mission · 4 of 7 done');
    const waiting = buildThreads({
      rows,
      handoffs: receipts,
      missions: [mission(5, { waiting_on: { reason: 'question' } as Mission['waiting_on'] })],
      operatorId: operator.id,
      opts,
      since,
    }).find((t) => t.kind === 'mission')!;
    expect(waiting.group).toBe('needs');
    expect(waiting.state).toBe('waiting');
    expect(waiting.line).toBe('Asks a question');
  });

  it('a receipt alone names a mission the list does not have', () => {
    const t = buildThreads({ rows: [], handoffs: receipts, missions: [], operatorId: null, opts, since });
    expect(t.map((x) => x.title)).toEqual(['Hub federation v2']);
  });

  it('the chips filter and count: All, Needs you, Missions', () => {
    expect(threadCounts(threads)).toEqual({ all: 5, needs: 1, missions: 1 });
    expect(filterThreads(threads, 'needs').map((t) => t.title)).toEqual(['release']);
    expect(filterThreads(threads, 'missions').map((t) => t.title)).toEqual(['Hub federation v2']);
  });
});

describe('thread siblings (the Session tab strip)', () => {
  it("a mission's workers are each other's siblings", () => {
    const a = session('m', 'a', { origin: 'mission', origin_ref: '9' });
    const b = session('m', 'b', { origin: 'mission', origin_ref: '9' });
    const c = session('m', 'c', { origin: 'mission', origin_ref: '8' });
    expect(threadSiblings(a, [a, b, c]).map((s) => s.tmux_name)).toEqual(['a', 'b']);
    expect(siblingsLabel(a, (id) => (id === 9 ? 'Release' : null))).toBe('In this mission · Release');
  });

  it('sessions on one work item are siblings; a lone session has no strip', () => {
    const work = { link_id: 1, item_id: 4, key: 'PD-1', title: 't', source: 'manual' } as never;
    const a = session('m', 'a', { work });
    const b = session('m', 'b', { work });
    const lone = session('m', 'lone');
    expect(threadSiblings(a, [a, b, lone]).map((s) => s.tmux_name)).toEqual(['a', 'b']);
    expect(siblingsLabel(a, () => null)).toBe('On PD-1');
    expect(threadSiblings(lone, [a, b, lone])).toEqual([]);
  });
});
