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
  /** `target` | `tmux` | `download` | `start` | `connect` | `done` */
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
  tmux: 'Checking for tmux (installing it if it is missing)',
  download: 'Downloading and checking it against SHA256SUMS',
  start: 'Starting fleet-agent',
  connect: 'Waiting for its first hello',
};

/** What a running job is doing, for the line beside its loader:
 *  "Installing fleet-agent 0.5.4 on mercury: Downloading…". */
export function installStepText(
  j: Pick<AgentInstall, 'host_alias' | 'version' | 'step'> & { detail?: string | null },
): string {
  // A step this client does not know yet reads the job's own detail
  // ("checking that the host has tmux"), never the raw key.
  const what = STEP_TEXT[j.step] ?? (j.detail || j.step);
  return `Installing fleet-agent ${j.version} on ${j.host_alias}: ${what}…`;
}

/** The newest job when it is still running, else null. */
export function runningInstall(jobs: readonly AgentInstall[] | null | undefined): AgentInstall | null {
  const j = jobs?.[0];
  return j && j.state === 'running' ? j : null;
}

/** The job the action shows on mount: the newest one while it runs, or
 *  when it failed within `FAILED_SHOWN_S`, so a job that failed while the
 *  view was closed (one started from the phone) still says why. */
export function shownInstall(
  jobs: readonly AgentInstall[] | null | undefined,
  nowS: number = Math.floor(Date.now() / 1000),
): AgentInstall | null {
  const j = jobs?.[0];
  if (!j) return null;
  if (j.state === 'running') return j;
  if (j.state === 'failed' && nowS - (j.finished_at ?? j.started_at) <= FAILED_SHOWN_S) return j;
  return null;
}

/** How long a failed job's line stays shown on mount, seconds (a day). */
export const FAILED_SHOWN_S = 24 * 60 * 60;

/** How often a running job is re-read, ms. */
export const INSTALL_POLL_MS = 2000;

/** The re-read delay after `failures` polls in a row failed: doubling from
 *  `INSTALL_POLL_MS`, capped, so a hub-link blip neither freezes the step
 *  nor hammers the link. */
export function installPollDelay(failures: number): number {
  if (failures <= 0) return INSTALL_POLL_MS;
  return Math.min(INSTALL_POLL_MS * 2 ** failures, INSTALL_POLL_MAX_MS);
}

/** The longest gap between re-reads while polls keep failing, ms. */
export const INSTALL_POLL_MAX_MS = 30_000;
