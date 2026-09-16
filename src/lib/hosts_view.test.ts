import { describe, it, expect } from 'vitest';
import {
  compactWindow,
  compareVersions,
  endpointOutage,
  filterGroups,
  freshnessMark,
  groupHostsByAccount,
  hostAttention,
  NO_ACCOUNT_LABEL,
  newestClaudeVersion,
  removeHostMessage,
  rotateTokenMessage,
  sessionCounts,
  sharedWith,
} from './hosts_view';
import {
  ADMIN,
  GMAIL,
  MIN,
  NOW,
  WORK,
  fleetAccounts,
  fleetHosts,
  fleetSessions,
  fleetUsage,
  host,
  outageUsage,
  snapshot,
} from './hosts_fixture';

const L = 'en-GB';
const TZ = 'UTC';

describe('groupHostsByAccount', () => {
  it('groups by account alphabetically, hosts alphabetically, and skips an account with no host', () => {
    const groups = groupHostsByAccount(fleetHosts(), fleetAccounts());
    expect(groups.map((g) => [g.label, g.hosts.map((h) => h.alias)])).toEqual([
      ['admin@32bit.sk', ['claude-fleet-oci', 'mefistos']],
      ['m.janci@32bit.sk', ['claude-fleet-htz']],
      ['mj.janci@gmail.com', ['claude-fleet-trn', 'local']],
    ]);
  });

  it('puts the No Claude account group last whatever its name sorts as', () => {
    const hosts = [...fleetHosts(), host('aaa-nas'), host('zz-box')];
    const groups = groupHostsByAccount(hosts, fleetAccounts());
    const last = groups[groups.length - 1];
    expect(last.label).toBe(NO_ACCOUNT_LABEL);
    expect(last.accountUuid).toBeNull();
    expect(last.hosts.map((h) => h.alias)).toEqual(['aaa-nas', 'zz-box']);
  });

  it('orders by the nickname when one is set, and never by headroom', () => {
    const accounts = fleetAccounts().map((a) => (a.uuid === ADMIN.uuid ? { ...a, nickname: 'work' } : a));
    expect(groupHostsByAccount(fleetHosts(), accounts).map((g) => g.label)).toEqual([
      'm.janci@32bit.sk',
      'mj.janci@gmail.com',
      'work',
    ]);
  });

  it('filters by alias, ssh alias or account', () => {
    const groups = groupHostsByAccount(fleetHosts(), fleetAccounts());
    expect(filterGroups(groups, 'OCI').flatMap((g) => g.hosts.map((h) => h.alias))).toEqual(['claude-fleet-oci']);
    expect(filterGroups(groups, 'gmail').flatMap((g) => g.hosts.map((h) => h.alias))).toEqual([
      'claude-fleet-trn',
      'local',
    ]);
    expect(filterGroups(groups, 'nothing-here')).toEqual([]);
  });

  it('sharedWith lists the other hosts on the account', () => {
    const hosts = fleetHosts();
    const by = (a: string) => hosts.find((h) => h.alias === a)!;
    expect(sharedWith(by('mefistos'), hosts)).toEqual(['claude-fleet-oci']);
    expect(sharedWith(by('local'), hosts)).toEqual(['claude-fleet-trn']);
    expect(sharedWith(by('claude-fleet-htz'), hosts)).toEqual([]);
    expect(sharedWith(host('nas'), hosts)).toEqual([]);
  });
});

describe('sessionCounts', () => {
  it('counts total, working and blocked, omitting zero parts', () => {
    const rows = fleetSessions();
    expect(sessionCounts('mefistos', rows).text).toBe('6 ⚡2 ⏸1');
    expect(sessionCounts('claude-fleet-trn', rows).text).toBe('14 ⚡2 ⏸1');
    expect(sessionCounts('local', rows)).toMatchObject({ text: '0', title: '0 sessions, 0 working, 0 blocked' });
  });

  it('ignores external rows (sessions running outside fleet)', () => {
    const rows = fleetSessions().filter((s) => s.host_alias === 'mefistos');
    const ext = (i: number, claude_status: 'working' | 'blocked') => ({
      ...rows[0],
      id: 9000 + i,
      tmux_name: `bg:ext-${i}`,
      kind: 'external',
      claude_status,
    });
    const counts = sessionCounts('mefistos', [...rows, ext(1, 'working'), ext(2, 'blocked')]);
    expect(counts.text).toBe('6 ⚡2 ⏸1');
    expect(counts.total).toBe(6);
  });
});

