# Device Communication Phase 2a — SSH path O(1) per tick, second chances

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One SSH round trip per host per reconcile tick instead of 5–6 + N, no login shell per tmux call, and a second chance after a ControlMaster dies — without changing what the pane heuristic sees or how statuses are derived (that is Phase 2b).

**Architecture:** (1) `TmuxExec` gains `probe_snapshot`, a default composition of today's per-call methods that `RemoteTmux` overrides with ONE delimited script parsed in Rust; `claude agents --json` becomes `Option` and runs on its own cadence. (2) `SshClient` resolves a per-host toolchain (login `PATH`, `$HOME`, absolute `tmux`/`claude`) once and caches it beside `homes`; `RemoteTmux` then runs `sh -c 'export PATH=…; <script>'` instead of `bash -lc`, and forwards `PATH` into `tmux new-session`. (3) A mux-failure exit 255 resets the master and retries once; the PTY gets its own ControlPath with a gentler keepalive; the tunnel gets `ConnectTimeout`/`BatchMode`; clones go through a temp dir; scrollback is clamped and tmux output capped.

**Tech Stack:** Rust (tokio, async-trait, dashmap), POSIX sh, tmux 3.x, OpenSSH ControlMaster.

**Spec:** `docs/specs/2026-09-21-device-communication-analysis.md` — "Roadmap → Phase 2" items 5, 6, 7 and findings table B. Item 8 (status heuristic, hooks) is deliberately NOT in this plan.

## Global Constraints

- Every value interpolated into a shell string is quoted with `crate::shell::quote` (always single-quotes). No second quoter.
- Never hold the `Store` mutex across an `.await`.
- `run` / `run_cancellable` keep returning `Ok(Output)` for any exit status; `Err` stays reserved for spawn failure, wall clock and cancel.
- The batched probe script's section delimiter is the literal line prefix `---FLEET:`; pane output lines that start with it are escaped by prefixing one space (`sed 's/^---FLEET/ &/'`). The script ends with `---FLEET:end`; output without it is a failed probe.
- The analyzer input for a pane is exactly what `capture-pane -S -<PANE_TAIL_LINES> -p` prints today (no `tail`), so Phase 2b can change it in one place.
- `claude agents --json` cadence: **60 s** per host (`ReconcileDeps::agents_every`); the test constructor `ReconcileDeps::fake` uses **0 s** (every pass).
- `HOST_PROBE_TIMEOUT` = **65 s** with a test pinning `>= 2 × SshClient::default_wall_clock(10 s) + 5 s`.
- Toolchain resolve: marker `FLEET-TC`, interactive login shell first (`"${SHELL:-/bin/sh}" -ilc`), plain login shell (`-lc`) as fallback; a resolved toolchain is cached for the process lifetime, a failed resolve for **5 minutes**; `tmux`/`claude` are kept only when absolute (start with `/`).
- Mux-failure classifier: exit **255** AND stderr containing one of `mux_client_request_session`, `Control socket`, `read from master failed`, `Broken pipe`, `Connection closed by remote host`. Exactly one retry, with the remaining wall clock (floor **5 s**).
- PTY ControlPath `cm-<host>-tty.sock`, `ServerAliveInterval=15`, `ServerAliveCountMax=3`; the probe master keeps `5`/`2`.
- Tunnel argv gains `-o ConnectTimeout=10 -o BatchMode=yes`.
- `MAX_SCROLLBACK_LINES = 20_000`; `TMUX_OUTPUT_CAP = 8 MiB`.
- Cargo: `export CARGO_TARGET_DIR=/Volumes/CargoSD/target/device-communication-fa2aec`. Frontend untouched by this plan (still run `npx vitest run` in the CI mirror).
- Git: commit only from this worktree (`git -C`); never pull/push/checkout/stash/rebase inside a task. No attribution lines.
- `docs/control-api-reference.md` is generated: if a `#[tool]` description changes, `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.

## File map

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/tmux.rs` | `TmuxExec::list_claude_agents -> Option`, `probe_snapshot` + `ProbeSnapshot`, the batched script + parser, `remote_sh` with toolchain, `-e PATH` |
| `crates/fleet-core/src/service/sessions/reconcile.rs` | `probe_with_timeout` on the snapshot, agents cadence, `HostProbe.agent_rows: Option`, budget, probe-error log |
| `crates/fleet-core/src/service/reconcile_tests.rs` | `Fleet` harness answers the batched script |
| `crates/fleet-core/src/service/sessions/tests.rs` | fake `TmuxExec` impls updated; `probe_with_timeout` tests |
| `crates/fleet-core/src/ssh.rs` | `ssh_bin`, mux-failure retry, toolchain resolve + cache, `mux_opts_for_pty`, `shutdown_all` |
| `crates/fleet-core/src/ssh_fake.rs` | `Call::script()` understands `sh -c`, `FakeSsh::set_toolchain` |
| `src-tauri/src/pty.rs` | attach uses `mux_opts_for_pty` |
| `crates/fleet-core/src/service/tunnel.rs` | argv |
| `crates/fleet-core/src/service/sessions/lifecycle.rs`, `lifecycle_tests.rs` | clone via temp dir |
| `crates/fleet-core/src/service/sessions/prompt.rs` | scrollback clamp |
| `docs/hub.md`, `docs/control-api.md`, the spec | docs |

---

### Task 1: `list_claude_agents` is `Option`, agents on a cadence, a failure never prunes

