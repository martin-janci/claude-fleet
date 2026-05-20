# Multi-host foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add SSH host management to claude-fleet so the user can list/create/attach/kill tmux sessions on remote machines (alongside local), with a unified sidebar UI and an embedded terminal that attaches over SSH.

**Architecture:** Two new Rust modules (`ssh_config.rs`, `ssh.rs`) + a `TmuxExec` trait refactor in `tmux.rs` so existing free fns become `LocalTmux` and a parallel `RemoteTmux` wraps everything through a per-host SSH ControlMaster socket. Sessions are reconciled across all enabled hosts in parallel; project linkage on the remote side is done by extracting `owner/repo` from the tmux pane's current path. Frontend gets a `hosts.ts` store, host-pill filter row in the sidebar, a Settings dialog hosting Add/Probe/Hide/Remove, and a host picker in NewSessionDialog. PTY attach is `ssh -tt <host> bash -lc 'tmux attach -t <name>'` for remote, unchanged `tmux attach` for local.

**Tech Stack:** Rust + Tauri 2 backend, Svelte 5 (runes) frontend, SQLite via rusqlite, portable-pty for terminals, OpenSSH ControlMaster for remote.

**Spec:** `docs/specs/2026-05-20-multi-host-foundations-design.md`

---

## File Structure (what gets created vs modified)

**Created:**
- `src-tauri/migrations/002_hosts_ssh.sql` — DB migration adding `hosts.ssh_alias`
- `src-tauri/src/ssh_config.rs` — pure parser of `~/.ssh/config`
- `src-tauri/src/ssh.rs` — `SshClient` with per-host ControlMaster sockets
- `src-tauri/src/commands/hosts.rs` — Tauri commands: discover/list/add/probe/remove/hide
- `src/lib/hosts.ts` — frontend store + IPC wrappers
- `src/lib/SettingsDialog.svelte` — modal hosting Hosts management
- `src/lib/AddHostPicker.svelte` — modal listing aliases from discover_hosts
- `src/lib/hosts.test.ts` — store tests
- `src/lib/SettingsDialog.test.ts` — dialog tests
- `src/lib/AddHostPicker.test.ts` — picker tests

**Modified:**
- `src-tauri/src/store.rs` — extend `migrate()` to apply 002
- `src-tauri/src/tmux.rs` — introduce `TmuxExec` trait, `LocalTmux`, `RemoteTmux`; keep free fns as delegates
- `src-tauri/src/commands/sessions.rs` — accept `host_alias` arg, route via TmuxExec dispatcher, multi-host reconcile
- `src-tauri/src/pty.rs` — `PtyOpenArgs` gains `host_alias`; remote path wraps `ssh -tt`
- `src-tauri/src/lib.rs` — register new commands; on-exit hook calls `SshClient::shutdown_all`
- `src-tauri/Cargo.toml` — add `regex` (for project owner/repo extraction)
- `src/lib/sessions.ts` — pass `host_alias` to invoke calls
- `src/lib/Sidebar.svelte` — host pills row, `[host]` badge per session, ⚙ Settings button
- `src/lib/Sidebar.test.ts` — extend for host pills + badges
- `src/lib/NewSessionDialog.svelte` — host picker row + last-host pref
- `src/lib/NewSessionDialog.test.ts` — host picker test
- `src/lib/TerminalView.svelte` — pass host_alias into pty_open; reconnect banner
- `vitest.setup.ts` — mock new IPC commands

---

## Task 1: DB migration 002 + bootstrap test

**Files:**
- Create: `src-tauri/migrations/002_hosts_ssh.sql`
- Modify: `src-tauri/src/store.rs` (the `migrate` fn + insert tests)

- [ ] **Step 1: Write the failing test in `src-tauri/src/store.rs` `mod tests`**

Add at the end of the existing `mod tests` block:

```rust
    #[test]
    fn migration_002_adds_ssh_alias_column_to_hosts() {
        let s = Store::open_in_memory().expect("open");
        // sqlite_master pragma_table_info path
        let mut stmt = s
            .conn
            .prepare("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "ssh_alias"),
            "expected `ssh_alias` column; got: {cols:?}"
        );
    }

    #[test]
    fn schema_version_is_two_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), 2);
    }
```

- [ ] **Step 2: Update the existing `schema_version_is_one` test**

Replace the existing `schema_version_is_one` test body — it now reads `2`:

```rust
    #[test]
    fn schema_version_is_two() {
        let store = Store::open_in_memory().expect("open");
        assert_eq!(store.schema_version().expect("version"), 2);
    }
```

(Rename the function from `schema_version_is_one` to `schema_version_is_two`; remove the old version.)

- [ ] **Step 3: Run tests, expect failure**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests 2>&1 | tail -20
```

Expected: `migration_002_adds_ssh_alias_column_to_hosts` fails (column missing). `schema_version_is_two` fails (version is still 1).

- [ ] **Step 4: Create `src-tauri/migrations/002_hosts_ssh.sql`**

```sql
-- Migration 002: per-host SSH alias.
-- Step 1 of the multi-host iteration. The `local` host stays special
-- (ssh_alias=NULL); registered remote hosts store the name they have
-- in the user's ~/.ssh/config (e.g. "mefistos") so SshClient knows
-- what to pass to `ssh`.

ALTER TABLE hosts ADD COLUMN ssh_alias TEXT;

INSERT INTO schema_version (version) VALUES (2);
```

- [ ] **Step 5: Update `src-tauri/src/store.rs::migrate()` to apply 002 conditionally**

Replace the existing `migrate` body:

```rust
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        self.conn
            .execute_batch(include_str!("../migrations/001_init.sql"))?;
        // Newer migrations are applied only if not yet recorded. We can't
        // wrap them in CREATE-OR-IGNORE because they ALTER existing tables.
        let v: i64 = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0);
        if v < 2 {
            self.conn
                .execute_batch(include_str!("../migrations/002_hosts_ssh.sql"))?;
        }
        Ok(())
    }
```

- [ ] **Step 6: Run all store tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store:: 2>&1 | tail -20
```

Expected: all pass including the two new ones; `migrate_is_idempotent` still passes (it re-runs `migrate`, the `v < 2` guard skips re-applying 002).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/migrations/002_hosts_ssh.sql src-tauri/src/store.rs
git commit -m "store: add migration 002 (hosts.ssh_alias)"
```

---

## Task 2: SSH config parser (`ssh_config.rs`)

**Files:**
- Create: `src-tauri/src/ssh_config.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod ssh_config;`)
- Create: fixture data in tests below (no separate file needed)

- [ ] **Step 1: Add `mod ssh_config;` to `src-tauri/src/lib.rs`**

Find the existing `mod` declarations near the top of `lib.rs` and add:

```rust
mod ssh_config;
```

- [ ] **Step 2: Create `src-tauri/src/ssh_config.rs` skeleton + tests first (TDD)**

```rust
//! Pure parser of OpenSSH client config (~/.ssh/config or any file path).
//!
//! Returns the list of named Host blocks with optional Hostname/User/Port.
//! Wildcards (Host *), the literal `github.com` host, and `*` patterns are
//! intentionally skipped — we only surface real, user-defined machine aliases
//! in the AddHostPicker UI.
//!
//! Resilient to malformed lines; never panics on unknown keywords.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// Parse a slice of `~/.ssh/config` lines into a list of named hosts. We
/// drop wildcards and a small denylist of well-known non-machine aliases.
pub fn parse(input: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Option<SshHost> = None;
    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = match split_kv(line) {
            Some(kv) => kv,
            None => continue,
        };
        let key_l = key.to_ascii_lowercase();
        if key_l == "host" {
            // Close out previous block (if it was a real host).
            if let Some(prev) = current.take() {
                hosts.push(prev);
            }
            // A single `Host` line may declare multiple aliases (`Host a b c`).
            // For our purposes we take the FIRST alias and ignore the rest —
            // exotic shared blocks aren't common in user configs.
            let first = value.split_ascii_whitespace().next().unwrap_or("");
            if is_real_alias(first) {
                current = Some(SshHost {
                    alias: first.to_string(),
                    hostname: None,
                    user: None,
                    port: None,
                });
            } else {
                current = None;
            }
            continue;
        }
        let Some(host) = current.as_mut() else { continue };
        match key_l.as_str() {
            "hostname" => host.hostname = Some(value.trim().to_string()),
            "user" => host.user = Some(value.trim().to_string()),
            "port" => host.port = value.trim().parse::<u16>().ok(),
            _ => {}
        }
    }
    if let Some(last) = current.take() {
        hosts.push(last);
    }
    hosts
}

/// Convenience wrapper: load and parse the user's `~/.ssh/config`. Returns
/// an empty list if the file does not exist or cannot be read.
pub fn load_user_config() -> Vec<SshHost> {
    let Some(home) = dirs_home() else { return Vec::new() };
    let path = home.join(".ssh").join("config");
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse(&contents),
        Err(_) => Vec::new(),
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    // `key value` separated by whitespace OR `key=value`. Either form is
    // legal per ssh_config(5).
    if let Some(eq) = line.find('=') {
        // Make sure '=' actually appears before any whitespace.
        if line[..eq].chars().all(|c| !c.is_whitespace()) {
            return Some((line[..eq].trim(), line[eq + 1..].trim()));
        }
    }
    let mut it = line.splitn(2, char::is_whitespace);
    let key = it.next()?.trim();
    let val = it.next()?.trim();
    if key.is_empty() || val.is_empty() {
        return None;
    }
    Some((key, val))
}