describe('hostAttention', () => {
  const base = {
    host: host('mefistos'),
    hasToken: true,
    tokensLoaded: true,
    hook: { state: 'seen' as const, lastAt: NOW },
    sessionCount: 6,
    newestClaude: '2.1.145',
  };

  it('is null for a healthy host', () => {
    expect(hostAttention(base)).toBeNull();
  });

  it('names a missing token (hooks missing) first, and waits for the tokens to load', () => {
    const noToken = { ...base, hasToken: false, hook: { state: 'not_installed' as const } };
    expect(hostAttention(noToken)?.kind).toBe('hooks_missing');
    expect(hostAttention({ ...noToken, tokensLoaded: false })).toBeNull();
    expect(hostAttention({ ...base, hasToken: false })?.kind).toBe('token_missing');
  });

  it('flags installed hooks that never reported while the host has sessions', () => {
    const silent = { ...base, hook: { state: 'never_seen' as const } };
    expect(hostAttention(silent)?.kind).toBe('hooks_stale');
    expect(hostAttention({ ...silent, sessionCount: 0 })).toBeNull();
  });

  it('flags a Claude Code older than the newest in the fleet, with a title', () => {
    const old = hostAttention({ ...base, host: host('claude-fleet-oci', { claude_version: '2.1.99 (Claude Code)' }) });
    expect(old?.kind).toBe('claude_old');
    expect(old?.title).toContain('older than 2.1.145');
  });

  it('compares versions numerically', () => {
    expect(compareVersions('2.1.99', '2.1.145')).toBeLessThan(0);
    expect(compareVersions('2.10.0', '2.9.9')).toBeGreaterThan(0);
    expect(compareVersions(null, '1')).toBe(0);
    expect(newestClaudeVersion([host('a', { claude_version: '2.1.99' }), host('b'), host('c', { claude_version: null })])).toBe('2.1.145');
  });
});

describe('compact usage', () => {
  it('shows % left and the reset together', () => {
    const s = fleetUsage()[ADMIN.uuid];
    expect(compactWindow('5h', s, NOW, L, TZ)).toEqual({ left: '91% left', reset: 'resets 15:10', freshness: 'fresh' });
    expect(compactWindow('weekly', s, NOW, L, TZ)).toEqual({ left: '58% left', reset: 'resets Thu 09:00', freshness: 'fresh' });
  });

  it('marks stale with ~, withholds expired, and says checking… before the first fetch', () => {
    const stale = fleetUsage()[WORK.uuid];
    expect(compactWindow('5h', stale, NOW, L, TZ).left).toBe('~91% left');
    expect(compactWindow('5h', snapshot(GMAIL.uuid, { fetched_at: NOW - 31 * MIN }), NOW, L, TZ)).toEqual({
      left: '? left',
      reset: null,
      freshness: 'expired',
    });
    expect(compactWindow('5h', null, NOW).left).toBe('checking…');
    expect(compactWindow('5h', snapshot(GMAIL.uuid, { usage: null, status: 'never_fetched' }), NOW).left).toBe('checking…');
  });

  it('freshness mark: age, ◷ when stale, ? when expired', () => {
    const u = fleetUsage();
    expect(freshnessMark(u[ADMIN.uuid], NOW)).toMatchObject({ mark: '2m', state: 'fresh' });
    expect(freshnessMark(u[WORK.uuid], NOW)).toMatchObject({ mark: '◷ 14m', state: 'stale' });
    expect(freshnessMark(snapshot(GMAIL.uuid, { fetched_at: NOW - 4 * 3600 }), NOW).mark).toBe('?');
    expect(freshnessMark(null, NOW).mark).toBe('…');
  });
});

describe('endpointOutage', () => {
  it('is shown when every account that reached the endpoint is unavailable, ignoring a host-less account', () => {
    const o = endpointOutage(Object.values(outageUsage()), NOW, L, TZ);
    expect(o?.text).toBe(
      "Usage unavailable since 13:10. Anthropic's usage endpoint returned an unexpected response (HTTP 404). It's undocumented and may have changed. Sessions are unaffected.",
    );
    expect(o?.nextTryAt).toBe(NOW + 8 * MIN);
    expect(o?.copyDetails).toBe('status: unavailable\ndetail: HTTP 404: <html>Not Found</html>');
    expect(o?.accountUuids).toHaveLength(3);
  });

  it('is not shown while any account is fine, or when nothing reached the endpoint', () => {
    const mixed = { ...outageUsage(), [WORK.uuid]: snapshot(WORK.uuid) };
    expect(endpointOutage(Object.values(mixed), NOW)).toBeNull();
    expect(endpointOutage([snapshot(GMAIL.uuid, { status: 'never_fetched', usage: null })], NOW)).toBeNull();
    expect(endpointOutage([], NOW)).toBeNull();
  });
});

describe('confirm copy', () => {
  it('says what removing a host does to its session rows, and that tmux keeps running', () => {
    const m = removeHostMessage('mefistos', 6);
    expect(m).toContain('Fleet deletes its 6 session rows from its database');
    expect(m).toContain('The tmux sessions on mefistos are not touched and keep running');
    expect(removeHostMessage('x', 1)).toContain('its 1 session row from');
    expect(removeHostMessage('x', 0)).toContain('It has no session rows.');
  });

  it('says the old token stops working and running sessions may need a restart', () => {
    const m = rotateTokenMessage('mefistos');
    expect(m).toContain('The old token stops working.');
    expect(m).toContain('until they are restarted');
  });
});
