// The New session dialog's agent picker (Orbit Fleet redesign step 12.4):
// which agents a session can be started with on a host. An agent is offered
// only when fleet can launch it (`normalize_agent` in
// crates/fleet-core/src/service/sessions/lifecycle.rs accepts it) and the
// host has its CLI on the PATH (`agents_on_path`, sampled by the health
// probe). Claude Code stays offered everywhere, as it always was: fleet
// installs and repairs it, so a host without it is a setup problem, not a
// reason to hide the default.
import type { HostRow } from './hosts';

/** The agents the picker lists, in its order. */
export type PickerAgent = 'claude' | 'codex' | 'agy';

/** Agents fleet can start today. Agy has an adapter but no launch yet. */
export const STARTABLE_AGENTS: readonly PickerAgent[] = ['claude', 'codex'];

export interface AgentChoice {
  agent: PickerAgent;
  label: string;
  enabled: boolean;
  /** Why it is greyed out, for the button's title and its small tag. */
  reason: string | null;
  /** The tag beside a greyed-out name: "coming" or "not on <host>". */
  tag: string | null;
}

const LABELS: Record<PickerAgent, string> = { claude: 'Claude Code', codex: 'Codex', agy: 'Agy' };

/** The picker's agents for `host` (by alias, as the dialog chose it). */
export function agentChoices(hostAlias: string, host: Pick<HostRow, 'agents_on_path'> | undefined): AgentChoice[] {
  const onPath = host?.agents_on_path ?? null;
  return (['claude', 'codex', 'agy'] as const).map((agent) => {
    const label = LABELS[agent];
    if (agent === 'claude') return { agent, label, enabled: true, reason: null, tag: null };
    if (!STARTABLE_AGENTS.includes(agent)) {
      return { agent, label, enabled: false, reason: `${label} sessions are coming`, tag: 'coming' };
    }
    if (onPath === null) {
      return {
        agent,
        label,
        enabled: false,
        reason: `Not checked on ${hostAlias} yet: its next health check says whether ${label} is installed`,
        tag: `not on ${hostAlias}`,
      };
    }
    if (!onPath.includes(agent)) {
      return {
        agent,
        label,
        enabled: false,
        reason: `${label} is not on ${hostAlias}'s PATH`,
        tag: `not on ${hostAlias}`,
      };
    }
    return { agent, label, enabled: true, reason: null, tag: null };
  });
}

/** The agent to keep when the host changes: the same one while it is still
 *  offered there, else Claude Code. */
export function keepAgent(current: PickerAgent, choices: readonly AgentChoice[]): PickerAgent {
  return choices.find((c) => c.agent === current)?.enabled ? current : 'claude';
}