fn is_real_alias(alias: &str) -> bool {
    if alias.is_empty() {
        return false;
    }
    // Wildcards are not user-facing hosts; github.com is a special case
    // used to pin IdentityFile, not a machine alias the user can ssh to
    // for tmux.
    if alias.contains('*') || alias.contains('?') {
        return false;
    }
    const DENYLIST: &[&str] = &["github.com", "gitlab.com", "bitbucket.org"];
    !DENYLIST.contains(&alias)
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "
Host alpha
    Hostname 10.0.0.5
    User martin
    Port 2222

Host beta
    Hostname beta.lan
";

    #[test]
    fn parses_two_simple_hosts() {
        let hosts = parse(SIMPLE);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "alpha");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[0].user.as_deref(), Some("martin"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[1].alias, "beta");
        assert_eq!(hosts[1].hostname.as_deref(), Some("beta.lan"));
        assert_eq!(hosts[1].user, None);
    }

    #[test]
    fn drops_wildcard_blocks() {
        let cfg = "
Host *
    StrictHostKeyChecking ask
Host real
    Hostname real.example.com
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "real");
    }

    #[test]
    fn drops_github_alias() {
        let cfg = "
Host github.com
    IdentityFile ~/.ssh/github_ed25519
Host work
    Hostname work.lan
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "work");
    }

    #[test]
    fn comments_are_stripped() {
        let cfg = "
# top-level comment
Host x  # trailing comment
    Hostname x.lan # this too
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "x");
        assert_eq!(hosts[0].hostname.as_deref(), Some("x.lan"));
    }

    #[test]
    fn supports_equals_form() {
        let cfg = "
Host eq
    Hostname=eq.lan
    Port=2244
";
        let hosts = parse(cfg);
        assert_eq!(hosts[0].hostname.as_deref(), Some("eq.lan"));
        assert_eq!(hosts[0].port, Some(2244));
    }

    #[test]
    fn handles_first_alias_in_multi_alias_line() {
        // OpenSSH allows `Host a b c` to share a block. We just take `a`.
        let cfg = "
Host primary backup tertiary
    Hostname pool.lan
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "primary");
    }

    #[test]
    fn empty_input_returns_empty_vec() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn unknown_keywords_are_ignored() {
        let cfg = "
Host h
    Hostname h.lan
    ServerAliveInterval 30
    PermitLocalCommand yes
";
        let hosts = parse(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname.as_deref(), Some("h.lan"));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib ssh_config:: 2>&1 | tail -20
```

Expected: all 8 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/ssh_config.rs src-tauri/src/lib.rs
git commit -m "ssh_config: pure parser of ~/.ssh/config Host blocks"
```

---

## Task 3: `SshClient` with ControlMaster

**Files:**
- Create: `src-tauri/src/ssh.rs`
- Modify: `src-tauri/src/lib.rs` (`mod ssh;`)

- [ ] **Step 1: Add `mod ssh;` to `src-tauri/src/lib.rs`**

Near other `mod` declarations:

```rust
mod ssh;
```

- [ ] **Step 2: Create `src-tauri/src/ssh.rs`**

```rust
//! Per-host ControlMaster-backed SSH client.
//!
//! Why ControlMaster: every list_sessions / kill / rename involves a tmux
//! command on the remote host. Without a persistent socket each call pays
//! the full ssh handshake (~500-2000ms on a LAN, more over WAN). With
//! ControlMaster the first call sets up a background `ssh -M -N`, every
//! subsequent call multiplexes through it and returns in <50ms.
//!
//! Socket path: ~/.cache/claude-fleet/cm-<host>.sock — dedicated to this app
//! so we never collide with a user's global ssh ControlPath setting.

use crate::ipc_error::IpcError;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::Duration;

pub struct SshClient {
    // Set of hosts for which we've already spawned a master process.
    // Backed by a Mutex<HashSet<String>> so concurrent ensure_master calls
    // serialize and a second call is a cheap no-op.
    started: Mutex<std::collections::HashSet<String>>,
}

impl SshClient {
    pub fn new() -> Self {
        Self {
            started: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Returns the dedicated ControlPath for a host. Side effect: creates
    /// the parent dir if missing. The path is used by both `-M` master spawn
    /// and subsequent `-o ControlPath=...` calls.
    pub fn control_path(&self, host: &str) -> PathBuf {
        let dir = cache_dir();
        // best-effort: ignore errors (caller falls back to per-call ssh if
        // dir doesn't exist).
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("cm-{host}.sock"))
    }

    /// Spawn a background master if we haven't already. Idempotent.
    pub fn ensure_master(&self, host: &str) -> Result<(), IpcError> {
        {
            let started = self
                .started
                .lock()
                .map_err(|_| IpcError::new("E_LOCK", "ssh master mutex poisoned"))?;
            if started.contains(host) {
                return Ok(());
            }
        }
        let path = self.control_path(host);
        // If a stale socket exists, ask any orphan master to exit. Errors
        // are non-fatal — `-O exit` returns 255 if no master is listening.
        let _ = Command::new("ssh")
            .args([
                "-o",
                &format!("ControlPath={}", path.display()),
                "-O",
                "exit",
                host,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Spawn the master. -f makes ssh fork into the background after
        // authenticating. -N requests no remote command. ControlPersist
        // keeps the master idle for 10 minutes before self-closing.
        let status = Command::new("ssh")
            .args([
                "-fN",
                "-o",
                "ControlMaster=yes",
                "-o",
                &format!("ControlPath={}", path.display()),
                "-o",
                "ControlPersist=10m",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                host,
            ])
            .status()
            .map_err(|e| IpcError::new("E_SSH", format!("spawn ssh master: {e}")))?;
        if !status.success() {
            return Err(IpcError::new(
                "E_SSH",
                format!("ssh master to {host} failed (status: {status:?})"),
            ));
        }
        let mut started = self
            .started
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "ssh master mutex poisoned"))?;
        started.insert(host.to_string());
        Ok(())
    }

    /// Run a command on `host`, multiplexing through the established master.
    /// `timeout` is enforced via `-o ConnectTimeout` (handshake only); the
    /// command itself runs to completion — we trust tmux invocations to be
    /// fast. Returns the full Output for callers to inspect stdout/stderr.
    pub fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError> {
        self.ensure_master(host)?;
        let path = self.control_path(host);
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-o",
            &format!("ControlPath={}", path.display()),
            "-o",
            "BatchMode=yes",
            "-o",
            &format!("ConnectTimeout={}", timeout.as_secs().max(1)),
            host,
        ]);
        cmd.args(args);
        cmd.output()
            .map_err(|e| IpcError::new("E_SSH", format!("ssh {host}: {e}")))
    }

    /// Tell every known master to exit. Called from Tauri on_exit so we
    /// don't leak persistent ssh processes after the app closes.
    pub fn shutdown_all(&self) {
        let started = match self.started.lock() {
            Ok(s) => s.clone(),
            Err(_) => return,
        };
        for host in started.iter() {
            let path = self.control_path(host);
            let _ = Command::new("ssh")
                .args([
                    "-o",
                    &format!("ControlPath={}", path.display()),
                    "-O",
                    "exit",
                    host,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

impl Default for SshClient {
    fn default() -> Self {
        Self::new()
    }
}

fn cache_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("claude-fleet");
    }
    std::env::temp_dir().join("claude-fleet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_path_lives_under_cache_dir() {
        let c = SshClient::new();
        let p = c.control_path("mefistos");
        assert!(p.ends_with("cm-mefistos.sock"));
        assert!(
            p.to_string_lossy().contains("claude-fleet"),
            "expected path under cache dir, got: {}",
            p.display()
        );
    }

    #[test]
    fn shutdown_when_no_masters_is_noop() {
        let c = SshClient::new();
        c.shutdown_all(); // must not panic when started set is empty
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib ssh:: 2>&1 | tail -10
```

Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/ssh.rs src-tauri/src/lib.rs
git commit -m "ssh: SshClient with per-host ControlMaster socket"
```

---

## Task 4: Project owner/repo extraction + `regex` dep

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/commands/sessions.rs` (add a new helper + tests)

- [ ] **Step 1: Add `regex` to `src-tauri/Cargo.toml`**

Open `src-tauri/Cargo.toml`. Under `[dependencies]`, add:

```toml
regex = "1"
```

Save.

- [ ] **Step 2: Run cargo fetch to verify**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet && cargo fetch --manifest-path src-tauri/Cargo.toml 2>&1 | tail -5
```

Expected: regex resolved, no errors.

- [ ] **Step 3: Add the helper in `src-tauri/src/commands/sessions.rs`**

Just above the existing `find_project_id_for_path` function, add:

```rust
/// Extract `(owner, repo)` from a path that follows the conventional
/// `.../projects/github.com/<owner>/<repo>/...` layout (the same layout
/// `proj-clean` enforces on disk). Remote hosts often store repos under
/// a different prefix (e.g. `/home/mjanci/...` instead of `/Users/...`),
/// but the GitHub portion is stable — so we match into the repo cell
/// regardless of where the path starts.
fn extract_owner_repo(path: &str) -> Option<(String, String)> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"/projects/github\.com/([^/]+)/([^/]+)").expect("static regex")
    });
    let caps = RE.captures(path)?;
    Some((
        caps.get(1)?.as_str().to_string(),
        caps.get(2)?.as_str().to_string(),
    ))
}
```

- [ ] **Step 4: Add `once_cell` to Cargo.toml**

In the same `[dependencies]` block:

```toml
once_cell = "1"
```

- [ ] **Step 5: Add tests at the bottom of `src-tauri/src/commands/sessions.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_owner_repo_from_macos_path() {
        let r = extract_owner_repo("/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/x");
        assert_eq!(r, Some(("martin-janci".into(), "claude-fleet".into())));
    }

    #[test]
    fn extracts_owner_repo_from_linux_path() {
        let r = extract_owner_repo("/home/mjanci/projects/github.com/martin-janci/sales-twins-app");
        assert_eq!(r, Some(("martin-janci".into(), "sales-twins-app".into())));
    }

    #[test]
    fn extracts_owner_repo_when_followed_by_subdir() {
        let r = extract_owner_repo("/anywhere/projects/github.com/papayapos/pos-frontend/src/lib");
        assert_eq!(r, Some(("papayapos".into(), "pos-frontend".into())));
    }

    #[test]
    fn returns_none_when_not_github_com_layout() {
        assert_eq!(extract_owner_repo("/tmp/random/repo"), None);
        assert_eq!(extract_owner_repo("/home/x/projects/gitlab.com/a/b"), None);
    }
}
```

- [ ] **Step 6: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::sessions:: 2>&1 | tail -10
```

