import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { builtInAgents, jevFeaturesOn, loopEvery, loopLine, money, runsToday, spendMicros, startOfToday } from './automation';
import { SETTING_DEFAULTS } from './fleet_settings';
import type { LoopHealth } from './ipc';
import type { RunRow } from './runs';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const NOW = 1_000_000;

function loop(over: Partial<LoopHealth> = {}): LoopHealth {
  return { name: 'gc', label: 'Garbage collection', pausable: true, runs: 1, failures: 0, ...over };
}

function run(over: Partial<RunRow> = {}): RunRow {
  return { id: 'aux:1', source: 'aux', kind: 'planner', owner: 'p', started_at: NOW - 60, outcome: 'ok', session_ids: [], ...over };
}

beforeEach(() => invoke.mockReset());

describe('automation (redesign step 8.4)', () => {
  it('counts today from local midnight and sums what runs cost', () => {
    const noon = new Date(2026, 9, 9, 12, 30).getTime();
    expect(startOfToday(noon)).toBe(new Date(2026, 9, 9).getTime() / 1000);
    expect(spendMicros([{ cost_micros: 380_000 }, {}, { cost_micros: 30_000 }])).toBe(410_000);
    expect(money(4_100_000)).toBe('$4.10');
  });

  it('says when a loop ran, runs next, failed or is paused', () => {
    expect(loopLine(loop({ last_run_at: NOW - 180, next_run_at: NOW + 300, result: 'ok' }), NOW, false)).toBe(
      'last run 3m ago · next in 5m',
    );
    expect(loopLine(loop({ last_run_at: NOW - 120, result: 'error', last_error: 'ssh: timeout' }), NOW, false)).toBe(
      'failed 2m ago: ssh: timeout',
    );
    expect(loopLine(loop({ last_run_at: NOW - 60, next_run_at: NOW + 60 }), NOW, true)).toBe('paused · last run 1m ago');
    // A loop that only observes keeps its schedule while paused.
    expect(loopLine(loop({ pausable: false, last_run_at: NOW - 60, next_run_at: NOW + 60 }), NOW, true)).toBe(
      'last run 1m ago · next in 1m',
    );
    expect(loopLine(loop(), NOW, false)).toBe('not run yet here');
    expect(loopEvery(loop({ last_run_at: NOW, next_run_at: NOW + 6 * 3600 }))).toBe('every 6h');
    expect(loopEvery(loop())).toBeNull();
  });

  it('names the three built-in agents with what each is doing', () => {
    const settings = { ...SETTING_DEFAULTS, 'decide.jev.enabled': 'true', 'decide.jev.status_map': 'assist', 'decide.jev.quick_answer': 'shadow' };
    expect(jevFeaturesOn(settings)).toBe(2);
    const agents = builtInAgents(
      settings,
      [loop({ name: 'missions', label: 'Missions', last_run_at: NOW - 60, next_run_at: NOW + 60 })],
      [run({ kind: 'operator', started_at: NOW - 600 })],
      NOW,
    );
    expect(agents.map((a) => [a.id, a.state])).toEqual([
      ['operator', 'last ran 10m ago'],
      ['orchestrator', 'last run 1m ago · next in 1m'],
      ['jev', 'on · 2 use cases'],
    ]);
    expect(builtInAgents(SETTING_DEFAULTS, [], [], NOW).map((a) => a.state)).toEqual(['no run today', 'not reported', 'off']);
  });

  it('reads every page of today’s runs', async () => {
    invoke.mockImplementation(async (_c: string, a?: { args: { offset?: number } }) => ({
      runs: !a?.args.offset ? Array.from({ length: 200 }, (_, i) => run({ id: `aux:${i}` })) : [run({ id: 'aux:x' })],
      total: 201,
    }));
    const r = await runsToday(NOW * 1000);
    expect(r.ok && r.value.length).toBe(201);
    expect(invoke.mock.calls.map((c) => [c[0], c[1]?.args?.offset])).toEqual([['list_runs', 0], ['list_runs', 200]]);
  });
});
