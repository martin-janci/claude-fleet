import { describe, it, expect } from 'vitest';
import {
  accountSummaries,
  countLine,
  fallbackRoutinesOn,
  historyPoints,
  HISTORY_SPAN_SECS,
  loginsFor,
  peakUsed,
  planLabel,
  sparkPath,
  type UsageSnapshotRow, refreshedLine, routinesRunningAs } from './accounts_page';
import {
  ADMIN,
  GMAIL,
  HOUR,
  NOW,
  SPARE,
  WORK,
  fleetAccounts,
  fleetHosts,
  fleetUsage,
  host,
  session,
} from './hosts_fixture';

function row(at: number, five: number | null, week: number | null): UsageSnapshotRow {
  return {
    account_uuid: ADMIN.uuid,
    fetched_at: at,
    usage: {
      five_hour: five === null ? null : { utilization: five, resets_at: null },
      seven_day: week === null ? null : { utilization: week, resets_at: null },
      seven_day_opus: null,
      seven_day_sonnet: null,
    },
    subscription: 'max',
    source_host: null,
  };
}

describe('accounts page helpers', () => {
  it('names the plan from the usage answer, else the seat tier', () => {
    expect(planLabel('max')).toBe('Max');
    expect(planLabel(null, 'pro')).toBe('Pro');
    expect(planLabel('', null)).toBeNull();
  });

  it('lists the host logins and profile logins of an account', () => {
    const hosts = [
      host('zeta', { account_uuid: ADMIN.uuid }),
      host('alpha', {
        account_uuid: WORK.uuid,
        claude_profiles: [
          { name: 'admin', account_uuid: ADMIN.uuid },
          { name: 'other', account_uuid: GMAIL.uuid },
        ],
      }),
    ];
    expect(loginsFor(ADMIN.uuid, hosts)).toEqual([
      { host: 'alpha', profile: 'admin' },
      { host: 'zeta', profile: null },
    ]);
  });

  it('summarises every known account with its usage, logins and sessions', () => {
    const sessions = [
      session('mefistos', 'a', { account_uuid: ADMIN.uuid }),
      session('mefistos', 'b', { account_uuid: ADMIN.uuid }),
      session('local', 'c', { account_uuid: GMAIL.uuid }),
    ];
    const list = accountSummaries(fleetAccounts(), fleetHosts(), sessions, fleetUsage());
    expect(list).toHaveLength(4);
    const admin = list.find((a) => a.uuid === ADMIN.uuid)!;
    expect(admin.plan).toBe('Max');
    expect(admin.sessions.map((s) => s.tmux_name)).toEqual(['a', 'b']);
    expect(admin.logins.map((l) => l.host)).toEqual(['claude-fleet-oci', 'mefistos']);
    const spare = list.find((a) => a.uuid === SPARE.uuid)!;
    expect(spare.logins).toEqual([]);
    expect(spare.usage?.usage).toBeNull();
  });

  it('keeps a window’s history inside its span, oldest first', () => {
    const rows = [
      row(NOW - HISTORY_SPAN_SECS['5h'] - 1, 90, 10),
      row(NOW - 2 * HOUR, 30, 20),
      row(NOW - HOUR, null, 25),
      row(NOW, 60, 30),
    ];
    expect(historyPoints(rows, '5h', NOW)).toEqual([
      { at: NOW - 2 * HOUR, used: 30 },
      { at: NOW, used: 60 },
    ]);
    expect(historyPoints(rows, 'weekly', NOW).map((p) => p.used)).toEqual([10, 20, 25, 30]);
  });

  it('draws a path only from two points, with 100% used at the top', () => {
    const span = HISTORY_SPAN_SECS['5h'];
    expect(sparkPath([{ at: NOW, used: 10 }], '5h', NOW, 100, 50)).toBe('');
    expect(
      sparkPath(
        [
          { at: NOW - span, used: 0 },
          { at: NOW, used: 100 },
        ],
        '5h',
        NOW,
        100,
        50,
      ),
    ).toBe('M0.0 50.0 L100.0 0.0');
  });

  it('reports the peak use', () => {
    expect(peakUsed([])).toBeNull();
    expect(
      peakUsed([
        { at: 1, used: 12.4 },
        { at: 2, used: 91.6 },
      ]),
    ).toBe(92);
  });
});

describe('fallback role (G4.5)', () => {
  it('counts the switched-on routines that fall back to an account', () => {
    const hosts = [host('mercury', { account_uuid: WORK.uuid }), host('venus', { account_uuid: GMAIL.uuid })];
    const routines = [
      { host_alias: 'mercury', fallback_host: 'venus', profile: null, enabled: true },
      { host_alias: 'mercury', fallback_host: 'venus', profile: null, enabled: false },
      { host_alias: 'venus', fallback_host: 'venus', profile: null, enabled: true },
      { host_alias: 'mercury', profile: null, enabled: true },
    ];
    expect(fallbackRoutinesOn(GMAIL.uuid, routines, hosts)).toBe(1);
    expect(fallbackRoutinesOn(WORK.uuid, routines, hosts)).toBe(0);
    expect(countLine(2, 1, null, 1)).toBe('2 sessions · 1 routine · fallback for 1 routine');
    expect(countLine(2, 0, '$1.00', 0)).toBe('2 sessions · $1.00 today');
  });
});

describe('the page header and paused panel (M15 G7.12)', () => {
  it('names the newest reading of any account', () => {
    expect(refreshedLine([], 1000)).toBeNull();
    expect(refreshedLine([null, undefined], 1000)).toBeNull();
    expect(refreshedLine([900, 970], 1000)).toBe('usage refreshed just now');
    expect(refreshedLine([400, 100], 1000)).toBe('usage refreshed 10m ago');
    expect(refreshedLine([1000 - 7200], 1000)).toBe('usage refreshed 2h ago');
  });

  it('lists the switched-on routines that run as the account', () => {
    const hosts = [{ alias: 'mac', account_uuid: 'A', claude_profiles: [{ name: 'work', account_uuid: 'B' }] }] as never;
    const routines = [
      { id: 1, host_alias: 'mac', profile: null, enabled: true },
      { id: 2, host_alias: 'mac', profile: null, enabled: false },
      { id: 3, host_alias: 'mac', profile: 'work', enabled: true },
    ];
    expect(routinesRunningAs('A', routines, hosts).map((r) => r.id)).toEqual([1]);
    expect(routinesRunningAs('B', routines, hosts).map((r) => r.id)).toEqual([3]);
  });
});