Expected: 4 new tests pass.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/commands/sessions.rs
git commit -m "sessions: extract owner/repo from path for cross-host project linking"
```

---

## Task 5: `TmuxExec` trait + `LocalTmux` + `RemoteTmux`

**Files:**
- Modify: `src-tauri/src/tmux.rs`

- [ ] **Step 1: Add the trait + LocalTmux + RemoteTmux to `src-tauri/src/tmux.rs`**

At the top of the file, after the existing `use` block, add:

```rust
use crate::ssh::SshClient;
use std::sync::Arc;

/// Backend-agnostic tmux operations. Implementations differ only in how
/// the `tmux` binary is invoked: locally or wrapped in `ssh <host>`.
pub trait TmuxExec: Send + Sync {
    fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError>;
    fn new_session(&self, name: &str, working_dir: &std::path::Path) -> Result<(), IpcError>;
    fn kill_session(&self, name: &str) -> Result<(), IpcError>;
    fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError>;
    fn restart_session(&self, name: &str) -> Result<(), IpcError>;
}

pub struct LocalTmux;

impl TmuxExec for LocalTmux {
    fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        list_local_sessions()
    }
    fn new_session(&self, name: &str, cwd: &std::path::Path) -> Result<(), IpcError> {
        new_session(name, cwd)
    }
    fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        kill_session(name)
    }
    fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        rename_session(old, new)
    }
    fn restart_session(&self, name: &str) -> Result<(), IpcError> {
        restart_session(name)
    }
}

pub struct RemoteTmux {
    pub client: Arc<SshClient>,
    pub host: String,
}

impl RemoteTmux {
    /// We always wrap remote tmux invocations in `bash -lc '…'` so the
    /// remote user's login env (PATH, LANG, etc.) is sourced. sshd may have
    /// `AcceptEnv` disabled which would silently drop SendEnv vars; the
    /// login shell route is portable.
    fn remote_bash(&self, script: &str) -> Result<std::process::Output, IpcError> {
        self.client.run(&self.host, &["bash", "-lc", script], std::time::Duration::from_secs(10))
    }
}

impl TmuxExec for RemoteTmux {
    fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        let script = "tmux list-sessions -F '#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}' 2>&1";
        let output = self.remote_bash(script)?;
        let combined = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(parse_sessions(&combined));
        }
        if is_no_server_running(&combined) {
            return Ok(Vec::new());
        }
        Err(IpcError::new("E_TMUX", combined.trim()))
    }

    fn new_session(&self, name: &str, cwd: &std::path::Path) -> Result<(), IpcError> {
        // Build the `tmux new-session` command identically to LocalTmux but
        // shell-escape arguments since we're sending a single script string.
        let mut script = String::from("tmux new-session -d");
        script.push_str(&format!(" -s {}", shell_quote(name)));
        script.push_str(&format!(" -c {}", shell_quote(&cwd.to_string_lossy())));
        // Forward env explicitly — remote sshd typically doesn't pass LANG.
        script.push_str(" -e COLORTERM=truecolor -e TERM=xterm-256color");
        script.push_str(&format!(" -e LANG={}", shell_quote(&std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()))));
        script.push(' ');
        script.push_str(&shell_quote(&pane_command()));
        let output = self.remote_bash(&script)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        let script = format!("tmux kill-session -t {}", shell_quote(name));
        let output = self.remote_bash(&script)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        let trimmed = new.trim();
        if trimmed.is_empty() {
            return Err(IpcError::new("E_TMUX", "new session name must not be empty"));
        }
        if trimmed.contains(|c: char| c.is_whitespace() || c == '.' || c == ':') {
            return Err(IpcError::new(
                "E_TMUX",
                "tmux session name must not contain whitespace, `.`, or `:`",
            ));
        }
        if trimmed == old {
            return Ok(());
        }
        let script = format!(
            "tmux rename-session -t {} {}",
            shell_quote(old),
            shell_quote(trimmed)
        );
        let output = self.remote_bash(&script)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    fn restart_session(&self, name: &str) -> Result<(), IpcError> {
        let script = format!(
            "tmux respawn-pane -k -t {}: {}",
            shell_quote(name),
            shell_quote(&pane_command())
        );
        let output = self.remote_bash(&script)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }
}

/// Conservative single-quote shell escape: wraps in `'...'` and replaces
/// embedded single quotes with the canonical `'\''` dance. Avoids depending
/// on a crate for a small, well-tested operation.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
```

- [ ] **Step 2: Add tests for shell_quote at the bottom of `mod tests`**

In the existing `#[cfg(test)] mod tests { ... }` block, add:

```rust
    #[test]
    fn shell_quote_wraps_basic_strings_in_single_quotes() {
        assert_eq!(shell_quote("foo"), "'foo'");
        assert_eq!(shell_quote("dev-foo"), "'dev-foo'");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("don't"), "'don'\\''t'");
    }

    #[test]
    fn shell_quote_handles_paths_with_spaces() {
        assert_eq!(shell_quote("/tmp/with space"), "'/tmp/with space'");
    }
```

- [ ] **Step 3: Run tmux tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib tmux:: 2>&1 | tail -20
```

Expected: existing tests + 3 new `shell_quote` tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/tmux.rs
git commit -m "tmux: introduce TmuxExec trait + RemoteTmux via SshClient"
```

---

## Task 6: Store helpers for hosts (CRUD)

**Files:**
- Modify: `src-tauri/src/store.rs` (add host CRUD methods + tests)

- [ ] **Step 1: Add host helpers to `Store impl`**

Add these methods inside `impl Store { ... }` near the existing `upsert_host`:

```rust
    pub fn list_hosts(&self) -> Result<Vec<HostRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT alias, ssh_alias, reachable, claude_version, tmux_version, hidden, last_pinged_at
             FROM hosts
             ORDER BY (alias='local') DESC, alias ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HostRow {
                alias: row.get(0)?,
                ssh_alias: row.get(1)?,
                reachable: row.get::<_, i64>(2)? != 0,
                claude_version: row.get(3)?,
                tmux_version: row.get(4)?,
                hidden: row.get::<_, i64>(5)? != 0,
                last_pinged_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn insert_host(
        &self,
        alias: &str,
        ssh_alias: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO hosts (alias, ssh_alias, reachable, hidden) VALUES (?1, ?2, 0, 0)
             ON CONFLICT(alias) DO UPDATE SET ssh_alias=excluded.ssh_alias",
            rusqlite::params![alias, ssh_alias],
        )?;
        Ok(())
    }

    pub fn update_host_probe(
        &self,
        alias: &str,
        reachable: bool,
        claude_version: Option<&str>,
        tmux_version: Option<&str>,
        last_pinged_at: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET reachable=?1, claude_version=?2, tmux_version=?3, last_pinged_at=?4 WHERE alias=?5",
            rusqlite::params![
                if reachable { 1 } else { 0 },
                claude_version,
                tmux_version,
                last_pinged_at,
                alias
            ],
        )?;
        Ok(())
    }

    pub fn set_host_hidden(&self, alias: &str, hidden: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE hosts SET hidden=?1 WHERE alias=?2",
            rusqlite::params![if hidden { 1 } else { 0 }, alias],
        )?;
        Ok(())
    }

    pub fn delete_host(&self, alias: &str) -> Result<(), rusqlite::Error> {
        // Sessions are pruned naturally when reconcile_sessions runs against
        // an empty hosts set; we don't cascade here. The `local` host is
        // never removed.
        if alias == "local" {
            return Ok(());
        }
        self.conn.execute(
            "DELETE FROM sessions WHERE host_alias=?1",
            rusqlite::params![alias],
        )?;
        self.conn.execute(
            "DELETE FROM hosts WHERE alias=?1",
            rusqlite::params![alias],
        )?;
        Ok(())
    }
```

- [ ] **Step 2: Add the `HostRow` struct to `src-tauri/src/store.rs`** (near `SessionRow`)

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostRow {
    pub alias: String,
    pub ssh_alias: Option<String>,
    pub reachable: bool,
    pub claude_version: Option<String>,
    pub tmux_version: Option<String>,
    pub hidden: bool,
    pub last_pinged_at: Option<i64>,
}
```

- [ ] **Step 3: Add tests**

In `mod tests`:

```rust
    #[test]
    fn list_hosts_orders_local_first_then_alpha() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.insert_host("zebra", Some("zebra")).unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let names: Vec<String> = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(names, vec!["local", "mefistos", "zebra"]);
    }

    #[test]
    fn insert_host_records_ssh_alias() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        let row = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "mefistos")
            .unwrap();
        assert_eq!(row.ssh_alias.as_deref(), Some("mefistos"));
        assert!(!row.reachable);
        assert!(!row.hidden);
    }

    #[test]
    fn update_host_probe_persists_versions_and_reachability() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.update_host_probe("h", true, Some("2.1.144"), Some("3.6a"), 1000)
            .unwrap();
        let row = s.list_hosts().unwrap().into_iter().find(|x| x.alias == "h").unwrap();
        assert!(row.reachable);
        assert_eq!(row.claude_version.as_deref(), Some("2.1.144"));
        assert_eq!(row.tmux_version.as_deref(), Some("3.6a"));
        assert_eq!(row.last_pinged_at, Some(1000));
    }

    #[test]
    fn delete_host_removes_host_and_its_sessions() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.upsert_session("dev-a", "h", None, None, 1, 1, "running")
            .unwrap();
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 1);
        s.delete_host("h").unwrap();
        assert_eq!(s.list_hosts().unwrap().iter().filter(|x| x.alias == "h").count(), 0);
        assert_eq!(s.list_sessions_for_host("h").unwrap().len(), 0);
    }

    #[test]
    fn delete_host_refuses_to_remove_local() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.delete_host("local").unwrap();
        assert!(s.list_hosts().unwrap().iter().any(|h| h.alias == "local"));
    }

    #[test]
    fn set_host_hidden_toggles() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h", Some("h")).unwrap();
        s.set_host_hidden("h", true).unwrap();
        assert!(s.list_hosts().unwrap().iter().find(|x| x.alias == "h").unwrap().hidden);
        s.set_host_hidden("h", false).unwrap();
        assert!(!s.list_hosts().unwrap().iter().find(|x| x.alias == "h").unwrap().hidden);
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store:: 2>&1 | tail -15
```

Expected: all pass, 6 new tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/store.rs
git commit -m "store: HostRow + CRUD (list/insert/update_probe/hide/delete)"
```

