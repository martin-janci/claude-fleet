# Multi-host foundations (SSH iteration 1)

**Date:** 2026-05-20
**Author:** brainstorming dialog (M.J. + Claude)
**Status:** Design (awaiting user review → implementation plan)

## Goal

Let claude-fleet manage tmux sessions on remote hosts the user already has in `~/.ssh/config`, alongside local sessions, in a single unified UI. This is iteration 1 of three; iteration 2 (account model) and iteration 3 (cross-host session tracking + prompt transfer) are out of scope here.

## Scope

In:

- Register SSH hosts (alias picker from `~/.ssh/config`)
- Probe host (`ssh <host> 'tmux -V; claude --version'`) on add + on demand
- List / create / kill / rename / restart tmux sessions on any registered host
- Attach to remote tmux via embedded PTY (`ssh -tt <host> tmux attach -t <name>`)
- Sidebar: host filter pills + per-session `[host]` badge; project tree groups sessions across hosts by `owner/repo`
- Settings modal hosting the Hosts management UI
- Status indicator (reachable / unknown / unreachable) per host

Out:

- Same / different claude account configuration per host (iteration 2)
- Per-worktree-per-account session memory (iteration 3)
- Prompt transfer between hosts (iteration 3)
- Multi-host selection (multi-pill filter — single-select only for now)
- Manual host config (hostname/user/port form) — alias-only via `~/.ssh/config` for now
- Scanning remote `~/projects` to populate the project tree from remote (project tree stays local-only; remote sessions match in via `owner/repo` extraction)
- Periodic background ping (probe is on add + on demand)

## Architecture

```
┌────────────────────────┐   ssh ControlMaster sockets   ┌──────────────┐
│  claude-fleet (Tauri)  │ ───────────────────────────►  │  mefistos    │
│                        │       (~/.cache/claude-fleet/ │  (sshd+tmux) │
│  ┌──────────────────┐  │        cm-mefistos.sock)      └──────────────┘
│  │  TmuxExec trait  │  │
│  │ ┌──────────────┐ │  │
│  │ │ LocalTmux    │ │  │   direct std::process::Command   ↓
│  │ ├──────────────┤ │  │                                  local tmux
│  │ │ RemoteTmux   │─┼──┼──► SshClient::run(host, cmd) ────┘
│  │ └──────────────┘ │  │
│  └──────────────────┘  │
└────────────────────────┘
```

Two new modules + a refactor:

- `src-tauri/src/ssh_config.rs` — pure parser of `~/.ssh/config`. Returns `Vec<SshHost { alias, hostname, user, port }>`, skipping wildcards (`Host *`) and well-known non-machine aliases (`github.com`). No I/O outside the file read.
- `src-tauri/src/ssh.rs` — `SshClient` owning a per-host ControlMaster socket map. API:
  ```rust
  impl SshClient {
    fn ensure_master(&self, host: &str) -> Result<(), IpcError>; // idempotent
    fn run(&self, host: &str, args: &[&str], timeout: Duration) -> Result<Output, IpcError>;
    fn shutdown_all(&self);
  }
  ```
  Sockets live at `~/.cache/claude-fleet/cm-<host>.sock`. `ControlPersist=10m` so an idle host frees up naturally. Cleanup hook in Tauri `on_exit` calls `shutdown_all`.
- `src-tauri/src/tmux.rs` — extract a `TmuxExec` trait from the existing free functions. Two impls:
  - `LocalTmux` — current behavior, `std::process::Command::new("tmux")`
  - `RemoteTmux { client: Arc<SshClient>, host: String }` — wraps each tmux invocation in `ssh.run(host, ["tmux", ...], timeout)`
  Existing `new_session`/`kill_session`/etc. as free fns remain, delegating to `LocalTmux` for one-shot callers.

