# MCP Provisioning (Phase 3) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A "Provision hosts" action that installs the `claude-fleet-control` skill + an `~/.claude.json` MCP entry on every reachable host, and maintains an app-supervised reverse SSH tunnel so remote hosts reach the central machine's localhost-bound MCP server.

**Architecture:** New `service/provision.rs` (skill install + `~/.claude.json` merge + orchestration) and `service/tunnel.rs` (a `TunnelSupervisor` of `ssh -R` children). Remote file I/O uses the existing `SshClient::run` with `printf '%s' <shell-quoted> > path` (handles `~` + arbitrary content); local uses `std::fs`. Pure helpers (`merge_mcp_entry`, `tunnel_argv`) are unit-tested; the SSH/tunnel I/O is manual-smoke verified.

**Tech Stack:** Rust (Tauri 2, tokio process, rusqlite, serde_json), Svelte/TS (settings button), `cargo test`.

> **Build/test:** from `src-tauri/`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`; frontend from repo root: `pnpm check`.

> **Spec:** `docs/specs/2026-05-22-mcp-provisioning-design.md`.
> **Working dir:** `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/mcp-provisioning` (branch `mcp-provisioning`). Run `pnpm install` once in this worktree before `pnpm check`.

> **Verified primitives:**
> - `SshClient::run(host: &str, args: &[&str], timeout: Duration) -> Result<std::process::Output, IpcError>` (over ControlMaster). `SshClient::upload_file` exists but single-quotes the remote path (no `~` expansion) — we use `run` + `printf` instead.
> - `crate::shell::quote(s) -> String` single-quote-escapes any string for the shell.
> - mcp settings consts: `mcp::SETTING_ENABLED/SETTING_PORT/SETTING_TOKEN`, `mcp::DEFAULT_PORT = 4180`, `mcp::generate_token()`. Server binds `127.0.0.1:<port>`. `McpRuntime` (Tauri `State<Mutex<McpRuntime>>`) with `.stop()`.
> - `HostRow { alias, ssh_alias, reachable, hidden, … }`. `store.list_hosts() -> Vec<HostRow>`. Store migrations: numbered `.sql` via `include_str!` in `Store::migrate()`, gated `if v < N`; latest is **010**, so the next is **011**.
> - `exec_for` / SSH use the host alias directly; `host == "local"` means the local machine.

---

## File structure

New:
- `src-tauri/src/service/provision.rs` — `merge_mcp_entry` (pure), remote/local file helpers, skill installer, config writer, `provision_hosts` orchestration + result types.
- `src-tauri/src/service/tunnel.rs` — `tunnel_argv` (pure), `TunnelSupervisor`.
- `src-tauri/migrations/011_host_provisioned.sql`

Modified:
- `src-tauri/src/store.rs` — `HostRow.provisioned`; row mappers; `set_host_provisioned`; migration 011 in `migrate()`.
- `src-tauri/src/service/mod.rs` — `pub mod provision; pub mod tunnel;`.
- `src-tauri/src/commands/mcp.rs` — `provision_hosts` command; re-provision on token/port change in `mcp_configure`; start/stop tunnels with the server.
- `src-tauri/src/mcp/tools.rs` — `provision_hosts` MCP tool.
- `src-tauri/src/lib.rs` — register command + `TunnelSupervisor` state; tear down tunnels on window-destroyed.
- `src/lib/mcp.ts` + the Control-API settings UI — "Provision hosts" button + result table.
- `docs/control-api.md` — document `provision_hosts` + the flow.

---

## Phase A — Data + pure helpers

### Task 1: `provisioned` host column

**Files:** Create `src-tauri/migrations/011_host_provisioned.sql`; Modify `src-tauri/src/store.rs`

- [ ] **Step 1: Migration file**

`src-tauri/migrations/011_host_provisioned.sql`:
```sql
ALTER TABLE hosts ADD COLUMN provisioned INTEGER NOT NULL DEFAULT 0;
INSERT OR IGNORE INTO schema_version (version) VALUES (11);
```

- [ ] **Step 2: Register in `migrate()`** — after the `if v < 10 { … }` block:
```rust
        if v < 11 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/011_host_provisioned.sql"))?;
            tx.commit()?;
        }