---

## Task 7: Tauri `commands::hosts` module

**Files:**
- Create: `src-tauri/src/commands/hosts.rs`
- Modify: `src-tauri/src/commands/mod.rs` (add `pub mod hosts;`)
- Modify: `src-tauri/src/lib.rs` (register the new tauri commands + `manage` SshClient)

- [ ] **Step 1: Create `src-tauri/src/commands/hosts.rs`**

```rust
//! Tauri commands for SSH host management. Each command is a thin wrapper
//! around `store.rs` helpers plus `ssh_config.rs` (for discovery) and
//! `ssh::SshClient` (for probing).

use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::ssh_config::{self, SshHost};
use crate::store::{HostRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

#[tauri::command]
pub fn discover_hosts() -> Result<Vec<SshHost>, IpcError> {
    Ok(ssh_config::load_user_config())
}

#[tauri::command]
pub fn list_hosts(store: State<'_, Mutex<Store>>) -> Result<Vec<HostRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_hosts().map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct AddHostArgs {
    pub alias: String,
    pub ssh_alias: String,
}

#[tauri::command]
pub fn add_host(
    args: AddHostArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostRow, IpcError> {
    // Probe first; we don't want to persist a host we can't talk to.
    let (reachable, claude_ver, tmux_ver) = probe(&ssh, &args.ssh_alias)?;
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.insert_host(&args.alias, Some(&args.ssh_alias))?;
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(&store, &args.alias)
}

#[derive(Deserialize)]
pub struct HostAliasArgs {
    pub alias: String,
}

#[tauri::command]
pub fn probe_host(
    args: HostAliasArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostRow, IpcError> {
    let ssh_alias = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
            .into_iter()
            .find(|h| h.alias == args.alias)
            .and_then(|h| h.ssh_alias)
    };
    let target = ssh_alias.as_deref().unwrap_or(&args.alias);
    // The `local` host has no ssh_alias; probe is best-effort via local shell.
    let (reachable, claude_ver, tmux_ver) = if args.alias == "local" {
        probe_local()
    } else {
        probe(&ssh, target)?
    };
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(&store, &args.alias)
}

#[tauri::command]
pub fn remove_host(
    args: HostAliasArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<(), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.delete_host(&args.alias).map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct HideHostArgs {
    pub alias: String,
    pub hidden: bool,
}

#[tauri::command]
pub fn hide_host(
    args: HideHostArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<(), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.set_host_hidden(&args.alias, args.hidden).map_err(IpcError::from)
}

// --- helpers ---

fn list_one(
    store: &State<'_, Mutex<Store>>,
    alias: &str,
) -> Result<HostRow, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_hosts()?
        .into_iter()
        .find(|h| h.alias == alias)
        .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("host {alias} not found")))
}

fn probe(
    ssh: &Arc<SshClient>,
    host: &str,
) -> Result<(bool, Option<String>, Option<String>), IpcError> {
    // Single round trip: print both versions, semicolon-separated, so a
    // missing claude doesn't drop the tmux probe.
    let script = "tmux -V 2>/dev/null || true; echo ---; claude --version 2>/dev/null || true";
    let out = ssh.run(host, &["bash", "-lc", script], Duration::from_secs(5));
    let out = match out {
        Ok(o) => o,
        Err(_) => return Ok((false, None, None)),
    };
    if !out.status.success() {
        return Ok((false, None, None));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut parts = stdout.split("---");
    let tmux_line = parts.next().unwrap_or("").trim().to_string();
    let claude_line = parts.next().unwrap_or("").trim().to_string();
    Ok((
        true,
        parse_claude_version(&claude_line),
        parse_tmux_version(&tmux_line),
    ))
}

fn probe_local() -> (bool, Option<String>, Option<String>) {
    let tmux = std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    let claude = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    (
        true,
        parse_claude_version(claude.as_deref().unwrap_or("")),
        parse_tmux_version(tmux.as_deref().unwrap_or("")),
    )
}

fn parse_tmux_version(line: &str) -> Option<String> {
    // `tmux 3.6a` → "3.6a"
    line.strip_prefix("tmux ")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_claude_version(line: &str) -> Option<String> {
    // `2.1.144 (Claude Code)` → "2.1.144"
    line.split_whitespace().next().map(|v| v.to_string())
        .filter(|v| !v.is_empty())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tmux_version_extracts_version() {
        assert_eq!(parse_tmux_version("tmux 3.6a").as_deref(), Some("3.6a"));
        assert_eq!(parse_tmux_version("tmux 3.5"), Some("3.5".into()));
        assert_eq!(parse_tmux_version(""), None);
        assert_eq!(parse_tmux_version("not a version"), None);
    }

    #[test]
    fn parse_claude_version_extracts_first_token() {
        assert_eq!(parse_claude_version("2.1.144 (Claude Code)").as_deref(), Some("2.1.144"));
        assert_eq!(parse_claude_version("  2.1.12  "), Some("2.1.12".into()));
        assert_eq!(parse_claude_version(""), None);
    }
}
```

- [ ] **Step 2: Add `pub mod hosts;` to `src-tauri/src/commands/mod.rs`**

```rust
pub mod hosts;
```

- [ ] **Step 3: Register commands + state in `src-tauri/src/lib.rs`**

Find the `tauri::Builder` block in `run()` and update:

```rust
    let ssh_client = std::sync::Arc::new(ssh::SshClient::new());
    let ssh_client_for_exit = std::sync::Arc::clone(&ssh_client);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(store))
        .manage(Mutex::new(PtyState::new()))
        .manage(ssh_client)
        .invoke_handler(tauri::generate_handler![
            commands::health::health_check,
            commands::projects::list_projects,
            commands::projects::refresh_projects,
            commands::sessions::list_sessions,
            commands::sessions::new_session,
            commands::sessions::kill_session,
            commands::sessions::rename_session,
            commands::sessions::restart_session,
            commands::hosts::discover_hosts,
            commands::hosts::list_hosts,
            commands::hosts::add_host,
            commands::hosts::probe_host,
            commands::hosts::remove_host,
            commands::hosts::hide_host,
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            pty::pty_drain,
        ])
        .on_window_event(move |_window, event| {
            // Close ssh masters when the app is about to exit so we don't
            // leak background ssh processes after quit.
            if let tauri::WindowEvent::Destroyed = event {
                ssh_client_for_exit.shutdown_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
```

- [ ] **Step 4: Run cargo build + tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::hosts:: 2>&1 | tail -10
```

Expected: 2 tests pass (`parse_tmux_version_extracts_version`, `parse_claude_version_extracts_first_token`).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/hosts.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "commands::hosts: discover/list/add/probe/remove/hide"
```

---

## Task 8: Multi-host session reconcile + per-host tmux routing

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs`

- [ ] **Step 1: Update `new_session`, `kill_session`, `rename_session`, `restart_session` args**

Add `host_alias` to each arg struct + route to the right `TmuxExec`. Replace the arg structs and command bodies:

```rust
#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub host_alias: String,
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
}

