import { describe, it, expect } from 'vitest';
import {
  compactAge,
  footerUsage,
  hostChipUsage,
  lowHeadroomWarning,
  selectedUsageLine,
} from './usage_glance';
import {
  ADMIN,
  GMAIL,
  HOUR,
  MIN,
  NOW,
  RESET_5H,
  SPARE,
  WORK,
  fleetAccounts,
  fleetHosts,
  fleetUsage,
  host,
  outageUsage,
  snapshot,
} from './hosts_fixture';
import type { AccountUsageSnapshot } from './account_usage_store';

const L = 'en-GB';
const TZ = 'UTC';
const hostsAll = fleetHosts();
const hostBy = (alias: string) => hostsAll.find((h) => h.alias === alias)!;

/** A snapshot whose 5-hour window has `left` % left (weekly roomier). */
function fiveLeft(uuid: string, left: number, over: Partial<AccountUsageSnapshot> = {}): AccountUsageSnapshot {
  return snapshot(uuid, {
    usage: {
      five_hour: { utilization: 100 - left, resets_at: RESET_5H },
      seven_day: { utilization: 2, resets_at: NOW + 3 * 86400 },
      seven_day_opus: null,
      seven_day_sonnet: null,
    },
    ...over,
  });
}

describe('hostChipUsage', () => {
  it('fresh: % left and the reset time, with equal weight', () => {
    expect(hostChipUsage(hostBy('mefistos'), ADMIN, fiveLeft(ADMIN.uuid, 91), NOW, L, TZ)).toBe('91% left · resets 15:10');
    // The window is named only when weekly binds.
    expect(hostChipUsage(hostBy('mefistos'), ADMIN, snapshot(ADMIN.uuid), NOW, L, TZ)).toBe('weekly 58% left · resets Thu 09:00');
  });

  it('stale: ~ and ◷, still with the reset', () => {
    const s = fiveLeft(ADMIN.uuid, 62, { fetched_at: NOW - 14 * MIN });
    expect(hostChipUsage(hostBy('mefistos'), ADMIN, s, NOW, L, TZ)).toBe('~62% left ◷ · resets 15:10');
  });

  it('expired: the number is withheld', () => {
    const s = fiveLeft(ADMIN.uuid, 62, { fetched_at: NOW - 45 * MIN });
    expect(hostChipUsage(hostBy('mefistos'), ADMIN, s, NOW, L, TZ)).toBe('? left');
  });

  it('low: the glyph leads', () => {
    expect(hostChipUsage(hostBy('mefistos'), ADMIN, fiveLeft(ADMIN.uuid, 8), NOW, L, TZ)).toBe('▲ 8% left · resets 15:10');
  });

  it('a host with no account says so; an offline host keeps offline', () => {
    expect(hostChipUsage(host('nas'), null, null, NOW)).toBe('no account');
    expect(hostChipUsage(hostBy('claude-fleet-htz'), WORK, snapshot(WORK.uuid), NOW)).toBe('offline');
    // local is always pickable, so it shows usage even when unreachable.
    expect(hostChipUsage(host('local', { reachable: false, account_uuid: GMAIL.uuid }), GMAIL, fiveLeft(GMAIL.uuid, 91), NOW, L, TZ)).toBe(
      '91% left · resets 15:10',
    );
  });
});

describe('selectedUsageLine', () => {
  it('names the account and both windows with the 5-hour reset and the age', () => {
    expect(selectedUsageLine(hostBy('mefistos'), ADMIN, snapshot(ADMIN.uuid), NOW, L, TZ)).toBe(
      'admin@32bit.sk · 5h 91% left, resets 15:10 · weekly 58% left · 2 min ago',
    );
  });

  it('uses the nickname, marks stale values and withholds expired ones', () => {
    const nick = { ...ADMIN, nickname: 'admin' };
    const stale = snapshot(ADMIN.uuid, { fetched_at: NOW - 14 * MIN });
    expect(selectedUsageLine(hostBy('mefistos'), nick, stale, NOW, L, TZ)).toBe(
      'admin · 5h ~91% left, resets 15:10 · weekly ~58% left · 14 min ago',
    );
    const old = snapshot(ADMIN.uuid, { fetched_at: NOW - 2 * HOUR });
    expect(selectedUsageLine(hostBy('mefistos'), nick, old, NOW, L, TZ)).toBe('admin · 5h ? left · weekly ~58% left · 2h 0m ago');
  });

  it('before the first fetch, and for a host with no account', () => {
    expect(selectedUsageLine(hostBy('mefistos'), ADMIN, null, NOW)).toBe('admin@32bit.sk · checking usage…');
    expect(selectedUsageLine(host('nas'), null, null, NOW)).toBe('Not logged in to Claude on this host — no usage to show.');
  });
});

