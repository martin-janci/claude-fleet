// What the agent is told about where you are standing, and what the composer
// shows you of it. Pure on purpose: the rules for "what does 'this one' mean
// in this view" will accumulate here, and they are cheaper to get right in a
// test than in a component.
import type { SessionRow } from './sessions';

export interface AgentContext {
  /** What the removable chip reads. */
  chipLabel: string;
  /** What is actually prepended to the prompt. */
  prefix: string;
}

export interface AgentContextInput {
  view: 'terminal' | 'hosts' | 'files';
  session: SessionRow | null;
  hostAlias: string | null;
  branch: string | null;
}

export function agentContext(input: AgentContextInput): AgentContext | null {
  if (input.session) {
    const name = input.session.friendly_name || input.session.tmux_name;
    const parts = [name, input.session.host_alias];
    if (input.branch) parts.push(input.branch);
    return {
      chipLabel: parts.join(' · '),
      prefix:
        `[context] the person is looking at session "${name}" on host ` +
        `${input.session.host_alias}` +
        (input.branch ? ` (branch ${input.branch})` : '') +
        `. "this one" and "here" mean that session unless they say otherwise.`,
    };
  }
  if (input.hostAlias) {
    return {
      chipLabel: input.hostAlias,
      prefix:
        `[context] the person is looking at host ${input.hostAlias} with no ` +
        `session selected. "here" means that host.`,
    };
  }
  return null;
}