```

- [ ] **Step 3: Add the field to `HostRow`** (after `account_uuid`):
```rust
    pub provisioned: bool,
```

- [ ] **Step 4: Update every `HostRow` mapper.** Find each `SELECT … FROM hosts` + `HostRow { … }` build (the `fetch_host` free fn and `list_hosts`; grep `alias: row.get` / `hidden: row.get`). Append `, provisioned` to each hosts SELECT column list and add `provisioned: row.get::<_, i64>(N)? != 0,` (N = its new index) to each build, mirroring how `hidden` is read (`row.get::<_, i64>(_)? != 0`). Run `cargo build` — the compiler flags any `HostRow { … }` literal missing the field (e.g. test builders); fix each.

- [ ] **Step 5: Setter + tests**

Add to `impl Store`:
```rust
    pub fn set_host_provisioned(&self, alias: &str, provisioned: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET provisioned=?1 WHERE alias=?2",
            rusqlite::params![if provisioned { 1 } else { 0 }, alias],
        )?;
        Ok(())
    }
```

Tests in `#[cfg(test)] mod tests`:
```rust
    #[test]
    fn host_provisioned_defaults_false_and_round_trips() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        assert!(!s.get_host_row("local").unwrap().unwrap().provisioned);
        s.set_host_provisioned("local", true).unwrap();
        assert!(s.get_host_row("local").unwrap().unwrap().provisioned);
    }
```
Bump any "latest schema_version == 10" assertions to **11** (grep `== 10` in store.rs/health.rs tests; same pattern as prior migrations).

- [ ] **Step 6: Verify + commit**

Run: `cd src-tauri && cargo test hosts provisioned schema && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add src-tauri/migrations/011_host_provisioned.sql src-tauri/src/store.rs
git commit -m "feat(store): host.provisioned column (migration 011)"
```

### Task 2: `merge_mcp_entry` (pure)

**Files:** Create `src-tauri/src/service/provision.rs`; Modify `src-tauri/src/service/mod.rs`

- [ ] **Step 1: Declare the module** — add `pub mod provision;` to `src-tauri/src/service/mod.rs`.

- [ ] **Step 2: Write failing tests** in `provision.rs`:
```rust
//! Provision a host's Claude with the fleet-control skill + MCP server entry.

use crate::ipc_error::IpcError;

/// Merge the claude-fleet HTTP MCP server entry into a host's `~/.claude.json`
/// content, preserving every existing key. Returns the new JSON (pretty).
/// Errors if `existing` is non-empty and not valid JSON.
pub fn merge_mcp_entry(existing: &str, url: &str, token: &str) -> Result<String, IpcError> {
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing)
            .map_err(|e| IpcError::new("E_PROVISION", format!("~/.claude.json is not valid JSON: {e}")))?
    };
    if !root.is_object() {
        return Err(IpcError::new("E_PROVISION", "~/.claude.json is not a JSON object"));
    }
    let servers = root
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        return Err(IpcError::new("E_PROVISION", "mcpServers is not a JSON object"));
    }
    servers.as_object_mut().unwrap().insert(
        "claude-fleet".to_string(),
        serde_json::json!({
            "type": "http",
            "url": url,
            "headers": { "Authorization": format!("Bearer {token}") }
        }),
    );
    serde_json::to_string_pretty(&root)
        .map_err(|e| IpcError::new("E_PROVISION", format!("serialize: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_adds_entry_to_empty() {
        let out = merge_mcp_entry("", "http://127.0.0.1:4180/mcp", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["claude-fleet"]["type"], "http");
        assert_eq!(v["mcpServers"]["claude-fleet"]["url"], "http://127.0.0.1:4180/mcp");
        assert_eq!(v["mcpServers"]["claude-fleet"]["headers"]["Authorization"], "Bearer tok");
    }

    #[test]
    fn merge_preserves_siblings_and_is_idempotent() {
        let existing = r#"{"oauthAccount":{"email":"x@y.z"},"mcpServers":{"other":{"type":"http","url":"u"}}}"#;
        let out = merge_mcp_entry(existing, "http://127.0.0.1:4180/mcp", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["oauthAccount"]["email"], "x@y.z"); // sibling preserved
        assert_eq!(v["mcpServers"]["other"]["url"], "u"); // other server preserved
        assert_eq!(v["mcpServers"]["claude-fleet"]["url"], "http://127.0.0.1:4180/mcp");
        // Re-merge with a new token replaces just our entry.
        let out2 = merge_mcp_entry(&out, "http://127.0.0.1:4180/mcp", "tok2").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["mcpServers"]["claude-fleet"]["headers"]["Authorization"], "Bearer tok2");
        assert_eq!(v2["mcpServers"]["other"]["url"], "u");
    }

    #[test]
    fn merge_rejects_invalid_json() {
        assert!(merge_mcp_entry("not json", "u", "t").is_err());
    }
}
```