describe('lowHeadroomWarning', () => {
  it('names the account and the other hosts that share it', () => {
    expect(lowHeadroomWarning(hostBy('mefistos'), hostsAll, ADMIN, fiveLeft(ADMIN.uuid, 8), NOW, L, TZ)).toBe(
      '▲ admin@32bit.sk has 8% of its 5-hour window left (resets 15:10). Also used by claude-fleet-oci.',
    );
  });

  it('a limit says so, with the extra-usage consequence when it applies', () => {
    const extra = { ...ADMIN, has_extra_usage: true };
    expect(lowHeadroomWarning(hostBy('claude-fleet-oci'), hostsAll, extra, fiveLeft(ADMIN.uuid, 0), NOW, L, TZ)).toBe(
      '■ admin@32bit.sk is at its 5-hour limit (resets 15:10). Further use spends extra usage. Also used by mefistos.',
    );
  });

  it('nothing at ok or caution, when expired, or without an account', () => {
    expect(lowHeadroomWarning(hostBy('mefistos'), hostsAll, ADMIN, snapshot(ADMIN.uuid), NOW)).toBeNull();
    expect(lowHeadroomWarning(hostBy('mefistos'), hostsAll, ADMIN, fiveLeft(ADMIN.uuid, 30), NOW)).toBeNull();
    expect(lowHeadroomWarning(hostBy('mefistos'), hostsAll, ADMIN, fiveLeft(ADMIN.uuid, 8, { fetched_at: NOW - HOUR }), NOW)).toBeNull();
    expect(lowHeadroomWarning(host('nas'), hostsAll, null, null, NOW)).toBeNull();
  });

  it('no "Also used by" for an account on one host', () => {
    expect(lowHeadroomWarning(hostBy('claude-fleet-htz'), hostsAll, WORK, fiveLeft(WORK.uuid, 8), NOW, L, TZ)).toBe(
      '▲ m.janci@32bit.sk has 8% of its 5-hour window left (resets 15:10).',
    );
  });
});

