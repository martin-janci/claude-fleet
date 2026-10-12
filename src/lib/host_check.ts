// The host detail's health checklist (Orbit Fleet redesign step 4.7, board
// HostDetail): SSH, tmux, the fleet-agent, agents on PATH, fleet's hooks,
// the worker guard and skills drift, each a row that says ok, warn or fail
// and why. `check_host` reads what the probe does not (agents on PATH, the
// hooks in the host's settings.json); the rest comes from the host row and
// the asset inventory. Pure but for `checkHost`.
import type { AssetInventoryRow } from "./assets";
import type { HostRow } from "./hosts";
import { writable } from "svelte/store";
import { invokeCmd, type Result } from "./result";

/** Mirrors `fleet_core::service::host_check::HostCheck`. */
export interface HostCheck {
  alias: string;
  checked_at: number;
  /** Why the host could not be asked; null when it answered. */
  error: string | null;
  tmux_version: string | null;
  agents_on_path: string[] | null;
  fleet_hooks: boolean | null;
  guard_hook: boolean | null;
  /** The projects base path asked about (M15 G7.12); absent when none was. */
  base_path?: BasePathCheck;
}

/** Mirrors `fleet_core::service::host_check::BasePathCheck`. */
export interface BasePathCheck {
  path: string;
  state: "ok" | "creatable" | "unwritable" | "not_dir";
  user?: string;
}

export function checkHost(alias: string, basePath?: string): Promise<Result<HostCheck>> {
  return invokeCmd<HostCheck>("check_host", {
    args: basePath ? { alias, base_path: basePath } : { alias },
  });
}

/** Settings › Projects: what a host said about a base path, as the line
 *  under its field. `problem` marks the ones that stop a clone there. */
export function basePathLine(
  alias: string,
  c: HostCheck,
): { text: string; problem: boolean } | null {
  if (c.error) return { text: `${alias} did not answer: ${c.error}`, problem: true };
  const b = c.base_path;
  if (!b) return null;
  const who = b.user ? ` by user ${b.user}` : "";
  switch (b.state) {
    case "ok":
      return { text: `${b.path} is writable${who} on ${alias}.`, problem: false };
    case "creatable":
      return { text: `${b.path} does not exist yet; fleet can create it on ${alias}.`, problem: false };
    case "not_dir":
      return { text: `${b.path} on ${alias} is a file, not a folder.`, problem: true };
    default:
      return { text: `${b.path} is not writable${who} on ${alias}.`, problem: true };
  }
}

export type CheckState = "ok" | "warn" | "fail" | "unknown" | "na";

export interface ChecklistRow {
  key: "ssh" | "tmux" | "agent" | "agents" | "hooks" | "guard" | "skills";
  label: string;
  state: CheckState;
  detail: string;
  /** The row offers "Install <version>" (fleet-agent, 4.9). */
  install?: boolean;
}

/** Display names for the agent binaries `check_host` looks for. */
const AGENT_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  agy: "Agy",
  gemini: "Gemini CLI",
};

export const REPROVISION_HINT = "re-provision to fix";

/**
 * The checklist rows, in the board's order. `check` is the last `check_host`
 * answer for this host (null before one), `inventory` the asset inventory
 * (every host; filtered here), `hubVersion` the version an agent should match.
 * `agentsAccepted`: this client's fleet accepts fleet-agent connections (a
 * paired desktop: the hub does), so an SSH host could move onto one and
 * the row says it is not installed, with the Install action (4.9).
 */
