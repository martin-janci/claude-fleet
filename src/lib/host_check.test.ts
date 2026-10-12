import { describe, it, expect } from 'vitest';
import { basePathLine, checklistLoaderText, checklistRows, needsReprovision, type HostCheck } from './host_check';
import type { AssetInventoryRow } from './assets';
import { host } from './hosts_fixture';

function check(over: Partial<HostCheck> = {}): HostCheck {
  return {
    alias: 'mercury',
    checked_at: 1,
    error: null,
    tmux_version: '3.5a',
    agents_on_path: ['claude', 'codex'],
    fleet_hooks: true,
    guard_hook: true,
    ...over,
  };
}

function skill(name: string, state: string, alias = 'mercury'): AssetInventoryRow {
  return {
    host_alias: alias,
    harness: 'claude',
    kind: 'skill',
    name,
    state,
    catalog_hash: null,
    host_hash: null,
    scanned_at: 1,
    managed: true,
  };
}

const rows = (args: Partial<Parameters<typeof checklistRows>[0]> = {}) =>
  checklistRows({ host: host('mercury'), check: check(), inventory: [], hubVersion: null, ...args });
const row = (rs: ReturnType<typeof checklistRows>, key: string) => rs.find((r) => r.key === key)!;

describe('host health checklist', () => {
  it('lists SSH, tmux, fleet-agent, agents on PATH, hooks, guard and skills in the board order', () => {
    expect(rows().map((r) => r.key)).toEqual(['ssh', 'tmux', 'agent', 'agents', 'hooks', 'guard', 'skills']);
  });

  it('a healthy SSH host checks out', () => {
    const rs = rows({ inventory: [skill('a', 'in_sync'), skill('b', 'in_sync'), skill('c', 'drifted', 'other')] });
    expect(row(rs, 'ssh')).toMatchObject({ state: 'ok', detail: 'answered' });
    expect(row(rs, 'tmux')).toMatchObject({ state: 'ok', detail: '3.5a' });
    expect(row(rs, 'agent')).toMatchObject({ state: 'na' });
    expect(row(rs, 'agents')).toMatchObject({ state: 'ok', detail: 'Claude Code, Codex' });
    expect(row(rs, 'skills')).toMatchObject({ state: 'ok', detail: '2 in sync' });
    expect(needsReprovision(rs, host('mercury'))).toBe(false);
  });

  it('a failing guard hook fails its row and asks for a re-provision', () => {
    const rs = rows({ check: check({ guard_hook: false }) });
    expect(row(rs, 'guard')).toMatchObject({ state: 'fail', detail: 'not installed · re-provision to fix' });
    expect(row(rs, 'hooks').state).toBe('ok');
    expect(needsReprovision(rs, host('mercury'))).toBe(true);
  });

  it('no settings file fails both hook rows', () => {
    const rs = rows({ check: check({ fleet_hooks: null, guard_hook: null }) });
    expect(row(rs, 'hooks').state).toBe('fail');
    expect(row(rs, 'guard').detail).toMatch(/no ~\/\.claude\/settings\.json/);
  });

  it('no Claude Code on PATH fails, whatever else is there', () => {
    expect(row(rows({ check: check({ agents_on_path: ['codex'] }) }), 'agents')).toMatchObject({
      state: 'fail',
      detail: 'Codex · no Claude Code',
    });
    expect(row(rows({ check: check({ agents_on_path: [] }) }), 'agents').detail).toBe('none found');
  });

  it('an unreachable check says why and leaves what it could not read unknown', () => {
    const rs = rows({ check: check({ error: 'ssh: connect timed out', tmux_version: null, agents_on_path: null }) });
    expect(row(rs, 'ssh')).toMatchObject({ state: 'fail', detail: 'ssh: connect timed out' });
    expect(row(rs, 'tmux')).toMatchObject({ state: 'ok', detail: '3.5a' });
    expect(row(rs, 'agents').state).toBe('unknown');
    expect(row(rs, 'guard').state).toBe('unknown');
  });

  it('before any check, the probe facts stand and the rest is unknown', () => {
    const rs = rows({ check: null, host: host('mercury', { latency_ms: 18 }) });
    expect(row(rs, 'ssh')).toMatchObject({ state: 'ok', detail: 'reachable · 18 ms' });
    expect(row(rs, 'hooks').state).toBe('unknown');
    expect(row(rows({ check: null, host: host('mercury', { reachable: false }) }), 'ssh').state).toBe('fail');
  });

  it('a check that found no tmux fails the tmux row', () => {
    expect(row(rows({ check: check({ tmux_version: null }) }), 'tmux')).toMatchObject({ state: 'fail', detail: 'not installed' });
  });

  it('an agent host checks its fleet-agent against the fleet version', () => {
    const agentHost = (v: string | null) => host('trn', { transport: 'agent', agent_version: v });
    expect(row(rows({ host: agentHost('0.5.4'), hubVersion: '0.5.4' }), 'agent').state).toBe('ok');
    expect(row(rows({ host: agentHost('0.5.2'), hubVersion: '0.5.4' }), 'agent')).toMatchObject({
      state: 'warn',
      detail: '0.5.2 · fleet is 0.5.4',
    });
    expect(row(rows({ host: agentHost(null) }), 'agent').state).toBe('fail');
  });

  it('drifted or missing skills warn with counts; an unscanned host is unknown', () => {
    const rs = rows({ inventory: [skill('a', 'drifted'), skill('b', 'missing'), skill('c', 'in_sync')] });
    expect(row(rs, 'skills')).toMatchObject({ state: 'warn', detail: '1 drifted · 1 missing' });
    expect(row(rows(), 'skills')).toMatchObject({ state: 'unknown', detail: 'not scanned' });
  });

  it('a stale provisioning asks for a re-provision too', () => {
    const h = host('mercury', { provision_stale: true });
    expect(needsReprovision(rows({ host: h }), h)).toBe(true);
  });
});

describe('checklistLoaderText (4.13)', () => {
  it('names what runs, re-provisioning first', () => {
    expect(checklistLoaderText('mercury', false, false)).toBeNull();
    expect(checklistLoaderText('mercury', true, false)).toMatch(/^Checking mercury/);
    expect(checklistLoaderText('mercury', true, true)).toMatch(/^Re-provisioning mercury/);
  });
});

describe('basePathLine (M15 G7.12)', () => {
  const answered = (base_path?: HostCheck['base_path'], error: string | null = null): HostCheck => ({
    alias: 'trn',
    checked_at: 1,
    error,
    tmux_version: null,
    agents_on_path: null,
    fleet_hooks: null,
    guard_hook: null,
    base_path,
  });

  it('says whether the user may write there, and which answers stop a save', () => {
    expect(basePathLine('trn', answered({ path: '/srv/work', state: 'unwritable', user: 'dev' }))).toEqual({
      text: '/srv/work is not writable by user dev on trn.',
      problem: true,
    });
    expect(basePathLine('trn', answered({ path: '~/p', state: 'ok', user: 'dev' }))?.problem).toBe(false);
    expect(basePathLine('trn', answered({ path: '~/new', state: 'creatable' }))?.text).toBe(
      '~/new does not exist yet; fleet can create it on trn.',
    );
    expect(basePathLine('trn', answered({ path: '/etc/passwd', state: 'not_dir' }))?.problem).toBe(true);
    expect(basePathLine('trn', answered(undefined))).toBeNull();
    expect(basePathLine('trn', answered(undefined, 'timed out'))?.text).toBe('trn did not answer: timed out');
  });
});
