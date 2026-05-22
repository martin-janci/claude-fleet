# Kill other instances on startup — design

**Date:** 2026-05-22
**Status:** Approved, ready for implementation plan

## Problem

`claude-fleet` runs as a single process that opens a shared appdata SQLite
database (`sk.rlt.claude-fleet/state.db`) and binds an embedded MCP control
server on `127.0.0.1:4180`. Both are singletons. When a second instance starts
— commonly a dev build launched while a release build (or another worktree's
dev build) is still running — the new process collides:

```
[mcp] could not bind 127.0.0.1:4180: Address already in use (os error 48)
```

and two instances racing on the same `state.db` is undefined behaviour. The
user wants a freshly launched instance to win: on startup it terminates every
other running instance of the app, **including instances built from a different
binary** (dev vs release, or a different worktree).

## Goal

On startup, before the app touches the database or binds any port, find all
other running `claude-fleet` processes and terminate them gracefully, leaving
exactly one instance (the new one) alive.

Active in **all builds** (dev and release) — a dev build kills a running
release build and vice versa.

## Approach

A new function `kill_other_instances()` called at the very top of `run()` in
`src-tauri/src/lib.rs`, **before** the env backfills and before
`tauri::Builder` runs setup (which opens the DB and binds the MCP port).
Running first means the freed DB lock and port 4180 are available by the time
setup executes.

### Identification

- Enumerate processes with the `sysinfo` crate.
- Determine our own executable file name from `std::env::current_exe()` →
  `file_name()` (normally `claude-fleet`). Match other processes whose
  executable file name equals ours.
- Matching on the file name (not the full path) is what makes this catch *every
  build*: dev (`target/debug/claude-fleet`), release bundle
  (`…/claude-fleet.app/Contents/MacOS/claude-fleet`), and any other worktree's
  binary all share the name `claude-fleet`.
- Exclude our own PID.

### Termination

For each matched PID:

1. Send `SIGTERM` (sysinfo `Process::kill_with(Signal::Term)`) so the other
   instance can release its SSH `ControlMaster` sockets and flush SQLite.
2. Poll for up to ~500ms for the matched PIDs to disappear.
3. Send `SIGKILL` to any survivors.

Log one `eprintln!` line per terminated PID, matching the existing
`[mcp]`-style stderr logging, e.g. `[startup] killed prior instance pid 96948`.

### Dependency

Add `sysinfo = "0.33"` to `src-tauri/Cargo.toml`.

## Units & testability

Separate the pure decision from the side-effecting kill, mirroring how
`lib.rs` already splits `compute_backfilled_path` (pure) from
`backfill_path_for_gui_launch` (side-effecting):

- **Pure (`instances_to_kill`)** — input: a list of `(pid, exe_file_name)`
  pairs, our own pid, and our own exe file name. Output: the list of pids to
  kill. Logic: keep entries whose name equals ours and whose pid differs from
  ours. Unit-tested.
- **Side-effecting (`kill_other_instances`)** — builds the process list via
  `sysinfo`, calls `instances_to_kill`, then performs the SIGTERM → poll →
  SIGKILL sequence and logging. Thin wrapper, not unit-tested.

### Test cases for `instances_to_kill`

- Excludes our own pid even when the name matches.
- Includes another pid with the same exe name.
- Ignores pids with a different exe name.
- Empty input → empty output.

## Out of scope

- The vite dev server (a `node` process on port 1420) is dev tooling, not an
  instance of this app, and is **not** touched. Two `tauri dev` runs from
  different worktrees can still collide on 1420; this design only frees the DB
  lock and the MCP port by removing the other app instance.
- No graceful "focus the existing window instead" behaviour (the Tauri
  single-instance plugin's model). The user explicitly wants the new instance
  to win by killing the old one.

## Risks

- Running `tauri dev` will kill an app instance the user had open
  intentionally. This is the explicitly requested behaviour ("even other
  build").
- A process unrelated to this app but coincidentally named `claude-fleet`
  would be matched. Considered acceptable — the name is specific.