- [ ] **Step 3:** Run `cd src-tauri && cargo test merge_mcp_entry` → FAIL (module new), then implement is already inline above → PASS.

- [ ] **Step 4: Verify + commit**

Run: `cd src-tauri && cargo test merge_mcp_entry && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add src-tauri/src/service/provision.rs src-tauri/src/service/mod.rs
git commit -m "feat(provision): merge_mcp_entry — splice the MCP server into ~/.claude.json"
```

### Task 3: `tunnel_argv` (pure) + tunnel module skeleton

**Files:** Create `src-tauri/src/service/tunnel.rs`; Modify `src-tauri/src/service/mod.rs`

- [ ] **Step 1: Declare** — add `pub mod tunnel;` to `service/mod.rs`.

- [ ] **Step 2: Write failing test** in `tunnel.rs`:
```rust
//! Supervised reverse SSH tunnels: expose the central localhost MCP server on
//! each remote host's localhost via `ssh -R`.

/// Build the `ssh` argv for a reverse tunnel that makes the central machine's
/// `127.0.0.1:<mcp_port>` reachable at `127.0.0.1:<remote_port>` on `host`.
/// `-N` (no command), fail fast if the forward can't bind, keepalives so a
/// dropped link is detected.
pub fn tunnel_argv(host: &str, remote_port: u16, mcp_port: u16) -> Vec<String> {
    vec![
        "-N".into(),
        "-o".into(), "ExitOnForwardFailure=yes".into(),
        "-o".into(), "ServerAliveInterval=30".into(),
        "-o".into(), "ServerAliveCountMax=3".into(),
        "-R".into(), format!("127.0.0.1:{remote_port}:127.0.0.1:{mcp_port}"),
        host.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_argv_builds_reverse_forward() {
        let a = tunnel_argv("mefistos", 4180, 4180);
        assert!(a.contains(&"-N".to_string()));
        assert!(a.iter().any(|s| s == "127.0.0.1:4180:127.0.0.1:4180"));
        assert!(a.iter().any(|s| s == "ExitOnForwardFailure=yes"));
        assert_eq!(a.last().unwrap(), "mefistos");
    }
}
```

- [ ] **Step 3:** Run `cd src-tauri && cargo test tunnel_argv` → PASS. (`#[allow(dead_code)]` on `tunnel_argv` if clippy flags it; removed in Task 6 where `TunnelSupervisor` uses it. Note it in the report.)

- [ ] **Step 4: Verify + commit**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add src-tauri/src/service/tunnel.rs src-tauri/src/service/mod.rs
git commit -m "feat(tunnel): tunnel_argv — reverse-forward ssh argument builder"
```

---

## Phase B — File I/O + per-host provisioning

### Task 4: Remote/local file read+write helpers

**Files:** Modify `src-tauri/src/service/provision.rs`

- [ ] **Step 1: Add the helpers** (in `provision.rs`):
```rust
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use std::sync::Arc;
use std::time::Duration;