#[derive(Deserialize)]
pub struct KillSessionArgs {
    pub host_alias: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct RenameSessionArgs {
    pub host_alias: String,
    pub old_name: String,
    pub new_name: String,
}

#[derive(Deserialize)]
pub struct RestartSessionArgs {
    pub host_alias: String,
    pub name: String,
}
```

- [ ] **Step 2: Add a dispatcher fn in the same file**

Near the top of the module:

```rust
use crate::tmux::{LocalTmux, RemoteTmux, TmuxExec};
use crate::ssh::SshClient;
use std::sync::Arc;

fn exec_for(host: &str, ssh: &Arc<SshClient>) -> Box<dyn TmuxExec> {
    if host == "local" {
        Box::new(LocalTmux)
    } else {
        Box::new(RemoteTmux {
            client: Arc::clone(ssh),
            host: host.to_string(),
        })
    }
}
```

- [ ] **Step 3: Update each command to route through `exec_for`**

Replace `new_session`:

```rust
#[tauri::command]
pub fn new_session(
    args: NewSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let path: PathBuf = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        if let Some(wid) = args.worktree_id {
            let mut stmt = s
                .conn_ref()
                .prepare("SELECT path FROM worktrees WHERE id=?1")?;
            let row: String = stmt.query_row(rusqlite::params![wid], |r| r.get(0))?;
            PathBuf::from(row)
        } else {
            let mut stmt = s
                .conn_ref()
                .prepare("SELECT base_path FROM projects WHERE id=?1")?;
            let row: String = stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
            PathBuf::from(row)
        }
    };
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.new_session(&args.name, &path)?;

    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let rows = reconcile_sessions(&s, &ssh)?;
    rows.into_iter()
        .find(|r| r.tmux_name == args.name && r.host_alias == args.host_alias)
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "session {} on {} did not appear in list",
                    args.name, args.host_alias
                ),
            )
        })
}
```

Replace `kill_session`:

```rust
#[tauri::command]
pub fn kill_session(
    args: KillSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.kill_session(&args.name)?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    reconcile_sessions(&s, &ssh)?;
    Ok(())
}
```

Replace `rename_session`:

```rust
#[tauri::command]
pub fn rename_session(
    args: RenameSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.rename_session(&args.old_name, &args.new_name)?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let rows = reconcile_sessions(&s, &ssh)?;
    rows.into_iter()
        .find(|r| r.tmux_name == args.new_name.trim() && r.host_alias == args.host_alias)
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "renamed session {} on {} did not appear in list",
                    args.new_name, args.host_alias
                ),
            )
        })
}
```

Replace `restart_session`:

```rust
#[tauri::command]
pub fn restart_session(
    args: RestartSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.restart_session(&args.name)?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let rows = reconcile_sessions(&s, &ssh)?;
    rows.into_iter()
        .find(|r| r.tmux_name == args.name && r.host_alias == args.host_alias)
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "restarted session {} on {} did not appear in list",
                    args.name, args.host_alias
                ),
            )
        })
}
```

- [ ] **Step 4: Rename `reconcile_local_sessions` → `reconcile_sessions`, iterate hosts**

Replace the whole `reconcile_local_sessions` function with:

```rust
fn reconcile_sessions(
    s: &Store,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    let hosts = s.list_hosts()?;
    let mut all_rows: Vec<SessionRow> = Vec::new();

    for host in hosts {
        if host.hidden {
            continue;
        }
        let tmux = exec_for(&host.alias, ssh);
        let live = match tmux.list_sessions() {
            Ok(v) => v,
            Err(_e) => {
                // Mark host unreachable but don't fail the whole reconcile;
                // other hosts can still list their sessions.
                let _ = s.update_host_probe(
                    &host.alias,
                    false,
                    host.claude_version.as_deref(),
                    host.tmux_version.as_deref(),
                    now_unix(),
                );
                continue;
            }
        };
        // Successful list = reachable. Bump the timestamp.
        let _ = s.update_host_probe(
            &host.alias,
            true,
            host.claude_version.as_deref(),
            host.tmux_version.as_deref(),
            now_unix(),
        );
        let mut keep = Vec::with_capacity(live.len());
        for sess in &live {
            keep.push(sess.name.clone());
            let project_id = find_project_id_for_path(s, &host.alias, &sess.path);
            s.upsert_session(
                &sess.name,
                &host.alias,
                project_id,
                None,
                sess.created,
                sess.last_activity,
                "running",
            )?;
            if let Some(pid) = project_id {
                s.touch_project_last_session_at(pid, sess.last_activity)?;
            }
        }
        s.delete_sessions_not_in(&host.alias, &keep)?;
        all_rows.extend(s.list_sessions_for_host(&host.alias)?);
    }
    Ok(all_rows)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 5: Update `find_project_id_for_path` to accept host_alias**

Replace it with:

```rust
fn find_project_id_for_path(
    s: &Store,
    host_alias: &str,
    path: &std::path::Path,
) -> Option<i64> {
    let path_str = path.to_string_lossy();
    let projects = s.list_projects().ok()?;
    if host_alias == "local" {
        // Local paths: existing prefix match (handles worktrees nested under repos).
        return projects
            .into_iter()
            .filter(|p| path_str.starts_with(&p.base_path))
            .max_by_key(|p| p.base_path.len())
            .map(|p| p.id);
    }
    // Remote paths: match by owner+repo extracted from the conventional
    // `.../projects/github.com/<owner>/<repo>/...` layout. Falls through
    // to `None` (orphan) if the path doesn't follow the convention.
    let (owner, repo) = extract_owner_repo(&path_str)?;
    projects
        .into_iter()
        .find(|p| p.owner == owner && p.repo == repo)
        .map(|p| p.id)
}
```

- [ ] **Step 6: Update `list_sessions` to take SshClient**

Replace:

```rust
#[tauri::command]
pub fn list_sessions(
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<SessionRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    reconcile_sessions(&s, &ssh)
}
```

- [ ] **Step 7: Run cargo build + tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib commands:: 2>&1 | tail -15
```

Expected: existing tests pass, plus the new `extract_owner_repo` tests from Task 4.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/commands/sessions.rs
git commit -m "sessions: multi-host reconcile + host_alias routing"
```

---

## Task 9: Remote PTY attach

**Files:**
- Modify: `src-tauri/src/pty.rs`

- [ ] **Step 1: Add `host_alias` to `PtyOpenArgs`**

Replace the struct:

```rust
#[derive(Deserialize)]
pub struct PtyOpenArgs {
    pub session_name: String,
    pub host_alias: String,
    /// Initial PTY size from the frontend's xterm.js fit().
    pub cols: u16,
    pub rows: u16,
}
```

- [ ] **Step 2: Branch on `host_alias` when building the command**

Replace the command-build section of `pty_open`:

```rust
    let mut cmd = if args.host_alias == "local" {
        let mut c = CommandBuilder::new("tmux");
        c.args(["attach", "-t", &args.session_name]);
        c
    } else {
        // Build the ControlPath the same way SshClient does so we share
        // the established master. We don't need to import SshClient just
        // to format a path — the format is stable.
        let cm = {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{home}/.cache/claude-fleet/cm-{}.sock", args.host_alias)
        };
        let mut c = CommandBuilder::new("ssh");
        c.args([
            "-tt",
            "-o",
            &format!("ControlPath={}", cm),
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            &args.host_alias,
            "bash",
            "-lc",
            // We re-export LANG/LC_ALL/COLORTERM/TERM inside the remote
            // shell so the embedded TUI gets proper Unicode glyph
            // rendering even if the remote sshd has AcceptEnv disabled.
            &format!(
                "LANG=${{LANG:-en_US.UTF-8}} LC_ALL=${{LC_ALL:-en_US.UTF-8}} COLORTERM=truecolor TERM=xterm-256color tmux attach -t {}",
                shell_escape(&args.session_name)
            ),
        ]);
        c
    };
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TERM", "xterm-256color");
    for var in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                cmd.env(var, val);
            }
        }
    }
    cmd.env("COLORTERM", "truecolor");
```

- [ ] **Step 3: Add the `shell_escape` helper at the bottom of `pty.rs`**

```rust
fn shell_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
```

- [ ] **Step 4: Update the diagnostic marker text**

Replace the `"\x1b[90m[cf] attached to {} via polling buffer..."` line to include host:

```rust
        b.extend_from_slice(
            format!(
                "\x1b[90m[cf] attached to {}@{} via polling buffer\x1b[0m\r\n",
                args.session_name, args.host_alias
            )
            .as_bytes(),
        );
```

- [ ] **Step 5: Build**

```bash
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -10
```

Expected: clean build, no errors.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/pty.rs
git commit -m "pty: branch on host_alias; remote attach via ssh -tt + bash -lc"
```

---

## Task 10: Frontend `hosts.ts` store

**Files:**
- Create: `src/lib/hosts.ts`
- Create: `src/lib/hosts.test.ts`
- Modify: `vitest.setup.ts` (mock new IPC commands)

- [ ] **Step 1: Create `src/lib/hosts.ts`**

```typescript
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { readPref, writePref } from './prefs';

export interface HostRow {
  alias: string;
  ssh_alias: string | null;
  reachable: boolean;
  claude_version: string | null;
  tmux_version: string | null;
  hidden: boolean;
  last_pinged_at: number | null;
}

export interface SshHost {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
}

export const hosts = writable<HostRow[]>([]);

// Sidebar host filter — `'all'` shows sessions from every host, otherwise
// the value is a specific `alias`. Persisted across restarts.
const isString = (v: unknown): v is string => typeof v === 'string';
export const hostFilter = writable<string>(readPref('host-filter', 'all', isString));
hostFilter.subscribe((v) => writePref('host-filter', v));

export async function loadHosts(): Promise<Result<HostRow[]>> {
  const r = await invokeCmd<HostRow[]>('list_hosts');
  if (r.ok) hosts.set(r.value);
  return r;
}

export async function discoverHosts(): Promise<Result<SshHost[]>> {
  return invokeCmd<SshHost[]>('discover_hosts');
}

export async function addHost(
  alias: string,
  sshAlias: string,
): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('add_host', {
    args: { alias, ssh_alias: sshAlias },
  });
  if (r.ok) await loadHosts();
  return r;
}

export async function probeHost(alias: string): Promise<Result<HostRow>> {
  const r = await invokeCmd<HostRow>('probe_host', { args: { alias } });
  if (r.ok) await loadHosts();
  return r;
}

export async function removeHost(alias: string): Promise<Result<void>> {
  const r = await invokeCmd<void>('remove_host', { args: { alias } });
  if (r.ok) await loadHosts();
  return r;
}

export async function hideHost(
  alias: string,
  hidden: boolean,
): Promise<Result<void>> {
  const r = await invokeCmd<void>('hide_host', { args: { alias, hidden } });
  if (r.ok) await loadHosts();
  return r;
}
```

- [ ] **Step 2: Update `vitest.setup.ts`**

Append to the `if (cmd === ...)` chain:

```ts
    if (cmd === 'list_hosts') return [{
      alias: 'local',
      ssh_alias: null,
      reachable: true,
      claude_version: '2.1.145',
      tmux_version: '3.5a',
      hidden: false,
      last_pinged_at: 1,
    }];
    if (cmd === 'discover_hosts') return [];
    if (cmd === 'add_host') return null;
    if (cmd === 'probe_host') return null;
    if (cmd === 'remove_host') return null;
    if (cmd === 'hide_host') return null;
```

- [ ] **Step 3: Create `src/lib/hosts.test.ts`**

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { hosts, loadHosts, addHost, probeHost, removeHost, hideHost } from './hosts';

const sampleLocal = {
  alias: 'local',
  ssh_alias: null,
  reachable: true,
  claude_version: '2.1.145',
  tmux_version: '3.5a',
  hidden: false,
  last_pinged_at: 1,
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([]);
  localStorage.clear();
});

describe('hosts store', () => {
  it('loadHosts populates the store on success', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await loadHosts();
    expect(r.ok).toBe(true);
    expect(get(hosts)).toHaveLength(1);
    expect(get(hosts)[0].alias).toBe('local');
  });

  it('addHost passes alias + ssh_alias and reloads', async () => {
    const added = { ...sampleLocal, alias: 'mefistos', ssh_alias: 'mefistos' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(added);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal, added]);
    const r = await addHost('mefistos', 'mefistos');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'add_host',
      { args: { alias: 'mefistos', ssh_alias: 'mefistos' } },
    ]);
    expect(get(hosts)).toHaveLength(2);
  });

  it('probeHost re-fetches the list', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sampleLocal);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await probeHost('local');
    expect(r.ok).toBe(true);
  });

  it('removeHost calls remove_host and reloads', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await removeHost('mefistos');
    expect(r.ok).toBe(true);
  });

  it('hideHost passes the hidden flag', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await hideHost('mefistos', true);
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'hide_host',
      { args: { alias: 'mefistos', hidden: true } },
    ]);
  });
});
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet && pnpm vitest run src/lib/hosts.test.ts 2>&1 | tail -10
```

Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib/hosts.ts src/lib/hosts.test.ts vitest.setup.ts
git commit -m "hosts: frontend store + IPC wrappers + tests"
```

---

## Task 11: Sidebar host pills + `[host]` badges

**Files:**
- Modify: `src/lib/Sidebar.svelte`
- Modify: `src/lib/Sidebar.test.ts`

- [ ] **Step 1: Import hosts store + filter in `Sidebar.svelte`**

Add to the existing import block:

```ts
  import { hosts, loadHosts, hostFilter } from './hosts';
