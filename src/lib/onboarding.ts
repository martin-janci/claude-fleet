import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { readPref, writePref } from './prefs';

// ---- Backend-mirrored types -------------------------------------------------

/** Mirrors backend `LocalPrereqs` (service/onboarding.rs). */
export interface LocalPrereqs {
  claude_ok: boolean;
  claude_version: string | null;
  tmux_ok: boolean;
  tmux_version: string | null;
  projects_path: string;
  projects_readable: boolean;
  projects_count: number;
}

export type TunnelState = 'up' | 'down' | 'not_started';
export interface TunnelStatusRow {
  host_alias: string;
  state: TunnelState;
}

// ---- Persisted flags --------------------------------------------------------

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

/** Has the one-time welcome modal been shown. */
export const onboardingWelcomed = writable<boolean>(
  readPref('onboarding-welcomed', false, isBool),
);
onboardingWelcomed.subscribe((v) => writePref('onboarding-welcomed', v));

/** Has the user dismissed the "Get started" card. */
export const onboardingDismissed = writable<boolean>(
  readPref('onboarding-dismissed', false, isBool),
);
onboardingDismissed.subscribe((v) => writePref('onboarding-dismissed', v));

// ---- Backend client ---------------------------------------------------------

export function checkLocalPrereqs(): Promise<Result<LocalPrereqs>> {
  return invokeCmd<LocalPrereqs>('check_local_prereqs');
}

export function tunnelStatus(): Promise<Result<TunnelStatusRow[]>> {
  return invokeCmd<TunnelStatusRow[]>('tunnel_status');
}

// ---- Pure step derivation ---------------------------------------------------

export type StepId = 'prereqs' | 'add-host' | 'provision' | 'projects' | 'mcp' | 'session';
export type StepStatus = 'done' | 'active' | 'pending';
export interface StepBadge {
  text: string;
  tone: 'up' | 'warn';
}
export interface OnboardingStep {
  id: StepId;
  label: string;
  sublabel?: string;
  status: StepStatus;
  optional: boolean;
  badge?: StepBadge;
}

export interface DeriveInputs {
  prereqs: LocalPrereqs | null;
  visibleHostCount: number;
  /** Any non-hidden host with provisioned === true. */
  provisionedHost: boolean;
  /** First non-hidden host alias, for sublabels. */
  firstHostAlias: string | null;
  tunnels: TunnelStatusRow[];
  /** Count of projects discovered by the app (the project store) — NOT the
   *  filesystem `LocalPrereqs.projects_count`. Drives the "Pick projects" step. */
  projectCount: number;
  mcpEnabled: boolean;
  /** Count of non-background ("work") sessions. */
  workSessionCount: number;
}

export function deriveSteps(i: DeriveInputs): OnboardingStep[] {
  const prereqsDone =
    !!i.prereqs && i.prereqs.claude_ok && i.prereqs.tmux_ok && i.prereqs.projects_readable;

  const prereqSub = (() => {
    if (!i.prereqs) return undefined;
    const missing: string[] = [];
    if (!i.prereqs.claude_ok) missing.push('claude');
    if (!i.prereqs.tmux_ok) missing.push('tmux');
    if (!i.prereqs.projects_readable) missing.push('projects path');
    if (missing.length) return `Missing: ${missing.join(', ')}`;
    return `claude ${i.prereqs.claude_version ?? '?'} · tmux ${i.prereqs.tmux_version ?? '?'}`;
  })();

  // Provision step: needs a provisioned host; tunnel meaning depends on MCP.
  const tunnelUp = i.tunnels.some((t) => t.state === 'up');
  let provisionDone: boolean;
  let provisionBadge: StepBadge | undefined;
  if (!i.provisionedHost) {
    provisionDone = false;
  } else if (!i.mcpEnabled) {
    provisionDone = true;
    provisionBadge = { text: 'tunnel: starts with Control API', tone: 'warn' };
  } else {
    provisionDone = tunnelUp;
    provisionBadge = tunnelUp
      ? { text: 'tunnel: up', tone: 'up' }
      : { text: 'tunnel: down — retrying', tone: 'warn' };
  }

  // Build with raw done-flags first; assign exactly one 'active' afterward.
  const raw: Array<Omit<OnboardingStep, 'status'> & { done: boolean }> = [
    { id: 'prereqs', label: 'Local prerequisites', sublabel: prereqSub, optional: false, done: prereqsDone },
    {
      id: 'add-host',
      label: 'Add a host',
      sublabel: i.firstHostAlias ?? undefined,
      optional: false,
      done: i.visibleHostCount > 0,
    },
    {
      id: 'provision',
      label: 'Provision & tunnels',
      optional: false,
      done: provisionDone,
      badge: provisionBadge,
    },
    {
      id: 'projects',
      label: 'Pick projects',
      sublabel: i.projectCount > 0 ? `${i.projectCount} found` : undefined,
      optional: false,
      done: i.projectCount > 0,
    },
    { id: 'mcp', label: 'Enable Control API', optional: true, done: i.mcpEnabled },
    { id: 'session', label: 'Create first session', optional: false, done: i.workSessionCount > 0 },
  ];

  // The first not-done REQUIRED step is 'active'; optional steps are never active.
  let activeAssigned = false;
  return raw.map((s) => {
    let status: StepStatus;
    if (s.done) status = 'done';
    else if (!s.optional && !activeAssigned) {
      status = 'active';
      activeAssigned = true;
    } else status = 'pending';
    const { done: _done, ...rest } = s;
    return { ...rest, status };
  });
}

/** True when every REQUIRED step is done (optional Control API excepted). */
export function allRequiredComplete(steps: OnboardingStep[]): boolean {
  return steps.filter((s) => !s.optional).every((s) => s.status === 'done');
}