const PROVISION_TIMEOUT: Duration = Duration::from_secs(15);

/// Read a file from a host. `local` → `std::fs`; remote → `cat` over SSH.
/// Missing file → `Ok(String::new())` (caller treats as empty config).
pub async fn read_host_file(ssh: &Arc<SshClient>, host: &str, path: &str) -> Result<String, IpcError> {
    if host == "local" {
        let expanded = expand_home_local(path)?;
        return Ok(std::fs::read_to_string(&expanded).unwrap_or_default());
    }
    // `~` is expanded by the remote login shell; missing file → empty.
    let script = format!("cat {} 2>/dev/null || true", shq(path));
    let out = ssh.run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT).await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Write a file to a host (creating parent dirs). `local` → fs; remote → a
/// shell that `mkdir -p`s the parent and `printf '%s'`s the (shell-quoted)
/// content to `path`. `dir` is the parent dir (for mkdir -p); `path` the file.
pub async fn write_host_file(
    ssh: &Arc<SshClient>,
    host: &str,
    dir: &str,
    path: &str,
    content: &str,
) -> Result<(), IpcError> {
    if host == "local" {
        let edir = expand_home_local(dir)?;
        std::fs::create_dir_all(&edir).map_err(|e| IpcError::new("E_PROVISION", format!("mkdir {edir}: {e}")))?;
        let epath = expand_home_local(path)?;
        std::fs::write(&epath, content).map_err(|e| IpcError::new("E_PROVISION", format!("write {epath}: {e}")))?;
        return Ok(());
    }
    let script = format!(
        "mkdir -p {} && printf '%s' {} > {}",
        shq(dir), shq(content), shq(path)
    );
    let out = ssh.run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT).await?;
    if !out.status.success() {
        return Err(IpcError::new(
            "E_PROVISION",
            format!("write {path} on {host}: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(())
}

/// Expand a leading `~/` against the LOCAL home dir.
fn expand_home_local(path: &str) -> Result<String, IpcError> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").map_err(|_| IpcError::new("E_PROVISION", "HOME not set"))?;
        Ok(format!("{home}/{rest}"))
    } else {
        Ok(path.to_string())
    }
}
```

- [ ] **Step 2: Test `expand_home_local`** (pure-ish; set HOME in-test):
```rust
    #[test]
    fn expand_home_local_expands_tilde() {
        std::env::set_var("HOME", "/Users/test");
        assert_eq!(super::expand_home_local("~/.claude.json").unwrap(), "/Users/test/.claude.json");
        assert_eq!(super::expand_home_local("/abs/path").unwrap(), "/abs/path");
    }
```

- [ ] **Step 3: Verify + commit**

Run: `cd src-tauri && cargo test expand_home_local && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
(The remote branches of `read_host_file`/`write_host_file` are exercised by manual smoke; `cargo build` confirms they compile.)
```bash
git add src-tauri/src/service/provision.rs
git commit -m "feat(provision): host file read/write helpers (local fs + ssh printf)"
```

### Task 5: Per-host provisioning (skill + config)

**Files:** Modify `src-tauri/src/service/provision.rs`

- [ ] **Step 1: Add the bundled skill + per-host provisioner**

The skill source is at the repo root `skills/claude-fleet-control/SKILL.md`. Bundle it relative to this file (`src-tauri/src/service/provision.rs` → repo root is `../../../..`):
```rust
const FLEET_SKILL: &str = include_str!("../../../skills/claude-fleet-control/SKILL.md");
const SKILL_DIR: &str = "~/.claude/skills/claude-fleet-control";
const SKILL_PATH: &str = "~/.claude/skills/claude-fleet-control/SKILL.md";
const CLAUDE_JSON: &str = "~/.claude.json";
const CLAUDE_DIR: &str = "~/.claude";

/// Install the skill + merge the MCP entry on one host. `url` is the MCP
/// endpoint that host should use (localhost:<mcp-port> for local; the tunnel's
/// remote port for a remote host).
pub async fn provision_one(
    ssh: &Arc<SshClient>,
    host: &str,
    url: &str,
    token: &str,
) -> Result<(), IpcError> {
    // 1. Skill (live-discovered, no restart).
    write_host_file(ssh, host, SKILL_DIR, SKILL_PATH, FLEET_SKILL).await?;
    // 2. MCP entry: read → merge (preserve siblings) → back up → write.
    let existing = read_host_file(ssh, host, CLAUDE_JSON).await?;
    let merged = merge_mcp_entry(&existing, url, token)?; // errors on invalid JSON, before any write
    if !existing.trim().is_empty() {
        write_host_file(ssh, host, CLAUDE_DIR, &format!("{CLAUDE_JSON}.fleet-bak"), &existing).await?;
    }
    write_host_file(ssh, host, CLAUDE_DIR, CLAUDE_JSON, &merged).await?;
    Ok(())
}
```

> Verify the `include_str!` path resolves: the file is `src-tauri/src/service/provision.rs`; `../../../skills/...` walks `service → src → src-tauri → repo-root/skills`. If `cargo build` reports the path wrong, adjust the `../` depth until it points at `skills/claude-fleet-control/SKILL.md`.

- [ ] **Step 2: Build + commit**

Run: `cd src-tauri && cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
(`provision_one` is exercised by Task 7's orchestration + manual smoke. If clippy flags it dead, `#[allow(dead_code)]` until Task 7 calls it; remove there.)
```bash
git add src-tauri/src/service/provision.rs
git commit -m "feat(provision): provision_one — install skill + merge MCP entry on a host"
```

---

## Phase C — Tunnel supervisor + orchestration

### Task 6: `TunnelSupervisor`

**Files:** Modify `src-tauri/src/service/tunnel.rs`

- [ ] **Step 1: Implement the supervisor**

```rust
use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// Owns one supervised `ssh -R` task per remote host. Held in Tauri state as
/// `Arc<TunnelSupervisor>`. Each task loops: spawn `ssh -R … host`, await exit,
/// and (unless asked to stop) restart after a capped backoff.
#[derive(Default)]
pub struct TunnelSupervisor {
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl TunnelSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure a tunnel for `host` is running (idempotent — no-op if already up).
    pub fn ensure(self: &Arc<Self>, host: &str, remote_port: u16, mcp_port: u16) {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.get(host).map(|h| !h.is_finished()).unwrap_or(false) {
            return; // already running
        }
        let host_s = host.to_string();
        let handle = tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            loop {
                let argv = super::tunnel::tunnel_argv(&host_s, remote_port, mcp_port);
                let status = tokio::process::Command::new("ssh")
                    .args(&argv)
                    .status()
                    .await;
                eprintln!("[tunnel] {host_s} ssh exited: {status:?}; restarting in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
            }
        });
        tasks.insert(host.to_string(), handle);
    }

    /// Stop a single host's tunnel.
    pub fn stop(&self, host: &str) {
        if let Some(h) = self.tasks.lock().unwrap().remove(host) {
            h.abort();
        }
    }

    /// Stop all tunnels (app exit / MCP disable).
    pub fn stop_all(&self) {
        let mut tasks = self.tasks.lock().unwrap();
        for (_, h) in tasks.drain() {
            h.abort();
        }
    }
}
```

> Note: `ensure` uses the same `ssh` host alias the rest of the app uses; the ControlMaster `-o` opts are NOT needed here (this is a dedicated long-lived forward, not a muxed command). Aborting the JoinHandle drops the future; the child `ssh` is killed because `tokio::process::Command` defaults to `kill_on_drop`? It does NOT by default — set `.kill_on_drop(true)` on the `Command` so aborting the task reaps the ssh child (prevents orphan tunnels). Add `.kill_on_drop(true)` before `.status()`.

- [ ] **Step 2: Remove the `tunnel_argv` dead_code allow** (if added in Task 3) — it now has a caller.

- [ ] **Step 3: Build + commit**

Run: `cd src-tauri && cargo build && cargo test tunnel && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add src-tauri/src/service/tunnel.rs
git commit -m "feat(tunnel): TunnelSupervisor — supervised ssh -R per host with restart"
```

### Task 7: `provision_hosts` orchestration

**Files:** Modify `src-tauri/src/service/provision.rs`

- [ ] **Step 1: Add result types + orchestration**

```rust
use crate::store::Store;
use crate::service::tunnel::TunnelSupervisor;
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProvisionResult {
    pub host: String,
    /// "provisioned" | "skipped" | "failed"
    pub status: String,
    pub detail: Option<String>,
}

/// Provision every non-hidden host. `local` gets a direct localhost URL + no
/// tunnel; remote hosts get the reverse tunnel + a localhost:<mcp_port> URL.
/// Per-host failures never abort the others.
pub async fn provision_hosts(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    tunnels: &Arc<TunnelSupervisor>,
    mcp_port: u16,
    token: &str,
) -> Result<Vec<HostProvisionResult>, IpcError> {
    let hosts = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
    };
    let mut results = Vec::new();
    for h in hosts {
        if h.hidden {
            continue;
        }
        if h.alias != "local" && !h.reachable {
            results.push(HostProvisionResult { host: h.alias, status: "skipped".into(), detail: Some("unreachable".into()) });
            continue;
        }
        // Same URL everywhere: local server, or remote tunnel bound to mcp_port.
        let url = format!("http://127.0.0.1:{mcp_port}/mcp");
        let outcome = provision_one(ssh, &h.alias, &url, token).await;
        match outcome {
            Ok(()) => {
                if h.alias != "local" {
                    tunnels.ensure(&h.alias, mcp_port, mcp_port);
                }
                if let Ok(s) = store.lock() {
                    let _ = s.set_host_provisioned(&h.alias, true);
                }
                results.push(HostProvisionResult {
                    host: h.alias.clone(),
                    status: "provisioned".into(),
                    detail: Some("restart Claude on this host to load the MCP server".into()),
                });
            }
            Err(e) => results.push(HostProvisionResult { host: h.alias, status: "failed".into(), detail: Some(e.message) }),
        }
    }
    Ok(results)
}

/// Re-establish tunnels for already-provisioned remote hosts (app start / MCP
/// re-enable). Does NOT re-write config.
pub fn reestablish_tunnels(store: &Mutex<Store>, tunnels: &Arc<TunnelSupervisor>, mcp_port: u16) -> Result<(), IpcError> {
    let hosts = { store.lock().map_err(|_| IpcError::new("E_LOCK", "poisoned"))?.list_hosts()? };
    for h in hosts {
        if h.provisioned && h.alias != "local" && !h.hidden {
            tunnels.ensure(&h.alias, mcp_port, mcp_port);
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Remove any `provision_one` dead_code allow** (now called).

- [ ] **Step 3: Build + clippy/fmt + commit**

Run: `cd src-tauri && cargo build && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add src-tauri/src/service/provision.rs
git commit -m "feat(provision): provision_hosts orchestration + reestablish_tunnels"
```

---

## Phase D — Wiring (command, MCP tool, UI, lifecycle, docs)

### Task 8: Tauri command + state + lifecycle

**Files:** Modify `src-tauri/src/commands/mcp.rs`, `src-tauri/src/lib.rs`

- [ ] **Step 1: Register `TunnelSupervisor` state in `lib.rs`** — where other state is `.manage(...)`d (next to `McpRuntime`):
```rust
        .manage(std::sync::Arc::new(crate::service::tunnel::TunnelSupervisor::new()))
```
And in the `WindowEvent::Destroyed` handler (next to `ssh_client_for_exit.shutdown_all()`), grab the supervisor and call `stop_all()` — capture an `Arc<TunnelSupervisor>` clone before the closure, same pattern as `ssh_client_for_exit`.

- [ ] **Step 2: Add the `provision_hosts` command** in `commands/mcp.rs`:
```rust
#[tauri::command]
pub async fn provision_hosts(
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    tunnels: State<'_, Arc<crate::service::tunnel::TunnelSupervisor>>,
) -> Result<Vec<crate::service::provision::HostProvisionResult>, IpcError> {
    let (port, token) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let port = s.get_setting(mcp::SETTING_PORT)?.and_then(|p| p.parse().ok()).unwrap_or(mcp::DEFAULT_PORT);
        let token = s.get_setting(mcp::SETTING_TOKEN)?.unwrap_or_default();
        (port, token)
    };
    if token.is_empty() {
        return Err(IpcError::new("E_PROVISION", "enable the control API first (no token yet)"));
    }
    crate::service::provision::provision_hosts(&store, &ssh, &tunnels, port, &token).await
}
```
Register it in `lib.rs`'s `invoke_handler!` (next to `mcp_configure`).

- [ ] **Step 3: Wire tunnel lifecycle into `mcp_configure`** — in `commands/mcp.rs`, where it `rt.stop()`s / restarts the server:
  - When stopping/disabling (`enabled=false`): also call `tunnels.stop_all()`.
  - When (re)enabling: after the server starts, call `crate::service::provision::reestablish_tunnels(&store, &tunnels, port)`.
  - When the token regenerates OR port changes AND there are provisioned hosts: call `provision_hosts(...)` to re-write config with the new token/url (idempotent). Add `tunnels: State<'_, Arc<TunnelSupervisor>>` to `mcp_configure`'s args and register the extra state param.