describe('footerUsage', () => {
  const fresh = (): Record<string, AccountUsageSnapshot> => ({
    [ADMIN.uuid]: snapshot(ADMIN.uuid, { source_host: 'mefistos', fetched_at: NOW - 3 * MIN }),
    [WORK.uuid]: snapshot(WORK.uuid, { source_host: 'claude-fleet-htz' }),
    [GMAIL.uuid]: snapshot(GMAIL.uuid, { source_host: 'claude-fleet-trn', fetched_at: NOW - 1 * MIN }),
  });
  const run = (snaps: Record<string, AccountUsageSnapshot>, now = NOW, hosts = hostsAll) =>
    footerUsage(hosts, fleetAccounts(), snaps, now, L, TZ)!;

  it('all accounts fresh and ok: ✓ with the age of the oldest check', () => {
    const f = run(fresh());
    expect(f.state).toBe('ok');
    expect(f.text).toBe('usage ✓ all accounts · 3m');
    expect(f.tone).toBe('normal');
    expect(f.ariaLabel).toContain('all 3 accounts have headroom; oldest check 3 min ago');
  });

  it('an account no host is logged in to never affects the footer', () => {
    // SPARE has no host; even a dead snapshot for it changes nothing.
    const snaps = { ...fresh(), [SPARE.uuid]: snapshot(SPARE.uuid, { usage: null, fetched_at: null, status: 'no_online_host' }) };
    expect(run(snaps).text).toBe('usage ✓ all accounts · 3m');
  });

  it('otherwise the worst account: highest severity, then fewest % left', () => {
    const snaps = {
      ...fresh(),
      [ADMIN.uuid]: fiveLeft(ADMIN.uuid, 30, { source_host: 'mefistos' }), // caution
      [GMAIL.uuid]: fiveLeft(GMAIL.uuid, 8, { source_host: 'claude-fleet-trn' }), // low
      [WORK.uuid]: fiveLeft(WORK.uuid, 12, { source_host: 'claude-fleet-htz' }), // low, more left
    };
    const f = run(snaps);
    expect(f.state).toBe('attention');
    expect(f.text).toBe('usage ▲ mj.janci@gmail.com 5h 8% left · resets 15:10');
    expect(f.tone).toBe('alarm');
    expect(f.host).toBe('claude-fleet-trn');
    expect(f.ariaLabel).toBe(
      'Account usage: mj.janci@gmail.com has 8% of its 5-hour window left, resets in 38 min (15:10). Open Hosts on claude-fleet-trn.',
    );
  });

  it('a limit outranks low and uses the LIMIT wording; the nickname labels it', () => {
    const accounts = fleetAccounts().map((a) => (a.uuid === ADMIN.uuid ? { ...a, nickname: 'admin' } : a));
    const snaps = { ...fresh(), [ADMIN.uuid]: fiveLeft(ADMIN.uuid, 0), [GMAIL.uuid]: fiveLeft(GMAIL.uuid, 8) };
    const f = footerUsage(hostsAll, accounts, snaps, NOW, L, TZ)!;
    expect(f.text).toBe('usage ■ admin 5h LIMIT · resets 15:10');
  });

  it('an account with no online host cannot raise the alarm or outrank a real low account', () => {
    const offline = snapshot(WORK.uuid, { usage: null, fetched_at: null, status: 'no_online_host', source_host: 'claude-fleet-htz' });
    // Everything else fine: no alarm glyph, no alarm tone.
    const calm = run({ ...fresh(), [WORK.uuid]: offline });
    expect(calm.state).not.toBe('unavailable');
    expect(calm.tone).toBe('normal');
    expect(calm.text).not.toMatch(/[▲■△]/);
    expect(calm.text).not.toContain('m.janci@32bit.sk');
    // A real low account elsewhere is what the footer names.
    const low = run({ ...fresh(), [WORK.uuid]: offline, [ADMIN.uuid]: fiveLeft(ADMIN.uuid, 8, { source_host: 'mefistos' }) });
    expect(low.text).toBe('usage ▲ admin@32bit.sk 5h 8% left · resets 15:10');
    expect(low.host).toBe('mefistos');
  });

  it('every account unavailable: ◷ unavailable since the last good check', () => {
    const f = run(outageUsage());
    expect(f.state).toBe('unavailable');
    expect(f.text).toBe('usage ◷ unavailable since 13:10');
    expect(f.ariaLabel).toBe('Account usage unavailable since 13:10. Open Hosts.');
    expect(f.host).not.toBeNull();
  });

  it('unavailable everywhere, but a still-showable low number is named rather than hidden', () => {
    const snaps = { ...outageUsage(), [ADMIN.uuid]: fiveLeft(ADMIN.uuid, 8, { status: 'unavailable', fetched_at: NOW - 14 * MIN }) };
    expect(run(snaps).text).toBe('usage ▲ admin@32bit.sk 5h ~8% left · resets 15:10');
  });

  it('collapses to a muted `usage off` after 24 hours unavailable', () => {
    const justUnder = run(outageUsage(), NOW - 82 * MIN + 24 * HOUR);
    expect(justUnder.state).toBe('unavailable');
    const f = run(outageUsage(), NOW - 82 * MIN + 24 * HOUR + 1);
    expect(f.state).toBe('off');
    expect(f.text).toBe('usage off');
    expect(f.tone).toBe('muted');
  });

  it('before the first fetch: checking…', () => {
    const never = (uuid: string) => snapshot(uuid, { usage: null, fetched_at: null, status: 'never_fetched' });
    const f = run({ [ADMIN.uuid]: never(ADMIN.uuid), [WORK.uuid]: never(WORK.uuid), [GMAIL.uuid]: never(GMAIL.uuid) });
    expect(f.state).toBe('checking');
    expect(f.text).toBe('usage checking…');
  });

  it('null when no host is logged in to any account', () => {
    expect(footerUsage([host('nas')], fleetAccounts(), fleetUsage(), NOW)).toBeNull();
  });

  it('opens on the account’s polling host, else an online host', () => {
    const snaps = { ...fresh(), [ADMIN.uuid]: fiveLeft(ADMIN.uuid, 8, { source_host: null }) };
    expect(run(snaps).host).toBe('claude-fleet-oci');
  });
});

describe('compactAge', () => {
  it('rounds down to one unit', () => {
    expect(compactAge(20)).toBe('<1m');
    expect(compactAge(3 * MIN + 59)).toBe('3m');
    expect(compactAge(2 * HOUR + 5 * MIN)).toBe('2h');
    expect(compactAge(26 * HOUR)).toBe('1d');
  });
});
