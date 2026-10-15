import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  buildLanes,
  hasTrace,
  listRuns,
  outcomeLabel,
  phaseRail,
  readTraceOn,
  runMark,
  runSpan,
  stepBar,
  tookWords,
  type RunRow,
  type RunTraceStep,
} from './runs';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

describe('listRuns', () => {
  beforeEach(() => invoke.mockReset());

  it('sends only the filters that are set, under args', async () => {
    invoke.mockResolvedValue({ runs: [], total: 0 });
    await listRuns({ since: 10, kind: 'jev', session_id: undefined, limit: 20 });
    expect(invoke).toHaveBeenCalledWith('list_runs', {
      args: { since: 10, kind: 'jev', limit: 20 },
    });
  });

  it('gives a null-stripped row its empty session list back', async () => {
    invoke.mockResolvedValue({
      runs: [{ id: 'jev:1', source: 'jev', kind: 'jev', owner: 'x', started_at: 1, outcome: 'ok' }],
      total: 1,
    });
    const r = await listRuns();
    expect(r.ok && r.value.runs[0].session_ids).toEqual([]);
    expect(r.ok && r.value.total).toBe(1);
  });

  it('passes a refusal through', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_INVALID', message: 'bad kind' });
    const r = await listRuns({ outcome: 'ok' });
    expect(r.ok).toBe(false);
  });
});

describe('outcomeLabel', () => {
  it('says each outcome in plain words', () => {
    expect(outcomeLabel({ outcome: 'ok' })).toBe('Done');
    expect(outcomeLabel({ outcome: 'needs_person' })).toBe('Needs you');
    expect(outcomeLabel({ outcome: 'nothing_to_do' })).toBe('Nothing to do');
    expect(outcomeLabel({ outcome: 'running' })).toBe('Working');
  });

  it('carries a failure its error', () => {
    expect(outcomeLabel({ outcome: 'failed', error: 'timeout' })).toBe('Failed: timeout');
    expect(outcomeLabel({ outcome: 'failed' })).toBe('Failed');
  });

  it('shows an outcome it does not know as sent', () => {
    expect(outcomeLabel({ outcome: 'later' })).toBe('later');
  });
});

const step = (seq: number, phase: string, kind = phase, extra: Partial<RunTraceStep> = {}): RunTraceStep => ({
  seq,
  attempt: 1,
  at: 100 + seq,
  phase,
  kind,
  actor: 'scheduler',
  summary: `${phase} ${seq}`,
  ...extra,
});
const row = (extra: Partial<RunRow>): RunRow => ({
  id: 'routine:1',
  source: 'routine',
  kind: 'routine',
  owner: 'Nightly',
  started_at: 100,
  outcome: 'ok',
  session_ids: [1],
  ...extra,
});

describe('phaseRail', () => {
  it('marks reached phases done and the working phase live while the run runs', () => {
    const rail = phaseRail([step(1, 'trigger'), step(2, 'checks', 'check'), step(3, 'spawn'), step(4, 'prompt'), step(5, 'working', 'tool_call')], 'running');
    expect(rail.map((c) => c.state)).toEqual(['done', 'done', 'todo', 'done', 'done', 'live', 'todo', 'todo']);
    expect(rail[5].meta).toBe('1 step so far');
  });

  it('puts the live edge after the newest phase before the prompt is in', () => {
    const rail = phaseRail([step(1, 'trigger'), step(2, 'checks', 'check')], 'running');
    expect(rail.map((c) => c.state).slice(0, 4)).toEqual(['done', 'done', 'live', 'todo']);
  });

  it('a failed start and a skipped fire say so', () => {
    const failed = phaseRail([step(1, 'trigger'), step(2, 'spawn', 'error'), step(3, 'settle', 'error')], 'failed');
    expect(failed.find((c) => c.phase === 'spawn')?.state).toBe('failed');
    const skipped = phaseRail([step(1, 'trigger'), step(2, 'checks', 'check', { data: { skipped: 'overlap' } }), step(3, 'settle')], 'nothing_to_do');
    expect(skipped.find((c) => c.phase === 'checks')?.state).toBe('skipped');
    expect(skipped.some((c) => c.state === 'live')).toBe(false);
  });

  it("sums a phase's step times", () => {
    const rail = phaseRail([step(1, 'spawn', 'spawn', { duration_ms: 3000 }), step(2, 'spawn', 'spawn', { duration_ms: 1200 })], 'ok');
    expect(rail.find((c) => c.phase === 'spawn')?.ms).toBe(4200);
    expect(tookWords(4200)).toBe('4 s');
    expect(tookWords(168_000)).toBe('2m 48s');
    expect(tookWords(410)).toBe('410 ms');
  });
});

