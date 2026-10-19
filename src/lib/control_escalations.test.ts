import { describe, it, expect } from 'vitest';
import { controlSessionIds, escalations, escalationKey, loginCardText, pruneLater, LATER_KEPT, LATER_MAX_AGE_SECS } from './control_escalations';
import type { ControlHandoff } from './handoffs';
import type { HostRow } from './hosts';
import type { SessionRow } from './sessions';
import type { AttentionOptions } from './attention';

const opts: AttentionOptions = { idleSecs: 3600, now: 1_800_000_000 };
const row = (over: Partial<SessionRow>): SessionRow =>
  ({
    id: 1,
    tmux_name: 's',
    friendly_name: null,
    host_alias: 'alpha',
    kind: 'claude',
    status: 'running',
    claude_status: 'idle',
    lost_at: null,
    stuck_kind: null,
    pending_input: null,
    turn_seq: 3,
    ...over,
  }) as SessionRow;

describe('escalations', () => {
  it('a session asking a question is a card that names the question; Control is never one', () => {
    const asking = row({
      id: 2,
      tmux_name: 'fed-v2',
      claude_status: 'blocked',
      pending_input: { kind: 'permission', question: 'Do you want to proceed?', options: [], detail: 'Bash(git push)' },
    });
    const control = row({ id: 9, tmux_name: 'fleet-operator', claude_status: 'blocked' });
    const out = escalations([asking, control, row({ id: 3 })], opts, 9, new Set());
    expect(out).toHaveLength(1);
    expect(out[0].line).toBe('fed-v2 is waiting for your answer');
    expect(out[0].detail).toBe('Bash(git push)');
  });

  it('Later puts a card off until the session needs you again', () => {
    const asking = row({ id: 2, claude_status: 'blocked' });
    const later = new Set([escalationKey(asking, 'waiting')]);
    expect(escalations([asking], opts, null, later)).toEqual([]);
    expect(escalations([{ ...asking, turn_seq: 4 }], opts, null, later)).toHaveLength(1);
  });

  it('a failed session is a card, after the ones waiting on you', () => {
    const out = escalations([row({ id: 4, tmux_name: 'b', claude_status: 'failed' }), row({ id: 2, tmux_name: 'a', claude_status: 'blocked' })], opts, null, new Set());
    expect(out.map((e) => e.bucket)).toEqual(['waiting', 'stop_failed']);
  });

  it("Control's own limit is a card in its own chat (review F7); its other states are not", () => {
    const limited: AttentionOptions = { ...opts, facts: { limited_accounts: { acct: { window: 'five_hour', resets_at: null } } } };
    const control = row({ id: 9, tmux_name: 'fleet-operator', account_uuid: 'acct' } as Partial<SessionRow>);
    const out = escalations([control], limited, 9, new Set());
    expect(out).toHaveLength(1);
    expect(out[0].bucket).toBe('account_limit');
    expect(out[0].line).toMatch(/^Control's account hit its limit/);
    expect(escalations([control], limited, 9, new Set([out[0].key]))).toEqual([]);
  });

  it('a signed-out login is one card per host and login, with the command (review F6)', () => {
    // 2026-08-09 12:00 UTC.
    const since = 1_786_276_800;
    const host = { alias: 'htz', hidden: false, signed_out_logins: [{ profile: null, since }, { profile: 'work', since }] } as unknown as HostRow;
    const hidden = { alias: 'gone', hidden: true, signed_out_logins: [{ profile: null, since }] } as unknown as HostRow;
    const out = escalations([], opts, null, new Set(), [host, hidden]);
    expect(out.map((e) => e.line)).toEqual([
      'Claude login expired on htz (since Aug 9) · ssh htz then claude /login',
      'Claude login expired on htz (profile work) (since Aug 9) · ssh htz then CLAUDE_CONFIG_DIR=~/.claude-profiles/work claude /login',
    ]);
    expect(out.every((e) => e.session === null)).toBe(true);
    expect(escalations([], opts, null, new Set([out[0].key]), [host])).toHaveLength(1);
    expect(loginCardText('htz', { profile: null, since })).toContain('(since Aug 9)');
  });

  it("by default only Control's threads raise a card; null scope is the whole fleet", () => {
    const mine = row({ id: 2, tmux_name: 'mine', claude_status: 'blocked', origin: 'operator' } as Partial<SessionRow>);
    const handed = row({ id: 5, tmux_name: 'handed', claude_status: 'failed' });
    const worker = row({ id: 6, tmux_name: 'worker', claude_status: 'failed', origin: 'mission', origin_ref: '11' } as Partial<SessionRow>);
    const stranger = row({ id: 3, tmux_name: 'other', claude_status: 'blocked' });
    const receipts = [
      { kind: 'session', session_id: 5, mission_id: null },
      { kind: 'mission', session_id: null, mission_id: 11 },
    ] as unknown as ControlHandoff[];
    const scope = controlSessionIds([mine, handed, worker, stranger], receipts);
    expect([...scope].sort()).toEqual([2, 5, 6]);
    const rows = [mine, handed, worker, stranger];
    expect(escalations(rows, opts, null, new Set(), [], scope).map((e) => e.session?.id).sort()).toEqual([2, 5, 6]);
    expect(escalations(rows, opts, null, new Set(), [], null)).toHaveLength(4);
  });
});

describe("Control's threads and the hub's follow state (F10)", () => {
  const blocked = (id: number, extra: Partial<SessionRow> = {}) => row({ id, tmux_name: `s${id}`, claude_status: 'blocked', ...extra } as Partial<SessionRow>);
  const rows = [
    blocked(2, { origin: 'operator' } as Partial<SessionRow>),
    blocked(5),
    blocked(6, { origin: 'mission', origin_ref: '11' } as Partial<SessionRow>),
    blocked(7),
    blocked(8, { origin: 'mission', origin_ref: '12' } as Partial<SessionRow>),
  ];
  const receipts = [
    { kind: 'session', session_id: 5, mission_id: null },
    { kind: 'session', session_id: 7, mission_id: null },
    { kind: 'mission', session_id: null, mission_id: 11 },
    { kind: 'mission', session_id: null, mission_id: 12 },
  ] as unknown as ControlHandoff[];

  it('leaves out a muted thread, one no longer followed, and the workers of a muted mission', () => {
    const follows = new Map([
      ['s:2', { state: 'following', muted: false }],
      ['s:5', { state: 'muted', muted: true }],
      ['s:7', { state: 'done', muted: false }],
      ['m:11', { state: 'following', muted: false }],
      ['m:12', { state: 'muted', muted: true }],
    ]);
    const scope = controlSessionIds(rows, receipts, follows);
    expect([...scope].sort()).toEqual([2, 6]);
    expect(escalations(rows, opts, null, new Set(), [], scope).map((e) => e.session?.id).sort()).toEqual([2, 6]);
    // All fleet still shows every one of them.
    expect(escalations(rows, opts, null, new Set(), [], null)).toHaveLength(5);
  });

  it('keeps every thread when the hub has not said (an older hub, no read yet)', () => {
    expect([...controlSessionIds(rows, receipts, null)].sort()).toEqual([2, 5, 6, 7, 8]);
    expect([...controlSessionIds(rows, receipts, new Map())].sort()).toEqual([2, 5, 6, 7, 8]);
  });
});

describe('pruneLater', () => {
  it('forgets a put-off card after a week and keeps the newest few hundred', () => {
    const now = 10_000_000;
    expect(pruneLater([['old', now - LATER_MAX_AGE_SECS], ['new', now - 60]], now)).toEqual([['new', now - 60]]);
    const many = Array.from({ length: LATER_KEPT + 5 }, (_, i) => [`k${i}`, now] as [string, number]);
    const kept = pruneLater(many, now);
    expect(kept).toHaveLength(LATER_KEPT);
    expect(kept[0][0]).toBe('k5');
  });
});

describe('Later', () => {
  it('survives a restart: a fresh module reads it back from prefs', async () => {
    const { vi } = await import('vitest');
    vi.resetModules();
    const first = await import('./control_escalations');
    first.putOff('7:waiting:3');
    vi.resetModules();
    const again = await import('./control_escalations');
    const { get } = await import('svelte/store');
    expect(get(again.laterEscalations).has('7:waiting:3')).toBe(true);
  });
});

describe('a host-down card put off with Later', () => {
  const hostRow = (alias: string, reachable: boolean) => ({ alias, reachable, hidden: false }) as unknown as HostRow;

  it('is forgotten once the host is up again, so its next outage raises a card', async () => {
    const { vi } = await import('vitest');
    vi.resetModules();
    const m = await import('./control_escalations');
    const { get } = await import('svelte/store');
    m.putOff('host:mac');
    m.putOff('7:waiting:3');
    m.forgetRecoveredHosts([hostRow('mac', false)]);
    expect(get(m.laterEscalations).has('host:mac')).toBe(true);
    m.forgetRecoveredHosts([hostRow('mac', true)]);
    expect(get(m.laterEscalations).has('host:mac')).toBe(false);
    expect(get(m.laterEscalations).has('7:waiting:3')).toBe(true);
  });

  it('dropRecoveredHosts keeps every other card', async () => {
    const { dropRecoveredHosts } = await import('./control_escalations');
    const entries: [string, number][] = [['host:mac', 1], ['host:htz', 2], ['login:mac::5', 3]];
    expect(dropRecoveredHosts(entries, [hostRow('mac', true), hostRow('htz', false)])).toEqual([['host:htz', 2], ['login:mac::5', 3]]);
  });
});
