# Background Session Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three new commands that talk to the Claude CLI directly — `new_bg_session` (launch a supervised background session), `peek_session` (stream recent output without opening a PTY), and `purge_project` (delete all Claude Code state for a project).

**Architecture:** A new `claude_cli.rs` module wraps the three CLI invocations (`claude --bg`, `claude logs`, `claude project purge`) with async helpers that work on both the local machine and remote hosts over SSH. The service layer calls these helpers, persists state changes in the Store, and fires EventBus events. Tauri command wrappers expose the service layer to the frontend. The UI adds a "New BG Session" action in the session list, a "Peek" button on sessions with a `claude_session_id`, and a "Purge Project" action on project headers.

**Tech Stack:** Rust/tokio, existing `SshClient`, `Store`, `EventBus`; Svelte 5 runes; existing IPC pattern

**Dependency:** Plan A (claude-session-intelligence) must be executed first. The `sessions.claude_session_id` column must exist for `peek_session` to be useful.

---

## File map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src-tauri/src/claude_cli.rs` | `claude_bg`, `claude_logs`, `claude_purge_project` async helpers |
| Modify | `src-tauri/src/lib.rs` | `mod claude_cli;` declaration |
| Create | `src-tauri/src/service/bg_sessions.rs` | `new_bg_session`, `peek_session`, `purge_project` service fns |
| Modify | `src-tauri/src/service/mod.rs` | `pub mod bg_sessions;` |
| Modify | `src-tauri/src/commands/sessions.rs` | Three new `#[tauri::command]` wrappers |
| Modify | `src-tauri/src/lib.rs` | Register three new commands in `invoke_handler!` |
| Modify | `src/lib/sessions.ts` | Three new `invoke` calls: `newBgSession`, `peekSession`, `purgeProject` |
| Modify | `src/lib/Sidebar.svelte` | "Peek" button on sessions with `claude_session_id`; `PeekPanel` component inline |
| Modify | `src/lib/Toolbar.svelte` or `App.svelte` | "New BG Session" action + "Purge Project" action |

---

### Task 1: `claude_cli.rs` — CLI helper module

**Files:**
- Create: `src-tauri/src/claude_cli.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod claude_cli;`)

These helpers are thin async wrappers around `claude` CLI invocations. They run either via `tokio::process::Command` (local) or `SshClient::run` (remote).

- [ ] **Step 1: Write failing unit tests**

```rust
// src-tauri/src/claude_cli.rs (new file)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bg_output_extracts_session_id() {
        // claude --bg outputs something like:
        //   Started background session: abc-123-def\nSession ID: abc-123-def
        // We grab the first UUID-ish token after "Session ID:" or "session:"
        let output = "Starting background session...\nSession ID: abc-123-def\n";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("abc-123-def".to_string()));
    }

    #[test]
    fn parse_bg_output_none_when_no_match() {
        let output = "error: claude not found\n";
        assert!(parse_session_id_from_bg_output(output).is_none());
    }

    #[test]
    fn shell_quote_for_claude_prompt_roundtrip() {
        // A prompt with quotes and spaces must survive shell quoting.
        let prompt = r#"Fix the "main" function in src/lib.rs"#;
        let quoted = crate::shell::quote(prompt);
        // Should be wrapped in single quotes with inner single-quotes escaped.
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test claude_cli 2>&1 | head -20`
Expected: compile error — module not found

- [ ] **Step 3: Create `src-tauri/src/claude_cli.rs`**

