# Kill Other Instances On Startup — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On startup, `claude-fleet` terminates every other running instance of the app (any build) before it opens the DB or binds the MCP port, so the new instance always wins.

**Architecture:** A pure decision function `instances_to_kill` (testable) decides which PIDs to kill from a process list; a thin side-effecting `kill_other_instances` enumerates processes via `sysinfo`, calls the pure function, then does SIGTERM → poll ~500ms → SIGKILL. `run()` calls `kill_other_instances()` as its first statement, before env backfills and `tauri::Builder`.

**Tech Stack:** Rust, Tauri 2, `sysinfo` crate.

**Spec:** `docs/superpowers/specs/2026-05-22-kill-other-instances-on-startup-design.md`

---

### Task 1: Add the `sysinfo` dependency

**Files:**
- Modify: `src-tauri/Cargo.toml` (the `[dependencies]` section)

- [ ] **Step 1: Add the dependency**

In `src-tauri/Cargo.toml`, under `[dependencies]`, add this line (keep alphabetical-ish ordering near other single-line deps such as `tauri-plugin-opener = "2"`):

```toml
sysinfo = "0.33"
```

- [ ] **Step 2: Verify it resolves and compiles**

Run: `cd src-tauri && cargo build`
Expected: PASS — `Finished` with no errors. `Cargo.lock` now contains `sysinfo`.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "build: add sysinfo dependency"
```

---

### Task 2: Pure decision function `instances_to_kill`

**Files:**
- Modify: `src-tauri/src/lib.rs` (add the function near `compute_backfilled_path`, ~line 36; add tests in the existing `#[cfg(test)] mod path_backfill_tests` at ~line 386)

This is the testable core: given the running processes as `(pid, exe_file_name)` pairs, our own pid, and our own exe file name, return the pids to kill (same name, different pid).

- [ ] **Step 1: Write the failing tests**

Add these tests inside `mod path_backfill_tests` in `src-tauri/src/lib.rs` (after the last existing test, before the closing `}` of the module):

```rust
    #[test]
    fn instances_to_kill_excludes_self_even_with_matching_name() {
        let procs = [(100u32, "claude-fleet")];
        assert!(instances_to_kill(&procs, 100, "claude-fleet").is_empty());
    }

    #[test]
    fn instances_to_kill_picks_other_same_named_process() {
        let procs = [(100u32, "claude-fleet"), (200u32, "claude-fleet")];
        assert_eq!(instances_to_kill(&procs, 100, "claude-fleet"), vec![200]);
    }

    #[test]
    fn instances_to_kill_ignores_other_names() {
        let procs = [
            (100u32, "claude-fleet"),
            (200u32, "node"),
            (300u32, "tmux"),
        ];
        assert!(instances_to_kill(&procs, 100, "claude-fleet").is_empty());
    }

    #[test]
    fn instances_to_kill_handles_empty_list() {
        let procs: [(u32, &str); 0] = [];
        assert!(instances_to_kill(&procs, 100, "claude-fleet").is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test instances_to_kill`
