// Dashboards from dynamic data (owner report 2026-11-06): the fleet's own
// rows a `fleet` widget counts, a feed's words, and what a summary widget
// with `sources` sends with its request.
import { describe, it, expect } from 'vitest';
import { feedBy, feedStat, fleetFacts, summaryContext, type DashboardFeed, type FleetData } from './dashboard_sources';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';
import type { AccountRow } from './accounts';
import type { AccountUsageSnapshot } from './account_usage_store';

const session = (id: number, over: Partial<SessionRow>): SessionRow =>
  ({ id, tmux_name: `s${id}`, host_alias: 'mac', kind: 'work', status: 'running', lost_at: null, claude_status: null, ...over }) as SessionRow;
const host = (alias: string, reachable: boolean, over: Partial<HostRow> = {}): HostRow => ({ alias, reachable, hidden: false, ...over }) as HostRow;
const account = (uuid: string, nickname: string): AccountRow => ({ uuid, nickname, email: null }) as AccountRow;
const NOW = 1_760_000_000;
const usage = (uuid: string, five: number | null, week: number | null, fetched_at: number | null = NOW - 60): AccountUsageSnapshot =>
  ({
    account_uuid: uuid,
    status: 'ok',
    fetched_at,
    usage: {
      five_hour: five == null ? null : { utilization: five, resets_at: null },
      seven_day: week == null ? null : { utilization: week, resets_at: null },
      seven_day_opus: null,
      seven_day_sonnet: null,
    },
  }) as AccountUsageSnapshot;

const data: FleetData = {
  sessions: [
    session(1, { claude_status: 'working' }),
    session(2, { claude_status: 'working' }),
    session(3, { claude_status: 'blocked' }),
    session(4, { claude_status: 'idle' }),
    session(5, { claude_status: 'working', lost_at: 9 }),
    session(6, { claude_status: 'working', kind: 'bg' }),
  ],
  hosts: [host('mac', true, { load_1m: 0.42 }), host('htz', false), host('old', false, { hidden: true })],
  accounts: [account('a', 'Work'), account('b', 'Home'), account('c', 'Spare')],
  usage: { a: usage('a', 35, 60), b: usage('b', 91, 20) },
};

describe('fleetFacts', () => {
  it('counts live sessions by state, background agents and lost ones left out', () => {
    const f = fleetFacts('sessions', data, NOW);
    expect(f.value).toBe('2');
    expect(f.note).toBe('working · 1 need you · 1 idle');
    expect(f.rows).toEqual([
      { label: 'Working', value: '2' },
      { label: 'Needs you', value: '1', warn: true },
      { label: 'Idle', value: '1' },
    ]);
  });

  it('counts who needs you as the Needs you badge does', () => {
    const rows = [
      session(1, { claude_status: 'idle', stuck_kind: 'auth_menu' } as Partial<SessionRow>),
      session(2, { claude_status: 'idle', pending_form: { id: 1 } } as unknown as Partial<SessionRow>),
      session(3, { claude_status: 'blocked', kind: 'external' }),
      session(4, { claude_status: 'blocked', kind: 'shell' }),
      session(5, { claude_status: 'idle', status: 'blocked' } as unknown as Partial<SessionRow>),
    ];
    const f = fleetFacts('sessions', { ...data, sessions: rows }, NOW);
    expect(f.note).toBe('working · 2 need you · 3 idle');
  });

  it('says how many shown hosts answer, and which do not', () => {
    const f = fleetFacts('hosts', data, NOW);
    expect([f.value, f.warn]).toEqual(['1/2', true]);
    expect(f.rows).toEqual([
      { label: 'mac', value: 'load 0.4' },
      { label: 'htz', value: 'offline', warn: true },
    ]);
    // This machine is up whatever its row says, as in the status bar.
    const local = fleetFacts('hosts', { ...data, hosts: [host('local', false)] }, NOW);
    expect([local.value, local.warn]).toEqual(['1/1', false]);
  });

  it('leads with the busiest account’s 5-hour use', () => {
    const f = fleetFacts('accounts', data, NOW);
    expect(f.value).toBe('91%');
    expect(f.note).toBe('highest 5-hour use · Home');
    expect(f.warn).toBe(true);
    expect(f.rows).toEqual([
      { label: 'Work', value: '5h 35% · week 60%' },
      { label: 'Home', value: '5h 91% · week 20%', warn: true },
      { label: 'Spare', value: 'no usage read' },
    ]);
    expect(fleetFacts('accounts', { ...data, usage: {} }, NOW).value).toBe('—');
  });

  it('withholds a use that is too old or past its reset, as every usage surface does', () => {
    const old = { ...data, usage: { b: usage('b', 91, 20, NOW - 4 * 3600) } };
    expect(fleetFacts('accounts', old, NOW).value).toBe('—');
    const reset = usage('b', 91, 20);
    reset.usage!.five_hour!.resets_at = NOW - 10;
    const f = fleetFacts('accounts', { ...data, usage: { b: reset } }, NOW);
    expect(f.rows[1]).toEqual({ label: 'Home', value: 'week 20%' });
  });
});

describe('feeds', () => {
  const feed: DashboardFeed = { key: 'deploys', value: '3 today\nall green', by: 'routine', by_ref: 'Nightly', updated_at: 1 };

  it('reads a feed’s first line as its number, and says who wrote it', () => {
    expect(feedStat(feed.value)).toBe('3 today');
    expect(feedStat('a very long first line indeed')).toBe('a very long fir…');
    expect(feedBy(feed)).toBe('routine Nightly');
    expect(feedBy({ ...feed, by: 'agent' })).toBe('Control');
    expect(feedBy({ ...feed, by: 'person' })).toBe('you');
  });

  it('sends a summary the sources it reads, but the work the hub counts itself', () => {
    expect(summaryContext(undefined, data, { deploys: feed })).toEqual({});
    expect(summaryContext(['work'], data, { deploys: feed })).toEqual({});
    const c = summaryContext(['work', 'hosts', 'feeds'], data, { deploys: feed }, ['deploys'], NOW);
    expect(Object.keys(c).sort()).toEqual(['feeds', 'hosts']);
    expect(c.hosts).toEqual({ summary: '1/2 hosts reachable', rows: ['mac: load 0.4', 'htz: offline'] });
    expect(c.feeds).toEqual({ deploys: { value: '3 today\nall green', by: 'routine Nightly', at: 1 } });
  });

  it('sends only the feeds its dashboard’s feed widgets name', () => {
    const other: DashboardFeed = { ...feed, key: 'notes', value: 'x'.repeat(4000) };
    const c = summaryContext(['feeds'], data, { deploys: feed, notes: other }, ['deploys', 'missing'], NOW);
    expect(Object.keys(c.feeds as object)).toEqual(['deploys']);
    expect(summaryContext(['feeds'], data, { deploys: feed }, [], NOW)).toEqual({ feeds: {} });
  });
});