`pty.rs` gains a `host_alias` field on `PtyOpenArgs`. Local path unchanged. Remote path builds `CommandBuilder::new("ssh")` with `-o ControlPath=… -tt <host> bash -lc '<env_assignments> tmux attach -t <name>'`. Env (LANG / LC_ALL / COLORTERM / TERM) is set inside the remote shell rather than via `SendEnv` to avoid sshd `AcceptEnv` configuration dependency.

## Data model

Migration `002_hosts_ssh.sql`:

```sql
ALTER TABLE hosts ADD COLUMN ssh_alias TEXT;
INSERT INTO schema_version (version) VALUES (2);
```

`store.rs::migrate()` checks `MAX(version)` and runs `002_*.sql` only when at version 1. Tests cover idempotency (existing test `migrate_is_idempotent` extended).

Hosts table semantics:

- `alias` (PK) — what the user / DB references this host by. `local` is reserved and always present.
- `ssh_alias` — name in `~/.ssh/config`. `NULL` for `local`.
- `claude_version` / `tmux_version` — populated by probe; nullable until first successful probe.
- `reachable` — `0` (unreachable / probing failed) or `1` (last probe succeeded). Updated on every probe and on `list_sessions` failure for that host.
- `hidden` — user can hide a host without removing it (keeps existing sessions visible? — design choice: hidden host's sessions are still listed in "Other sessions").

Local row is inserted at first run by the existing `s.upsert_host("local")` call. Probe for local is a no-op that fills in `tmux -V` from the bundled tmux discovery and `claude --version` from `which claude` + run.

## Session discovery (multi-host)

`reconcile_sessions` (renamed from `reconcile_local_sessions`):

1. Read enabled hosts from DB (`WHERE hidden=0`).
2. For each host, run `tmux_exec.list_sessions()` in parallel (Rayon `rayon::scope` or `std::thread::scope`).
3. On failure: mark `reachable=0`, log the host, continue with others.
4. Collect `(host_alias, TmuxSession)` rows. Upsert into `sessions` table keyed by `(host_alias, tmux_name)`.
5. Project mapping per session:
   - **Local path** (`host_alias='local'`) — existing prefix match `path_str.starts_with(&p.base_path)`.
   - **Remote path** — regex `^.*?/projects/github\.com/(?P<owner>[^/]+)/(?P<repo>[^/]+)`. If captured, look up `projects` table by `(owner, repo)` and link `project_id`. Otherwise `project_id=NULL` (orphan).
6. Prune sessions per `(host_alias, *)` — `delete_sessions_not_in(host_alias, keep_names)` already supports this contract.

## Tauri commands (additions)

New `src-tauri/src/commands/hosts.rs`:

- `discover_hosts` → `Vec<SshHost>` — parses `~/.ssh/config`, **not** persisted. Used by the AddHostPicker dropdown.
- `list_hosts` → `Vec<HostRow>` — from DB, ordered by alias (with `local` first).
- `add_host { alias, ssh_alias }` → `HostRow` — probes; on success inserts; on failure returns `IpcError E_PROBE` with stderr included; nothing persisted on failure.
- `probe_host { alias }` → `HostRow` — re-probes existing host, updates versions + `reachable`.
- `remove_host { alias }` — DELETE host + cascade drop sessions for that host. `local` cannot be removed.
- `hide_host { alias, hidden: bool }` — toggle `hidden` flag.

`commands::sessions` modifications:

- `new_session` → add `host_alias: String` arg. Routes to `LocalTmux` or `RemoteTmux`.
- `kill_session` / `rename_session` / `restart_session` → add `host_alias: String` arg.
- `list_sessions` — body unchanged (calls `reconcile_sessions`); now returns sessions across all hosts.

`commands::pty::pty_open` → add `host_alias: String` to `PtyOpenArgs`. Local path uses native `tmux`; remote path uses `ssh -tt <host> bash -lc 'LANG=... tmux attach -t <name>'`.

## Frontend

New stores:

- `src/lib/hosts.ts` —
  ```ts
  export const hosts = writable<HostRow[]>([]);
  export const hostFilter = writable<'all' | string>(readPref('host-filter', 'all', isString));
  export async function loadHosts(): Promise<Result<HostRow[]>>;
  export async function discoverHosts(): Promise<Result<SshHost[]>>;
  export async function addHost(alias: string, sshAlias: string): Promise<Result<HostRow>>;
  export async function probeHost(alias: string): Promise<Result<HostRow>>;
  export async function removeHost(alias: string): Promise<Result<void>>;
  ```

Sidebar changes (`Sidebar.svelte`):

- Header gains a third row after recency pills: **host pills**. `[ all ]` + one pill per host (with status dot `🟢/⚪/🔴`) + trailing `⚙` Settings button. `+` to add host lives inside Settings.
- Session row: `[<host_alias>]` badge in monospace before the session name. Color-coded same as host status dot (green if host reachable, gray if unknown, red if unreachable). Local sessions get `[local]` badge for consistency.
- Existing search (`matchesSearch`) extended to also match `host_alias` substring.
- `selectedHostFilter` filters the sessions tree (`'all'` shows everything; otherwise only matching host).

New components:

- `src/lib/SettingsDialog.svelte` — modal with one tab for now ("Hosts"). Shows registered hosts with per-row actions: `↻ Probe`, `👁 Show/Hide`, `× Remove`. `+ Add host` button at the bottom opens AddHostPicker.
- `src/lib/AddHostPicker.svelte` — modal listing aliases from `discover_hosts`. Each row shows `alias → hostname (user@host:port)`. Click → spinner → probe result → confirm `Add`. Errors shown inline; cancel returns to picker without inserting.

`NewSessionDialog.svelte` — first row becomes a `Host:` picker (segment buttons of all reachable hosts). Default = last-used host stored in `cf:pref:last-host`. tmux-name template unchanged when host changes.

`TerminalView.svelte` — passes `selectedSession.host_alias` into `pty_open` and `pty_resize` calls.

## Status / reachability UI

Status dot semantics (matches existing session status dot styling):

- 🟢 green = `reachable=1` AND last_pinged_at within last 60s
- ⚪ gray = unknown (never probed) or stale (>10m)
- 🔴 red = `reachable=0` from last probe / last `list_sessions` attempt

Host pill click filters; the small dot is visual only. Re-probe is via Settings dialog (`↻` next to host) or via the host pill's right-click… wait, computer-use tier restrictions matter here for testability. **Decision:** add a small `⋯` button on hover of the host pill that opens a 3-item menu (`Re-probe`, `Hide`, `Remove`). Right-click works too, but the visible menu is needed for click-only environments.

When a host goes unreachable mid-session (remote tmux dies, ssh master drops), `TerminalView` shows a banner above the terminal: `"Connection to <host> lost — click to reconnect"`. Reconnect calls `pty_open` again.

## Error handling

| Scenario                                        | Behavior                                                                              |
|-------------------------------------------------|---------------------------------------------------------------------------------------|
| `ssh_config.rs` can't read file                 | Empty list; user sees "No SSH config found. Edit ~/.ssh/config to add hosts."         |
| Probe times out (>5s)                           | `IpcError E_PROBE`, stderr message shown in AddHostPicker; host NOT added             |
| Host reachable but no tmux                      | Probe succeeds with `tmux_version=NULL`; warn in Settings: "tmux not found on host"   |
| Host reachable but no claude                    | Probe succeeds with `claude_version=NULL`; warn: "claude not found; new_session will fail" |
| ControlMaster socket conflict / stale           | `ensure_master` first runs `ssh -O exit` to clear, then `-M -N`                       |
| Remote tmux command fails (network drop)        | Mark host `reachable=0`; sessions for that host show red dot; existing UI still works |
| Path doesn't match `/projects/github.com/.../` | `project_id=NULL`; session appears under "Other sessions"                            |
| `remove_host` while sessions are attached       | TerminalView sees host disappear → closes PTY, clears selection                       |

## Test plan

Pure-logic unit tests:

- `ssh_config::parse` — small fixture files: simple aliases, multi-Host blocks, wildcards skipped, `Hostname` / `User` / `Port` extraction, comments stripped, missing file returns `Vec::new()`.
- `tmux::project_match_owner_repo` — string extraction regex; positive and negative cases.
- `store::migrate` — extends `migrate_is_idempotent` to start from migration 001 state and verify 002 applies.

Integration-ish tests (mockable):

- `RemoteTmux` — wrap `SshClient` in a trait, mock impl returns canned stdout; verify command strings sent are correct (`["tmux","list-sessions","-F","#{session_name}|..."]`).
- Tauri commands — existing pattern with `Store::open_in_memory` plus a mocked `TmuxExec`.

Frontend tests:

- `hosts.ts` — load/add/probe/remove store + invoke mocking.
- `Sidebar.svelte` — host pills render from store, badge appears on session rows, filter narrows displayed sessions.
- `AddHostPicker.svelte` — discover → pick → probe → add roundtrip with mocked invoke.
- `SettingsDialog.svelte` — host row actions invoke correct IPC.

Live verification (manual, on `mefistos`):

1. Add `mefistos` via Settings → probe succeeds → host pill appears.
2. Create new session in `claude-fleet` project on `mefistos` → tmux session created on remote, claude launches.
3. Attach in embedded terminal → claude TUI renders properly (verifies remote PTY + env passthrough).
4. Rename + restart from sidebar → still works.
5. Drop network (disable WiFi briefly) → host pill turns red, sessions stay listed, terminal shows reconnect banner.
6. Kill session from sidebar → tmux session gone on remote, sidebar updates.

## Implementation slices

Each slice is a focused commit:

1. **Migration 002 + ssh_config.rs parser** with unit tests; no behavior change yet.
2. **ssh.rs SshClient** with ControlMaster + integration test using local sshd loopback or a `MockSsh` trait for unit testing.
3. **TmuxExec trait + RemoteTmux impl** + project owner/repo matching; existing local path unchanged.
4. **Tauri commands::hosts** (discover/list/add/probe/remove/hide) with tests.
5. **commands::sessions multi-host routing** — new_session/kill/rename/restart take `host_alias`; reconcile_sessions iterates hosts.
6. **Frontend hosts.ts + Sidebar host pills + [host] badges + filter**.
7. **SettingsDialog + AddHostPicker** UI.
8. **NewSessionDialog host picker** + last-host pref.
9. **Remote PTY** in pty.rs + TerminalView reconnect banner.
10. **Live verify on mefistos** + bug fixes.

## Open risks

- **ssh `AcceptEnv` limitation** — sidestepped by setting env inside `bash -lc '…'` rather than via `SendEnv`. Cost: remote shell startup overhead (login shell sources zshrc, ~50-200ms). Acceptable for one-shot attach.
- **ControlMaster on flaky networks** — if the master dies, subsequent runs reopen. `ControlPersist=10m` is the timeout; idle hosts close after 10 min and reopen on next demand.
- **Project owner/repo collision** — if user has two unrelated projects with the same `owner/repo` literal across hosts (rare), they collapse into one project node. Acceptable; iteration 2 may add explicit per-host project rows if needed.
- **Locale env on Linux remote** — Linux sshd typically resets LC_ALL. Setting via `bash -lc` should be fine because user's `~/.zshrc` on mefistos has `en_US.UTF-8` already (verified during brainstorming).

## Non-goals (re-affirmed)

- Iteration 1 does NOT touch claude account configuration, transfer flows, or any per-account session memory. Those are deferred to iterations 2 and 3.
- No SSH key generation or password auth UI — relies entirely on existing key-based `ssh <host>` working at the shell.
