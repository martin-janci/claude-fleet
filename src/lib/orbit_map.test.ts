import { describe, it, expect } from 'vitest';
import { orbitModel, orbitStageLabel, hostLabels, LAP_BUSY, LAP_QUIET, RECENT_SECS, RING_SECS } from './orbit_map';
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

  it('review w19-12: names the waiting and failed hosts in the stage label, not only by colour', () => {
    const m = orbitModel({
      runs: [
        run({ id: 'routine:1', outcome: 'needs_person', ended_at: NOW - 100 }),
        run({ id: 'routine:2', outcome: 'failed', ended_at: NOW - 60, host: 'nas' }),
      ],
      hostAliases: ['htz', 'nas'],
      sessions: [],
      routines: [],
      now: NOW,
    });
    expect(orbitStageLabel(m)).toBe('0 running and 0 working across 2 hosts · a run waits for you on htz · a run failed lately on nas');
    expect(orbitStageLabel({ ...m, hosts: m.hosts.map((h) => ({ ...h, needs: false, failed: false })) })).toBe(
      '0 running and 0 working across 2 hosts',
    );
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
    // The right half of the ring is the next 12 hours: a quarter of them is 45°.
    expect(m.fires.map((f) => [f.routine, f.deg])).toEqual([['Usage report', 45]]);
    expect(m.hosts[0].caption).toMatch(/^quiet · next in 3h$/);
  });

  it('draws what ran lately: moons on the host, dots down the left of the ring, a ripple and a hub ripple', () => {
    const m = orbitModel({
      runs: [
        run({ id: 'routine:1', outcome: 'done', started_at: NOW - 400, ended_at: NOW - 300 }),
        run({ id: 'routine:2', outcome: 'failed', started_at: NOW - 7200, ended_at: NOW - 7000 }),
        run({ id: 'hook:3', owner: 'turn_outcome', summary: 'turn_outcome (assist): working', outcome: 'done', host: undefined, ended_at: NOW - 30 }),
      ],
      hostAliases: ['htz'],
      sessions: [],
      routines: [],
      now: NOW,
    });
    const htz = m.hosts[0];
    expect(htz.moons.map((x) => [x.run_id, x.tone])).toEqual([['routine:1', 'done']]);
    expect(htz.heat).toBeCloseTo(1 - 300 / RECENT_SECS, 2);
    expect(htz.caption).toBe('quiet · last ran 5m ago');
    expect(m.past.map((p) => p.run_id)).toEqual(['hook:3', 'routine:1', 'routine:2']);
    expect(m.past.every((p) => p.deg <= 0 && p.deg >= -180)).toBe(true);
    expect(m.hubHeat).toBeGreaterThan(0.9);
    // A summary that repeats its owner says it once.
    expect(m.feed[0]).toMatchObject({ who: 'turn_outcome', what: '(assist): working' });
  });

  it('circles a satellite per session at work that no live run draws', () => {
    const m = orbitModel({
      runs: [run({ id: 'routine:1', session_ids: [1] })],
      hostAliases: ['htz'],
      sessions: [
        { id: 1, host_alias: 'htz', claude_status: 'working' },
        { id: 2, host_alias: 'htz', claude_status: 'working' },
        { id: 3, host_alias: 'htz', claude_status: 'idle' },
      ],
      routines: [],
      now: NOW,
    });
    expect(m.hosts[0].working).toBe(1);
    expect(m.working).toBe(2);
  });
});

describe('hostLabels', () => {
  it('keeps short names, and drops the leading segments long ones share', () => {
    const l = hostLabels(['claude-fleet-ci', 'claude-fleet-trn', 'claude-fleet', 'mac', 'mefistos', 'buildbox-primary']);
    expect(l.get('claude-fleet-ci')).toBe('ci');
    expect(l.get('claude-fleet-trn')).toBe('trn');
    expect(l.get('claude-fleet')).toBe('fleet');
    expect(l.get('mac')).toBe('mac');
    expect(l.get('buildbox-primary')).toBe('buildbox-primary');
  });

  it('puts segments back until two shortened labels differ', () => {
    const l = hostLabels(['prod-eu-web1', 'prod-eu-db01', 'prod-us-web1', 'prod-us-db01']);
    expect(l.get('prod-eu-web1')).toBe('eu-web1');
    expect(l.get('prod-us-web1')).toBe('us-web1');
    expect(l.get('prod-eu-db01')).toBe('eu-db01');
    expect(l.get('prod-us-db01')).toBe('us-db01');
    expect(new Set(l.values()).size).toBe(4);
  });

  it('never prints a shortened label equal to another host\'s short alias', () => {
    const l = hostLabels(['web1', 'claude-fleet-web1', 'claude-fleet-ci']);
    expect(l.get('web1')).toBe('web1');
    expect(l.get('claude-fleet-web1')).toBe('fleet-web1');
    expect(l.get('claude-fleet-ci')).toBe('ci');
  });
});