Expected: FAIL to compile — `cannot find function instances_to_kill in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add this function in `src-tauri/src/lib.rs` immediately after `compute_backfilled_path` (after its closing `}`, around line 54):

```rust
/// Pure: given the running processes as `(pid, exe_file_name)` pairs, our own
/// pid and our own exe file name, return the pids of *other* instances of this
/// app — same executable file name, different pid. Lifted out of
/// `kill_other_instances` so the decision is unit-testable without touching
/// real processes.
fn instances_to_kill(procs: &[(u32, &str)], my_pid: u32, my_name: &str) -> Vec<u32> {
    procs
        .iter()
        .filter(|(pid, name)| *pid != my_pid && *name == my_name)
        .map(|(pid, _)| *pid)
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test instances_to_kill`
Expected: PASS — 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: instances_to_kill pure decision helper"
```

---

### Task 3: Side-effecting `kill_other_instances` and wire into `run()`

**Files:**
- Modify: `src-tauri/src/lib.rs` (add `kill_other_instances` after `instances_to_kill`; call it at the top of `run()` at ~line 238)

No new unit test — this wrapper sends real signals. It is exercised manually (verification step below).

- [ ] **Step 1: Write `kill_other_instances`**

Add this function in `src-tauri/src/lib.rs` directly after the `instances_to_kill` function added in Task 2:

```rust
/// Terminate every other running instance of this app before we open the DB or
/// bind the MCP port. Matches by executable file name, so it catches *all*
/// builds (dev `target/debug/claude-fleet`, release bundle, other worktrees).
/// SIGTERM first so the other instance can release its SSH ControlMasters and
/// flush SQLite, then SIGKILL any straggler after a short grace window.
fn kill_other_instances() {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

    let my_pid = std::process::id();
    let my_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let Some(my_name) = my_name else {
        eprintln!("[startup] could not resolve own exe name; skipping instance reaper");
        return;
    };

    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing(),
    );

    // Collect (pid, exe file name) for every process sysinfo can see.
    let procs: Vec<(u32, String)> = sys
        .processes()
        .iter()
        .map(|(pid, proc_)| {
            let name = proc_
                .exe()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| proc_.name().to_string_lossy().into_owned());
            (pid.as_u32(), name)
        })
        .collect();

    let proc_refs: Vec<(u32, &str)> =
        procs.iter().map(|(pid, name)| (*pid, name.as_str())).collect();
    let targets = instances_to_kill(&proc_refs, my_pid, &my_name);
    if targets.is_empty() {
        return;
    }

    for pid in &targets {
        if let Some(proc_) = sys.process(Pid::from_u32(*pid)) {
            proc_.kill_with(Signal::Term);
            eprintln!("[startup] sent SIGTERM to prior instance pid {pid}");
        }
    }

    // Poll up to ~500ms for graceful exit.
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        if targets
            .iter()
            .all(|pid| sys.process(Pid::from_u32(*pid)).is_none())
        {
            return;
        }
    }

    // SIGKILL whatever is left.
    for pid in &targets {
        if let Some(proc_) = sys.process(Pid::from_u32(*pid)) {
            proc_.kill();
            eprintln!("[startup] SIGKILLed unresponsive prior instance pid {pid}");
        }
    }
}
```

- [ ] **Step 2: Call it first thing in `run()`**

In `src-tauri/src/lib.rs`, `run()` currently begins (~line 238) with a long comment block followed by `if !env_looks_complete() {`. Insert the call as the first statement of `run()`, immediately after the opening `pub fn run() {` line and before the existing leading comment:

```rust
pub fn run() {
    // Win the singleton race before opening the DB or binding the MCP port:
    // kill any other running instance of this app (any build).
    kill_other_instances();

    // Layered env recovery for Finder-launched apps:
```

(Keep the rest of the existing comment and body unchanged.)

- [ ] **Step 3: Verify it compiles**

Run: `cd src-tauri && cargo build`
Expected: PASS — `Finished`, no warnings about unused imports.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: kill other app instances on startup"
```

---

### Task 4: Full verification

**Files:** none (verification only)

- [ ] **Step 1: Lint & format**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS — no warnings, no formatting diff. (If `cargo fmt --check` reports a diff, run `cargo fmt`, then `git commit -am "chore(fmt): cargo fmt"`.)

- [ ] **Step 2: Backend tests**

Run: `cd src-tauri && cargo test`
Expected: PASS — all tests pass, including the 4 new `instances_to_kill_*` tests.

- [ ] **Step 3: Manual end-to-end check**

1. Launch one instance: `pnpm tauri dev` (wait for the window).
2. In a second terminal, confirm it is running: `pgrep -fl "target/debug/claude-fleet"` — note the PID.
3. Launch a second instance: `pnpm tauri dev` again from another shell (or rebuild + run the binary directly).
4. Expected: the second instance's stderr shows `[startup] sent SIGTERM to prior instance pid <first-pid>`, the first window disappears, and the second instance starts **without** the `[mcp] could not bind 127.0.0.1:4180: Address already in use` error.
5. Confirm exactly one process remains: `pgrep -fl "target/debug/claude-fleet"` shows a single PID.

- [ ] **Step 4: Final commit (if fmt/clippy required changes)**

```bash
git status   # clean if nothing changed in this task
```
