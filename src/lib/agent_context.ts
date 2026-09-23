// What the agent is told about where you are standing, and what the composer
// shows you of it. Pure on purpose: the rules for "what does 'this one' mean
// in this view" will accumulate here, and they are cheaper to get right in a
// test than in a component.
import { displayName } from './attention';
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
  /**
   * The `showFriendlyNames` preference. Passed in rather than read out of the
   * store here, so this module stays pure and unit-testable and the app keeps
   * ONE name policy (`displayName` in attention.ts) instead of a third
   * private one.
   *
   * REQUIRED on purpose. This field exists because the chip and the prompt
   * prefix used to name the session unconditionally by its friendly name
   * (UX-129 / round-20 F12), contradicting the sidebar row and the terminal
   * header whenever the preference was off. An optional field with a default
   * would let the next caller reintroduce exactly that divergence silently;
   * a required one makes the compiler ask. Production passes
   * `$showFriendlyNames` from App.svelte.
   */
  friendly: boolean;
}

export function agentContext(input: AgentContextInput): AgentContext | null {
  if (input.session) {
    // The same name the sidebar row and the terminal header show. Naming the
    // session by a friendly name the user has switched off would leave the
    // chip and the prompt prefix talking about something they cannot see.
    const name = displayName(input.session, input.friendly);
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
