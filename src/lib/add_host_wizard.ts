// The add-host wizard (Orbit Fleet redesign step 4.9, board Wizard): five
// steps — Connection, Check the host, Agents, Accounts, Done — with live
// checks that land one by one, and a draft the backend keeps so the wizard
// resumes after the app restarts. Nothing is installed without asking: the
// checks only read, and fleet-agent installs only after its row's Install. Pure but for the invoke wrappers; AddHostWizard.svelte
// renders.
import { invokeCmd, type Result } from "./result";
import { STARTABLE_AGENTS } from "./agent_picker";

/** Mirrors `fleet_core::store::SetupCheck`. */
export interface SetupCheck {
  key: CheckKey;
  /** `running` and `pending` exist only here, while a pass is under way. */
  state: "ok" | "warn" | "fail" | "na" | "running" | "pending";
  label: string;
  detail: string;
}

/** Mirrors `fleet_core::store::HostSetupRow`. */
export interface HostSetup {
  ssh_alias: string;
  alias: string;
  step: number;
  checks: SetupCheck[];
  answers: WizardAnswers;
  created_at: number;
  updated_at: number;
}

/** What the later steps record on the draft (the backend keeps it as is). */
export interface WizardAnswers {
  /** The account the host is signed in to, as the Accounts step read it. */
  account?: string | null;
  /** The person pressed "Install <version>" on the fleet-agent row (4.9):
   *  the job starts once the host is added, since it installs on a host
   *  of the fleet. Absent: nothing is installed. */
  install_agent?: boolean;
}

/** The fleet-agent row offers the install: the checks found none, and the
 *  backend said a hub here could use one (`warn`; a standalone desktop
 *  reaches its hosts over SSH and says `na`, "not needed"). */
export function offersAgentInstall(c: Pick<SetupCheck, "key" | "state">): boolean {
  return c.key === "agent" && c.state === "warn";
}

export type CheckKey = "ssh" | "tmux" | "git" | "agent" | "disk" | "agents";

/** The checks, in the board's order (`service::host_setup::SETUP_CHECKS`). */
export const CHECK_KEYS: readonly CheckKey[] = [
  "ssh",
  "tmux",
  "git",
  "agent",
  "disk",
  "agents",
];

/** What a check's row says before it has an answer. */
const PENDING_LABEL: Record<CheckKey, string> = {
  ssh: "SSH",
  tmux: "tmux",
  git: "git and gh",
  agent: "fleet-agent",
  disk: "Disk space for worktrees",
  agents: "Agents on PATH",
};

export const STEPS = [
  "Connection",
  "Check the host",
  "Agents",
  "Accounts",
  "Done",
] as const;
export const LAST_STEP = STEPS.length;

export const RESUME_NOTE =
  "You can leave now and continue later. Fleet keeps what you entered.";

/** The board's row glyphs. */
export const CHECK_GLYPH: Record<SetupCheck["state"], string> = {
  ok: "✓",
  warn: "!",
  fail: "✗",
  na: "–",
  running: "◌",
  pending: "○",
};

/** The Check step's heading: `Check mercury`. */
export function stepHeading(step: number, alias: string): string {
  switch (step) {
    case 1:
      return "Add a host";
    case 2:
      return `Check ${alias}`;
    case 3:
      return `Agents on ${alias}`;
    case 4:
      return `Accounts on ${alias}`;
    default:
      return `Add ${alias}`;
  }
}

/** `Next: Agents ›`, or the last step's own action. */
export function nextLabel(step: number, alias: string): string {
  if (step >= LAST_STEP) return `Add ${alias}`;
  return `Next: ${STEPS[step]} ›`;
}

/**
 * Every check's row for a pass: the answers known so far (in board order),
 * `running` for the one under way, `pending` for the rest. A key with no
 * answer and nothing running is pending too.
 */
export function checkRows(
  known: readonly SetupCheck[],
  running: CheckKey | null,
): SetupCheck[] {
  return CHECK_KEYS.map((key) => {
    if (key === running)
      return {
        key,
        state: "running",
        label: PENDING_LABEL[key],
        detail: "checking…",
      };
    const k = known.find((c) => c.key === key);
    if (k) return k;
    return { key, state: "pending", label: PENDING_LABEL[key], detail: "" };
  });
}

/** The loader beside a live wait (redesign step 4.13): its kit name and the
 *  step text next to it. */
export interface LiveLoader {
  name: "radar" | "sonar" | "hex-field";
  text: string;
}

/** Radar while ~/.ssh/config is read for hosts. */
export function discoveryLoader(): LiveLoader {
  return { name: "radar", text: "Looking for hosts in ~/.ssh/config…" };
}

/**
 * The loader for the check under way: Sonar while the SSH check waits for
 * the host to answer, the Hex field while the host is checked once it has.
 * `null` when no check runs.
 */