```rust
//! Async wrappers around the `claude` CLI for background sessions and log peeking.
//!
//! Each function has a local variant (tokio::process) and a remote variant
//! (SshClient::run via bash -lc). The caller passes `host_alias` and `ssh`;
//! the function dispatches internally.
//!
//! IMPORTANT: `claude` is invoked via `bash -lc` even locally so the user's
//! PATH (which includes ~/.local/bin where claude lives) is honoured.

use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use std::sync::Arc;
use std::time::Duration;

const CLAUDE_TIMEOUT: Duration = Duration::from_secs(30);

// ─── session ID extraction ────────────────────────────────────────────────────

/// Extract the Claude session ID from `claude --bg` stdout. The CLI prints
/// a line like `Session ID: <id>` or `session: <id>` (format may vary across
/// versions). We match the first word after that prefix.
pub fn parse_session_id_from_bg_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let lower = line.to_lowercase();
        if let Some(rest) = lower
            .strip_prefix("session id:")
            .or_else(|| lower.strip_prefix("session:"))
        {
            let token = rest.trim().split_whitespace().next()?;
            if !token.is_empty() {
                // Return the original-case token from the original line.
                let start = line.to_lowercase().find(token)?;
                return Some(line[start..start + token.len()].to_string());
            }
        }
    }
    None
}

// ─── new_bg_session ──────────────────────────────────────────────────────────

/// Launch `claude --bg --name <name> "<prompt>"` on `host_alias`.
/// Returns the Claude session ID printed by the CLI (if parseable), or `None`
/// if the CLI didn't print a recognisable ID (older CLI versions).
pub async fn claude_bg(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    name: &str,
    prompt: &str,
) -> Result<Option<String>, IpcError> {
    let quoted_name = shq(name);
    let quoted_prompt = shq(prompt);
    let script = format!("claude --bg --name {quoted_name} {quoted_prompt}");

    let output = run_claude_script(ssh, host_alias, &script).await?;
    Ok(parse_session_id_from_bg_output(&output))
}

// ─── peek_session ────────────────────────────────────────────────────────────

/// Run `claude logs <session_id>` on `host_alias` and return the output.
/// This is a quick non-PTY peek at recent session output, capped at 4 KB.
pub async fn claude_logs(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    session_id: &str,
) -> Result<String, IpcError> {
    let quoted_id = shq(session_id);
    let script = format!("claude logs {quoted_id}");
    run_claude_script(ssh, host_alias, &script).await
}

// ─── purge_project ───────────────────────────────────────────────────────────

/// Run `claude project purge <project_path> --yes` on `host_alias`.
/// The `--yes` flag skips the interactive confirmation prompt.
pub async fn claude_purge_project(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    project_path: &str,
) -> Result<(), IpcError> {
    let quoted_path = shq(project_path);
    let script = format!("claude project purge {quoted_path} --yes");
    run_claude_script(ssh, host_alias, &script).await?;
    Ok(())
}

// ─── shared runner ───────────────────────────────────────────────────────────

/// Run a shell script through `claude` on the given host. For the local host
/// this spawns a bash login shell via tokio::process; for remote hosts it
/// sends the script over SSH (remote_bash pattern from tmux.rs).
async fn run_claude_script(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<String, IpcError> {
    let output = if host_alias == "local" {
        tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output()
            .await
            .map_err(|e| IpcError::new("E_SPAWN", format!("spawn bash: {e}")))?
    } else {
        let quoted = crate::shell::quote(script);
        ssh.run(host_alias, &["bash", "-lc", &quoted], CLAUDE_TIMEOUT)
            .await?
    };

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(IpcError::new(
            "E_CLAUDE_CLI",
            format!(
                "claude CLI failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bg_output_extracts_session_id() {
        let output = "Starting background session...\nSession ID: abc-123-def\n";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("abc-123-def".to_string()));
    }

    #[test]
    fn parse_bg_output_none_when_no_match() {
        let output = "error: claude not found\n";
        assert!(parse_session_id_from_bg_output(output).is_none());
    }

    #[test]
    fn parse_bg_output_session_colon_prefix() {
        let output = "session: xyz-789\n";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("xyz-789".to_string()));
    }

    #[test]
    fn shell_quote_for_claude_prompt_roundtrip() {
        let prompt = r#"Fix the "main" function in src/lib.rs"#;
        let quoted = crate::shell::quote(prompt);
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
    }
}
```

- [ ] **Step 4: Add `mod claude_cli;` to `src-tauri/src/lib.rs`**

Find the module declarations near the top of `lib.rs` (where `mod store;`, `mod tmux;` etc. live) and add:
```rust
mod claude_cli;
```

- [ ] **Step 5: Run tests**