**Files:**
- Modify: `crates/fleet-core/src/tmux.rs:63` (trait), `:315-321` (local impl), `:544-552` (remote impl)
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs:175-209` (`HostProbe`), `ReconcileDeps` (grep `pub(super) struct ReconcileDeps`), `:1099-1112` (`probe_one_host`), `:1163-1276` (`probe_with_timeout`), the `Ok(live)` arm that calls `reconcile_agent_rows` (grep `reconcile_agent_rows(` in the write path)
- Modify: every fake `TmuxExec` in `crates/fleet-core/src/service/sessions/tests.rs` (`AgentsTmux`, `IdentityTmux`, `FailingListTmux`, `SleepyTmux`, `HangingTmux`, `ScriptedTmux`, `AccountTmux`, `FakeTmux`) and any other `impl TmuxExec` `cargo check --tests` reports
- Test: `crates/fleet-core/src/service/sessions/tests.rs`, `crates/fleet-core/src/service/reconcile_tests.rs`

**Interfaces:**
- Produces: `TmuxExec::list_claude_agents(&self) -> Option<Vec<ClaudeAgentRow>>` (`None` = could not ask); `HostProbe.agent_rows: Option<Vec<ClaudeAgentRow>>`; `ReconcileDeps { agents_every: Duration, last_agents: DashMap<String, std::time::Instant>, .. }`; `probe_with_timeout(host, tmux, timeout, pr_probe, fetch_agents: bool)`; `pub(super) fn agents_due(deps: &ReconcileDeps, alias: &str, now: Instant) -> bool` (marks the host as probed when it returns true).

- [ ] **Step 1: Failing tests**

In `crates/fleet-core/src/service/sessions/tests.rs`, next to the existing `AgentsTmux` tests (grep `AgentsTmux`), add:

```rust
/// A `claude agents --json` that could not be asked (ssh 255, timeout) is
/// `None`, and a `None` never reaches the pruner: every bg row survives.
#[tokio::test]
async fn an_unanswerable_agents_call_keeps_every_bg_row() {
    struct NoAgentsAnswer;
    #[async_trait]
    impl TmuxExec for NoAgentsAnswer {
        async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
            Ok(Vec::new())
        }
        async fn new_session(&self, _: &str, _: &std::path::Path, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn kill_session(&self, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn restart_session(&self, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn capture_pane(&self, _: &str) -> Result<String, IpcError> { Ok(String::new()) }
        async fn capture_pane_scrollback(&self, _: &str, _: u32) -> Result<String, IpcError> { Ok(String::new()) }
        async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>> { None }
    }
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("h").unwrap();
    let host = store.list_hosts().unwrap().into_iter().find(|h| h.alias == "h").unwrap();
    let probe = probe_with_timeout(host, Box::new(NoAgentsAnswer), Duration::from_secs(5), None, true).await;
    assert!(probe.result.is_ok());
    assert!(probe.agent_rows.is_none(), "an unanswered call is None, not an empty list");
    assert!(probe.agent_mtimes.is_none());
}

#[tokio::test]
async fn agents_are_not_asked_when_the_cadence_says_no() {
    struct CountingAgents(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait]
    impl TmuxExec for CountingAgents {
        async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> { Ok(Vec::new()) }
        async fn new_session(&self, _: &str, _: &std::path::Path, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn kill_session(&self, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn restart_session(&self, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
        async fn capture_pane(&self, _: &str) -> Result<String, IpcError> { Ok(String::new()) }
        async fn capture_pane_scrollback(&self, _: &str, _: u32) -> Result<String, IpcError> { Ok(String::new()) }
        async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(Vec::new())
        }
    }
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("h").unwrap();
    let host = store.list_hosts().unwrap().into_iter().find(|h| h.alias == "h").unwrap();
    let probe = probe_with_timeout(host.clone(), Box::new(CountingAgents(calls.clone())), Duration::from_secs(5), None, false).await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(probe.agent_rows.is_none());
    let probe = probe_with_timeout(host, Box::new(CountingAgents(calls.clone())), Duration::from_secs(5), None, true).await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(probe.agent_rows, Some(Vec::new()));
}

#[test]
fn agents_due_fires_on_first_contact_then_only_after_the_cadence() {
    let deps = ReconcileDeps::fake(|_| Box::new(FakeTmux::default()), Duration::from_secs(5));
    // `fake` sets agents_every = 0 → always due.
    let t0 = std::time::Instant::now();
    assert!(agents_due(&deps, "h", t0));
    assert!(agents_due(&deps, "h", t0));
    let deps = ReconcileDeps { agents_every: Duration::from_secs(60), ..deps_with_cadence_for_tests() };
    assert!(agents_due(&deps, "h", t0), "first contact is due");
    assert!(!agents_due(&deps, "h", t0 + Duration::from_secs(20)));
    assert!(agents_due(&deps, "h", t0 + Duration::from_secs(61)));
    assert!(agents_due(&deps, "other", t0), "per host");
}
```
(`FakeTmux::default()` — if the existing `FakeTmux` has no `Default`, construct it the way its other tests do. `deps_with_cadence_for_tests()` is a small helper you add in the test module that builds a `ReconcileDeps` exactly like `ReconcileDeps::fake` does; if `ReconcileDeps` fields are not all `pub(super)`, add a `pub(super) fn with_agents_every(self, d: Duration) -> Self` builder on `ReconcileDeps` instead and use it — pick one and delete the other from the test.)

In `crates/fleet-core/src/service/reconcile_tests.rs`, near `bg_agents_surface_prune_and_filter_unknown_statuses`, add:

```rust
/// One unreachable `claude agents --json` (ssh 255) must not ghost a single
/// background row: the pruner is skipped, not fed an empty list.
#[tokio::test]
async fn a_failed_agents_call_does_not_ghost_bg_rows() {
    let f = Fleet::new(&["h"]);
    f.list("h", "");
    f.agents("h", r#"[{"id":"bg-1","kind":"background","status":"running","session_id":"s1"}]"#);
    f.pass().await;
    let before = { let s = f.store.lock().unwrap(); s.list_sessions_for_host("h").unwrap() };
    assert!(!before.is_empty(), "the bg row exists after the first pass: {before:?}");
    f.fake.on_host("h", Match::script_contains("claude agents --json"), Reply::Unreachable);
    f.pass().await;
    let after = { let s = f.store.lock().unwrap(); s.list_sessions_for_host("h").unwrap() };
    assert_eq!(after.iter().filter(|r| r.status != "ghost").count(), before.len(), "no row was ghosted: {after:?}");
}
```
(Read `bg_agents_surface_prune_and_filter_unknown_statuses` first and copy its exact agents JSON shape and the way it reads rows; the JSON above is a guess at the shape — the existing test is authoritative.)

- [ ] **Step 2: Run to verify they fail**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/device-communication-fa2aec
cargo test -p fleet-core --lib an_unanswerable_agents_call 2>&1 | tail -5
```
Expected: compile errors (`list_claude_agents` returns `Vec`, `probe_with_timeout` takes 4 args, `agents_due` missing).

- [ ] **Step 3: Trait and impls**

`crates/fleet-core/src/tmux.rs`: change the trait method to
```rust
    /// `claude agents --json` on the host. `None` when the host could not be
    /// asked (ssh failure, timeout, non-zero exit): the caller must not treat
    /// it as "no agents" — that is what ghosts every background row.
    async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>>;
```
Remote impl:
```rust
    async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>> {
        let output = self.remote_bash("claude agents --json 2>/dev/null").await.ok()?;
        if !output.status.success() {
            return None;
        }
        Some(crate::claude_agents::parse_claude_agents_json(
            &String::from_utf8_lossy(&output.stdout),
        ))
    }
```
Local impl: same shape over the local command (`None` on spawn error or non-zero exit). Drop the `|| echo '[]'`.

- [ ] **Step 4: Reconcile deps, cadence, probe**

In `reconcile.rs`:
- `HostProbe.agent_rows: Option<Vec<crate::claude_agents::ClaudeAgentRow>>` with doc: "`None`: not asked this pass (cadence) or unanswerable — the bg pruner is skipped."
- `ReconcileDeps` gains
```rust
    /// How often `claude agents --json` (a node cold start) is asked per
    /// host. `0` = every pass (tests).
    pub(super) agents_every: std::time::Duration,
    /// When each host was last asked.
    pub(super) last_agents: dashmap::DashMap<String, std::time::Instant>,
```
  production constructor sets `agents_every = Duration::from_secs(60)`, `ReconcileDeps::fake` sets `Duration::ZERO`.
```rust
/// Whether this pass asks `host` for its agents; records the ask.
pub(super) fn agents_due(deps: &ReconcileDeps, alias: &str, now: std::time::Instant) -> bool {
    let due = match deps.last_agents.get(alias) {
        Some(last) => now.duration_since(*last) >= deps.agents_every,
        None => true,
    };
    if due {
        deps.last_agents.insert(alias.to_string(), now);
    }
    due
}
```
- `probe_one_host`: `let fetch_agents = agents_due(deps, &host.alias, std::time::Instant::now());` and pass it.
- `probe_with_timeout(host, tmux, timeout, pr_probe, fetch_agents: bool)`: replace the agents block with
```rust
        let agent_rows = if fetch_agents && tmux_result.is_ok() {
            tmux.list_claude_agents().await
        } else {
            None
        };
        let agent_mtimes = match &agent_rows {
            None => None,
            Some(rows) => {
                let bg_ids: Vec<String> = rows.iter()
                    .filter(|a| a.kind == crate::claude_agents::AgentKind::Background)
                    .filter_map(|a| a.session_id.clone())
                    .collect();
                if bg_ids.is_empty() { Some(std::collections::HashMap::new()) } else { tmux.transcript_mtimes(&bg_ids).await }
            }
        };
```
  and the timeout arm's `agent_rows: Vec::new()` → `None`.
- In the `Ok(live)` write arm: wrap the `reconcile_agent_rows(...)` call in `if let Some(agents) = &probe.agent_rows { … }`; also anything else in that arm that reads `probe.agent_rows` (the `claude agents` status preference at ~574-576) must use `probe.agent_rows.as_deref().unwrap_or(&[])`.
- Update every test fake's `list_claude_agents` to return `Some(...)`; update every direct `probe_with_timeout(...)` call in tests to pass `true` as the fifth argument (except the new cadence test).

- [ ] **Step 5: Run, then the full suite**

```bash
cargo test -p fleet-core --lib service::sessions 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p fleet-core --lib service::reconcile_tests 2>&1 | grep -E "^test result|FAILED|panicked"
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
```
Expected: all `ok`.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/tmux.rs crates/fleet-core/src/service/sessions crates/fleet-core/src/service/reconcile_tests.rs
git commit -m "fix(reconcile): an unanswerable claude-agents call never prunes; agents asked on a 60 s cadence"
```

---

### Task 2: One delimited probe script per host

**Files:**
- Modify: `crates/fleet-core/src/tmux.rs` (trait: `probe_snapshot`; `ProbeSnapshot`; `PROBE_SNAPSHOT_SCRIPT` builder + `parse_probe_snapshot`; `RemoteTmux` override)
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs:16-27` (`HOST_PROBE_TIMEOUT`), `probe_with_timeout`, `capture_pane_intel` → `intel_from_tails`, the `Err(_e)` arm (~807)
- Modify: `crates/fleet-core/src/service/reconcile_tests.rs:65-140` (`Fleet` harness)
- Test: `crates/fleet-core/src/tmux.rs` `mod tests`, `sessions/tests.rs`, `reconcile_tests.rs`

**Interfaces:**
- Produces:
```rust
pub struct ProbeSnapshot {
    pub identity: Option<HostIdentity>,
    pub sessions: Result<Vec<TmuxSession>, IpcError>,
    pub account: Option<crate::service::hosts::OauthAccount>,
    /// Pane text per live session, exactly `capture-pane -S -<n> -p`.
    pub pane_tails: std::collections::HashMap<String, String>,
}
// TmuxExec:
async fn probe_snapshot(&self, tail_lines: u32) -> ProbeSnapshot  // default: composes the per-call methods
pub const PROBE_DELIM: &str = "---FLEET:";
pub fn probe_snapshot_script(tail_lines: u32) -> String;
pub fn parse_probe_snapshot(stdout: &str) -> Result<ProbeSnapshot, IpcError>;
// reconcile.rs:
pub(super) fn intel_from_tails(tails: &HashMap<String, String>) -> PaneIntelMap;
pub(crate) const HOST_PROBE_TIMEOUT: Duration = Duration::from_secs(65);
```

- [ ] **Step 1: Failing parser tests** (in `tmux.rs` `mod tests`)

```rust
    fn snapshot_text(sessions_rc: i32, sessions: &str, account: &str, panes: &[(&str, &str)]) -> String {
        let mut s = String::new();
        s.push_str("---FLEET:identity\nboot=abc-123\ntmuxrc=0\ntmuxout=4242\n");
        s.push_str(&format!("---FLEET:sessions\nrc={sessions_rc}\n{sessions}\n"));
        s.push_str(&format!("---FLEET:account\n{account}\n"));
        s.push_str("---FLEET:panes\n");
        for (name, tail) in panes {
            s.push_str(&format!("---FLEET:pane {name}\n{tail}\n"));
        }
        s.push_str("---FLEET:end\n");
        s
    }

    #[test]
    fn probe_snapshot_parses_every_section() {
        let text = snapshot_text(
            0,
            "dev-a|1700000000|1700000100|0|/home/u/p|%3\ndev-b|1700000000|1700000200|1|/home/u/q|%7",
            r#"{"accountUuid":"u-1","emailAddress":"a@b.c"}"#,
            &[("dev-a", "❯ \n? for shortcuts"), ("dev-b", "Thinking… (3s · esc to interrupt)")],
        );
        let snap = parse_probe_snapshot(&text).unwrap();
        let sessions = snap.sessions.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "dev-a");
        assert_eq!(snap.identity.unwrap().tmux_server_pid, Some(4242));
        assert_eq!(snap.account.unwrap().uuid.as_deref(), Some("u-1"));
        assert_eq!(snap.pane_tails["dev-b"], "Thinking… (3s · esc to interrupt)");
        assert_eq!(snap.pane_tails.len(), 2);
    }

    #[test]
    fn probe_snapshot_reads_no_server_running_as_zero_sessions() {
        let text = snapshot_text(1, "no server running on /tmp/tmux-501/default", "{}", &[]);
        let snap = parse_probe_snapshot(&text).unwrap();
        assert_eq!(snap.sessions.unwrap().len(), 0);
        assert!(snap.account.is_none());
        assert!(snap.pane_tails.is_empty());
    }

    #[test]
    fn probe_snapshot_refuses_garbage_and_truncation() {
        let text = snapshot_text(0, "this is not a session line", "{}", &[]);
        assert!(parse_probe_snapshot(&text).unwrap().sessions.is_err(), "garbage must not read as zero sessions");
        let mut truncated = snapshot_text(0, "", "{}", &[]);
        truncated.truncate(truncated.len() - "---FLEET:end\n".len());
        let e = parse_probe_snapshot(&truncated).unwrap_err();
        assert_eq!(e.code, "E_TMUX");
        assert!(e.message.contains("truncated"), "{}", e.message);
        let e = parse_probe_snapshot("ssh: connect to host h port 22: No route to host").unwrap_err();
        assert_eq!(e.code, "E_TMUX");
    }

    #[test]
    fn probe_snapshot_keeps_an_escaped_delimiter_inside_a_pane() {
        let text = snapshot_text(0, "dev-a|1|2|0|/p|%1", "{}", &[("dev-a", " ---FLEET:panes is just text\nline2")]);
        let snap = parse_probe_snapshot(&text).unwrap();
        assert_eq!(snap.pane_tails["dev-a"], " ---FLEET:panes is just text\nline2");
    }

    #[test]
    fn probe_snapshot_script_has_every_section_and_escapes_pane_lines() {
        let s = probe_snapshot_script(8);
        for section in ["---FLEET:identity", "---FLEET:sessions", "---FLEET:account", "---FLEET:panes", "---FLEET:end"] {
            assert!(s.contains(&format!("printf '%s\\n' '{section}'")), "{section} missing in {s}");
        }
        assert!(s.contains(HOST_IDENTITY_SCRIPT));
        assert!(s.contains(crate::service::hosts::OAUTH_ACCOUNT_SCRIPT));
        assert!(s.contains("tmux capture-pane -t \"=$s:\" -S -8 -p 2>/dev/null | sed 's/^---FLEET/ &/'"), "{s}");
        assert!(s.contains("while IFS= read -r s; do"), "{s}");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p fleet-core --lib tmux::tests::probe_snapshot 2>&1 | tail -3
```
Expected: compile errors.

- [ ] **Step 3: Script, parser, trait**

In `tmux.rs`:

```rust
/// Section delimiter of the batched probe. A pane line that starts with it
/// is escaped by the script (one leading space), so the parser never
/// mistakes pane text for a section.
pub const PROBE_DELIM: &str = "---FLEET:";

/// Everything a reconcile pass reads from a host, in ONE round trip.
pub struct ProbeSnapshot {
    pub identity: Option<HostIdentity>,
    pub sessions: Result<Vec<TmuxSession>, IpcError>,
    pub account: Option<crate::service::hosts::OauthAccount>,
    /// Pane text per live session, exactly `capture-pane -S -<n> -p`.
    pub pane_tails: std::collections::HashMap<String, String>,
}

const SESSIONS_FORMAT: &str = "#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}|#{pane_id}";

/// The batched probe: identity, the session list with its exit code, the
/// oauth account, then one pane capture per live session, each behind a
/// `---FLEET:` line. Ends with `---FLEET:end` so a capped or cut output is
/// recognisable. Pane lines starting with the delimiter get one leading
/// space so they cannot open a section.
pub fn probe_snapshot_script(tail_lines: u32) -> String {
    let start = scrollback_start(tail_lines);
    format!(
        "printf '%s\\n' '---FLEET:identity'; {HOST_IDENTITY_SCRIPT}; \
         printf '%s\\n' '---FLEET:sessions'; out=$(tmux list-sessions -F '{SESSIONS_FORMAT}' 2>&1); rc=$?; printf 'rc=%s\\n' \"$rc\"; printf '%s\\n' \"$out\"; \
         printf '%s\\n' '---FLEET:account'; {}; \
         printf '%s\\n' '---FLEET:panes'; \
         tmux list-sessions -F '#{{session_name}}' 2>/dev/null | while IFS= read -r s; do printf '%s\\n' \"---FLEET:pane $s\"; tmux capture-pane -t \"=$s:\" -S {start} -p 2>/dev/null | sed 's/^---FLEET/ &/'; done; \
         printf '%s\\n' '---FLEET:end'",
        crate::service::hosts::OAUTH_ACCOUNT_SCRIPT
    )
}
```
Check: `HOST_IDENTITY_SCRIPT` is a `pub const` in this file; `OAUTH_ACCOUNT_SCRIPT` is `pub(crate)` in hosts.rs; the `#{{session_name}}` doubles the braces for `format!`. The existing `list_sessions` uses the same `-F` string — replace its literal with `SESSIONS_FORMAT` so the two cannot drift.

```rust
/// Parse [`probe_snapshot_script`] output. `Err` only when the text is not
/// a probe at all (ssh's own error, or a capped/cut output without the end
/// marker); a section that is present but unusable degrades to that
/// section's "unknown" value, except the session list, whose garbage is an
/// `Err` inside the snapshot exactly as `list_sessions` reports it.
pub fn parse_probe_snapshot(stdout: &str) -> Result<ProbeSnapshot, IpcError> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        if let Some(name) = line.strip_prefix(PROBE_DELIM) {
            sections.push((name.to_string(), String::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !sections.iter().any(|(n, _)| n == "end") {
        return Err(IpcError::new(
            codes::E_TMUX,
            format!("probe output truncated or not a probe: {}", stdout.trim().lines().next().unwrap_or("")),
        ));
    }
    let section = |name: &str| sections.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_str());
    let identity = section("identity").and_then(parse_host_identity);
    let sessions = match section("sessions") {
        None => Err(IpcError::new(codes::E_TMUX, "probe output has no sessions section")),
        Some(body) => {
            let mut lines = body.lines();
            let rc: Option<i32> = lines.next().and_then(|l| l.strip_prefix("rc=")).and_then(|v| v.trim().parse().ok());
            let combined: String = lines.collect::<Vec<_>>().join("\n");
            match rc {
                Some(0) => parse_sessions_checked(&combined),
                Some(_) if is_no_server_running(&combined) => Ok(Vec::new()),
                Some(_) => Err(IpcError::new(codes::E_TMUX, combined.trim())),
                None => Err(IpcError::new(codes::E_TMUX, "probe output has no sessions exit code")),
            }
        }
    };
    let account = section("account").and_then(|b| crate::service::hosts::parse_oauth_account(b.trim()));
    let mut pane_tails = std::collections::HashMap::new();
    for (name, body) in &sections {
        if let Some(pane) = name.strip_prefix("pane ") {
            pane_tails.insert(pane.to_string(), body.trim_end_matches('\n').to_string());
        }
    }
    Ok(ProbeSnapshot { identity, sessions, account, pane_tails })
}
```
Note: `parse_sessions_checked` on an empty `combined` must still return `Ok(vec![])` (a live server with zero sessions prints nothing with rc 0) — read that function; if it refuses empty input, handle `combined.trim().is_empty()` → `Ok(Vec::new())` before calling it.

Trait addition (with a default):
```rust
    /// Everything a reconcile pass needs from the host. The default composes
    /// the per-call methods (local tmux, test fakes); `RemoteTmux` overrides
    /// it with one script so a pass costs one round trip, not 5 + N.
    async fn probe_snapshot(&self, tail_lines: u32) -> ProbeSnapshot {
        let identity = self.host_identity().await;
        let sessions = self.list_sessions().await;
        let account = if sessions.is_ok() { self.read_oauth_account().await } else { None };
        let mut pane_tails = std::collections::HashMap::new();
        if let Ok(live) = &sessions {
            for s in live {
                if let Ok(tail) = self.capture_pane_scrollback(&s.name, tail_lines).await {
                    pane_tails.insert(s.name.clone(), tail);
                }
            }
        }
        ProbeSnapshot { identity, sessions, account, pane_tails }
    }
```
`RemoteTmux` override:
```rust
    async fn probe_snapshot(&self, tail_lines: u32) -> ProbeSnapshot {
        let script = probe_snapshot_script(tail_lines);
        match self.remote_bash(&script).await {
            Err(e) => ProbeSnapshot { identity: None, sessions: Err(e), account: None, pane_tails: Default::default() },
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                match parse_probe_snapshot(&text) {
                    Ok(snap) => snap,
                    Err(e) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let why = if out.status.success() { e.message } else { format!("{} ({})", stderr.trim(), e.message) };
                        ProbeSnapshot { identity: None, sessions: Err(IpcError::new(codes::E_SSH, why)), account: None, pane_tails: Default::default() }
                    }
                }
            }
        }
    }
```

- [ ] **Step 4: Reconcile uses the snapshot**

In `reconcile.rs`:
- `HOST_PROBE_TIMEOUT` → `Duration::from_secs(65)`, doc: "Safety net above the per-call wall clock: the batched probe is one call (30 s wall clock, which already resets a wedged master), agents a second; 2 × 30 + 5." Add a test in this file's tests: `assert!(HOST_PROBE_TIMEOUT >= crate::ssh::SshClient::default_wall_clock(Duration::from_secs(10)) * 2 + Duration::from_secs(5))`.
- Replace `capture_pane_intel` with
```rust
pub(super) fn intel_from_tails(tails: &std::collections::HashMap<String, String>) -> PaneIntelMap {
    tails.iter()
        .filter(|(_, t)| !t.is_empty())
        .map(|(name, t)| (name.clone(), crate::service::pane_intel::analyze(t)))
        .collect()
}
```
- In `probe_with_timeout`'s inner future: `let snap = tmux.probe_snapshot(PANE_TAIL_LINES).await; let tmux_result = snap.sessions; let identity = if tmux_result.is_ok() { snap.identity } else { None }; let account = if tmux_result.is_ok() { snap.account } else { None }; let intel = intel_from_tails(&snap.pane_tails);` then the agents block from Task 1.
- The `Err(_e)` arm at ~807: rename to `Err(e)` and add `tracing::warn!(host = %host.alias, code = %e.code, error = %e.message, "[reconcile] host unreachable");` before the `apply_host_reconcile`.

- [ ] **Step 5: The `Fleet` harness answers one script**

In `reconcile_tests.rs`, `Fleet` keeps per-host pieces and re-registers the batched reply on every helper call (later registrations win in `FakeSsh`):

```rust
#[derive(Default, Clone)]
struct Pieces {
    sessions: Option<String>,   // None → "no server running", rc 1
    panes: Vec<(String, String)>,
    account: String,            // "{}" when unset
}

impl Fleet {
    fn rebuild(&self, host: &str) {
        let p = self.pieces.lock().unwrap().get(host).cloned().unwrap_or_default();
        let mut text = String::from("---FLEET:identity\nboot=boot-1\ntmuxrc=0\ntmuxout=100\n---FLEET:sessions\n");
        match &p.sessions {
            Some(lines) => { text.push_str("rc=0\n"); text.push_str(lines); text.push('\n'); }
            None => text.push_str("rc=1\nno server running on /tmp/tmux-1000/default\n"),
        }
        text.push_str("---FLEET:account\n");
        text.push_str(if p.account.is_empty() { "{}" } else { &p.account });
        text.push_str("\n---FLEET:panes\n");
        for (name, tail) in &p.panes {
            text.push_str(&format!("---FLEET:pane {name}\n{tail}\n"));
        }
        text.push_str("---FLEET:end\n");
        self.fake.on_host(host, Match::script_contains("---FLEET:end"), Reply::ok(&text));
    }
    fn list(&self, host: &str, lines: &str) {
        self.pieces.lock().unwrap().entry(host.to_string()).or_default().sessions = Some(lines.to_string());
        self.rebuild(host);
    }
    fn pane(&self, host: &str, name: &str, tail: &str) {
        let mut g = self.pieces.lock().unwrap();
        let p = g.entry(host.to_string()).or_default();
        p.panes.retain(|(n, _)| n != name);
        p.panes.push((name.to_string(), tail.to_string()));
        drop(g);
        self.rebuild(host);
    }
}
```
`Fleet` gets `pieces: std::sync::Mutex<HashMap<String, Pieces>>`; `Fleet::new` registers `local` through `rebuild("local")` (no sessions → "no server running") instead of the old `LIST_SCRIPT` rule; `agents()` stays a separate rule (`claude agents --json`). Any test that scripted `Reply::hang()` / garbage / `Unreachable` on `LIST_SCRIPT` must now do so on `Match::script_contains("---FLEET:end")` (grep `LIST_SCRIPT` and fix each site; delete the constant if unused). Any test that scripted an account via `OAUTH_ACCOUNT_SCRIPT` directly needs `Pieces.account` set — add a `fn account(&self, host, json)` helper mirroring `list`.

- [ ] **Step 6: Run, then the full suite**

```bash
cargo test -p fleet-core --lib tmux:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p fleet-core --lib service:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
```
Expected: all `ok`, including `wedged_host_probe_times_out_into_unreachable` and `multi_host_pass_isolates_timeout_and_garbage_hosts_and_frees_the_gate`.

- [ ] **Step 7: Real-tmux smoke test**

```bash
tmux new-session -d -s fleet-probe-smoke 'cat' && sleep 0.3
cat > /tmp/fleet-probe.sh <<'EOF'
printf '%s\n' '---FLEET:identity'; printf 'boot=%s\n' "x"; out=$(tmux list-sessions -F '#{pid}' 2>&1); rc=$?; printf 'tmuxrc=%s\n' "$rc"; printf 'tmuxout=%s\n' "$(printf '%s' "$out" | head -n 1)"; printf '%s\n' '---FLEET:sessions'; out=$(tmux list-sessions -F '#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}|#{pane_id}' 2>&1); rc=$?; printf 'rc=%s\n' "$rc"; printf '%s\n' "$out"; printf '%s\n' '---FLEET:account'; echo '{}'; printf '%s\n' '---FLEET:panes'; tmux list-sessions -F '#{session_name}' 2>/dev/null | while IFS= read -r s; do printf '%s\n' "---FLEET:pane $s"; tmux capture-pane -t "=$s:" -S -8 -p 2>/dev/null | sed 's/^---FLEET/ &/'; done; printf '%s\n' '---FLEET:end'
EOF
bash /tmp/fleet-probe.sh | head -20; tmux kill-session -t '=fleet-probe-smoke'; rm /tmp/fleet-probe.sh
```
Expected: every section header present, the `fleet-probe-smoke` line in sessions with `rc=0`, a `---FLEET:pane fleet-probe-smoke` section, and `---FLEET:end` last. Paste the output into the report.

- [ ] **Step 8: Commit**

```bash
git add crates/fleet-core/src/tmux.rs crates/fleet-core/src/service/sessions crates/fleet-core/src/service/reconcile_tests.rs
git commit -m "perf(reconcile): one delimited probe script per host instead of 5 + N ssh calls"
```

---

### Task 3: `SshClient`: a swappable ssh binary, and one retry after a mux failure

**Files:**
- Modify: `crates/fleet-core/src/ssh.rs` (`SshClientInner`, `build`, the eight `Command::new("ssh")` sites, `run_bounded`, `run_bounded_capped`, `run_cancellable`, `run_bounded_cancellable`)
- Test: `crates/fleet-core/src/ssh.rs` `mod tests`

**Interfaces:**
- Produces: `pub fn SshClient::with_ssh_binary(path: impl Into<std::path::PathBuf>) -> Self` (SSH-only client that spawns `path` instead of `ssh`); `pub(crate) fn is_mux_failure(out: &Output) -> bool`; retry semantics documented on `run_bounded`.

- [ ] **Step 1: Failing tests**

```rust
    fn fake_ssh(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("ssh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn mux_failures_are_exit_255_with_a_master_message() {
        let out = |code: i32, stderr: &str| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        };
        use std::os::unix::process::ExitStatusExt;
        assert!(is_mux_failure(&out(255, "mux_client_request_session: read from master failed")));
        assert!(is_mux_failure(&out(255, "Control socket connect(/x/cm.sock): Connection refused")));
        assert!(is_mux_failure(&out(255, "client_loop: send disconnect: Broken pipe")));
        assert!(!is_mux_failure(&out(255, "ssh: connect to host h port 22: No route to host")), "a dead host is not a mux failure");
        assert!(!is_mux_failure(&out(1, "mux_client_request_session: read from master failed")), "only 255");
    }

    #[tokio::test]
    async fn a_mux_failure_resets_the_master_and_retries_once() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("first-call-done");
        let bin = fake_ssh(dir.path(), &format!(
            "case \"$*\" in *'-O exit'*|*'-O check'*) exit 0;; esac\n\
             if [ ! -f '{m}' ]; then touch '{m}'; echo 'mux_client_request_session: read from master failed' >&2; exit 255; fi\n\
             echo ok",
            m = mark.display()
        ));
        let c = SshClient::with_ssh_binary(bin);
        let out = c.run("h-retry", &["true"], Duration::from_secs(1)).await.unwrap();
        assert!(out.status.success(), "the retry answers: {out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
        assert_eq!(c.master_reset_counts().get("h-retry"), Some(&1), "the master was reset before the retry");
    }

    #[tokio::test]
    async fn a_mux_failure_is_retried_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_ssh(dir.path(), "case \"$*\" in *'-O exit'*|*'-O check'*) exit 0;; esac\necho 'mux_client_request_session: read from master failed' >&2; exit 255");
        let c = SshClient::with_ssh_binary(bin);
        let out = c.run("h-twice", &["true"], Duration::from_secs(1)).await.unwrap();
        assert_eq!(out.status.code(), Some(255), "the second failure is returned, not retried again");
        assert_eq!(c.master_reset_counts().get("h-twice"), Some(&1));
    }

    #[tokio::test]
    async fn a_dead_host_is_not_retried() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("calls");
        let bin = fake_ssh(dir.path(), &format!("echo x >> '{c}'; echo 'ssh: connect to host h port 22: No route to host' >&2; exit 255", c = count.display()));
        let c = SshClient::with_ssh_binary(bin);
        let out = c.run("h-dead", &["true"], Duration::from_secs(1)).await.unwrap();
        assert_eq!(out.status.code(), Some(255));
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1, "exactly one attempt");
        assert!(c.master_reset_counts().get("h-dead").is_none());
    }
```
(`tempfile` is a dev-dependency of fleet-core? `grep tempfile crates/fleet-core/Cargo.toml`; if absent, use `std::env::temp_dir().join(format!("fleet-ssh-{}", uuid::Uuid::new_v4().simple()))` with `create_dir_all` and a cleanup at the end.)

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p fleet-core --lib ssh::tests::a_mux_failure 2>&1 | tail -3
```
Expected: compile errors (`with_ssh_binary`, `is_mux_failure` missing).

- [ ] **Step 3: Implement**

- `SshClientInner` gains `ssh_bin: std::path::PathBuf` (default `"ssh"`); `build` takes it; `with_ssh_binary(path)` = `Self::build_with(None, path.into())`. Add `fn ssh_command(&self) -> tokio::process::Command { tokio::process::Command::new(&self.inner.ssh_bin) }` and use it at every `Command::new("ssh")` site (also the `std::process::Command::new("ssh")` in `shutdown_all` → `std::process::Command::new(&self.inner.ssh_bin)`).
- Classifier:
```rust
/// ssh exiting 255 because its ControlMaster died under it (laptop sleep,
/// roaming, the remote sshd restarting) — as opposed to 255 because the host
/// is down. The former deserves a fresh master and one more try.
pub(crate) fn is_mux_failure(out: &Output) -> bool {
    if out.status.code() != Some(255) {
        return false;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    ["mux_client_request_session", "Control socket", "read from master failed", "Broken pipe", "Connection closed by remote host"]
        .iter()
        .any(|needle| stderr.contains(needle))
}
```
- Retry: introduce one private helper used by `run_bounded`, `run_bounded_capped`, `run_cancellable` and `run_bounded_cancellable` (NOT `upload_file`, whose stdin is consumed):
```rust
    /// Spawn `build()` under the wall clock; on a mux failure reset the
    /// master (counted) and run it once more with what is left of the wall
    /// clock (at least 5 s).
    async fn run_with_mux_retry(
        &self,
        host: &str,
        build: impl Fn() -> tokio::process::Command,
        wall_clock: Duration,
        token: Option<CancellationToken>,
        spawn_code: &str,
        max_output: Option<usize>,
    ) -> Result<Output, IpcError> {
        let started = tokio::time::Instant::now();
        let first = self.run_child_capped(host, build(), wall_clock, token.clone(), spawn_code, max_output).await?;
        if !is_mux_failure(&first) {
            return Ok(first);
        }
        tracing::warn!(host = %host, "[ssh] the ControlMaster died under a command; resetting it and retrying once");
        *self.inner.master_resets.entry(host.to_string()).or_insert(0) += 1;
        self.reset_master(host).await;
        let left = wall_clock.saturating_sub(started.elapsed()).max(Duration::from_secs(5));
        self.run_child_capped(host, build(), left, token, spawn_code, max_output).await
    }
```
  Each of the four callers builds its `Command` inside a closure (they already build one; wrap the construction in `let build = || { let mut cmd = self.ssh_command(); …; cmd };`). Agent-routed branches are unchanged (they return before this).
- Doc on `run_bounded`: one paragraph on the retry.

- [ ] **Step 4: Run, fmt, clippy, full suite, commit**

```bash
cargo test -p fleet-core --lib ssh:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
git add crates/fleet-core/src/ssh.rs crates/fleet-core/Cargo.toml Cargo.lock
git commit -m "fix(ssh): reset the ControlMaster and retry once when it dies under a command"
```

---

### Task 4: Resolve the host toolchain once; tmux under `sh -c` with the login PATH

**Files:**
- Modify: `crates/fleet-core/src/ssh.rs` (`HostToolchain`, `toolchain_script`, `parse_toolchain`, `SshClientInner.toolchains`, `SshClient::toolchain`, `SshExec::toolchain` default)
- Modify: `crates/fleet-core/src/tmux.rs` (`remote_bash` → `remote_sh`, `new_session` `-e PATH`)
- Modify: `crates/fleet-core/src/ssh_fake.rs` (`Call::script` for `sh -c`, `FakeSsh::set_toolchain`, `impl SshExec::toolchain`)
- Test: `ssh.rs` tests, `tmux.rs` tests, `ssh_fake.rs` tests

**Interfaces:**
- Produces:
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostToolchain { pub home: String, pub path: String, pub tmux: Option<String>, pub claude: Option<String> }
pub const TOOLCHAIN_MARKER: &str = "FLEET-TC";
pub fn toolchain_script(interactive: bool) -> String;
pub fn parse_toolchain(stdout: &str) -> Option<HostToolchain>;
// SshExec:
async fn toolchain(&self, host: &str) -> Option<HostToolchain> { None }   // default
// SshClient: cached resolve (positive forever, negative 5 min)
pub const TOOLCHAIN_RETRY_AFTER: Duration = Duration::from_secs(300);
// FakeSsh:
pub fn set_toolchain(&self, host: &str, tc: HostToolchain) -> &Self;
// tmux.rs:
pub const TMUX_OUTPUT_CAP: usize = 8 * 1024 * 1024;
```

- [ ] **Step 1: Failing tests**

`ssh.rs` tests:
```rust
    #[test]
    fn toolchain_script_asks_the_users_shell_and_marks_every_line() {
        let s = toolchain_script(true);
        assert!(s.starts_with("\"${SHELL:-/bin/sh}\" -ilc "), "{s}");
        assert!(s.contains("FLEET-TC home=%s"), "{s}");
        assert!(s.contains("command -v tmux 2>/dev/null || true"), "{s}");
        assert!(s.contains("command -v claude 2>/dev/null || true"), "{s}");
        assert!(s.ends_with("</dev/null 2>/dev/null"), "{s}");
        assert!(toolchain_script(false).starts_with("\"${SHELL:-/bin/sh}\" -lc "));
    }

    #[test]
    fn parse_toolchain_ignores_shell_noise_and_keeps_only_absolute_binaries() {
        let out = "Welcome to fishbowl\nFLEET-TC home=/Users/u\nFLEET-TC path=/opt/homebrew/bin:/usr/bin:/bin\nFLEET-TC tmux=/opt/homebrew/bin/tmux\nFLEET-TC claude=claude: aliased to /Users/u/.local/bin/claude\n";
        let tc = parse_toolchain(out).unwrap();
        assert_eq!(tc.home, "/Users/u");
        assert_eq!(tc.path, "/opt/homebrew/bin:/usr/bin:/bin");
        assert_eq!(tc.tmux.as_deref(), Some("/opt/homebrew/bin/tmux"));
        assert_eq!(tc.claude, None, "an alias text is not a path");
        assert!(parse_toolchain("FLEET-TC home=/u\n").is_none(), "no PATH, no toolchain");
        assert!(parse_toolchain("").is_none());
    }

    #[tokio::test]
    async fn toolchain_is_resolved_once_and_a_failure_is_retried_after_the_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("calls");
        let bin = fake_ssh(dir.path(), &format!(
            "echo x >> '{c}'\n\
             n=$(wc -l < '{c}')\n\
             if [ \"$n\" -le 2 ]; then exit 255; fi\n\
             printf 'FLEET-TC home=/h\\nFLEET-TC path=/p/bin:/usr/bin\\nFLEET-TC tmux=/p/bin/tmux\\nFLEET-TC claude=\\n'",
            c = count.display()
        ));
        let c = SshClient::with_ssh_binary(bin);
        assert!(c.toolchain("h").await.is_none(), "interactive and login both failed");
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2, "one interactive + one login attempt");
        assert!(c.toolchain("h").await.is_none(), "negative cache: no new call");
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
        c.forget_toolchain_for_tests("h");
        let tc = c.toolchain("h").await.expect("resolved");
        assert_eq!(tc.path, "/p/bin:/usr/bin");
        assert_eq!(tc.tmux.as_deref(), Some("/p/bin/tmux"));
        let calls_after = std::fs::read_to_string(&count).unwrap().lines().count();
        assert!(c.toolchain("h").await.is_some());
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), calls_after, "positive cache: no new call");
    }
```
(`forget_toolchain_for_tests` is a `#[cfg(test)] pub(crate) fn` that removes the entry — the negative TTL is 5 min, too long for a test.)

`tmux.rs` tests:
```rust
    #[tokio::test]
    async fn remote_tmux_runs_under_sh_with_the_login_path_once_the_toolchain_is_known() {
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.set_toolchain("h", crate::ssh::HostToolchain {
            home: "/h".into(), path: "/opt/homebrew/bin:/usr/bin".into(), tmux: Some("/opt/homebrew/bin/tmux".into()), claude: None,
        });
        fake.on_host("h", crate::ssh_fake::Match::script_contains("tmux list-sessions"), crate::ssh_fake::Reply::ok(""));
        let t = RemoteTmux { client: fake.clone(), host: "h".into() };
        let _ = t.list_sessions().await;
        let call = fake.calls_for("h").pop().unwrap();
        assert_eq!(&call.args[..2], &["sh".to_string(), "-c".to_string()], "{:?}", call.args);
        let body = crate::ssh_fake::unquote(&call.args[2]).unwrap();
        assert!(body.starts_with("export PATH='/opt/homebrew/bin:/usr/bin'; "), "{body}");
        assert!(call.script().unwrap().starts_with("tmux list-sessions"), "Call::script strips the export prefix");
    }

    #[tokio::test]
    async fn remote_tmux_falls_back_to_a_login_shell_without_a_toolchain() {
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.on_host("h", crate::ssh_fake::Match::script_contains("tmux list-sessions"), crate::ssh_fake::Reply::ok(""));
        let t = RemoteTmux { client: fake.clone(), host: "h".into() };
        let _ = t.list_sessions().await;
        let call = fake.calls_for("h").pop().unwrap();
        assert_eq!(&call.args[..2], &["bash".to_string(), "-lc".to_string()]);
    }

    #[tokio::test]
    async fn remote_new_session_forwards_the_login_path_into_the_pane() {
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.set_toolchain("h", crate::ssh::HostToolchain { home: "/h".into(), path: "/a:/b".into(), tmux: None, claude: None });
        let t = RemoteTmux { client: fake.clone(), host: "h".into() };
        t.new_session("s", std::path::Path::new("/w"), "cl").await.unwrap();
        let script = fake.calls_for("h").pop().unwrap().script().unwrap();
        assert!(script.contains(" -e PATH='/a:/b'"), "{script}");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p fleet-core --lib toolchain 2>&1 | tail -3
```
Expected: compile errors.

- [ ] **Step 3: Implement in `ssh.rs`**

```rust
/// What the user's own shell knows about a host, resolved once: the login
/// `PATH` (Homebrew, `~/.local/bin`, nvm…), `$HOME`, and where `tmux` and
/// `claude` are. Lets every tmux call run under `sh -c` with that PATH
/// instead of paying for `bash -l` on each call, and lets a new pane inherit
/// the same PATH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostToolchain {
    pub home: String,
    pub path: String,
    pub tmux: Option<String>,
    pub claude: Option<String>,
}

pub const TOOLCHAIN_MARKER: &str = "FLEET-TC";
/// A failed resolve is retried after this long, so one bad first contact
/// does not pin a host to the slow path for the process lifetime.
pub const TOOLCHAIN_RETRY_AFTER: Duration = Duration::from_secs(300);

/// The script that prints the toolchain, run by the user's shell. The
/// interactive variant (`-ilc`) sees `.zshrc`/`.bashrc` PATH additions, which
/// is where Homebrew and `cl` usually live on a Mac; the login variant
/// (`-lc`) is the fallback when an rc file misbehaves without a tty. Noise
/// from rc files is harmless: only marked lines are read.
pub fn toolchain_script(interactive: bool) -> String {
    let flags = if interactive { "-ilc" } else { "-lc" };
    let inner = format!(
        "printf '{m} home=%s\\n{m} path=%s\\n{m} tmux=%s\\n{m} claude=%s\\n' \"$HOME\" \"$PATH\" \"$(command -v tmux 2>/dev/null || true)\" \"$(command -v claude 2>/dev/null || true)\"",
        m = TOOLCHAIN_MARKER
    );
    format!("\"${{SHELL:-/bin/sh}}\" {flags} {} </dev/null 2>/dev/null", crate::shell::quote(&inner))
}

pub fn parse_toolchain(stdout: &str) -> Option<HostToolchain> {
    let mut home = None; let mut path = None; let mut tmux = None; let mut claude = None;
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix(TOOLCHAIN_MARKER).and_then(|r| r.strip_prefix(' ')) else { continue };
        let Some((k, v)) = rest.split_once('=') else { continue };
        let v = v.trim().to_string();
        match k {
            "home" if !v.is_empty() => home = Some(v),
            "path" if !v.is_empty() => path = Some(v),
            "tmux" if v.starts_with('/') => tmux = Some(v),
            "claude" if v.starts_with('/') => claude = Some(v),
            _ => {}
        }
    }
    Some(HostToolchain { home: home?, path: path?, tmux, claude })
}
```
`SshClientInner` gains `toolchains: DashMap<String, (std::time::Instant, Option<HostToolchain>)>`. On `SshClient`:
```rust
    /// The host's toolchain, resolved on first use and cached. `None` when
    /// neither an interactive nor a login shell answered — callers fall back
    /// to `bash -lc`; the miss itself is cached for
    /// [`TOOLCHAIN_RETRY_AFTER`].
    pub async fn toolchain(&self, host: &str) -> Option<HostToolchain> {
        if let Some(entry) = self.inner.toolchains.get(host) {
            let (at, tc) = entry.value();
            if tc.is_some() || at.elapsed() < TOOLCHAIN_RETRY_AFTER {
                return tc.clone();
            }
        }
        let mut resolved = None;
        for interactive in [true, false] {
            let script = toolchain_script(interactive);
            let args = ["sh", "-c", &crate::shell::quote(&script)];
            if let Ok(out) = self.run(host, &args, Duration::from_secs(10)).await {
                if out.status.success() {
                    resolved = parse_toolchain(&String::from_utf8_lossy(&out.stdout));
                    if resolved.is_some() { break; }
                }
            }
        }
        match &resolved {
            Some(tc) => tracing::info!(host = %host, path = %tc.path, tmux = ?tc.tmux, claude = ?tc.claude, "[ssh] toolchain resolved"),
            None => tracing::warn!(host = %host, "[ssh] toolchain could not be resolved; tmux calls stay on bash -lc"),
        }
        self.inner.toolchains.insert(host.to_string(), (std::time::Instant::now(), resolved.clone()));
        resolved
    }

    #[cfg(test)]
    pub(crate) fn forget_toolchain_for_tests(&self, host: &str) {
        self.inner.toolchains.remove(host);
    }
```
Add to the `SshExec` trait: `async fn toolchain(&self, _host: &str) -> Option<HostToolchain> { None }` and implement it for `SshClient` by delegating to the inherent method (the `impl SshExec for SshClient` block).

- [ ] **Step 4: `ssh_fake.rs`**

- `FakeSsh` gets `toolchains: Arc<Mutex<HashMap<String, HostToolchain>>>` and `pub fn set_toolchain(&self, host: &str, tc: HostToolchain) -> &Self`; `impl SshExec for FakeSsh` gets `async fn toolchain(&self, host) -> Option<HostToolchain>` returning the stored one.
- `Call::script()`:
```rust
    /// For a `bash -lc '<script>'` call, or a `sh -c 'export PATH=…; <script>'`
    /// call (a toolchain-aware tmux call), the script with the outer
    /// `shell::quote` undone and the PATH export stripped. `None` for any
    /// other argv shape.
    pub fn script(&self) -> Option<String> {
        match self.args.as_slice() {
            [b, l, s] if b == "bash" && l == "-lc" => unquote(s),
            [b, l, s] if b == "sh" && l == "-c" => {
                let body = unquote(s)?;
                Some(match body.strip_prefix("export PATH=") {
                    Some(rest) => rest.split_once("; ").map(|(_, script)| script.to_string()).unwrap_or(body.clone()),
                    None => body,
                })
            }
            _ => None,
        }
    }
```
  (The quoted PATH cannot contain `; ` unless a PATH entry does; acceptable for the fake.)

- [ ] **Step 5: `tmux.rs`**

Rename `remote_bash` → `remote_sh` (keep a one-line `remote_bash` alias if many call sites; otherwise rename all):
```rust
pub const TMUX_OUTPUT_CAP: usize = 8 * 1024 * 1024;

    /// One tmux call on the host. With a resolved toolchain: `sh -c 'export
    /// PATH=<login PATH>; <script>'` — no login shell spawned per call. Without
    /// one: `bash -lc '<script>'` as before. Output is capped at
    /// [`TMUX_OUTPUT_CAP`] and the call bounded like every `run`.
    async fn remote_sh(&self, script: &str) -> Result<std::process::Output, IpcError> {
        let connect = std::time::Duration::from_secs(10);
        let args: Vec<String> = match self.client.toolchain(&self.host).await {
            Some(tc) => vec!["sh".into(), "-c".into(), quote(&format!("export PATH={}; {script}", quote(&tc.path)))],
            None => vec!["bash".into(), "-lc".into(), quote(script)],
        };
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        self.client
            .run_bounded_capped(&self.host, &argv, connect, SshClient::default_wall_clock(connect), TMUX_OUTPUT_CAP)
            .await
    }
```
`new_session` (remote): after the `LANG` line add
```rust
        if let Some(tc) = self.client.toolchain(&self.host).await {
            script.push_str(&format!(" -e PATH={}", quote(&tc.path)));
        }
```

- [ ] **Step 6: Run, fmt, clippy, full suite, commit**

```bash
cargo test -p fleet-core --lib ssh:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p fleet-core --lib tmux:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p fleet-core --lib service:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
git add crates/fleet-core/src/ssh.rs crates/fleet-core/src/ssh_fake.rs crates/fleet-core/src/tmux.rs
git commit -m "perf(ssh): resolve each host's login PATH once; tmux calls run under sh -c and panes inherit it"
```

---

### Task 5: The PTY gets its own ControlPath and keepalive; the tunnel gets connect/batch options

**Files:**
- Modify: `crates/fleet-core/src/ssh.rs` (`control_path_for_pty`, `mux_opts_for_pty`, `shutdown_all`)
- Modify: `src-tauri/src/pty.rs:514`
- Modify: `crates/fleet-core/src/service/tunnel.rs:170-197` and its argv tests (`tunnel_argv_opts_out_of_ssh_multiplexing`, `tunnel_argv_builds_reverse_forward`, line ~887)
- Test: `ssh.rs` tests, `tunnel.rs` tests, `src-tauri/src/pty.rs` tests (grep `attach_argv` tests)

- [ ] **Step 1: Failing tests**

`ssh.rs`:
```rust
    #[test]
    fn pty_mux_opts_use_their_own_socket_and_a_gentler_keepalive() {
        let c = SshClient::new();
        let opts = c.mux_opts_for_pty("h", Duration::from_secs(5)).join(" ");
        assert!(opts.contains("ControlPath=") && opts.contains("cm-h-tty.sock"), "{opts}");
        assert!(opts.contains("ServerAliveInterval=15") && opts.contains("ServerAliveCountMax=3"), "{opts}");
        assert!(opts.contains("ControlMaster=auto") && opts.contains("BatchMode=yes") && opts.contains("ConnectTimeout=5"), "{opts}");
        let probe = c.mux_opts("h", Duration::from_secs(5)).join(" ");
        assert!(probe.contains("ServerAliveInterval=5") && probe.contains("cm-h.sock"), "the probe master is unchanged: {probe}");
    }
```
`tunnel.rs`: extend `tunnel_argv_opts_out_of_ssh_multiplexing` (or add `tunnel_argv_bounds_the_connect_and_never_prompts`) asserting the argv contains `-o ConnectTimeout=10` and `-o BatchMode=yes` as adjacent pairs.
`src-tauri/src/pty.rs`: find the existing `attach_argv` test and add an assertion that the production attach passes `mux_opts_for_pty` — since `attach_argv` takes the opts as a parameter, test the call site by extracting `pub(crate) fn attach_mux_opts(ssh: &SshClient, host: &str) -> Vec<String>` (returns `Vec::new()` for `local`, else `ssh.mux_opts_for_pty(host, 5 s)`) and asserting its output contains `cm-h-tty.sock`.

- [ ] **Step 2: Run to verify they fail** (`cargo test -p fleet-core --lib pty_mux_opts`, `cargo test -p claude-fleet --lib attach_mux_opts`; expected: compile errors)

- [ ] **Step 3: Implement**

`ssh.rs`:
```rust
    /// The attached terminal's own ControlPath: a probe's master reset must
    /// never take the user's terminal down with it.
    pub fn control_path_for_pty(&self, host: &str) -> PathBuf {
        self.control_path(host).with_file_name(format!("cm-{host}-tty.sock"))
    }

    /// `mux_opts` for the interactive attach: its own socket, and a keepalive
    /// that tolerates a 45 s stall (Wi-Fi roam, VPN rekey) instead of 10 s.
    pub fn mux_opts_for_pty(&self, host: &str, timeout: Duration) -> Vec<String> {
        let mut opts = self.mux_opts(host, timeout);
        for pair in opts.chunks_mut(2) {
            match pair[1].as_str() {
                s if s.starts_with("ControlPath=") => pair[1] = format!("ControlPath={}", self.control_path_for_pty(host).display()),
                "ServerAliveInterval=5" => pair[1] = "ServerAliveInterval=15".into(),
                "ServerAliveCountMax=2" => pair[1] = "ServerAliveCountMax=3".into(),
                _ => {}
            }
        }
        opts
    }
```
`shutdown_all`: for every seen host also `-O exit` the pty socket path (same command shape, best effort).
`pty.rs`: `let mux_opts = attach_mux_opts(&ssh, &args.host_alias);` with the helper above.
`tunnel.rs`: add `"-o".into(), "ConnectTimeout=10".into(), "-o".into(), "BatchMode=yes".into(),` after `ExitOnForwardFailure=yes`; update the tests that compare whole argv.

- [ ] **Step 4: Run, fmt, clippy, full suite, commit**

```bash
cargo test -p fleet-core --lib ssh:: 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p fleet-core --lib service::tunnel 2>&1 | grep -E "^test result|FAILED|panicked"
cargo test -p claude-fleet --lib pty 2>&1 | grep -E "^test result|FAILED|panicked"
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
git add crates/fleet-core/src/ssh.rs crates/fleet-core/src/service/tunnel.rs src-tauri/src/pty.rs
git commit -m "fix(ssh): the terminal attach gets its own ControlMaster and keepalive; the tunnel bounds its connect"
```

---

### Task 6: Clone through a temp dir; clamp scrollback

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/lifecycle.rs:163-197` (`ensure_remote_project_script`)
- Modify: `crates/fleet-core/src/service/sessions/lifecycle_tests.rs` (`ensure_script_with_no_worktree_only_clones`, `ensure_script_quotes_paths_with_shell_metacharacters`, and any other test asserting the clone line)
- Modify: `crates/fleet-core/src/service/sessions/prompt.rs:511-525` (`capture_session_output`)
- Test: `lifecycle_tests.rs`, `sessions/tests.rs`

- [ ] **Step 1: Failing tests**

`lifecycle_tests.rs`:
```rust
/// A cancelled or killed clone must not leave a half `.git` that the
/// `[ ! -d root/.git ]` guard then treats as "already cloned" forever: the
/// clone lands in a sibling temp dir and is moved into place only when it
/// finished.
#[test]
fn ensure_script_clones_into_a_temp_dir_and_moves_it_into_place() {
    let script = ensure_remote_project_script("/home/u/projects/github.com/o/r", "git@github.com:o/r.git", None);
    assert!(script.contains("tmp=\"$(dirname -- '/home/u/projects/github.com/o/r')/.fleet-clone-$$\""), "{script}");
    assert!(script.contains("git clone 'git@github.com:o/r.git' \"$tmp\" && mv \"$tmp\" '/home/u/projects/github.com/o/r'"), "{script}");
    assert!(script.contains("|| { rm -rf \"$tmp\"; exit 1; }"), "{script}");
    assert!(script.contains("rm -rf \"$tmp\";"), "a stale temp dir from an earlier attempt is cleared first: {script}");
}
```
`sessions/tests.rs` (near `capture_session_output` tests, or new):
```rust
#[test]
fn scrollback_lines_are_clamped() {
    assert_eq!(clamp_scrollback(Some(5)), Some(5));
    assert_eq!(clamp_scrollback(Some(4_000_000_000)), Some(MAX_SCROLLBACK_LINES));
    assert_eq!(clamp_scrollback(None), None);
}
```

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

`lifecycle.rs` — the clone half of `ensure_remote_project_script` becomes:
```rust
    let mut script = format!(
        "set -e\n\
         if [ ! -d {root}/.git ]; then \
           mkdir -p \"$(dirname -- {root})\"; \
           tmp=\"$(dirname -- {root})/.fleet-clone-$$\"; rm -rf \"$tmp\"; \
           git clone {url} \"$tmp\" && mv \"$tmp\" {root} || {{ rm -rf \"$tmp\"; exit 1; }}; \
         fi\n",
        url = quote(clone_url),
    );
```
(Keep the worktree half unchanged. Update the existing script tests' `contains` strings for the new clone line.)

`prompt.rs`:
```rust
/// The most scrollback one capture reads. `capture-pane -S -<n>` with an
/// unbounded `n` pulls the whole history of a pane through ssh; nothing in
/// the UI or the control API needs more than this.
pub const MAX_SCROLLBACK_LINES: u32 = 20_000;

pub fn clamp_scrollback(lines: Option<u32>) -> Option<u32> {
    lines.map(|n| n.min(MAX_SCROLLBACK_LINES))
}
```
and in `capture_session_output`: `match clamp_scrollback(scrollback_lines) { … }`.

- [ ] **Step 4: Run, fmt, clippy, full suite, commit**

```bash
cargo test -p fleet-core --lib service::sessions 2>&1 | grep -E "^test result|FAILED|panicked"
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | head -20
git add crates/fleet-core/src/service/sessions
git commit -m "fix(sessions): clone into a temp dir and move on success; clamp capture scrollback"
```

---

### Task 7: Docs and the CI mirror

**Files:**
- Modify: `docs/hub.md` (host requirements: the interactive-shell toolchain resolve; the "A host that cannot be reached" section if it describes the probe; troubleshooting: `[ssh] toolchain could not be resolved` log line), `docs/control-api.md` (`capture_session`: `scrollback_lines` clamp at 20 000), `docs/troubleshooting.md` (a ControlMaster that died is reset and the command retried once; the terminal has its own master)
- Modify: `docs/specs/2026-09-21-device-communication-analysis.md` — under "### Phase 2" add: `Items 5–7 landed 2026-09-22 on branch feature/device-communication-phase-2 (plan: docs/superpowers/plans/2026-09-22-device-communication-phase-2a.md). Item 8 is Phase 2b. Deviations: no E_CL_MISSING — main's cl fallback (331d49f8) covers a missing cl; the toolchain is resolved from the user's interactive login shell and cached for the process lifetime, not persisted.`

- [ ] **Step 1: Write the docs** (three short paragraphs; grep each file for the section first).

- [ ] **Step 2: The full CI mirror, unpiped, each command in the foreground**

```bash
export CARGO_TARGET_DIR=/Volumes/CargoSD/target/device-communication-fa2aec
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo build -p fleet-hub --locked
pnpm install --frozen-lockfile
npx svelte-check
npx vitest run
pnpm run build
cargo test -p fleet-core reference_is_current
cargo test -p claude-fleet --lib verdict_gen
cargo test -p claude-fleet --lib backend::contract::tests
```
Expected: every command exits 0.

- [ ] **Step 3: Commit**

```bash
git add docs
git commit -m "docs: toolchain resolve, batched probe, ControlMaster retry; phase 2a landed"
```

---

## Self-review

**Spec coverage.** Item 5: Task 1 (`Option`, cadence, never prunes), Task 2 (one script, budget 65 s with the per-call wall clock now the effective bound so `maybe_reset_master` runs from `run_child`'s arm, probe error logged). Item 6: Task 4 (toolchain, `sh -c`, `-e PATH`); `E_CL_MISSING` deliberately dropped because main's `CL_FALLBACK` already launches Claude without `cl` — recorded in Task 7. Item 7: Task 3 (retry after reset), Task 5 (PTY ControlPath + keepalive, tunnel options), Task 6 (clone temp+mv, scrollback clamp), Task 4 (`TMUX_OUTPUT_CAP` via `run_bounded_capped`). Item 8: out of scope by design.

**Type consistency.** `probe_with_timeout(host, tmux, timeout, pr_probe, fetch_agents)` is five-argument in Tasks 1 and 2. `HostProbe.agent_rows: Option<Vec<ClaudeAgentRow>>` in Task 1 is what Task 2's write arm reads. `ProbeSnapshot` fields (`identity`, `sessions`, `account`, `pane_tails`) are used identically by the default trait impl, the `RemoteTmux` override and `probe_with_timeout`. `HostToolchain { home, path, tmux, claude }` is the same in `ssh.rs`, `ssh_fake.rs` and the `tmux.rs` tests. `SshClient::with_ssh_binary` (Task 3) is what Task 4's cache test uses. `mux_opts_for_pty` (Task 5) is the only new `ssh.rs` public method Task 5 needs.

**Placeholders.** None; where the plan could not read a fixture (the agents JSON shape in `reconcile_tests.rs`) it names the authoritative test to copy from.
