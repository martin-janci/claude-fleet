import { describe, it, expect } from 'vitest';
import { escalations, escalationKey } from './control_escalations';
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
});