describe('readTraceOn', () => {
  beforeEach(() => invoke.mockReset());

  it('reads on from the last step held while more follow', async () => {
    invoke
      .mockResolvedValueOnce({ run_id: 'routine:1', steps: [step(3, 'prompt')], more: true })
      .mockResolvedValueOnce({ run_id: 'routine:1', steps: [step(4, 'working', 'tool_call')], more: false });
    const r = await readTraceOn('routine:1', [step(1, 'trigger'), step(2, 'checks')]);
    expect(r.ok && r.value.map((s) => s.seq)).toEqual([1, 2, 3, 4]);
    expect(invoke).toHaveBeenNthCalledWith(1, 'run_trace', { args: { run_id: 'routine:1', after_seq: 2 } });
    expect(invoke).toHaveBeenNthCalledWith(2, 'run_trace', { args: { run_id: 'routine:1', after_seq: 3 } });
  });

  it('only routine fires and agent jobs keep a trace', () => {
    expect(hasTrace({ source: 'routine' })).toBe(true);
    expect(hasTrace({ source: 'agent' })).toBe(true);
    expect(hasTrace({ source: 'jev' })).toBe(false);
  });
});

describe('lanes', () => {
  it('groups runs by owner, live lanes first, and drops runs outside the window', () => {
    const groups = buildLanes(
      [
        row({ id: 'routine:1', owner: 'Zeta', started_at: 200, ended_at: 260 }),
        row({ id: 'routine:2', owner: 'Alpha', started_at: 300, ended_at: 320 }),
        row({ id: 'routine:3', owner: 'Zeta', started_at: 400, outcome: 'running' }),
        row({ id: 'agent:1', source: 'agent', kind: 'review', owner: 'gh:o/r#9', started_at: 300, ended_at: 310 }),
        row({ id: 'jev:1', source: 'jev', kind: 'jev', owner: 'status_map', started_at: 50 }),
      ],
      100,
      500,
    );
    expect(groups.map((g) => [g.group, g.lanes.map((l) => l.name)])).toEqual([
      ['Routines', ['Zeta', 'Alpha']],
      ['Agents', ['review']],
    ]);
    expect(groups[0].lanes[0].runs).toHaveLength(2);
  });

  it('draws a skipped fire and a Jev decision as marks', () => {
    expect(runMark(row({ outcome: 'nothing_to_do', session_ids: [] }))).toBe('skip');
    expect(runMark(row({ outcome: 'nothing_to_do' }))).toBe('bar');
    expect(runMark(row({ source: 'jev' }))).toBe('jev');
  });

  it('a live run reaches now; spans and step bars stay inside the clock', () => {
    expect(runSpan(row({ started_at: 200, outcome: 'running' }), 100, 500, 300)).toEqual({ left: 25, width: 25 });
    expect(runSpan(row({ started_at: 0, ended_at: 1000 }), 100, 500, 300)).toEqual({ left: 0, width: 100 });
    expect(stepBar(step(1, 'working', 'tool_call', { at: 300, duration_ms: 100_000 }), 100, 500)).toEqual({ left: 25, width: 25 });
  });
});
