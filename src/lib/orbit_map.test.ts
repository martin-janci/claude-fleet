import { describe, it, expect } from 'vitest';
import { orbitModel, LAP_BUSY, LAP_QUIET, RING_SECS } from './orbit_map';
import type { RunRow } from './runs';

const NOW = 1_800_000_000;
const run = (extra: Partial<RunRow>): RunRow => ({
  id: 'routine:1',
  source: 'routine',
  kind: 'routine',
  owner: 'Nightly',
  started_at: NOW - 60,
  outcome: 'running',
  session_ids: [1],
  host: 'htz',
  ...extra,
});

describe('orbitModel', () => {
  it('puts a comet per live run on its host, dim and slow once its session went quiet', () => {
    const m = orbitModel({
      runs: [run({ id: 'routine:1', session_ids: [1] }), run({ id: 'agent:2', source: 'agent', owner: 'PR reviewer', session_ids: [2] })],
      hostAliases: ['htz', 'nas'],
      sessions: [
        { id: 1, host_alias: 'htz', claude_status: 'working' },
        { id: 2, host_alias: 'htz', claude_status: 'idle' },
        { id: 3, host_alias: 'nas', claude_status: null },
      ],
      routines: [],
      now: NOW,
    });
    expect(m.hosts.map((h) => h.alias)).toEqual(['htz', 'nas']);
    const htz = m.hosts[0];
    expect(htz.comets.map((c) => [c.run_id, c.lap, c.quiet])).toEqual([
      ['routine:1', LAP_BUSY, false],
      ['agent:2', LAP_QUIET, true],
    ]);
    expect(htz.caption).toBe('Nightly · PR reviewer · going quiet');
    expect(htz.sessions).toBe(2);
    expect(htz.scale).toBeGreaterThan(m.hosts[1].scale);
    expect(m.running).toBe(2);
  });

  it('haloes a host with a run waiting for you and sparks one that failed lately', () => {
    const m = orbitModel({
      runs: [
        run({ id: 'routine:1', outcome: 'needs_person', ended_at: NOW - 100 }),
        run({ id: 'routine:2', outcome: 'failed', ended_at: NOW - 60, host: 'nas', error: 'time cap' }),
        run({ id: 'routine:3', outcome: 'failed', ended_at: NOW - 3600, host: 'mercury' }),
      ],
      hostAliases: ['htz', 'nas', 'mercury'],
      sessions: [],
      routines: [],
      now: NOW,
    });
    const by = Object.fromEntries(m.hosts.map((h) => [h.alias, h]));
    expect(by.htz.needs).toBe(true);
    expect(by.nas.failed).toBe(true);
    expect(by.mercury.failed).toBe(false);
    expect(m.feed[0]).toMatchObject({ who: 'Nightly', what: 'failed: time cap', tone: 'failed' });
  });

  it("ticks the next fires on the outer ring, and names a quiet host's next one", () => {
    const routine = (name: string, at: number, extra = {}) => ({ name, enabled: true, trigger: 'cron', host_alias: 'nas', next_run_at: at, ...extra });
    const m = orbitModel({
      runs: [],
      hostAliases: ['nas'],
      sessions: [],
      routines: [
        routine('Usage report', NOW + RING_SECS / 4),
        routine('Far off', NOW + RING_SECS * 2),
        routine('Skipped', NOW + 60, { skip_next: true }),
        routine('Off', NOW + 60, { enabled: false }),
      ],
      now: NOW,
    });
    expect(m.fires.map((f) => [f.routine, f.deg])).toEqual([['Usage report', 90]]);
    expect(m.hosts[0].caption).toMatch(/^quiet · next fire \d\d:\d\d$/);
  });
});