export function checkLoader(
  running: CheckKey | null,
  sshAlias: string,
): LiveLoader | null {
  if (running === null) return null;
  const host = sshAlias.trim() || "the host";
  const text: Record<CheckKey, string> = {
    ssh: `Waiting for ${host} to answer…`,
    tmux: `Checking tmux on ${host}…`,
    git: `Checking git and gh on ${host}…`,
    agent: `Looking for fleet-agent on ${host}…`,
    disk: `Measuring free disk space on ${host}…`,
    agents: `Looking for agents on ${host}'s PATH…`,
  };
  return { name: running === "ssh" ? "sonar" : "hex-field", text: text[running] };
}

/** Whether every check has an answer. */
export function checksComplete(known: readonly SetupCheck[]): boolean {
  return CHECK_KEYS.every((k) => known.some((c) => c.key === k));
}

/**
 * Whether Next is open on `step`. The Connection step needs an alias; the
 * Check step needs every answer and an SSH that answered (a host fleet
 * cannot reach cannot be added). Missing tmux or Claude Code is a warning
 * the person reads, not a lock: the old picker added such hosts too.
 */
export function canAdvance(
  step: number,
  sshAlias: string,
  alias: string,
  checks: readonly SetupCheck[],
  running: boolean,
): boolean {
  if (step === 1) return sshAlias.trim() !== "" && alias.trim() !== "";
  if (step === 2) {
    if (running || !checksComplete(checks)) return false;
    return checks.find((c) => c.key === "ssh")?.state === "ok";
  }
  return true;
}

/** The checks that ask the person to do something, worst first. */
export function attentionChecks(
  checks: readonly SetupCheck[],
): SetupCheck[] {
  const rank = { fail: 0, warn: 1 } as Record<string, number>;
  return checks
    .filter((c) => c.state === "fail" || c.state === "warn")
    .sort((a, b) => rank[a.state] - rank[b.state]);
}

/** A saved draft's line in the resume list: `mercury · step 2 of 5, Check the host`. */
export function resumeLine(d: HostSetup): string {
  const step = Math.min(Math.max(d.step, 1), LAST_STEP);
  const name = d.alias === d.ssh_alias ? d.alias : `${d.alias} (${d.ssh_alias})`;
  return `${name} · step ${step} of ${LAST_STEP}, ${STEPS[step - 1]}`;
}

/** The agents the Agents step lists, with what the check found. */
export interface AgentLine {
  bin: string;
  name: string;
  found: boolean;
  /** Claude Code is the one fleet cannot run sessions without. */
  required: boolean;
  /** Fleet can start a session with it (`STARTABLE_AGENTS`); the others
   *  are listed as they arrive. */
  startable: boolean;
}

const AGENTS: readonly { bin: string; name: string }[] = [
  { bin: "claude", name: "Claude Code" },
  { bin: "codex", name: "Codex" },
  { bin: "agy", name: "Agy" },
  { bin: "gemini", name: "Gemini CLI" },
];

/** Read the `agents` check's label (`Claude Code, Codex on PATH`). */
export function agentLines(checks: readonly SetupCheck[]): AgentLine[] {
  const c = checks.find((x) => x.key === "agents");
  const label = c?.label ?? "";
  return AGENTS.map((a) => ({
    ...a,
    found: label.includes(a.name),
    required: a.bin === "claude",
    startable: (STARTABLE_AGENTS as readonly string[]).includes(a.bin),
  }));
}

/** The note at an agent row's end: `on PATH`, `on PATH · sessions coming`
 *  for one fleet cannot start yet, else why it is missing. */
export function agentDetail(a: AgentLine): string {
  if (!a.found) return a.required ? "not found" : "not installed";
  return a.startable ? "on PATH" : "on PATH · sessions coming";
}

/** The Agents step's lead (Wizard board, "Claude Code, Codex on PATH"):
 *  which agents fleet starts sessions with, from the same list the New
 *  session picker offers. */
export function agentsLead(alias: string): string {
  const names = STARTABLE_AGENTS.map((b) => AGENTS.find((a) => a.bin === b)?.name ?? b);
  const runs = names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}` : names[0];
  return `The agents fleet can start on ${alias}. Fleet runs ${runs} sessions; the others are listed as they arrive.`;
}

export const CLAUDE_INSTALL_HINT =
  "npm install -g @anthropic-ai/claude-code";

// ---- the backend ---------------------------------------------------------

export function listHostSetups(): Promise<Result<HostSetup[]>> {
  return invokeCmd<HostSetup[]>("list_host_setups");
}

export function saveHostSetup(args: {
  ssh_alias: string;
  alias: string;
  step: number;
  answers: WizardAnswers;
}): Promise<Result<HostSetup>> {
  return invokeCmd<HostSetup>("save_host_setup", { args });
}

export function discardHostSetup(sshAlias: string): Promise<Result<boolean>> {
  return invokeCmd<boolean>("discard_host_setup", {
    args: { ssh_alias: sshAlias, call_id: null },
  });
}

export function runHostSetupCheck(
  sshAlias: string,
  key: CheckKey,
): Promise<Result<SetupCheck>> {
  return invokeCmd<SetupCheck>("run_host_setup_check", {
    args: { ssh_alias: sshAlias, key },
  });
}