export function checklistRows(args: {
  host: HostRow;
  check: HostCheck | null;
  inventory: readonly AssetInventoryRow[];
  hubVersion: string | null;
  agentsAccepted?: boolean;
}): ChecklistRow[] {
  const { host, check, inventory, hubVersion, agentsAccepted = false } = args;
  const local = host.alias === "local";
  const answered = check !== null && check.error === null;
  const rows: ChecklistRow[] = [];

  // SSH: the check's own round trip when there is one, else the probe's.
  if (check?.error)
    rows.push({
      key: "ssh",
      label: local ? "Local shell" : "SSH",
      state: "fail",
      detail: check.error,
    });
  else if (answered)
    rows.push({
      key: "ssh",
      label: local ? "Local shell" : "SSH",
      state: "ok",
      detail: "answered",
    });
  else if (local)
    rows.push({
      key: "ssh",
      label: "Local shell",
      state: "ok",
      detail: "this machine",
    });
  else
    rows.push({
      key: "ssh",
      label: "SSH",
      state: host.reachable ? "ok" : "fail",
      detail: host.reachable
        ? `reachable${host.latency_ms != null ? ` · ${host.latency_ms} ms` : ""}`
        : "offline",
    });

  // tmux: a check that answered is authoritative, else the probe's version.
  const tmux = answered ? check!.tmux_version : host.tmux_version;
  if (tmux)
    rows.push({ key: "tmux", label: "tmux", state: "ok", detail: tmux });
  else if (answered)
    rows.push({
      key: "tmux",
      label: "tmux",
      state: "fail",
      detail: "not installed",
    });
  else
    rows.push({
      key: "tmux",
      label: "tmux",
      state: "unknown",
      detail: "not read yet",
    });

  // fleet-agent: only an agent-transport host runs one. Where a hub could
  // take one (paired), an SSH host other than the hub's own machine says it
  // has none, and Host detail offers to install it.
  if (host.transport !== "agent" && agentsAccepted && !local) {
    rows.push({
      key: "agent",
      label: "fleet-agent",
      state: "warn",
      detail: "not installed · the hub reaches it over SSH",
      install: true,
    });
  } else if (host.transport !== "agent") {
    rows.push({
      key: "agent",
      label: "fleet-agent",
      state: "na",
      detail: "not used (SSH)",
    });
  } else if (!host.agent_version) {
    rows.push({
      key: "agent",
      label: "fleet-agent",
      state: "fail",
      detail: "never said hello",
    });
  } else if (hubVersion && host.agent_version !== hubVersion) {
    rows.push({
      key: "agent",
      label: "fleet-agent",
      state: "warn",
      detail: `${host.agent_version} · fleet is ${hubVersion}`,
    });
  } else {
    rows.push({
      key: "agent",
      label: "fleet-agent",
      state: "ok",
      detail: host.agent_version,
    });
  }

  // Agents on PATH: Claude Code is the one fleet cannot start without.
  if (answered && check!.agents_on_path) {
    const found = check!.agents_on_path;
    const names = found.map((a) => AGENT_NAMES[a] ?? a).join(", ");
    if (!found.includes("claude"))
      rows.push({
        key: "agents",
        label: "Agents on PATH",
        state: "fail",
        detail: found.length ? `${names} · no Claude Code` : "none found",
      });
    else
      rows.push({
        key: "agents",
        label: "Agents on PATH",
        state: "ok",
        detail: names,
      });
  } else {
    rows.push({
      key: "agents",
      label: "Agents on PATH",
      state: "unknown",
      detail: "run the checks",
    });
  }

  // Fleet's reporting hooks and the worker guard, from the host's settings.json.
  for (const [key, label, value] of [
    ["hooks", "Fleet hooks", answered ? check!.fleet_hooks : undefined],
    ["guard", "Guard hook", answered ? check!.guard_hook : undefined],
  ] as const) {
    if (value === true)
      rows.push({ key, label, state: "ok", detail: "installed" });
    else if (value === false)
      rows.push({
        key,
        label,
        state: "fail",
        detail: `not installed · ${REPROVISION_HINT}`,
      });
    else if (value === null)
      rows.push({
        key,
        label,
        state: "fail",
        detail: `no ~/.claude/settings.json · ${REPROVISION_HINT}`,
      });
    else rows.push({ key, label, state: "unknown", detail: "run the checks" });
  }

  // Skills drift, from the last asset scan of this host.
  const skills = inventory.filter(
    (r) => r.host_alias === host.alias && r.kind === "skill",
  );
  if (!skills.length) {
    rows.push({
      key: "skills",
      label: "Skills",
      state: "unknown",
      detail: "not scanned",
    });
  } else {
    const drifted = skills.filter((r) => r.state === "drifted").length;
    const missing = skills.filter((r) => r.state === "missing").length;
    if (!drifted && !missing)
      rows.push({
        key: "skills",
        label: "Skills",
        state: "ok",
        detail: `${skills.length} in sync`,
      });
    else {
      const parts = [];
      if (drifted) parts.push(`${drifted} drifted`);
      if (missing) parts.push(`${missing} missing`);
      rows.push({
        key: "skills",
        label: "Skills",
        state: "warn",
        detail: parts.join(" · "),
      });
    }
  }
  return rows;
}

/** Whether re-provisioning is what the checklist asks for. */
export function needsReprovision(
  rows: readonly ChecklistRow[],
  host: HostRow,
): boolean {
  return (
    !!host.provision_stale ||
    !!host.provision_warning ||
    rows.some(
      (r) => (r.key === "hooks" || r.key === "guard") && r.state === "fail",
    )
  );
}

export const CHECK_GLYPH: Record<CheckState, string> = {
  ok: "✓",
  warn: "!",
  fail: "✗",
  unknown: "?",
  na: "–",
};

/** The last check per host alias, so the detail shows it again on return. */
export const hostChecks = writable<Map<string, HostCheck>>(new Map());

/** Run `check_host` and remember the answer. */
export async function runHostCheck(alias: string): Promise<Result<HostCheck>> {
  const r = await checkHost(alias);
  if (r.ok) hostChecks.update((m) => new Map(m).set(alias, r.value));
  return r;
}

/** The step text beside the Hex field while the host detail checks or
 *  re-provisions a host (redesign step 4.13); `null` when neither runs. */
export function checklistLoaderText(
  alias: string,
  checking: boolean,
  provisioning: boolean,
): string | null {
  if (provisioning)
    return `Re-provisioning ${alias}: writing fleet's hooks, skills and CLAUDE.md block…`;
  if (checking) return `Checking ${alias}: tmux, hooks, agents and the guard…`;
  return null;
}
