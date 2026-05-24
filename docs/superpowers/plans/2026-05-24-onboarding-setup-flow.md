# Onboarding & Setup Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a non-blocking, resumable guided first-run setup experience — a one-time welcome modal plus a docked sidebar "Get started" checklist — that walks a new user from a fresh install to a running session and surfaces the (previously invisible) MCP reverse-tunnel status.

**Architecture:** The checklist is a *derived view over real fleet state* (hosts/projects/sessions stores + `mcp_status` + two new read-only backend snapshots), not a stored state machine. Only two booleans persist via the existing `prefs.ts` localStorage helpers. Two small Tauri-only commands (`check_local_prereqs`, `tunnel_status`) are added; provisioning reuses the existing blocking `provision_hosts`.

**Tech Stack:** Rust (Tauri 2 commands/service split, `tokio::process`), Svelte 5 runes + TypeScript, Vitest, `cargo test`.

**Spec:** `docs/specs/2026-05-24-onboarding-setup-flow-design.md`

**Branch:** `feat/onboarding-setup-flow` (already created off `main`; the design spec is committed there).

---

## Conventions for this plan

- All `git`/`cargo`/`pnpm` commands assume you are at the repo root: `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3`.
- Frontend tests: `npx vitest run <path>` (the project's `pnpm test` binary is not on PATH — use `npx`).
- Type-check: `npx svelte-check --tsconfig ./tsconfig.json`.
- Backend tests need Tauri system libs; if `cargo test` fails inside a build script on a headless box, that is the documented environment gap (see CLAUDE.md), not your code. Still write the tests; note the gap if they can't run.
- Do NOT register the two new commands as MCP `#[tool(...)]`s — Tauri-only, so `control-api-reference.md` does not need regeneration.

---

## File structure

**Backend (create):**
- `src-tauri/src/service/onboarding.rs` — pure logic: `LocalPrereqs`, `parse_tool_version`, `local_prereqs()`, `TunnelState`, `TunnelStatusRow`, `map_tunnel_states()`.
- `src-tauri/src/commands/onboarding.rs` — two thin Tauri command wrappers.

**Backend (modify):**
- `src-tauri/src/service/tunnel.rs` — add `TunnelSupervisor::snapshot()`.
- `src-tauri/src/service/projects.rs` — make `projects_base()` `pub(crate)`.
- `src-tauri/src/service/mod.rs` — `pub mod onboarding;`.
- `src-tauri/src/commands/mod.rs` — `pub mod onboarding;`.
- `src-tauri/src/lib.rs` — register the two commands in `generate_handler!`.

**Frontend (create):**
- `src/lib/onboarding.ts` — types, persisted flag stores, client fns, pure `deriveSteps()`.
- `src/lib/onboarding.test.ts` — Vitest for `deriveSteps` + flags.
- `src/lib/WelcomeDialog.svelte` — one-time welcome modal.
- `src/lib/OnboardingCard.svelte` — sidebar checklist card.

**Frontend (modify):**
- `src/lib/Sidebar.svelte` — mount `OnboardingCard` at top of scroller.
- `src/App.svelte` — show `WelcomeDialog` once on first run.
- `src/lib/SettingsDialog.svelte` — add "Replay setup guide" button.

---

## Task 1: Tunnel liveness snapshot

**Files:**
- Modify: `src-tauri/src/service/tunnel.rs`
- Test: `src-tauri/src/service/tunnel.rs` (existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add inside the existing `mod tests` block in `src-tauri/src/service/tunnel.rs` (after `tunnel_argv_builds_reverse_forward`):

```rust
    #[tokio::test]
    async fn snapshot_reports_known_hosts_only() {
        let sup = TunnelSupervisor::new();
        // No tasks yet → empty snapshot.
        assert!(sup.snapshot().is_empty());

        // A long-lived task counts as alive.
        sup.ensure("mefistos", 4180, 4180);
        let snap = sup.snapshot();
        assert_eq!(snap.get("mefistos"), Some(&true));
        // A host we never started is simply absent (caller maps to NotStarted).
        assert!(snap.get("never").is_none());

        sup.stop_all();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml snapshot_reports_known_hosts_only`
Expected: FAIL — `no method named snapshot found for ... TunnelSupervisor`.

- [ ] **Step 3: Add the `snapshot` method**

In `src-tauri/src/service/tunnel.rs`, add to `impl TunnelSupervisor` (after `ensure`):

```rust
    /// Per-host liveness for the onboarding/status UI. `true` = the supervised
    /// task is still running (tunnel up or mid-backoff), `false` = it has
    /// finished. Hosts with no task are simply absent from the map; callers map
    /// absence to "not started" (e.g. MCP disabled).
    pub fn snapshot(&self) -> std::collections::HashMap<String, bool> {
        let tasks = self.tasks.lock().unwrap();
        tasks
            .iter()
            .map(|(host, handle)| (host.clone(), !handle.is_finished()))
            .collect()
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml snapshot_reports_known_hosts_only`
Expected: PASS. (If the build fails in a Tauri system-lib build script on a headless box, that's the documented environment gap — note it and continue.)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/tunnel.rs
git commit -m "feat(tunnel): TunnelSupervisor::snapshot for status surfacing"
```

---

## Task 2: Onboarding service logic (prereqs + tunnel mapping)

**Files:**
- Create: `src-tauri/src/service/onboarding.rs`
- Modify: `src-tauri/src/service/projects.rs` (line 14), `src-tauri/src/service/mod.rs`
- Test: `src-tauri/src/service/onboarding.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Expose `projects_base`**

In `src-tauri/src/service/projects.rs`, change line 14 from:

```rust
fn projects_base() -> PathBuf {
```

to:

```rust
pub(crate) fn projects_base() -> PathBuf {
```

- [ ] **Step 2: Declare the module**

In `src-tauri/src/service/mod.rs`, add (keep the list alphabetical — insert after `pub mod messages;`):

```rust
pub mod onboarding;
```

- [ ] **Step 3: Write the failing tests + create the file**

Create `src-tauri/src/service/onboarding.rs` with the full content below. It includes the pure helpers and their tests; the I/O command logic comes in later steps of this task.

```rust
//! Read-only logic backing the first-run onboarding checklist:
//! local prerequisite detection and a tunnel-status snapshot mapping.

use crate::store::HostRow;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Debug, PartialEq)]
pub struct LocalPrereqs {
    pub claude_ok: bool,
    pub claude_version: Option<String>,
    pub tmux_ok: bool,
    pub tmux_version: Option<String>,
    pub projects_path: String,
    pub projects_readable: bool,
    pub projects_count: u32,
}

#[derive(Serialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Up,
    Down,
    NotStarted,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct TunnelStatusRow {
    pub host_alias: String,
    pub state: TunnelState,
}

/// Pull a semver-ish token out of a `--version` line. Returns the first
/// whitespace-separated chunk that starts with a digit (`tmux 3.4` -> `3.4`,
/// `1.0.39 (Claude Code)` -> `1.0.39`). `None` if nothing looks like a version.
pub fn parse_tool_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|tok| tok.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|tok| tok.trim_start_matches('v').to_string())
}

/// Map a per-host liveness snapshot (from `TunnelSupervisor::snapshot`) onto the
/// non-hidden hosts. Absent host => `NotStarted` (e.g. MCP disabled); present &
/// alive => `Up`; present & finished => `Down`.
pub fn map_tunnel_states(
    hosts: &[HostRow],
    alive: &HashMap<String, bool>,
) -> Vec<TunnelStatusRow> {
    hosts
        .iter()
        .filter(|h| !h.hidden)
        .map(|h| TunnelStatusRow {
            host_alias: h.alias.clone(),
            state: match alive.get(&h.alias) {
                Some(true) => TunnelState::Up,
                Some(false) => TunnelState::Down,
                None => TunnelState::NotStarted,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str, hidden: bool) -> HostRow {
        HostRow {
            alias: alias.to_string(),
            ssh_alias: None,
            reachable: true,
            claude_version: None,
            tmux_version: None,
            hidden,
            last_pinged_at: None,
            account_uuid: None,
            provisioned: true,
        }
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_tool_version("tmux 3.4"), Some("3.4".into()));
        assert_eq!(
            parse_tool_version("1.0.39 (Claude Code)"),
            Some("1.0.39".into())
        );
        assert_eq!(parse_tool_version("git version v2.40"), Some("2.40".into()));
        assert_eq!(parse_tool_version("no digits here"), None);
        assert_eq!(parse_tool_version(""), None);
    }

    #[test]
    fn maps_tunnel_states() {
        let hosts = vec![host("up", false), host("dead", false), host("none", false), host("hidden", true)];
        let mut alive = HashMap::new();
        alive.insert("up".to_string(), true);
        alive.insert("dead".to_string(), false);

        let rows = map_tunnel_states(&hosts, &alive);
        assert_eq!(
            rows,
            vec![
                TunnelStatusRow { host_alias: "up".into(), state: TunnelState::Up },
                TunnelStatusRow { host_alias: "dead".into(), state: TunnelState::Down },
                TunnelStatusRow { host_alias: "none".into(), state: TunnelState::NotStarted },
            ]
        );
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml service::onboarding`
Expected: PASS for `parses_versions` and `maps_tunnel_states` (or the documented headless build-script gap).

- [ ] **Step 5: Add the prereq I/O function (no new test — exercised manually + by the command)**

Append to `src-tauri/src/service/onboarding.rs` (before the `#[cfg(test)]` block):

```rust
/// Run a `<bin> <arg>` and return its combined trimmed stdout if it exits 0.
async fn tool_version(bin: &str, arg: &str) -> Option<String> {
    let out = tokio::process::Command::new(bin)
        .arg(arg)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_tool_version(&text)
}

/// Detect local prerequisites: the `claude` CLI, `tmux`, and the projects scan
/// directory. Never errors — a missing tool is reported as `*_ok = false`.
pub async fn local_prereqs() -> LocalPrereqs {
    let claude_version = tool_version("claude", "--version").await;
    let tmux_version = tool_version("tmux", "-V").await;

    let base = crate::service::projects::projects_base();
    let projects_path = base.to_string_lossy().to_string();
    let (projects_readable, projects_count) = match std::fs::read_dir(&base) {
        Ok(rd) => (true, rd.filter_map(|e| e.ok()).count() as u32),
        Err(_) => (false, 0),
    };

    LocalPrereqs {
        claude_ok: claude_version.is_some(),
        claude_version,
        tmux_ok: tmux_version.is_some(),
        tmux_version,
        projects_path,
        projects_readable,
        projects_count,
    }
}
```

- [ ] **Step 6: Build to verify it compiles**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: compiles (or documented headless gap). Fix any unused-import/clippy issues.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/service/onboarding.rs src-tauri/src/service/projects.rs src-tauri/src/service/mod.rs
git commit -m "feat(onboarding): service logic for local prereqs + tunnel status"
```

---

## Task 3: Onboarding Tauri commands

**Files:**
- Create: `src-tauri/src/commands/onboarding.rs`
- Modify: `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs:515-573`

- [ ] **Step 1: Create the command wrappers**

Create `src-tauri/src/commands/onboarding.rs`:

```rust
//! Tauri IPC wrappers for the first-run onboarding checklist. Read-only; not
//! exposed as MCP tools (so control-api-reference.md needs no regeneration).

use crate::ipc_error::IpcError;
use crate::service::hosts;
use crate::service::onboarding::{self, LocalPrereqs, TunnelStatusRow};
use crate::service::tunnel::TunnelSupervisor;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn check_local_prereqs() -> Result<LocalPrereqs, IpcError> {
    Ok(onboarding::local_prereqs().await)
}

#[tauri::command]
pub fn tunnel_status(
    store: State<'_, Arc<Mutex<Store>>>,
    tunnels: State<'_, Arc<TunnelSupervisor>>,
) -> Result<Vec<TunnelStatusRow>, IpcError> {
    let hosts = hosts::list_hosts(&store)?;
    let alive = tunnels.snapshot();
    Ok(onboarding::map_tunnel_states(&hosts, &alive))
}
```

- [ ] **Step 2: Declare the module**

In `src-tauri/src/commands/mod.rs`, add after `pub mod mutate;` (keep alphabetical):

```rust
pub mod onboarding;
```

- [ ] **Step 3: Register the commands**

In `src-tauri/src/lib.rs`, inside the `generate_handler![ … ]` block (after `commands::mcp::provision_hosts,` at line 566), add:

```rust
            commands::onboarding::check_local_prereqs,
            commands::onboarding::tunnel_status,
```

- [ ] **Step 4: Build to verify it compiles**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: compiles (or documented headless gap).

- [ ] **Step 5: Verify the MCP reference is still current (no tool added)**

Run: `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`
Expected: PASS unchanged — these are Tauri-only commands, not MCP tools, so the reference doc is untouched.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/onboarding.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "feat(onboarding): check_local_prereqs + tunnel_status commands"
```

---

## Task 4: Frontend onboarding store + pure step derivation

**Files:**
- Create: `src/lib/onboarding.ts`
- Test: `src/lib/onboarding.test.ts`

- [ ] **Step 1: Write the failing tests + create the file skeleton**

Create `src/lib/onboarding.ts`:

```ts
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
```

Create `src/lib/onboarding.test.ts`:

```ts
import { describe, it, expect, beforeEach } from 'vitest';
import { deriveSteps, allRequiredComplete, type DeriveInputs, type LocalPrereqs } from './onboarding';

const okPrereqs: LocalPrereqs = {
  claude_ok: true,
  claude_version: '1.0.39',
  tmux_ok: true,
  tmux_version: '3.4',
  projects_path: '/home/u/projects/github.com',
  projects_readable: true,
  projects_count: 5,
};

const base: DeriveInputs = {
  prereqs: null,
  visibleHostCount: 0,
  provisionedHost: false,
  firstHostAlias: null,
  tunnels: [],
  projectCount: 0,
  mcpEnabled: false,
  workSessionCount: 0,
};

const byId = (steps: ReturnType<typeof deriveSteps>, id: string) =>
  steps.find((s) => s.id === id)!;

describe('deriveSteps', () => {
  it('fresh state: prereqs is active, rest pending, none done', () => {
    const steps = deriveSteps(base);
    expect(byId(steps, 'prereqs').status).toBe('active');
    expect(byId(steps, 'add-host').status).toBe('pending');
    expect(allRequiredComplete(steps)).toBe(false);
  });

  it('marks prereqs done and lists versions', () => {
    const steps = deriveSteps({ ...base, prereqs: okPrereqs });
    const p = byId(steps, 'prereqs');
    expect(p.status).toBe('done');
    expect(p.sublabel).toContain('1.0.39');
    // next required step becomes active
    expect(byId(steps, 'add-host').status).toBe('active');
  });

  it('lists missing tools in the prereq sublabel', () => {
    const steps = deriveSteps({ ...base, prereqs: { ...okPrereqs, tmux_ok: false } });
    expect(byId(steps, 'prereqs').status).not.toBe('done');
    expect(byId(steps, 'prereqs').sublabel).toContain('tmux');
  });

  it('provisioned host with MCP off: provision done with warn badge', () => {
    const steps = deriveSteps({ ...base, provisionedHost: true, mcpEnabled: false });
    const prov = byId(steps, 'provision');
    expect(prov.status).toBe('done');
    expect(prov.badge).toEqual({ text: 'tunnel: starts with Control API', tone: 'warn' });
  });

  it('provisioned host with MCP on + tunnel up: provision done with up badge', () => {
    const steps = deriveSteps({
      ...base,
      provisionedHost: true,
      mcpEnabled: true,
      tunnels: [{ host_alias: 'mefistos', state: 'up' }],
    });
    const prov = byId(steps, 'provision');
    expect(prov.status).toBe('done');
    expect(prov.badge).toEqual({ text: 'tunnel: up', tone: 'up' });
  });

  it('provisioned host with MCP on + tunnel down: provision not done, retry badge', () => {
    const steps = deriveSteps({
      ...base,
      provisionedHost: true,
      mcpEnabled: true,
      tunnels: [{ host_alias: 'mefistos', state: 'down' }],
    });
    const prov = byId(steps, 'provision');
    expect(prov.status).not.toBe('done');
    expect(prov.badge?.tone).toBe('warn');
  });

  it('Control API is optional and never active', () => {
    const steps = deriveSteps({ ...base, prereqs: okPrereqs, visibleHostCount: 1 });
    const mcp = byId(steps, 'mcp');
    expect(mcp.optional).toBe(true);
    expect(mcp.status).not.toBe('active');
  });

  it('all required complete ignores the optional Control API step', () => {
    const steps = deriveSteps({
      ...base,
      prereqs: okPrereqs,
      visibleHostCount: 1,
      provisionedHost: true,
      firstHostAlias: 'mefistos',
      projectCount: 3,
      mcpEnabled: false, // optional, still incomplete
      workSessionCount: 1,
    });
    expect(allRequiredComplete(steps)).toBe(true);
  });
});
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `npx vitest run src/lib/onboarding.test.ts`
Expected: all `deriveSteps` tests PASS. (If you see `localStorage is undefined` noise it is the pre-existing test-env issue from CLAUDE.md — but these tests don't touch localStorage, so they should pass cleanly.)

- [ ] **Step 3: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors in `onboarding.ts` / `onboarding.test.ts`.

- [ ] **Step 4: Commit**

```bash
git add src/lib/onboarding.ts src/lib/onboarding.test.ts
git commit -m "feat(onboarding): store, backend client, and pure step derivation"
```

---

## Task 5: Welcome modal component

**Files:**
- Create: `src/lib/WelcomeDialog.svelte`

This component is presentational; it renders only when its parent decides to. Verification is by type-check + manual.

- [ ] **Step 1: Create the component**

Create `src/lib/WelcomeDialog.svelte`:

```svelte
<script lang="ts">
  // One-time welcome shown on first run. The parent owns visibility and the
  // `onboarding-welcomed` flag; this component just renders + emits intent.
  let { onstart, onskip }: { onstart: () => void; onskip: () => void } = $props();
</script>

<div class="backdrop" role="presentation" onclick={onskip}>
  <div
    class="panel"
    role="dialog"
    aria-modal="true"
    aria-labelledby="welcome-title"
    onclick={(e) => e.stopPropagation()}
  >
    <div class="logo" aria-hidden="true"></div>
    <h2 id="welcome-title">Welcome to claude-fleet</h2>
    <p>
      Run long-lived Claude Code sessions in tmux across your machines. Let's get
      you set up — add a host, pick a project, and start your first session.
      Takes about a minute.
    </p>
    <div class="actions">
      <button class="primary" onclick={onstart}>Let's set up →</button>
      <button class="ghost" onclick={onskip}>Skip for now</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .panel {
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 24px;
    width: 380px;
    max-width: 90vw;
    display: flex;
    flex-direction: column;
    gap: 12px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.3);
  }
  .logo {
    width: 40px;
    height: 40px;
    border-radius: 10px;
    background: linear-gradient(135deg, #2563eb, #60a5fa);
  }
  h2 {
    margin: 0;
    font-size: 1.2rem;
  }
  p {
    margin: 0;
    color: var(--fg-muted, #777);
    font-size: 0.9rem;
    line-height: 1.5;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
  button {
    padding: 8px 14px;
    border-radius: 7px;
    font-size: 0.9rem;
    cursor: pointer;
  }
  .primary {
    background: var(--accent);
    color: #fff;
    border: none;
  }
  .ghost {
    background: transparent;
    color: var(--fg-muted, #777);
    border: 1px solid var(--border);
  }
</style>
```

- [ ] **Step 2: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no errors for `WelcomeDialog.svelte`.

- [ ] **Step 3: Commit**

```bash
git add src/lib/WelcomeDialog.svelte
git commit -m "feat(onboarding): one-time welcome dialog component"
```

---

## Task 6: Get-started checklist card

**Files:**
- Create: `src/lib/OnboardingCard.svelte`

The card reads the stores, calls the backend snapshots, runs `deriveSteps`, and renders clickable rows. Row clicks emit callbacks the parent (Sidebar) wires to existing dialogs/actions; provision and project-scan and prereq-check run inline here.

- [ ] **Step 1: Create the component**

Create `src/lib/OnboardingCard.svelte`:

```svelte
<script lang="ts">
  import { hosts } from './hosts';
  import { projects, refreshProjects } from './projects';
  import { sessions } from './sessions';
  import { mcpStatus, mcpConfigure } from './mcp';
  import { provisionHosts } from './mcp';
  import {
    deriveSteps,
    allRequiredComplete,
    checkLocalPrereqs,
    tunnelStatus,
    onboardingDismissed,
    type LocalPrereqs,
    type TunnelStatusRow,
    type StepId,
  } from './onboarding';

  // Parent supplies actions that open existing dialogs.
  let { onaddhost, onnewsession }: { onaddhost: () => void; onnewsession: () => void } =
    $props();

  // Async snapshots not derivable from stores.
  let prereqs = $state<LocalPrereqs | null>(null);
  let tunnels = $state<TunnelStatusRow[]>([]);
  let mcpEnabled = $state(false);

  // In-flight UI per step.
  let busy = $state<StepId | null>(null);
  let errorText = $state<string | null>(null);

  // Refresh backend snapshots on mount and whenever hosts change.
  async function refreshSnapshots() {
    const [p, t, m] = await Promise.all([checkLocalPrereqs(), tunnelStatus(), mcpStatus()]);
    if (p.ok) prereqs = p.value;
    if (t.ok) tunnels = t.value;
    if (m.ok) mcpEnabled = m.value.enabled;
  }
  $effect(() => {
    // Re-read snapshots when host list size changes (host added/provisioned).
    void $hosts.length;
    refreshSnapshots();
  });

  const visibleHosts = $derived($hosts.filter((h) => !h.hidden));
  const workSessions = $derived($sessions.filter((s) => s.kind !== 'bg'));

  const steps = $derived(
    deriveSteps({
      prereqs,
      visibleHostCount: visibleHosts.length,
      provisionedHost: visibleHosts.some((h) => h.provisioned),
      firstHostAlias: visibleHosts[0]?.alias ?? null,
      tunnels,
      projectCount: $projects.length,
      mcpEnabled,
      workSessionCount: workSessions.length,
    }),
  );

  const doneCount = $derived(steps.filter((s) => !s.optional && s.status === 'done').length);
  const requiredCount = $derived(steps.filter((s) => !s.optional).length);
  const complete = $derived(allRequiredComplete(steps));

  async function runStep(id: StepId) {
    errorText = null;
    if (id === 'prereqs') {
      busy = 'prereqs';
      await refreshSnapshots();
      busy = null;
    } else if (id === 'add-host') {
      onaddhost();
    } else if (id === 'provision') {
      busy = 'provision';
      const r = await provisionHosts();
      if (!r.ok) errorText = r.error.message;
      else {
        const failed = r.value.find((h) => h.status === 'failed');
        if (failed) errorText = `${failed.host}: ${failed.detail ?? 'provision failed'}`;
      }
      await refreshSnapshots();
      busy = null;
    } else if (id === 'projects') {
      busy = 'projects';
      const r = await refreshProjects();
      if (!r.ok) errorText = r.error.message;
      busy = null;
    } else if (id === 'mcp') {
      busy = 'mcp';
      const r = await mcpConfigure({ enabled: true });
      if (!r.ok) errorText = r.error.message;
      else mcpEnabled = r.value.enabled;
      await refreshSnapshots();
      busy = null;
    } else if (id === 'session') {
      onnewsession();
    }
  }

  function dismiss() {
    onboardingDismissed.set(true);
  }
</script>

<div class="card" data-testid="onboarding-card">
  <div class="top">
    <b>Get started</b>
    <button class="x" onclick={dismiss} aria-label="Dismiss setup guide" title="Dismiss">✕</button>
  </div>

  {#if complete}
    <p class="done-msg">You're all set 🎉</p>
    <button class="dismiss-all" onclick={dismiss}>Dismiss</button>
  {:else}
    <div class="prog">{doneCount} of {requiredCount} done</div>
    <div class="pbar"><i style="width:{(doneCount / requiredCount) * 100}%"></i></div>

    {#each steps as step (step.id)}
      <button
        class="step"
        class:muted={step.status === 'pending'}
        onclick={() => runStep(step.id)}
        disabled={busy !== null}
      >
        <span class="ic {step.status}">{step.status === 'done' ? '✓' : busy === step.id ? '◐' : ''}</span>
        <span class="body">
          <span class="label">
            {step.label}
            {#if step.optional}<span class="opt">optional</span>{/if}
          </span>
          {#if busy === step.id}
            <span class="sub">Working…</span>
          {:else if step.sublabel}
            <span class="sub">{step.sublabel}</span>
          {/if}
          {#if step.badge}
            <span class="badge {step.badge.tone}">{step.badge.text}</span>
          {/if}
        </span>
      </button>
    {/each}

    {#if errorText}
      <p class="err">{errorText}</p>
    {/if}
  {/if}
</div>

<style>
  .card {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 11px 12px;
    margin: 0 0 10px;
  }
  .top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 4px;
  }
  .top b {
    font-size: 0.72rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .x {
    background: none;
    border: none;
    color: var(--fg-muted, #777);
    cursor: pointer;
    font-size: 0.85rem;
  }
  .prog {
    font-size: 0.7rem;
    color: var(--fg-muted, #777);
    margin-bottom: 7px;
  }
  .pbar {
    height: 4px;
    background: var(--border);
    border-radius: 3px;
    overflow: hidden;
    margin-bottom: 9px;
  }
  .pbar > i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
  .step {
    display: flex;
    gap: 9px;
    align-items: flex-start;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-top: 1px solid var(--border);
    padding: 6px 0;
    cursor: pointer;
    font-size: 0.82rem;
    color: var(--fg);
  }
  .step:first-of-type {
    border-top: none;
  }
  .step:disabled {
    cursor: default;
    opacity: 0.8;
  }
  .step.muted .body {
    color: var(--fg-muted, #777);
  }
  .ic {
    width: 17px;
    height: 17px;
    flex: 0 0 17px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.7rem;
    margin-top: 1px;
  }
  .ic.done {
    background: var(--accent);
    color: #fff;
  }
  .ic.active {
    border: 1.5px solid var(--accent);
    color: var(--accent);
  }
  .ic.pending {
    border: 1.5px solid var(--border);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .opt {
    font-size: 0.6rem;
    color: var(--fg-muted, #777);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 0 5px;
    margin-left: 6px;
  }
  .sub {
    font-size: 0.68rem;
    color: var(--fg-muted, #777);
  }
  .badge {
    font-size: 0.62rem;
    padding: 1px 6px;
    border-radius: 10px;
    margin-top: 3px;
    align-self: flex-start;
  }
  .badge.up {
    background: #e7f6ec;
    color: #1a7f37;
  }
  .badge.warn {
    background: #fdf1e3;
    color: #b06a00;
  }
  .err {
    font-size: 0.7rem;
    color: #c0392b;
    margin: 6px 0 0;
  }
  .done-msg {
    font-size: 0.9rem;
    margin: 4px 0 8px;
  }
  .dismiss-all {
    font-size: 0.8rem;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 5px 10px;
    cursor: pointer;
  }
</style>
```

- [ ] **Step 2: Confirm `refreshProjects` returns a `Result`**

Open `src/lib/projects.ts` and check the `refreshProjects` signature. The card assumes `refreshProjects(): Promise<Result<...>>`. If it returns the rows directly (throws on error) instead, adjust the `projects` branch of `runStep` to a `try/catch` that sets `errorText = String(e)`. Make the code match the real signature before moving on.

- [ ] **Step 3: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no errors for `OnboardingCard.svelte`. Fix any store-name or signature mismatches surfaced here.

- [ ] **Step 4: Commit**

```bash
git add src/lib/OnboardingCard.svelte
git commit -m "feat(onboarding): get-started checklist card"
```

---

## Task 7: Wire welcome + card into the app

**Files:**
- Modify: `src/lib/Sidebar.svelte` (imports ~line 24; scroller ~line 768)
- Modify: `src/App.svelte` (imports ~line 19; onMount ~line 99-106)
- Modify: `src/lib/SettingsDialog.svelte`

- [ ] **Step 1: Mount the card in the sidebar**

In `src/lib/Sidebar.svelte`, add to the imports near line 24 (after the `SettingsDialog` import):

```ts
  import OnboardingCard from './OnboardingCard.svelte';
  import { onboardingDismissed } from './onboarding';
```

Then in the template, change the scroller opening at line 768 from:

```svelte
  <div class="scroller">
    {#if filtered.length > 0}
```

to:

```svelte
  <div class="scroller">
    {#if !$onboardingDismissed}
      <OnboardingCard
        onaddhost={() => (showSettings = true)}
        onnewsession={() => openNew(filtered[0])}
      />
    {/if}
    {#if filtered.length > 0}
```

Note: `onaddhost` opens Settings (which hosts the "Add SSH host" → `AddHostPicker` flow). `onnewsession` reuses the sidebar's existing `openNew(row)` — confirm its signature in this file; if it requires a project row and `filtered` is empty, guard with `filtered[0] && openNew(filtered[0])` or call the footer's "+ New session" handler instead. Match the real handler before finishing.

- [ ] **Step 2: Type-check the sidebar wiring**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors. Resolve any mismatch in `openNew`'s expected argument here.

- [ ] **Step 3: Commit the sidebar wiring**

```bash
git add src/lib/Sidebar.svelte
git commit -m "feat(onboarding): mount get-started card atop the sidebar"
```

- [ ] **Step 4: Show the welcome modal on first run**

In `src/App.svelte`, add to imports near line 19:

```ts
  import WelcomeDialog from './lib/WelcomeDialog.svelte';
  import { onboardingWelcomed, onboardingDismissed } from './lib/onboarding';
  import { hosts } from './lib/hosts';
  import { sessions } from './lib/sessions';
  import { get } from 'svelte/store';
```

Add a state declaration alongside the other `$state` in App.svelte (e.g. after line 50's block):

```ts
  let showWelcome = $state(false);
```

In `onMount`, after `restoreLastSession();` (line 106), add:

```ts
    // First-run welcome: only when the user hasn't seen it AND the fleet is
    // empty (no visible hosts, no work sessions). Derived data is already
    // bootstrapped above.
    const visibleHosts = get(hosts).filter((h) => !h.hidden).length;
    const workSessions = get(sessions).filter((s) => s.kind !== 'bg').length;
    if (!get(onboardingWelcomed) && visibleHosts === 0 && workSessions === 0) {
      showWelcome = true;
    }
```

Then render the modal in App.svelte's markup (near the top of the template, before the panes):

```svelte
{#if showWelcome}
  <WelcomeDialog
    onstart={() => {
      onboardingWelcomed.set(true);
      onboardingDismissed.set(false);
      showWelcome = false;
    }}
    onskip={() => {
      onboardingWelcomed.set(true);
      showWelcome = false;
    }}
  />
{/if}
```

- [ ] **Step 5: Type-check the App wiring**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors.

- [ ] **Step 6: Commit the welcome wiring**

```bash
git add src/App.svelte
git commit -m "feat(onboarding): show one-time welcome modal on first run"
```

- [ ] **Step 7: Add "Replay setup guide" to Settings**

Open `src/lib/SettingsDialog.svelte`. Add to its `<script>` imports:

```ts
  import { onboardingDismissed, onboardingWelcomed } from './onboarding';
```

Read the file to find a sensible general/top section of the settings body. Add this control there (it un-dismisses the card so the flow reappears in the sidebar):

```svelte
<div class="setting-row">
  <button
    class="replay-btn"
    onclick={() => {
      onboardingWelcomed.set(true);
      onboardingDismissed.set(false);
    }}
  >
    Replay setup guide
  </button>
  <span class="hint">Re-show the "Get started" checklist in the sidebar.</span>
</div>
```

If `SettingsDialog.svelte` has an existing button/row style, reuse its class names instead of `replay-btn`/`setting-row`/`hint` so it visually matches; otherwise add minimal styles in the component's `<style>`.

- [ ] **Step 8: Type-check + commit**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no new errors.

```bash
git add src/lib/SettingsDialog.svelte
git commit -m "feat(onboarding): add Replay setup guide to Settings"
```

---

## Task 8: Full verification

- [ ] **Step 1: Run the full frontend test suite**

Run: `npx vitest run`
Expected: `onboarding.test.ts` passes. Pre-existing `localStorage is undefined` failures in `session_ui.test.ts` / `App.test.ts` may appear — verify they fail identically on `main` (per CLAUDE.md) before attributing anything to this work.

- [ ] **Step 2: Type-check the whole frontend**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: no errors introduced by this branch.

- [ ] **Step 3: Backend tests + lints**

Run:
```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --check
```
Expected: pass (or the documented headless Tauri-system-lib build gap; note it if so).

- [ ] **Step 4: Manual walkthrough (if a dev build is runnable)**

`pnpm tauri dev`, then with fresh state (or after "Replay setup guide"):
- Welcome modal appears on first run; "Skip for now" leaves the card visible; "Let's set up" keeps the card.
- Each step opens the right action; ticks off as state changes.
- With MCP off, Provision shows the amber "starts with Control API" badge; enabling Control API flips a provisioned host's badge to green "tunnel: up".
- "You're all set 🎉" appears once all required steps are done; dismiss hides the card; Settings → "Replay setup guide" brings it back.

- [ ] **Step 5: Final commit (if any cleanup remains)**

```bash
git add -A
git commit -m "chore(onboarding): verification cleanup" || echo "nothing to commit"
```

---

## Self-review notes (addressed)

- **Spec coverage:** welcome modal (Task 5/7), sidebar checklist (Task 6/7), all 7 steps incl. local prereqs (Task 2), add host / provision+tunnels / projects / optional MCP / first session (Task 6 `runStep`), persisted flags + reopen (Task 4/7), honest tunnel badge (Task 4 `deriveSteps`, Task 1+2+3 backend), error handling (Task 6 inline), tests (Tasks 1,2,4 + Task 8). 
- **Deviation from spec — `TunnelState::Starting` dropped:** the supervisor can't reliably distinguish "starting" from "up" via `is_finished()`, so the enum is `up | down | not_started`. This is a deliberate simplification; "down — retrying" wording in the badge covers the transient case. If a true "starting" signal is wanted later, the supervisor must track per-host phase explicitly (deferred).
- **Type consistency:** `LocalPrereqs`, `TunnelStatusRow`, `TunnelState` names/fields match across Rust (`service/onboarding.rs`) and TS (`onboarding.ts`); `deriveSteps`/`allRequiredComplete`/`StepId` are used identically in `OnboardingCard.svelte` and the tests.
- **Open verification points flagged inline** (not placeholders — real "confirm the existing signature" checks): `refreshProjects` return shape (Task 6 Step 2) and `openNew` argument (Task 7 Step 1). These depend on existing code the implementer can read directly.
```
