import { describe, it, expect } from 'vitest';
import {
  compactWindow,
  compareVersions,
  diskMeter,
  endpointOutage,
  filterGroups,
  freshnessMark,
  groupHostsByAccount,
  healthLine,
  healthSampleFresh,
  HEALTH_SAMPLE_FRESH_SECS,
  hostAttention,
  NO_ACCOUNT_LABEL,
  newestClaudeVersion,
  removeHostMessage,
  rotateTokenMessage,
  sessionCounts,
  sharedWith,
  versionAge,
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
import type { UsageWindow } from './account_usage_store';

const L = 'en-GB';
const TZ = 'UTC';

describe('groupHostsByAccount', () => {
  it('groups by account alphabetically, hosts alphabetically, and skips an account with no host', () => {
    const groups = groupHostsByAccount(fleetHosts(), fleetAccounts());
    expect(groups.map((g) => [g.label, g.hosts.map((h) => h.alias)])).toEqual([
      ['admin-janci@users.noreply.github.com', ['claude-fleet-oci', 'mefistos']],
      ['m-janci@users.noreply.github.com', ['claude-fleet-htz']],
      ['mj-janci@users.noreply.github.com', ['claude-fleet-trn', 'local']],
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
      'm-janci@users.noreply.github.com',
      'mj-janci@users.noreply.github.com',
      'work',
    ]);
  });

  it('filters by alias, ssh alias or account', () => {
    const groups = groupHostsByAccount(fleetHosts(), fleetAccounts());
    expect(filterGroups(groups, 'OCI').flatMap((g) => g.hosts.map((h) => h.alias))).toEqual(['claude-fleet-oci']);
    expect(filterGroups(groups, 'mj-janci').flatMap((g) => g.hosts.map((h) => h.alias))).toEqual([
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
    expect(sessionCounts('mefistos', rows).text).toBe('6 · 2 working · 1 needs you');
    expect(sessionCounts('claude-fleet-trn', rows).text).toBe('14 · 2 working · 1 needs you');
    expect(sessionCounts('local', rows)).toMatchObject({ text: '0', title: '0 sessions, 0 working, 0 need you' });
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
    expect(counts.text).toBe('6 · 2 working · 1 needs you');
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
    now: NOW,
    versionMaxAgeSecs: 86400,
    diskLowPct: 90,
    hubVersion: '0.3.1',
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
    expect(
      newestClaudeVersion([host('a', { claude_version: '2.1.99' }), host('b'), host('c', { claude_version: null })], NOW, 86400),
    ).toBe('2.1.145');
  });

  it('flags an older Claude only while its version stamp is fresh', () => {
    // ux F-13: the badge was wrong on 3 of 4 hosts because the compared
    // number was a provisioning-day cache stamped with today's ping.
    const old = host('claude-fleet-oci', { claude_version: '2.1.99 (Claude Code)', claude_version_at: NOW - 3600 });
    expect(hostAttention({ ...base, host: old })?.kind).toBe('claude_old');
    expect(hostAttention({ ...base, host: old })?.title).toContain('checked 1h ago');
    const stale = host('claude-fleet-oci', { claude_version: '2.1.99', claude_version_at: NOW - 2 * 86400 });
    expect(hostAttention({ ...base, host: stale })).toBeNull();
    const never = host('claude-fleet-oci', { claude_version: '2.1.99', claude_version_at: null });
    expect(hostAttention({ ...base, host: never })).toBeNull();
  });

  it('the fleet newest ignores stale stamps', () => {
    const hosts = [
      host('a', { claude_version: '2.1.99', claude_version_at: NOW - 60 }),
      host('b', { claude_version: '2.1.277', claude_version_at: NOW - 3 * 86400 }),
    ];
    expect(newestClaudeVersion(hosts, NOW, 86400)).toBe('2.1.99');
  });
});

describe('host health helpers', () => {
  it('diskMeter reads used percent and levels it', () => {
    const h = host('htz', { disk_home_free_kb: 3_600_000, disk_home_total_kb: 150_000_000 });
    expect(diskMeter(h)).toEqual({ pct: 98, text: '98% · 3.4 GB free', level: 'crit' });
    expect(diskMeter(host('x', { disk_home_free_kb: 72_000_000, disk_home_total_kb: 96_000_000 }))?.level).toBe('ok');
    expect(diskMeter(host('x', { disk_home_free_kb: 8_000_000, disk_home_total_kb: 96_000_000 }))?.level).toBe('warn');
    expect(diskMeter(host('x', { disk_home_free_kb: null, disk_home_total_kb: null }))).toBeNull();
  });

  it('healthLine and versionAge render what is known and skip the rest', () => {
    const h = host('trn', {
      disk_home_free_kb: 3_600_000,
      disk_home_total_kb: 150_000_000,
      load_1m: 5.25,
      uptime_secs: 144 * 86400,
      agent_version: '0.2.26',
      transport: 'agent',
      claude_version_at: NOW - 2 * 3600,
    });
    expect(healthLine(h, NOW)).toBe('disk 98% · 3.4 GB free · load 5.3 · up 144d · agent 0.2.26 · sampled 2m ago');
    expect(healthLine(host('bare', { health_at: null }), NOW)).toBe('not sampled yet');
    expect(versionAge(h, NOW)).toBe('checked 2h ago');
    expect(versionAge(host('bare', { claude_version_at: null }), NOW)).toBe('never checked');
  });

  it('a degraded provisioning names its reason, outranking the generic provision_stale', () => {
    const base = {
      hasToken: true,
      tokensLoaded: true,
      hook: { state: 'seen' as const, lastAt: NOW },
      sessionCount: 1,
      newestClaude: '2.1.145',
      now: NOW,
      versionMaxAgeSecs: 86400,
      diskLowPct: 90,
      hubVersion: '0.3.1',
    };
    // A degraded run clears the fingerprint, so the host is ALSO provision_stale.
    // Saying "provisioned with an older fleet" would be the wrong reason.
    const degraded = host('htz', {
      provision_stale: true,
      provision_warning: 'ag launcher not installed: install.sh exited 5',
    });
    const a = hostAttention({ ...base, host: degraded });
    expect(a?.kind).toBe('provision_warning');
    expect(a?.title).toContain('ag launcher not installed');
    // stale with no warning is still the generic one
    expect(hostAttention({ ...base, host: host('oci', { provision_stale: true }) })?.kind).toBe('provision_stale');
    // and a clean host earns no mark at all
    expect(hostAttention({ ...base, host: host('ok', {}) })).toBeNull();
  });

  it('auth_override names the variables that outrank the login and outranks disk_low', () => {
    const base = {
      hasToken: true,
      tokensLoaded: true,
      hook: { state: 'seen' as const, lastAt: NOW },
      sessionCount: 1,
      newestClaude: null,
      now: NOW,
      versionMaxAgeSecs: 86400,
      diskLowPct: 90,
      hubVersion: null,
    };
    const keyed = host('mef', {
      auth_overrides: ['ANTHROPIC_API_KEY'],
      disk_home_free_kb: 3_600_000,
      disk_home_total_kb: 150_000_000,
      health_at: NOW,
    });
    const a = hostAttention({ ...base, host: keyed });
    expect(a?.kind).toBe('auth_override');
    expect(a?.title).toContain('ANTHROPIC_API_KEY is set');
    // none set, or unknown (older hub), earns no mark
    expect(hostAttention({ ...base, host: host('ok', { auth_overrides: [] }) })).toBeNull();
    expect(hostAttention({ ...base, host: host('old', { auth_overrides: null }) })).toBeNull();
  });

  it('disk_low outranks claude_old; agent_old fires when the agent is older than the hub version', () => {
    const base = {
      hasToken: true,
      tokensLoaded: true,
      hook: { state: 'seen' as const, lastAt: NOW },
      sessionCount: 1,
      newestClaude: '2.1.145',
      now: NOW,
      versionMaxAgeSecs: 86400,
      diskLowPct: 90,
      hubVersion: '0.3.1',
    };
    const full = host('htz', { disk_home_free_kb: 3_600_000, disk_home_total_kb: 150_000_000, claude_version: '2.1.99', claude_version_at: NOW - 60 });
    expect(hostAttention({ ...base, host: full })?.kind).toBe('disk_low');
    expect(hostAttention({ ...base, host: full })?.title).toContain('98%');
    const agent = host('trn', { transport: 'agent', agent_version: '0.2.26' });
    expect(hostAttention({ ...base, host: agent })?.kind).toBe('agent_old');
    expect(hostAttention({ ...base, host: host('ok', { transport: 'agent', agent_version: '0.3.1' }) })).toBeNull();
    // After a hub rollback the agent is ahead: no "upgrade the agent" mark.
    expect(hostAttention({ ...base, host: host('ahead', { transport: 'agent', agent_version: '0.4.0' }) })).toBeNull();
  });

  it('disk_low waits on a fresh health sample; the health line says how old it is', () => {
    const base = {
      hasToken: true,
      tokensLoaded: true,
      hook: { state: 'seen' as const, lastAt: NOW },
      sessionCount: 1,
      newestClaude: null,
      now: NOW,
      versionMaxAgeSecs: 86400,
      diskLowPct: 90,
      hubVersion: '0.3.1',
    };
    const full = { disk_home_free_kb: 3_600_000, disk_home_total_kb: 150_000_000 };
    const stale = host('htz', { ...full, health_at: NOW - 2 * 3600 });
    expect(healthSampleFresh(stale, NOW)).toBe(false);
    expect(hostAttention({ ...base, host: stale })).toBeNull();
    expect(healthLine(stale, NOW)).toMatch(/ · sampled 2h ago \(stale\)$/);
    const fresh = host('htz', { ...full, health_at: NOW - 60 });
    expect(healthSampleFresh(fresh, NOW)).toBe(true);
    expect(hostAttention({ ...base, host: fresh })?.kind).toBe('disk_low');
    expect(healthSampleFresh(host('x', { health_at: NOW - HEALTH_SAMPLE_FRESH_SECS }), NOW)).toBe(true);
    expect(healthSampleFresh(host('x', { health_at: null }), NOW)).toBe(false);
  });

  it('provision_stale outranks disk_low and names the content-only refresh', () => {
    const base = {
      hasToken: true,
      tokensLoaded: true,
      hook: { state: 'seen' as const, lastAt: NOW },
      sessionCount: 1,
      newestClaude: null,
      now: NOW,
      versionMaxAgeSecs: 86400,
      diskLowPct: 90,
      hubVersion: '0.3.1',
    };
    const stale = host('x', { provision_stale: true, disk_home_free_kb: 3_600_000, disk_home_total_kb: 150_000_000 });
    const mark = hostAttention({ ...base, host: stale });
    expect(mark?.kind).toBe('provision_stale');
    expect(mark?.title).toContain('fleet-hub provision --host x --content-only');
  });
});

describe('compact usage', () => {
  it('shows % left and the reset together', () => {
    const s = fleetUsage()[ADMIN.uuid];
    expect(compactWindow('5h', s, NOW, L, TZ)).toEqual({ left: '91% left', reset: 'resets 15:10', freshness: 'fresh' });
    expect(compactWindow('weekly', s, NOW, L, TZ)).toEqual({ left: '58% left', reset: 'resets Thu 09:00', freshness: 'fresh' });
  });

  // Seen live: a hub sent an unused five-hour window as
  // `{"utilization":0.0}` (nulls stripped), and the undefined reset took the
  // Hosts view down.
  it('treats a reset the wire left out like a null one', () => {
    const s = fleetUsage()[ADMIN.uuid];
    const noReset = { ...s, usage: { ...s.usage!, five_hour: { utilization: 0 } as UsageWindow } };
    expect(compactWindow('5h', noReset, NOW, L, TZ)).toEqual({ left: '100% left', reset: null, freshness: 'fresh' });
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