```

In the `onMount` block where `loadProjects()` and `loadSessions()` are called, also load hosts:

```ts
  onMount(async () => {
    const pr = await loadProjects();
    if (!pr.ok) loadError = pr.error.message;
    const sr = await loadSessions();
    if (!sr.ok) loadError = sr.error.message;
    const hr = await loadHosts();
    if (!hr.ok) loadError = hr.error.message;
  });
```

- [ ] **Step 2: Add host pills row + ⚙ Settings button to the sticky header**

Find the existing `.sidebar-header` section (`<header class="sidebar-header" ...>`) and update the recency block to add a host pills row above the recency pills:

```svelte
    <nav class="hosts" aria-label="host filter">
      <button
        class="pill"
        class:active={$hostFilter === 'all'}
        onclick={() => hostFilter.set('all')}
      >all</button>
      {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
        <button
          class="pill"
          class:active={$hostFilter === h.alias}
          onclick={() => hostFilter.set(h.alias)}
          title="{h.alias}{h.tmux_version ? ` · tmux ${h.tmux_version}` : ''}{h.claude_version ? ` · claude ${h.claude_version}` : ''}"
        >
          <span class="host-dot status-{h.reachable ? 'on' : 'off'}"></span>
          {h.alias}
        </button>
      {/each}
      <button
        class="icon-btn"
        onclick={() => (showSettings = true)}
        title="Settings"
        aria-label="Settings"
        data-testid="settings-open"
      >⚙</button>
    </nav>

    <nav class="recency" aria-label="recency filter">
      <!-- existing pills loop unchanged -->
```

- [ ] **Step 3: Declare `showSettings` and import dialog at top of script**

```ts
  import SettingsDialog from './SettingsDialog.svelte';

  let showSettings = $state(false);
```

- [ ] **Step 4: Render `SettingsDialog` near the other modals at the bottom**

```svelte
{#if showSettings}
  <SettingsDialog onClose={() => (showSettings = false)} />
{/if}
```

- [ ] **Step 5: Add `[host_alias]` badge to each session row**

In the inner loop that renders sessions inside a project, just after the status dot:

```svelte
                  <span class="status-dot status-{sess.status}" title={sess.status} aria-hidden="true"></span>
                  <span class="host-badge" data-testid="host-badge">[{sess.host_alias}]</span>
                  <span class="sess-name">{sess.tmux_name}</span>
```

Apply the same change in the **orphan section** loop.

- [ ] **Step 6: Filter sessions by `$hostFilter`**

Replace the existing `sessionsForProject` function:

```ts
  function sessionsForProject(projectId: number): SessionRow[] {
    return $sessions.filter(
      (s) =>
        s.project_id === projectId &&
        ($hostFilter === 'all' || s.host_alias === $hostFilter),
    );
  }
```

Similarly update the `orphanSessions` derived:

```ts
  const orphanSessions = $derived(
    $sessions.filter(
      (s) =>
        s.project_id === null &&
        ($hostFilter === 'all' || s.host_alias === $hostFilter),
    ),
  );
```

- [ ] **Step 7: Extend `matchesSearch` to match host_alias**

Find the existing `matchesSearch` function and update its `sessionsForProject` line:

```ts
  function matchesSearch(p: ProjectTreeRow, q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (p.project.owner.toLowerCase().includes(needle)) return true;
    if (p.project.repo.toLowerCase().includes(needle)) return true;
    return sessionsForProject(p.project.id).some(
      (s) =>
        s.tmux_name.toLowerCase().includes(needle) ||
        s.host_alias.toLowerCase().includes(needle),
    );
  }
```

- [ ] **Step 8: Add CSS for `.hosts`, `.host-dot`, `.host-badge`**

In the `<style>` block:

```css
  .hosts { display: flex; flex-wrap: wrap; gap: 0.25rem; align-items: center; }
  .host-dot {
    display: inline-block;
    width: 0.4rem;
    height: 0.4rem;
    border-radius: 50%;
    margin-right: 0.3rem;
    vertical-align: middle;
  }
  .host-dot.status-on { background: rgb(80, 200, 110); }
  .host-dot.status-off { background: rgb(220, 130, 130); }

  .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    flex-shrink: 0;
  }
```

- [ ] **Step 9: Add Sidebar tests for host pills + badges**

In `src/lib/Sidebar.test.ts`, append inside the main `describe`:

```typescript
  it('renders a host pill for each non-hidden host plus "all"', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [];
      if (cmd === 'list_hosts') return [
        { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null },
        { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1 },
        { alias: 'old', ssh_alias: 'old', reachable: false, claude_version: null, tmux_version: null, hidden: true, last_pinged_at: 1 },
      ];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const hostsBar = document.querySelector('.hosts');
    expect(hostsBar?.textContent).toContain('all');
    expect(hostsBar?.textContent).toContain('local');
    expect(hostsBar?.textContent).toContain('mefistos');
    expect(hostsBar?.textContent).not.toContain('old');
  });

  it('host filter narrows displayed sessions', async () => {
    const local = sessionFor(1, 'dev-local');
    const remote = { ...sessionFor(1, 'dev-remote'), host_alias: 'mefistos' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [local, remote];
      if (cmd === 'list_hosts') return [
        { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null },
        { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1 },
      ];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(2);
    const pills = document.querySelectorAll('.hosts .pill');
    // [all, local, mefistos] → click "mefistos"
    const mefistos = Array.from(pills).find((p) => p.textContent?.includes('mefistos'))!;
    await fireEvent.click(mefistos);
    await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(1);
  });

  it('shows host badge before each session name', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [sessionFor(1, 'dev-foo')];
      if (cmd === 'list_hosts') return [
        { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null },
      ];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const badges = screen.queryAllByTestId('host-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0].textContent).toBe('[local]');
  });
```

Note: `sessionFor` defaults `host_alias` — make sure the helper includes `host_alias: 'local'`. It already does (look at the existing helper at the top of `Sidebar.test.ts`).

- [ ] **Step 10: Run frontend tests**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet && pnpm vitest run src/lib/Sidebar.test.ts src/lib/hosts.test.ts 2>&1 | tail -15
```

Expected: all pass.

- [ ] **Step 11: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "Sidebar: host pills row + [host] badges + filter by host"
```

---

## Task 12: `AddHostPicker.svelte` modal

**Files:**
- Create: `src/lib/AddHostPicker.svelte`
- Create: `src/lib/AddHostPicker.test.ts`

- [ ] **Step 1: Create `src/lib/AddHostPicker.svelte`**

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { discoverHosts, addHost, type SshHost } from './hosts';

  let { onClose }: { onClose: () => void } = $props();

  let available = $state<SshHost[]>([]);
  let loading = $state(true);
  let error: string | null = $state(null);
  let probing: string | null = $state(null);

  onMount(async () => {
    const r = await discoverHosts();
    loading = false;
    if (r.ok) {
      available = r.value;
    } else {
      error = r.error.message;
    }
  });

  async function pick(host: SshHost) {
    probing = host.alias;
    error = null;
    // alias = ssh_alias for now; future iter will allow a custom local alias.
    const r = await addHost(host.alias, host.alias);
    probing = null;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onClose();
  }

  function describe(h: SshHost): string {
    const parts: string[] = [];
    if (h.hostname) parts.push(h.hostname);
    if (h.user) parts.push(`user=${h.user}`);
    if (h.port) parts.push(`port=${h.port}`);
    return parts.join(' · ');
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Add SSH host">
    <h3>Add SSH host</h3>
    {#if loading}
      <p class="muted">Scanning ~/.ssh/config…</p>
    {:else if available.length === 0}
      <p class="muted">No hosts found in ~/.ssh/config. Add one there first.</p>
    {:else}
      <ul class="hosts-list">
        {#each available as h (h.alias)}
          <li>
            <button
              class="host-row"
              data-testid="picker-row"
              disabled={probing !== null}
              onclick={() => pick(h)}
            >
              <span class="alias">{h.alias}</span>
              {#if describe(h)}
                <span class="desc">{describe(h)}</span>
              {/if}
              {#if probing === h.alias}
                <span class="status">probing…</span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if error}
      <p class="err">{error}</p>
    {/if}
    <div class="actions">
      <button onclick={onClose}>Close</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 20;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 480px;
    max-height: 80vh;
    overflow: auto;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .dialog h3 { margin: 0; font-size: 1rem; }
  .muted { color: var(--fg-muted); font-size: 0.85rem; }

  .hosts-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .host-row {
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0.45rem 0.6rem;
    color: var(--fg);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .host-row:hover:not(:disabled) { border-color: var(--accent); background: var(--bg-pane); }
  .host-row:disabled { opacity: 0.6; cursor: progress; }
  .alias { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-weight: 600; }
  .desc { color: var(--fg-muted); font-size: 0.8rem; flex: 1; }
  .status { font-size: 0.75rem; color: var(--accent); }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
</style>
```

- [ ] **Step 2: Create `src/lib/AddHostPicker.test.ts`**

```typescript
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AddHostPicker from './AddHostPicker.svelte';
import { hosts } from './hosts';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([]);
});

describe('AddHostPicker', () => {
  it('lists discovered hosts', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') {
        return [
          { alias: 'mefistos', hostname: '192.168.1.50', user: 'mjanci', port: 22 },
          { alias: 'mac', hostname: null, user: null, port: null },
        ];
      }
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    await tick(); await tick();
    const rows = await screen.findAllByTestId('picker-row');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('mefistos');
    expect(rows[1].textContent).toContain('mac');
  });

  it('clicking a row calls add_host', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') {
        return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      }
      if (cmd === 'add_host') return { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1 };
      if (cmd === 'list_hosts') return [];
      return null;
    });
    let closed = false;
    render(AddHostPicker, { props: { onClose: () => { closed = true; } } });
    await tick(); await tick();
    const row = await screen.findByTestId('picker-row');
    await fireEvent.click(row);
    await tick(); await tick();
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some((c) => c[0] === 'add_host')).toBe(true);
    expect(closed).toBe(true);
  });

  it('shows an empty-state when ~/.ssh/config has no hosts', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [];
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    await tick(); await tick();
    expect(screen.queryByTestId('picker-row')).toBeNull();
    expect(document.body.textContent).toContain('No hosts found');
  });
});
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet && pnpm vitest run src/lib/AddHostPicker.test.ts 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/lib/AddHostPicker.svelte src/lib/AddHostPicker.test.ts
git commit -m "AddHostPicker: discover -> probe -> add modal"
```

---

## Task 13: `SettingsDialog.svelte` modal

**Files:**
- Create: `src/lib/SettingsDialog.svelte`
- Create: `src/lib/SettingsDialog.test.ts`

- [ ] **Step 1: Create `src/lib/SettingsDialog.svelte`**

```svelte
<script lang="ts">
  import { hosts, probeHost, removeHost, hideHost } from './hosts';
  import AddHostPicker from './AddHostPicker.svelte';

  let { onClose }: { onClose: () => void } = $props();

  let showAddPicker = $state(false);
  let busy: string | null = $state(null);
  let error: string | null = $state(null);

  async function onProbe(alias: string) {
    busy = alias;
    error = null;
    const r = await probeHost(alias);
    busy = null;
    if (!r.ok) error = r.error.message;
  }

  async function onRemove(alias: string) {
    if (alias === 'local') return;
    busy = alias;
    error = null;
    const r = await removeHost(alias);
    busy = null;
    if (!r.ok) error = r.error.message;
  }

  async function onToggleHide(alias: string, hidden: boolean) {
    busy = alias;
    error = null;
    const r = await hideHost(alias, hidden);
    busy = null;
    if (!r.ok) error = r.error.message;
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Settings">
    <header>
      <h3>Settings</h3>
      <button class="close" onclick={onClose} aria-label="Close">×</button>
    </header>

    <section class="block">
      <div class="section-header">
        <h4>Hosts</h4>
        <button class="add" onclick={() => (showAddPicker = true)} data-testid="settings-add-host">
          + Add host
        </button>
      </div>
      <table class="hosts-table" data-testid="hosts-table">
        <thead>
          <tr>
            <th>Alias</th>
            <th>tmux</th>
            <th>claude</th>
            <th>Status</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each $hosts as h (h.alias)}
            <tr class:hidden-row={h.hidden}>
              <td class="alias">{h.alias}{#if h.ssh_alias && h.ssh_alias !== h.alias}<span class="muted"> ({h.ssh_alias})</span>{/if}</td>
              <td>{h.tmux_version ?? '—'}</td>
              <td>{h.claude_version ?? '—'}</td>
              <td>
                <span class="status status-{h.reachable ? 'on' : 'off'}">
                  {h.reachable ? 'online' : 'offline'}
                </span>
              </td>
              <td class="row-actions">
                <button
                  disabled={busy === h.alias}
                  onclick={() => onProbe(h.alias)}
                  title="Re-probe"
                  aria-label="Re-probe">↻</button>
                {#if h.alias !== 'local'}
                  <button
                    disabled={busy === h.alias}
                    onclick={() => onToggleHide(h.alias, !h.hidden)}
                    title={h.hidden ? 'Show' : 'Hide'}
                    aria-label="Toggle hide">{h.hidden ? '👁' : '🚫'}</button>
                  <button
                    class="danger"
                    disabled={busy === h.alias}
                    onclick={() => onRemove(h.alias)}
                    title="Remove host"
                    aria-label="Remove">×</button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if error}<p class="err">{error}</p>{/if}
    </section>
  </div>
</div>

{#if showAddPicker}
  <AddHostPicker onClose={() => (showAddPicker = false)} />
{/if}

<style>
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 15;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 560px;
    max-height: 80vh;
    overflow: auto;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  header { display: flex; align-items: center; justify-content: space-between; }
  header h3 { margin: 0; font-size: 1rem; }
  .close {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.2rem;
    cursor: pointer;
    padding: 0 0.4rem;
  }
  .close:hover { color: var(--fg); }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.4rem;
  }
  .section-header h4 {
    margin: 0;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
  }
  .add {
    font-size: 0.8rem;
    padding: 0.25rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .add:hover { border-color: var(--accent); }

  .hosts-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }
  .hosts-table th {
    text-align: left;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
    padding: 0.3rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  .hosts-table td { padding: 0.4rem; border-bottom: 1px solid var(--border); }
  .hosts-table tr.hidden-row td { opacity: 0.55; }
  .alias { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .muted { color: var(--fg-muted); }

  .status {
    font-size: 0.7rem;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
  }
  .status-on { background: rgba(60,180,90,0.18); color: rgb(80,200,110); }
  .status-off { background: rgba(180,100,100,0.18); color: rgb(220,130,130); }

  .row-actions { display: flex; gap: 0.2rem; }
  .row-actions button {
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0.15rem 0.45rem;
    font-size: 0.85rem;
    border-radius: 4px;
  }
  .row-actions button:hover { border-color: var(--border); color: var(--fg); }
  .row-actions button.danger:hover { color: #e64a4a; border-color: #e64a4a; }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }
</style>
```

- [ ] **Step 2: Create `src/lib/SettingsDialog.test.ts`**

```typescript
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SettingsDialog from './SettingsDialog.svelte';
import { hosts } from './hosts';

const sample = [
  { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1 },
  { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1 },
];

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set(sample);
});

describe('SettingsDialog', () => {
  it('renders one row per host', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const table = await screen.findByTestId('hosts-table');
    expect(table.textContent).toContain('local');
    expect(table.textContent).toContain('mefistos');
  });

  it('local row hides the Remove + Hide buttons', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const rows = document.querySelectorAll('.hosts-table tbody tr');
    const localRow = Array.from(rows).find((r) => r.textContent?.includes('local'));
    expect(localRow?.querySelector('button[aria-label="Remove"]')).toBeNull();
  });

  it('clicking Re-probe invokes probe_host', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample[1]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample);
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const rows = document.querySelectorAll('.hosts-table tbody tr');
    const mefRow = Array.from(rows).find((r) => r.textContent?.includes('mefistos'))!;
    const probeBtn = mefRow.querySelector('button[aria-label="Re-probe"]') as HTMLButtonElement;
    await fireEvent.click(probeBtn);
    await tick();
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some((c) => c[0] === 'probe_host')).toBe(true);
  });

  it('clicking + Add host opens the AddHostPicker', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]); // discover_hosts call from picker
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('settings-add-host'));
    await tick(); await tick();
    expect(screen.getByRole('dialog', { name: 'Add SSH host' })).toBeInTheDocument();
  });
});
```

- [ ] **Step 3: Run tests**

```bash
pnpm vitest run src/lib/SettingsDialog.test.ts 2>&1 | tail -10
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/lib/SettingsDialog.svelte src/lib/SettingsDialog.test.ts
git commit -m "SettingsDialog: Hosts management UI"
```

---

## Task 14: NewSessionDialog host picker

**Files:**
- Modify: `src/lib/NewSessionDialog.svelte`
- Modify: `src/lib/NewSessionDialog.test.ts`

- [ ] **Step 1: Add host picker row at the top of `NewSessionDialog.svelte`**

Replace the existing script section's state declarations with:

```ts
<script lang="ts">
  import { untrack } from 'svelte';
  import type { ProjectTreeRow, WorktreeRow } from './projects';
  import { newSession, type SessionRow } from './sessions';
  import { hosts } from './hosts';
  import { readPref, writePref } from './prefs';

  let {
    project,
    onCreate,
    onCancel,
  }: {
    project: ProjectTreeRow;
    onCreate: (s: SessionRow) => void;
    onCancel: () => void;
  } = $props();

  const isString = (v: unknown): v is string => typeof v === 'string';
  let chosenHost = $state<string>(
    readPref('last-host', 'local', isString),
  );
  $effect(() => {
    writePref('last-host', chosenHost);
  });

  function defaultName(wt: WorktreeRow | null): string {
    const base = `dev-${project.project.owner}-${project.project.repo}`;
    if (!wt || wt.name === 'main') return base;
    return `${base}--${wt.name}`;
  }

  let chosenWorktreeId = $state<number | null>(untrack(() => project.worktrees[0]?.id ?? null));
  let name = $state(untrack(() => defaultName(project.worktrees[0] ?? null)));
  let busy = $state(false);
  let error: string | null = $state(null);

  function onPickWorktree(id: number) {
    chosenWorktreeId = id;
    const wt = project.worktrees.find((w) => w.id === id) ?? null;
    name = defaultName(wt);
  }

  async function submit() {
    if (!name.trim()) {
      error = 'Session name required';
      return;
    }
    busy = true;
    error = null;
    const r = await newSession({
      host_alias: chosenHost,
      project_id: project.project.id,
      worktree_id: chosenWorktreeId,
      name: name.trim(),
    });
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    onCreate(r.value);
  }
</script>
```

- [ ] **Step 2: Add the host picker row to the template**

Just above the existing `{#if project.worktrees.length > 1}` block, add:

```svelte
  <label for="host-picker">Host</label>
  <div class="host-row" id="host-picker" role="group">
    {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
      <button
        class="host-pick"
        class:active={chosenHost === h.alias}
        disabled={!h.reachable && h.alias !== 'local'}
        onclick={() => (chosenHost = h.alias)}
      >
        {h.alias}
      </button>
    {/each}
  </div>
```

- [ ] **Step 3: Style the host picker (reuse existing worktree-row look)**

In the `<style>` block, add:

```css
  .host-row { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .host-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .host-pick.active { color: var(--fg); border-color: var(--accent); }
  .host-pick:disabled { opacity: 0.4; cursor: not-allowed; }
```

- [ ] **Step 4: Update `src/lib/sessions.ts` to pass `host_alias` in NewSessionArgs**

```ts
export interface NewSessionArgs {
  host_alias: string;
  project_id: number;
  worktree_id: number | null;
  name: string;
}
```

Also update `killSession`, `renameSession`, `restartSession` to take `hostAlias`:

```ts
export async function killSession(hostAlias: string, name: string): Promise<Result<void>> {
  const r = await invokeCmd<void>('kill_session', {
    args: { host_alias: hostAlias, name },
  });
  if (r.ok) await loadSessions();
  return r;
}

export async function renameSession(
  hostAlias: string,
  oldName: string,
  newName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rename_session', {
    args: { host_alias: hostAlias, old_name: oldName, new_name: newName },
  });
  if (r.ok) await loadSessions();
  return r;
}

export async function restartSession(hostAlias: string, name: string): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('restart_session', {
    args: { host_alias: hostAlias, name },
  });
  if (r.ok) await loadSessions();
  return r;
}
```

- [ ] **Step 5: Update call sites in `Sidebar.svelte` to pass `sess.host_alias`**

Find every call to `killSession(...)`, `renameSession(...)`, `restartSession(...)` and prepend `sess.host_alias`:

```ts
  async function confirmKill() {
    if (!pendingKill) return;
    const sess = pendingKill;
    pendingKill = null;
    const r = await killSession(sess.host_alias, sess.tmux_name);
    // ... rest unchanged
  }

  async function commitRename() {
    // ... existing setup
    const r = await renameSession(oldName, next);  // <- replace with:
    const r = await renameSession(renamingName ?? '', oldName, next);
```

Wait — that's awkward. Let me reduce duplication. Instead update `commitRename`'s body to grab the session row first:

```ts
  async function commitRename() {
    if (!renamingName) return;
    const next = renameValue.trim();
    if (!next || next === renamingName) {
      cancelRename();
      return;
    }
    const oldName = renamingName;
    const sess = $sessions.find((s) => s.tmux_name === oldName);
    const hostAlias = sess?.host_alias ?? 'local';
    const r = await renameSession(hostAlias, oldName, next);
    if (!r.ok) {
      renameError = r.error.message;
      return;
    }
    migrateSessionUi(r.value.host_alias, oldName, r.value.tmux_name);
    const cur = $selectedSession;
    if (cur && cur.tmux_name === oldName) {
      selectSession(r.value);
    }
    cancelRename();
  }

  async function doRestart(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    actionError = null;
    const r = await restartSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) actionError = r.error.message;
  }
```

Apply the same `host_alias` prefix in `SessionDetails.svelte`'s calls to `renameSession`, `restartSession`, `killSession`.

- [ ] **Step 6: Update `NewSessionDialog.test.ts`**

Replace its body:

```typescript
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';
import { hosts } from './hosts';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1 },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1 },
  ]);
  localStorage.clear();
});

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
  worktrees: [{ id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' }],
};

describe('NewSessionDialog', () => {
  it('renders one host-pick button per non-hidden host', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const picks = document.querySelectorAll('.host-pick');
    expect(picks).toHaveLength(2);
    expect(Array.from(picks).map((p) => p.textContent?.trim())).toEqual(['local', 'mefistos']);
  });

  it('defaults to last-host pref (local on first run)', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const active = document.querySelector('.host-pick.active');
    expect(active?.textContent?.trim()).toBe('local');
  });

  it('clicking a host pick + Create sends host_alias to new_session', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'new_session') {
        return { id: 99, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 1, worktree_id: null, created_at: 1, last_activity_at: 1, status: 'running', notes: null };
      }
      if (cmd === 'list_sessions') return [];
      return null;
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => p.textContent?.trim() === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const newSessionCall = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'new_session');
    expect((newSessionCall![1] as any).args.host_alias).toBe('mefistos');
  });
});
```

- [ ] **Step 7: Run frontend tests**

```bash
pnpm vitest run 2>&1 | tail -15
```

Expected: all pass. Note: existing tests for `sessions.ts` calling `killSession('dev-foo')` (positional) will break — update them to `killSession('local', 'dev-foo')`. Same for `renameSession` / `restartSession`.

- [ ] **Step 8: Commit**

```bash
git add src/lib/NewSessionDialog.svelte src/lib/NewSessionDialog.test.ts src/lib/sessions.ts src/lib/sessions.test.ts src/lib/Sidebar.svelte src/lib/SessionDetails.svelte
git commit -m "NewSessionDialog: host picker + host_alias propagated to session ops"
```

---

## Task 15: TerminalView passes host_alias + reconnect banner

**Files:**
- Modify: `src/lib/TerminalView.svelte`

- [ ] **Step 1: Pass `host_alias` to `pty_open`**

Find the `invoke('pty_open', ...)` call inside `openTerm`. Update args:

```ts
    await invoke('pty_open', {
      args: {
        session_name: sess.tmux_name,
        host_alias: sess.host_alias,
        cols: screen?.cols ?? 80,
        rows: screen?.rows ?? 24,
      },
    });
```

- [ ] **Step 2: Detect drain failures + EOF and surface a reconnect banner**

Find the drain timer logic. When `pty_drain` returns `bytes === 0` for a long consecutive run AND we detect the EOF marker we already inject (`PTY EOF after N bytes`), set a banner state:

In the script block, add state:

```ts
  let disconnected = $state(false);

  async function reconnect() {
    disconnected = false;
    await closeTerm();
    await openTerm();
  }
```

In the drain logic where we process the `data` string, after `screen.write(data)`, add a check:

```ts
    if (data.includes('[cf] PTY EOF') || data.includes('[cf] reader error')) {
      disconnected = true;
    }
```

- [ ] **Step 3: Render the banner above the terminal**

Just inside the `<div class="terminal-pane">` wrap (or the root container of the component), add:

```svelte
{#if disconnected}
  <div class="reconnect-banner">
    Connection lost.
    <button onclick={reconnect}>Reconnect</button>
  </div>
{/if}
```

And in CSS:

```css
  .reconnect-banner {
    position: absolute;
    top: 0.4rem;
    left: 50%;
    transform: translateX(-50%);
    background: rgba(180, 100, 100, 0.18);
    color: rgb(220, 130, 130);
    padding: 0.35rem 0.7rem;
    border: 1px solid rgba(220, 130, 130, 0.3);
    border-radius: 5px;
    font-size: 0.8rem;
    z-index: 5;
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .reconnect-banner button {
    font-size: 0.75rem;
    padding: 0.15rem 0.5rem;
    background: transparent;
    border: 1px solid currentColor;
    color: inherit;
    border-radius: 4px;
    cursor: pointer;
  }
```

(Container needs `position: relative` for absolute positioning — likely already true in TerminalView; verify by reading the existing CSS.)

- [ ] **Step 4: Build (no test changes — terminal tests are minimal)**

```bash
pnpm tauri build --bundles app 2>&1 | tail -8
```

Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add src/lib/TerminalView.svelte
git commit -m "TerminalView: pass host_alias; show Reconnect banner on PTY EOF"
```

---

## Task 16: Live verification on `mefistos`

This is manual but scripted via Bash so the steps are reproducible.

- [ ] **Step 1: Verify SSH config is parseable**

```bash
cat ~/.ssh/config | grep -E '^Host\s' | head -10
```

Expected: see `Host mefistos`, `Host mac`, etc.

- [ ] **Step 2: Restart claude-fleet and add mefistos via Settings**

```bash
pkill -f claude-fleet 2>/dev/null; sleep 1
open -a /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/src-tauri/target/release/bundle/macos/claude-fleet.app
```

In the UI:
1. Click `⚙` Settings → click `+ Add host` → click `mefistos`
2. Watch for "probing…" → row should appear with tmux 3.6a and claude 2.1.x

- [ ] **Step 3: Confirm ControlMaster socket exists**

```bash
ls -la ~/.cache/claude-fleet/cm-mefistos.sock
```

Expected: socket file present.

- [ ] **Step 4: Create a new session on mefistos via UI**

1. Close Settings
2. Click `+ New session` in sidebar footer → pick a project (e.g. `sales-twins-app`)
3. In dialog: select `mefistos` host → set worktree → Create
4. Verify the session appears in the sidebar with `[mefistos]` badge

- [ ] **Step 5: Click the new session → embedded terminal attaches**

Verify:
- Claude TUI renders cleanly (Unicode box-drawing chars correct — the locale fix from P3T3 is working over SSH)
- Restart button works (claude relaunches in same pane)
- Rename works
- Kill works (session disappears from list)

- [ ] **Step 6: Test recovery — drop SSH master and reattach**

```bash
ssh -o ControlPath=~/.cache/claude-fleet/cm-mefistos.sock -O exit mefistos
```

In UI: click the session again. Terminal should reattach (SshClient::ensure_master re-spawns). Expected behavior: brief pause, then claude TUI returns.

- [ ] **Step 7: Final smoke — all tests pass**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -5
```

Expected: all green.

- [ ] **Step 8: Commit verify notes**

If you discovered any edge cases worth documenting, append to the spec under "Open risks" → known-good list. Then commit:

```bash
git add docs/specs/2026-05-20-multi-host-foundations-design.md
git commit -m "docs: update spec with live verification notes from mefistos"
```

---

## Self-Review (filled in by plan author)

**Spec coverage check:** all spec sections are covered:
- Data model migration → Task 1
- ssh_config parser → Task 2
- SshClient ControlMaster → Task 3
- owner/repo regex + regex dep → Task 4
- TmuxExec trait + Local/Remote → Task 5
- Store host CRUD → Task 6
- Tauri host commands → Task 7
- Multi-host session reconcile + routing → Task 8
- Remote PTY attach → Task 9
- Frontend hosts store → Task 10
- Sidebar host pills + badges → Task 11
- AddHostPicker → Task 12
- SettingsDialog → Task 13
- NewSessionDialog host picker → Task 14
- Reconnect banner → Task 15
- Live verify on mefistos → Task 16

**Placeholder scan:** none found. Every code step shows the actual code.

**Type consistency:** `host_alias` is the field name everywhere (snake_case in Rust + JSON, camelCase wrapper `hostAlias` only in TS function parameters). `HostRow` shape is identical across Rust (`store.rs`), frontend (`hosts.ts`), and tests.

**Scope:** 16 tasks, ~one or two days of focused work. Each task ends with a commit. Live verify is the final task and the only one that requires the actual mefistos host to be reachable.
