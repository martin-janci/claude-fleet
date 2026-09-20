import { describe, it, expect } from 'vitest';
import { agentContext } from './agent_context';
import type { SessionRow } from './sessions';

const session = (over: Partial<SessionRow> = {}) =>
  ({
    id: 7,
    tmux_name: 'blue-sirius',
    host_alias: 'mefistos',
    friendly_name: null,
    kind: 'work',
    ...over,
  }) as SessionRow;

describe('agentContext', () => {
  it('a selected session is host, name and branch — what "this one" means', () => {
    const c = agentContext({
      view: 'terminal',
      session: session(),
      hostAlias: 'mefistos',
      branch: 'feature/x',
    });
    expect(c?.chipLabel).toBe('blue-sirius · mefistos · feature/x');
    expect(c?.prefix).toContain('blue-sirius');
    expect(c?.prefix).toContain('mefistos');
    expect(c?.prefix).toContain('feature/x');
  });

  it('prefers the friendly name, because that is what the person sees', () => {
    const c = agentContext({
      view: 'terminal',
      session: session({ friendly_name: 'the payments one' }),
      hostAlias: 'mefistos',
      branch: null,
    });
    expect(c?.chipLabel).toBe('the payments one · mefistos');
  });

  it('in the Hosts view the context is the host, not a session', () => {
    const c = agentContext({ view: 'hosts', session: null, hostAlias: 'hetzner', branch: null });
    expect(c?.chipLabel).toBe('hetzner');
    expect(c?.prefix).toContain('hetzner');
  });

  it('with nothing selected there is no chip and no prefix to remove', () => {
    expect(agentContext({ view: 'terminal', session: null, hostAlias: null, branch: null })).toBeNull();
    expect(agentContext({ view: 'hosts', session: null, hostAlias: null, branch: null })).toBeNull();
  });
});
