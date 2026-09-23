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
      friendly: true,
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
      friendly: true,
    });
    expect(c?.chipLabel).toBe('the payments one · mefistos');
  });

  // One name policy for the whole app: `displayName` in attention.ts. With the
  // preference off, the sidebar row and the terminal header show the tmux name
  // — the chip and the prompt prefix must not go on naming the session by the
  // friendly name the user has told the app not to show.
  it('obeys the show-friendly-names preference, in the chip AND in the prefix', () => {
    const s = session({ friendly_name: 'the payments one' });
    const off = agentContext({ view: 'terminal', session: s, hostAlias: 'mefistos', branch: null, friendly: false });
    expect(off?.chipLabel).toBe('blue-sirius · mefistos');
    expect(off?.prefix).toContain('blue-sirius');
    expect(off?.prefix).not.toContain('the payments one');

    const on = agentContext({ view: 'terminal', session: s, hostAlias: 'mefistos', branch: null, friendly: true });
    expect(on?.prefix).toContain('the payments one');
  });

  it('falls back to the tmux name when there is no friendly name to show', () => {
    const c = agentContext({ view: 'terminal', session: session(), hostAlias: 'mefistos', branch: null, friendly: true });
    expect(c?.chipLabel).toBe('blue-sirius · mefistos');
  });

  it('in the Hosts view the context is the host, not a session', () => {
    const c = agentContext({ view: 'hosts', session: null, hostAlias: 'hetzner', branch: null, friendly: true });
    expect(c?.chipLabel).toBe('hetzner');
    expect(c?.prefix).toContain('hetzner');
  });

  it('with nothing selected there is no chip and no prefix to remove', () => {
    expect(agentContext({ view: 'terminal', session: null, hostAlias: null, branch: null, friendly: true })).toBeNull();
    expect(agentContext({ view: 'hosts', session: null, hostAlias: null, branch: null, friendly: true })).toBeNull();
  });
});
