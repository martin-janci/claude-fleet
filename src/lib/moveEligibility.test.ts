import { describe, it, expect } from 'vitest';
import { canMoveSession, moveTargetsFor } from './moveEligibility';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';

const s = (over: Partial<SessionRow>) =>
  ({ id: 1, host_alias: 'a', kind: 'work', worktree_id: 10, claude_session_id: 'x', ...over }) as SessionRow;
const h = (alias: string, over: Partial<HostRow> = {}) =>
  ({ alias, ssh_alias: alias, reachable: true, hidden: false, provisioned: true, transport: 'ssh', ...over }) as HostRow;

describe('canMoveSession', () => {
  it('needs a work session with a worktree and a Claude session id', () => {
    expect(canMoveSession(s({}))).toBe(true);
    expect(canMoveSession(s({ kind: 'shell' }))).toBe(false);
    expect(canMoveSession(s({ worktree_id: null }))).toBe(false);
    expect(canMoveSession(s({ claude_session_id: null }))).toBe(false);
  });
});

describe('moveTargetsFor', () => {
  it('offers other visible, reachable hosts that are provisioned or local', () => {
    const hosts = [
      h('a'), h('b'), h('down', { reachable: false }), h('hid', { hidden: true }),
      h('bare', { provisioned: false }), h('local', { provisioned: false }),
    ];
    expect(moveTargetsFor(s({}), hosts).map((x) => x.alias)).toEqual(['b', 'local']);
  });
});