Run: `cd src-tauri && cargo test claude_cli`
Expected: all 4 tests pass

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/claude_cli.rs src-tauri/src/lib.rs
git commit -m "feat: claude_cli.rs — bg session launch, logs peek, project purge helpers"
```

---

### Task 2: `service/bg_sessions.rs` — service layer

**Files:**
- Create: `src-tauri/src/service/bg_sessions.rs`
- Modify: `src-tauri/src/service/mod.rs`

- [ ] **Step 1: Add `pub mod bg_sessions;` to `service/mod.rs`**

```rust
pub mod bg_sessions;
```

alongside the existing module declarations.

- [ ] **Step 2: Write failing test in `service/bg_sessions.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopEventBus;
    use crate::store::Store;
    use std::sync::{Arc, Mutex};

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(
            Store::open_in_memory(Arc::new(NoopEventBus)).unwrap(),
        ))
    }

    #[test]
    fn new_bg_session_args_validates_empty_prompt() {
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "test-session".into(),
            prompt: "".into(),
        };
        // Validate must reject empty prompt before any async work.
        assert!(args.validate().is_err());
    }

    #[test]
    fn new_bg_session_args_validates_empty_name() {
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "".into(),
            prompt: "Do the thing".into(),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn peek_session_args_validates_missing_session_id() {
        let args = PeekSessionArgs {
            host_alias: "local".into(),
            claude_session_id: "".into(),
        };
        assert!(args.validate().is_err());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cd src-tauri && cargo test service::bg_sessions 2>&1 | head -20`
Expected: compile error — module not found

- [ ] **Step 4: Create `src-tauri/src/service/bg_sessions.rs`**

```rust
//! Service functions for Claude CLI background-session operations.
//!
//! `new_bg_session` — launch a supervised background Claude session.
//! `peek_session`  — fetch recent output without opening a PTY.
//! `purge_project` — delete all Claude Code state for a project.

use crate::claude_cli;
use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// ─── args / result types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct NewBgSessionArgs {
    pub host_alias: String,
    /// Name for the background session (also used as the tmux session name
    /// when the session later becomes visible via `claude agents --json`).
    pub name: String,
    /// The initial prompt sent to Claude when the session starts.
    pub prompt: String,
}

impl NewBgSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.name.trim().is_empty() {
            return Err(IpcError::new("E_INVALID", "session name must not be empty"));
        }
        if self.prompt.trim().is_empty() {
            return Err(IpcError::new("E_INVALID", "prompt must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct NewBgSessionResult {
    /// The Claude session ID returned by the CLI (null if CLI didn't print one).
    pub claude_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PeekSessionArgs {
    pub host_alias: String,
    /// Claude session ID (`sessions.claude_session_id`) to fetch logs for.
    pub claude_session_id: String,
}

impl PeekSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.claude_session_id.trim().is_empty() {
            return Err(IpcError::new(
                "E_INVALID",
                "claude_session_id must not be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct PurgeProjectArgs {
    /// The filesystem path to the project root on `host_alias`.
    pub host_alias: String,
    pub project_path: String,
    /// Fleet database project_id to remove after purging.
    pub project_id: i64,
}

// ─── service functions ───────────────────────────────────────────────────────

/// Launch a supervised Claude background session.
/// Returns the new session's Claude session ID (if the CLI printed one).
///
/// The session runs outside tmux — Claude Code's supervisor manages it.
/// It will appear in `claude agents --json` output and get picked up by the
/// next reconcile enrichment cycle (Plan A), linking it to a DB session row.
pub async fn new_bg_session(
    args: NewBgSessionArgs,
    _store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<NewBgSessionResult, IpcError> {
    args.validate()?;
    let claude_session_id = claude_cli::claude_bg(ssh, &args.host_alias, &args.name, &args.prompt).await?;
    Ok(NewBgSessionResult { claude_session_id })
}

/// Fetch recent output from a Claude background session without a PTY.
/// Returns the raw log text (may be multi-line, up to ~4 KB).
pub async fn peek_session(
    args: PeekSessionArgs,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    args.validate()?;
    claude_cli::claude_logs(ssh, &args.host_alias, &args.claude_session_id).await
}

/// Delete all Claude Code project state on `host_alias` and remove the project
/// from the fleet database.
///
/// WARNING: This is irreversible. Claude's conversation history, context, and
/// all related files in `~/.claude/projects/` are deleted on the remote machine.
pub async fn purge_project(
    args: PurgeProjectArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    if args.project_path.trim().is_empty() {
        return Err(IpcError::new("E_INVALID", "project_path must not be empty"));
    }
    // Step 1: Run the CLI command on the host.
    claude_cli::claude_purge_project(ssh, &args.host_alias, &args.project_path).await?;
    // Step 2: Remove the project from the fleet DB.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.delete_project(args.project_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_bg_session_args_validates_empty_prompt() {
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "test-session".into(),
            prompt: "".into(),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn new_bg_session_args_validates_empty_name() {
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "".into(),
            prompt: "Do the thing".into(),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn peek_session_args_validates_missing_session_id() {
        let args = PeekSessionArgs {
            host_alias: "local".into(),
            claude_session_id: "".into(),
        };
        assert!(args.validate().is_err());
    }
}
```

- [ ] **Step 5: Add `delete_project` to `Store`**

Read `src-tauri/src/store.rs` to find the `impl Store` block. Add:

```rust
/// Delete a project and all associated sessions and worktrees.
/// Called after `claude project purge` removes Claude's state on the machine.
pub fn delete_project(&self, project_id: i64) -> Result<(), IpcError> {
    self.conn
        .execute_batch(&format!(
            "DELETE FROM sessions WHERE project_id={project_id};
             DELETE FROM worktrees WHERE project_id={project_id};
             DELETE FROM projects WHERE id={project_id};"
        ))
        .map_err(IpcError::from)?;
    Ok(())
}
```

- [ ] **Step 6: Run tests**

Run: `cd src-tauri && cargo test service::bg_sessions`
Expected: all 3 tests pass

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/service/bg_sessions.rs src-tauri/src/service/mod.rs src-tauri/src/store.rs
git commit -m "feat: service/bg_sessions.rs with new_bg_session, peek_session, purge_project"
```

---

### Task 3: Tauri command wrappers

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Read `src-tauri/src/commands/sessions.rs`**

Skim to understand the existing command wrapper pattern (`State<'_>` extraction, async/await, IpcError passthrough).

- [ ] **Step 2: Add three command wrappers at the bottom of `commands/sessions.rs`**

```rust
use crate::service::bg_sessions::{self, NewBgSessionArgs, PeekSessionArgs, PurgeProjectArgs};

/// Launch a Claude background session on the given host.
/// Returns the new session's claude_session_id (may be null if CLI didn't print one).
#[tauri::command]
pub async fn new_bg_session(
    args: NewBgSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<bg_sessions::NewBgSessionResult, IpcError> {
    bg_sessions::new_bg_session(args, &store, &ssh).await
}

/// Fetch recent log output from a background Claude session without opening a PTY.
#[tauri::command]
pub async fn peek_session(
    args: PeekSessionArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<String, IpcError> {
    bg_sessions::peek_session(args, &ssh).await
}

/// Delete all Claude Code state for a project (irreversible) and remove it
/// from the fleet database.
#[tauri::command]
pub async fn purge_project(
    args: PurgeProjectArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    bg_sessions::purge_project(args, &store, &ssh).await
}
```

- [ ] **Step 3: Register commands in `lib.rs`**

Find `invoke_handler![]` in `src-tauri/src/lib.rs` and add:
```rust
commands::sessions::new_bg_session,
commands::sessions::peek_session,
commands::sessions::purge_project,
```

- [ ] **Step 4: Verify compile**

Run: `cd src-tauri && cargo build 2>&1 | head -30`
Expected: clean build

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/sessions.rs src-tauri/src/lib.rs
git commit -m "feat: new_bg_session, peek_session, purge_project Tauri commands"
```

---

### Task 4: Frontend TypeScript bindings

**Files:**
- Modify: `src/lib/sessions.ts`

- [ ] **Step 1: Read `src/lib/sessions.ts`**

Check the end of the file for the existing `invoke` patterns so the new functions match the style.

- [ ] **Step 2: Write failing Vitest test**

Create or add to `src/lib/sessions.test.ts`:

```typescript
import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock the Tauri invoke so tests run in Node.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { newBgSession, peekSession, purgeProject } from "./sessions";

beforeEach(() => {
  vi.clearAllMocks();
});

describe("newBgSession", () => {
  it("calls new_bg_session with correct args", async () => {
    (invoke as ReturnType<typeof vi.fn>).mockResolvedValue({
      claude_session_id: "abc-123",
    });
    const result = await newBgSession("local", "my-session", "Do the thing");
    expect(invoke).toHaveBeenCalledWith("new_bg_session", {
      args: {
        host_alias: "local",
        name: "my-session",
        prompt: "Do the thing",
      },
    });
    expect(result.claude_session_id).toBe("abc-123");
  });
});

describe("peekSession", () => {
  it("calls peek_session with correct args", async () => {
    (invoke as ReturnType<typeof vi.fn>).mockResolvedValue("log output here");
    const logs = await peekSession("local", "sess-id-456");
    expect(invoke).toHaveBeenCalledWith("peek_session", {
      args: { host_alias: "local", claude_session_id: "sess-id-456" },
    });
    expect(logs).toBe("log output here");
  });
});

describe("purgeProject", () => {
  it("calls purge_project with correct args", async () => {
    (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    await purgeProject("local", "/home/user/my-project", 42);
    expect(invoke).toHaveBeenCalledWith("purge_project", {
      args: {
        host_alias: "local",
        project_path: "/home/user/my-project",
        project_id: 42,
      },
    });
  });
});
```

- [ ] **Step 3: Run to verify failure**

Run: `pnpm test -- sessions.test 2>&1 | tail -20`
Expected: fail — `newBgSession` not exported

- [ ] **Step 4: Add three functions to `src/lib/sessions.ts`**

```typescript
// ─── background sessions ─────────────────────────────────────────────────────

export interface NewBgSessionResult {
  claude_session_id: string | null;
}

/** Launch a supervised Claude background session on `hostAlias`. */
export async function newBgSession(
  hostAlias: string,
  name: string,
  prompt: string,
): Promise<NewBgSessionResult> {
  return invoke<NewBgSessionResult>("new_bg_session", {
    args: { host_alias: hostAlias, name, prompt },
  });
}

/** Fetch recent log output from a background Claude session (no PTY). */
export async function peekSession(
  hostAlias: string,
  claudeSessionId: string,
): Promise<string> {
  return invoke<string>("peek_session", {
    args: { host_alias: hostAlias, claude_session_id: claudeSessionId },
  });
}

/** Delete all Claude Code state for a project and remove it from the DB. */
export async function purgeProject(
  hostAlias: string,
  projectPath: string,
  projectId: number,
): Promise<void> {
  return invoke<void>("purge_project", {
    args: {
      host_alias: hostAlias,
      project_path: projectPath,
      project_id: projectId,
    },
  });
}
```

- [ ] **Step 5: Run tests**

Run: `pnpm test -- sessions.test`
Expected: all 3 new tests pass

- [ ] **Step 6: Type-check**

Run: `pnpm check`
Expected: no type errors

- [ ] **Step 7: Commit**

```bash
git add src/lib/sessions.ts src/lib/sessions.test.ts
git commit -m "feat: newBgSession, peekSession, purgeProject frontend bindings"
```

---

### Task 5: Sidebar — "Peek" button on sessions with a claude_session_id

**Files:**
- Modify: `src/lib/Sidebar.svelte`

The "Peek" button appears on non-ghost session rows that have `claude_session_id != null`. Clicking it fetches logs and shows an inline panel below the session row.

- [ ] **Step 1: Read `src/lib/Sidebar.svelte`**

Skim the session row rendering block to understand where to place the Peek button.

- [ ] **Step 2: Add Peek state variables to `<script>`**

```svelte
<script lang="ts">
  // ...existing imports and state...
  import { peekSession } from "$lib/sessions";

  // Per-session peek panel state: session id → log string | "loading" | null
  let peekState = $state<Record<number, string | "loading" | null>>({});

  async function doPeek(sess: SessionRow) {
    if (!sess.claude_session_id) return;
    peekState[sess.id] = "loading";
    try {
      const logs = await peekSession(sess.host_alias, sess.claude_session_id);
      peekState[sess.id] = logs || "(no output yet)";
    } catch (e: unknown) {
      peekState[sess.id] =
        "Error: " + (e instanceof Error ? e.message : String(e));
    }
  }

  function closePeek(sessId: number) {
    peekState[sessId] = null;
  }
</script>
```

- [ ] **Step 3: Add Peek button inside the session row template**

In the session row's action area (the right-hand side buttons, same area as other per-session actions), add:

```svelte
{#if sess.claude_session_id && sess.status !== 'ghost'}
  <button
    class="peek-btn"
    title="Peek at session logs"
    onclick={(e) => { e.stopPropagation(); doPeek(sess); }}
    data-testid="peek-session"
  >
    📋
  </button>
{/if}
```

- [ ] **Step 4: Add the inline peek panel below each session row**

After the closing tag of the session row `<div>`, add:

```svelte
{#if peekState[sess.id] !== undefined && peekState[sess.id] !== null}
  <div class="peek-panel" data-testid="peek-panel">
    <div class="peek-header">
      <span>Session logs — {sess.tmux_name}</span>
      <button onclick={() => closePeek(sess.id)} class="peek-close">✕</button>
    </div>
    {#if peekState[sess.id] === "loading"}
      <p class="peek-loading">Loading…</p>
    {:else}
      <pre class="peek-output">{peekState[sess.id]}</pre>
    {/if}
  </div>
{/if}
```

- [ ] **Step 5: Add CSS for peek panel**

In the `<style>` section:

```css
.peek-btn {
  background: none;
  border: none;
  cursor: pointer;
  opacity: 0.6;
  padding: 2px 4px;
  font-size: 12px;
}
.peek-btn:hover {
  opacity: 1;
}
.peek-panel {
  background: var(--color-surface-2, #1e1e2e);
  border: 1px solid var(--color-border, #444);
  border-radius: 4px;
  margin: 2px 8px 4px 8px;
  padding: 8px;
  font-size: 12px;
}
.peek-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 6px;
  font-weight: 600;
}
.peek-close {
  background: none;
  border: none;
  cursor: pointer;
  color: var(--color-text-muted, #888);
}
.peek-loading {
  color: var(--color-text-muted, #888);
  font-style: italic;
}
.peek-output {
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
  font-family: monospace;
  font-size: 11px;
  margin: 0;
}
```

- [ ] **Step 6: Type-check and test**

Run: `pnpm check && pnpm test`
Expected: no type errors, no test regressions

- [ ] **Step 7: Commit**

```bash
git add src/lib/Sidebar.svelte
git commit -m "feat: Peek button and inline log panel on sessions with claude_session_id"
```

---

### Task 6: "New BG Session" and "Purge Project" UI actions

**Files:**
- Modify: `src/lib/Sidebar.svelte` (or `src/lib/App.svelte` — check where project headers and toolbar actions live)

- [ ] **Step 1: Read the component that renders project headers**

Run: `grep -n "project\|purge\|new.*session\|bg" src/lib/Sidebar.svelte src/lib/App.svelte 2>/dev/null | head -30`

Identify where project group headers are rendered and what component holds the "New Session" action button.

- [ ] **Step 2: Add "New BG Session" modal state**

In the script section of the appropriate component:

```svelte
<script lang="ts">
  import { newBgSession } from "$lib/sessions";

  let showBgModal = $state(false);
  let bgModalHost = $state("local");
  let bgModalName = $state("");
  let bgModalPrompt = $state("");
  let bgModalError = $state<string | null>(null);
  let bgModalLoading = $state(false);

  async function doNewBgSession() {
    bgModalError = null;
    bgModalLoading = true;
    try {
      await newBgSession(bgModalHost, bgModalName, bgModalPrompt);
      showBgModal = false;
      bgModalName = "";
      bgModalPrompt = "";
    } catch (e: unknown) {
      bgModalError = e instanceof Error ? e.message : String(e);
    } finally {
      bgModalLoading = false;
    }
  }
</script>
```

- [ ] **Step 3: Add "New BG Session" button to the toolbar or sidebar header**

Find where the existing "New Session" button is (likely in the toolbar or sidebar top). Add a sibling button:

```svelte
<button
  class="action-btn"
  title="Launch a supervised Claude background session"
  onclick={() => (showBgModal = true)}
  data-testid="new-bg-session-btn"
>
  ⚡ BG Session
</button>
```

- [ ] **Step 4: Add the "New BG Session" modal**

```svelte
{#if showBgModal}
  <div class="modal-backdrop" onclick={() => (showBgModal = false)}>
    <div class="modal" onclick={(e) => e.stopPropagation()} data-testid="bg-session-modal">
      <h3>New Background Session</h3>
      <label>
        Host
        <select bind:value={bgModalHost}>
          {#each hosts as host}
            <option value={host.alias}>{host.alias}</option>
          {/each}
        </select>
      </label>
      <label>
        Session name
        <input
          type="text"
          bind:value={bgModalName}
          placeholder="e.g. fix-auth-bug"
          data-testid="bg-session-name"
        />
      </label>
      <label>
        Initial prompt
        <textarea
          bind:value={bgModalPrompt}
          rows="4"
          placeholder="What should Claude work on?"
          data-testid="bg-session-prompt"
        ></textarea>
      </label>
      {#if bgModalError}
        <p class="error">{bgModalError}</p>
      {/if}
      <div class="modal-actions">
        <button onclick={() => (showBgModal = false)}>Cancel</button>
        <button
          class="btn-primary"
          onclick={doNewBgSession}
          disabled={bgModalLoading || !bgModalName.trim() || !bgModalPrompt.trim()}
          data-testid="bg-session-submit"
        >
          {bgModalLoading ? "Launching…" : "Launch"}
        </button>
      </div>
    </div>
  </div>
{/if}
```

- [ ] **Step 5: Add "Purge Project" action on project group headers**

In the project group header (wherever the project name is rendered), add a context-menu or kebab button:

```svelte
<script lang="ts">
  import { purgeProject } from "$lib/sessions";

  async function doPurgeProject(project: ProjectRow) {
    const confirmed = await confirm(
      `Purge ALL Claude Code state for "${project.repo}"? This is irreversible.`
    );
    if (!confirmed) return;
    try {
      await purgeProject("local", project.base_path, project.id);
    } catch (e: unknown) {
      alert("Purge failed: " + (e instanceof Error ? e.message : String(e)));
    }
  }
</script>

<!-- In project header template: -->
<button
  class="purge-btn"
  title="Purge Claude Code project state"
  onclick={() => doPurgeProject(project)}
  data-testid="purge-project"
>
  🗑️
</button>
```

Note: `confirm()` and `alert()` are available in Tauri webview. If the codebase uses a custom dialog, match that pattern instead (check existing usage in the file).

- [ ] **Step 6: Type-check and test**

Run: `pnpm check && pnpm test`
Expected: no type errors, no regressions

- [ ] **Step 7: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/App.svelte
git commit -m "feat: New BG Session modal and Purge Project action in UI"
```

---

### Task 7: Final verification

- [ ] **Step 1: Run all backend tests**

Run: `cd src-tauri && cargo test`
Expected: all tests pass

- [ ] **Step 2: Run frontend tests and type-check**

Run: `pnpm test && pnpm check`
Expected: no failures

- [ ] **Step 3: Manual smoke test — new_bg_session**

With the app running and a real `claude` CLI available locally:
1. Click "⚡ BG Session" button
2. Fill in host: local, name: `test-bg`, prompt: `echo hello and exit`
3. Click "Launch" — should dismiss without error
4. Wait for next reconcile — the session should appear in the agent list

- [ ] **Step 4: Manual smoke test — peek_session**

After a session has a `claude_session_id` (from reconcile enrichment):
1. Look for the 📋 button on a session row
2. Click it — a panel should expand showing log output or "(no output yet)"

- [ ] **Step 5: Commit any final tweaks**

```bash
git status && git add -p && git commit -m "chore: plan C final verification tweaks"
```