- [ ] **Step 4: On app startup**, if MCP is enabled, re-establish tunnels — in `lib.rs` setup (or wherever the MCP server is auto-started), call `reestablish_tunnels` after the server binds. (If there's no startup auto-start, the first `mcp_configure(enabled=true)` covers it; note which.)

- [ ] **Step 5: Build + verify + commit**

Run: `cd src-tauri && cargo build && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
```bash
git add src-tauri/src/commands/mcp.rs src-tauri/src/lib.rs
git commit -m "feat(mcp): provision_hosts command + tunnel lifecycle tied to the server"
```

### Task 9: `provision_hosts` MCP tool

**Files:** Modify `src-tauri/src/mcp/tools.rs`

- [ ] **Step 1: Add the tool** (in the `#[tool_router] impl FleetTools` block). The MCP server already holds `store`/`ssh`; it also needs the `TunnelSupervisor`. Add a `tunnels: Arc<TunnelSupervisor>` field to `FleetTools` and thread it through `FleetTools::new(...)` (update the constructor + its call site in `mcp/mod.rs`). Then:
```rust
    #[tool(description = "Install the claude-fleet-control skill and register \
        this fleet's MCP server into every reachable host's Claude config \
        (~/.claude.json), with a reverse SSH tunnel for remote hosts. Returns a \
        per-host status list. Hosts must restart Claude to load the server.")]
    async fn provision_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("provision_hosts", "");
        let (port, token) = {
            let s = self.store.lock().map_err(|_| to_mcp_err(IpcError::new("E_LOCK", "poisoned")))?;
            let port = s.get_setting(crate::mcp::SETTING_PORT).map_err(to_mcp_err)?
                .and_then(|p| p.parse().ok()).unwrap_or(crate::mcp::DEFAULT_PORT);
            let token = s.get_setting(crate::mcp::SETTING_TOKEN).map_err(to_mcp_err)?.unwrap_or_default();
            (port, token)
        };
        let res = crate::service::provision::provision_hosts(&self.store, &self.ssh, &self.tunnels, port, &token)
            .await.map_err(to_mcp_err)?;
        ok_json(&res)
    }
```

- [ ] **Step 2: Build + verify + commit**

Run: `cd src-tauri && cargo build && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`; `grep -c '#\[tool(' src-tauri/src/mcp/tools.rs` → 32.
```bash
git add src-tauri/src/mcp/tools.rs src-tauri/src/mcp/mod.rs
git commit -m "feat(mcp): provision_hosts tool"
```

### Task 10: Settings UI — "Provision hosts" button

**Files:** Modify `src/lib/mcp.ts` + the Control-API settings UI (`src/lib/SettingsDialog.svelte`)

- [ ] **Step 1: IPC wrapper** in `src/lib/mcp.ts`:
```ts
export interface HostProvisionResult {
  host: string;
  status: string; // provisioned | skipped | failed
  detail: string | null;
}

export function provisionHosts(): Promise<Result<HostProvisionResult[]>> {
  return invokeCmd<HostProvisionResult[]>('provision_hosts');
}
```

- [ ] **Step 2: Button + result table** — in the Control-API section of `SettingsDialog.svelte` (find where MCP enable/port/token are rendered), add a "Provision hosts" button (disabled when the control API is not enabled). On click, call `provisionHosts()`, store the result, and render a small table of `host · status · detail`. Show a note: "Restart Claude on each host to load the server (the skill is picked up live)." Follow the file's existing button/table styling.

- [ ] **Step 3: Verify + commit**

Run (repo root): `pnpm check` → no new errors in the touched files.
```bash
git add src/lib/mcp.ts src/lib/SettingsDialog.svelte
git commit -m "feat(settings): Provision hosts button + per-host result table"
```

### Task 11: Docs + full verification

**Files:** Modify `docs/control-api.md`

- [ ] **Step 1: Document** — add a "Provisioning hosts" section to `docs/control-api.md`: what the `provision_hosts` tool/button does (skill + `~/.claude.json` entry + reverse tunnel), that the server stays localhost-bound, and that **each host must restart Claude** to load the MCP server (skill is live). Add `provision_hosts` to the tool list.

- [ ] **Step 2: Full backend + frontend verification**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Run (repo root): `pnpm check` (no new errors).

- [ ] **Step 3: Manual smoke**

`pnpm tauri dev`, enable the control API, click **Provision hosts**:
- A reachable remote host shows `provisioned`; verify on that host: `~/.claude/skills/claude-fleet-control/SKILL.md` exists; `~/.claude.json` has `mcpServers.claude-fleet` with the bearer token and siblings intact; a tunnel is listening (`lsof -iTCP:4180 -sTCP:LISTEN` on the remote); after restarting Claude there, `/mcp` lists `claude-fleet` and a tool call works.
- An unreachable host shows `skipped`. Disable the control API → tunnels stop (`lsof` empty). Regenerate the token → re-provision rewrites the entry.

- [ ] **Step 4: Commit**

```bash
git add docs/control-api.md
git commit -m "docs(control-api): document host provisioning + provision_hosts tool"
```

---

## Self-review notes (resolved)

- **Spec coverage:** `provisioned` column (Task 1); skill install + config merge preserving siblings (Tasks 2,4,5); reverse-tunnel supervisor (Tasks 3,6); orchestration with skip/fail-per-host + local-vs-remote URL (Task 7); command + MCP tool + UI button + token/port-rotation re-provision + server-coupled tunnel lifecycle + app-exit teardown (Tasks 8,9,10); docs + manual smoke (Task 11). Covered.
- **Single fixed remote port:** every host uses `127.0.0.1:<mcp_port>` (remote via the `-R` bind, local direct) — consistent across `tunnel_argv`, `provision_one` URL, and `provision_hosts`.
- **Safety:** `merge_mcp_entry` parses before any write (invalid JSON → fail, no clobber); a `.fleet-bak` backup is written before overwriting a non-empty config; the token never leaves localhost+tunnel; `kill_on_drop(true)` prevents orphan tunnels.
- **Dead-code windows:** `tunnel_argv` (Task 3→6) and `provision_one` (Task 5→7) may need a transient `#[allow(dead_code)]` removed when their caller lands — noted in those tasks.
- **Verify-points flagged for the implementer:** the `include_str!` relative depth to `skills/…/SKILL.md`; the exact `HostRow` mapper indices for the new column; `mcp_configure`'s existing structure when threading the new `tunnels` state.
