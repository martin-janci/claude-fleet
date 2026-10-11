import { describe, it, expect } from 'vitest';
import {
  hostTidyHint,
  moveTargetFacts,
  accountsSignedIn,
  agentCell,
  connectionText,
  diskCell,
  machineLine,
  sessionsCell,
  sizeText,
  tableOrder,
} from './hosts_table';
import { ADMIN, GMAIL, NOW, WORK, fleetAccounts, host, session } from './hosts_fixture';

const GB = 1024 * 1024;
const byUuid = new Map(fleetAccounts().map((a) => [a.uuid, a]));
const opts = { idleSecs: 0, now: NOW };

describe('hosts table: Connection', () => {
  it('reads local, SSH or agent with the round trip, and offline', () => {
    expect(connectionText(host('local', { latency_ms: null }))).toBe('local');
    expect(connectionText(host('mercury', { latency_ms: 18 }))).toBe('SSH · 18 ms');
    expect(connectionText(host('trn', { transport: 'agent', latency_ms: 44 }))).toBe('agent · 44 ms');
    expect(connectionText(host('nas', { latency_ms: null }))).toBe('SSH');
    expect(connectionText(host('htz', { reachable: false, latency_ms: 18 }))).toBe('offline');
  });
});

describe('hosts table: Sessions', () => {
  it('counts the total, who needs you and what failed; external rows are not counted', () => {
    const rows = [
      session('trn', 'a'),
      session('trn', 'b', { claude_status: 'blocked' }),
      session('trn', 'c', { claude_status: 'failed' }),
      session('trn', 'd', { kind: 'external', claude_status: 'blocked' }),
      session('mefistos', 'e', { claude_status: 'blocked' }),
    ];
    expect(sessionsCell('trn', rows, opts)).toEqual({ total: 3, needsYou: 1, failed: 1, text: '3 · 1 needs you · 1 failed' });
    expect(sessionsCell('nas', rows, opts).text).toBe('0');
  });
});

describe('hosts table: Disk', () => {
  it('shows used of total, with the disk meter levels', () => {
    expect(diskCell(host('mac', { disk_home_total_kb: 994 * GB, disk_home_free_kb: 582 * GB }))).toEqual({
      text: '412 GB of 994 GB',
      level: 'ok',
    });
    expect(diskCell(host('trn', { disk_home_total_kb: 100 * GB, disk_home_free_kb: 4 * GB })).level).toBe('crit');
    expect(diskCell(host('x', { disk_home_total_kb: null, disk_home_free_kb: null }))).toEqual({ text: '—', level: null });
  });

  it('sizes in MB, GB and TB', () => {
    expect(sizeText(512 * 1024)).toBe('512 MB');
    expect(sizeText(1.2 * 1024 * GB)).toBe('1.2 TB');
    expect(sizeText(5.5 * GB)).toBe('5.5 GB');
  });
});

describe('hosts table: Agent', () => {
  it('shows the Claude Code version and flags one older than the newest in the fleet', () => {
    expect(agentCell(host('a', { claude_version: '2.1.145 (Claude Code)' }), '2.1.150')).toEqual({ text: '2.1.145', update: true });
    expect(agentCell(host('b', { claude_version: '2.1.150' }), '2.1.150')).toEqual({ text: '2.1.150', update: false });
    expect(agentCell(host('c', { claude_version: null }), '2.1.150')).toEqual({ text: '—', update: false });
  });
});

describe('hosts table: Accounts signed in', () => {
  it("lists the host's login, then each profile's, by short label and once each", () => {
    const h = host('mac', {
      account_uuid: WORK.uuid,
      claude_profiles: [
        { name: 'admin', account_uuid: ADMIN.uuid },
        { name: 'again', account_uuid: WORK.uuid },
        { name: 'fresh', account_uuid: null },
        { name: 'unknown', account_uuid: 'acc-elsewhere', email: 'x@example.com' },
      ],
    });
    expect(accountsSignedIn(h, byUuid)).toEqual(['m-janci', 'admin-janci', 'x']);
    expect(accountsSignedIn(host('nas', { account_uuid: null }), byUuid)).toEqual([]);
  });

  it('uses the nickname when the account has one', () => {
    const m = new Map(byUuid);
    m.set(GMAIL.uuid, { ...GMAIL, nickname: 'personal' });
    expect(accountsSignedIn(host('local', { account_uuid: GMAIL.uuid }), m)).toEqual(['personal']);
  });
});

describe('hosts table: probe facts', () => {
  it('shows CPUs, memory, worktree size and boot time, only what the host answered', () => {
    const h = host('mercury', {
      cpu_count: 16,
      mem_total_kb: 64 * GB,
      worktree_kb: 9 * GB,
      boot_at: NOW - 3 * 86400 - 60,
    });
    expect(machineLine(h, NOW)).toBe('16 CPU · 64 GB RAM · worktrees 9.0 GB · booted 3d ago');
    expect(machineLine(host('old-hub'), NOW)).toBe('');
  });
});

describe('hosts table: order', () => {
  it('puts local first, then reachable hosts, then offline ones, each by alias', () => {
    const order = tableOrder([
      host('zeta'),
      host('beta', { reachable: false }),
      host('local'),
      host('alpha'),
    ]).map((h) => h.alias);
    expect(order).toEqual(['local', 'alpha', 'zeta', 'beta']);
  });
});

describe('Tidy hint and Move to host facts (G4.5)', () => {
  it('sums the measured worktrees of one host', () => {
    const cands = [
      { session_id: 1, host_alias: 'mercury', worktree_kb: 2 * 1024 * 1024 },
      { session_id: 2, host_alias: 'mercury', worktree_kb: 1024 * 1024 },
      { session_id: 3, host_alias: 'mercury', worktree_kb: null },
      { session_id: 4, host_alias: 'venus', worktree_kb: 1024 * 1024 },
    ];
    expect(hostTidyHint(cands, 'mercury')).toEqual({
      sessionIds: [1, 2],
      kb: 3 * 1024 * 1024,
      text: '2 stopped sessions on mercury hold 3.0 GB of worktrees.',
    });
    expect(hostTidyHint(cands, 'venus')?.text).toBe('1 stopped session on venus holds 1.0 GB of worktrees.');
    expect(hostTidyHint(cands, 'pluto')).toBeNull();
  });

  it('says free disk, load and an account at its limit', () => {
    const h = host('mercury', { account_uuid: WORK.uuid, disk_home_free_kb: 50 * 1024 * 1024, load_1m: 0.42 });
    expect(moveTargetFacts(h, {}, NOW)).toBe('50 GB free · load 0.4');
    expect(moveTargetFacts(h, { [WORK.uuid]: { resets_at: NOW + 60 } }, NOW)).toBe(
      '50 GB free · load 0.4 · account at limit',
    );
    expect(moveTargetFacts(h, { [WORK.uuid]: { resets_at: NOW - 60 } }, NOW)).toBe('50 GB free · load 0.4');
    expect(moveTargetFacts(host('x', { disk_home_free_kb: null, load_1m: null }), undefined, NOW)).toBe('');
  });

  it('leads with the latency the last probe measured (M15 G7.12)', () => {
    const h = host('mercury', { latency_ms: 18, disk_home_free_kb: 50 * 1024 * 1024, load_1m: null });
    expect(moveTargetFacts(h, {}, NOW)).toBe('18 ms · 50 GB free');
  });
});
