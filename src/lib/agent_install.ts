// The fleet-agent install job (Orbit Fleet 4.9; `service::agent_install`):
// a hub installs its agent on a host it reaches over SSH today, then moves
// the host onto it, step by step (target, download, start, connect). Only a
// hub accepts agents, so the desktop offers it when paired (the commands
// route to the hub's `install_agent` / `agent_installs`), and the add-host
// wizard offers it on its fleet-agent row when the checks said a hub could
// use one. Nothing installs without a person's click.
import { invokeCmd, type Result } from './result';

/** Mirrors `store::AgentInstallRow`. */
export interface AgentInstall {
  id: number;
  host_alias: string;
  version: string;
  /** `running` | `done` | `failed` */
  state: string;
  /** `target` | `download` | `start` | `connect` | `done` */
  step: string;
  detail?: string | null;
  started_at: number;
  finished_at?: number | null;
}

/** Start the job; it runs on, and `agentInstalls` follows it. The version
 *  defaults to the hub's own. */
export function installAgent(alias: string, version?: string | null): Promise<Result<AgentInstall>> {
  return invokeCmd<AgentInstall>('install_agent', {
    args: version ? { alias, version } : { alias },
  });
}

/** One host's jobs, newest first. */
export function agentInstalls(alias: string): Promise<Result<AgentInstall[]>> {
  return invokeCmd<AgentInstall[]>('agent_installs', { args: { alias } });
}

const STEP_TEXT: Record<string, string> = {
  target: 'Reading which build fits',
  download: 'Downloading and checking it against SHA256SUMS',
  start: 'Starting fleet-agent',
  connect: 'Waiting for its first hello',
};

/** What a running job is doing, for the line beside its loader:
 *  "Installing fleet-agent 0.5.4 on mercury: Downloading…". */
export function installStepText(j: Pick<AgentInstall, 'host_alias' | 'version' | 'step'>): string {
  const what = STEP_TEXT[j.step] ?? j.step;
  return `Installing fleet-agent ${j.version} on ${j.host_alias}: ${what}…`;
}

/** The newest job when it is still running, else null. */
export function runningInstall(jobs: readonly AgentInstall[] | null | undefined): AgentInstall | null {
  const j = jobs?.[0];
  return j && j.state === 'running' ? j : null;
}

/** How often a running job is re-read, ms. */
export const INSTALL_POLL_MS = 2000;
