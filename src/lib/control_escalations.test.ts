import { describe, it, expect } from 'vitest';
import { escalations, escalationKey, loginCardText } from './control_escalations';
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
});
