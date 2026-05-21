# claude-fleet — Architecture

A reference for how `claude-fleet` is built: the process model, the backend and
frontend internals, the data model, the IPC surface, and the lifecycle of each
operation. For *what the app is for* see [USE_CASES.md](USE_CASES.md); for the
original product design see
[specs/2026-05-19-claude-fleet-design.md](specs/2026-05-19-claude-fleet-design.md).

## 1. The big picture

`claude-fleet` is a Tauri 2 desktop app. It does **not** run Claude itself —
Claude Code (`cl` / `claude`) keeps running inside `tmux` on whichever host owns
each session. The app is a control panel: it discovers sessions, creates and
kills them, attaches an embedded terminal, and pushes prompts — all over plain
`ssh` to hosts in the user's `~/.ssh/config`.

```
┌──────────────────────────────────────────────────────┐
│                  Svelte 5 frontend (WKWebView)        │
│   Sidebar (tree)   │   Details (center)  │  Terminal   │
│   stores: hosts, sessions, projects, accounts          │
└───────────────────────────┬──────────────────────────-┘
              Tauri IPC (commands)  +  row events
┌───────────────────────────┴──────────────────────────-┐
│                   Rust backend (Tauri)                 │
│  commands/  ssh.rs  tmux.rs  pty.rs  store.rs           │
│  events.rs  cancel.rs  validate.rs  shell.rs            │
└───────┬───────────────────────────────┬────────────────┘
        │ system `ssh` (ControlMaster)   │ portable-pty
        ▼                                ▼
  remote host: tmux + cl           local PTY running
  (mefistos / hetzner / …)         `ssh -t <host> tmux attach`
```

Two distinct channels reach a remote host:

1. **Control plane** — short-lived `ssh <host> bash -lc '<script>'` calls for
   discovery, session create/kill, prompt send. Multiplexed over a per-host SSH
   `ControlMaster` so each call skips a fresh TCP+auth handshake.
2. **Terminal plane** — one long-lived local PTY running
   `ssh -t <host> 'tmux attach -t <name>'`, streamed byte-for-byte to the
   embedded terminal.

## 2. Backend (`src-tauri/src/`)

| Module | Responsibility |
|---|---|
| `lib.rs` | Tauri app setup, command registration, GUI-launch env recovery (PATH/locale backfill). |
| `commands/` | Tauri command handlers — the IPC surface (see §5). |
| `ssh.rs` | `SshClient`: per-host `ControlMaster`, async `tokio::process` shell-out, cancellable runs, `remote_home` cache. |
| `tmux.rs` | `TmuxExec` trait with `LocalTmux` / `RemoteTmux` impls — builds and parses tmux porcelain. |
| `pty.rs` | The single global PTY (`PtyState`) — open / write / resize / drain / close. |
| `store.rs` | `Store`: the `rusqlite` connection, migrations, all SQL, and event emission. |
| `events.rs` | `EventBus` trait + `RowChange` enum — emits row events to the frontend after a commit. |
| `cancel.rs` | `CancellationRegistry` — maps a frontend `call_id` to a `CancellationToken` so long calls can be aborted. |
| `projects.rs` | Filesystem scan of `~/projects/github.com/<owner>/<repo>` + `git worktree` parsing. |
| `ssh_config.rs` | Parses `~/.ssh/config` into selectable host aliases. |
| `validate.rs` | Input guards — `host_alias`, `tmux_name`, `path_component`, `git_ref`. Rejects hostile input before it reaches `ssh`/`git`. |
| `shell.rs` | `quote` — POSIX single-quote shell escaping for values interpolated into remote command strings. |
| `ipc_error.rs` | `IpcError` with `E_*` codes — the wire error type. |

### 2.1 SSH multiplexing (`ssh.rs`)

`SshClient` keeps one `ControlMaster` per host. The first call to a host runs
`ensure_master`, which opens a background master connection at a control socket
path; every later call reuses it (`-o ControlPath=…`), so a discovery probe or
a prompt send is a single round trip with no re-auth. `shutdown_all` closes all
masters on app exit so no background `ssh` processes leak.

`run` / `run_cancellable` shell out via `tokio::process` with a timeout;
`run_cancellable` also takes a `CancellationToken` so a slow `git clone` inside
`new_session` can be aborted from the UI.

### 2.2 tmux abstraction (`tmux.rs`)

`TmuxExec` is a trait so the same command logic runs `local` (direct
`tokio::process`) or `remote` (wrapped in `ssh <host> bash -lc`). It exposes
`list_sessions`, `new_session`, `kill_session`, `rename_session`,
`restart_session`, `capture_pane`. Sessions are listed with a `-F` format
string and parsed into `TmuxSession { name, path, created, last_activity }`.

### 2.3 The single global PTY (`pty.rs`)

Only **one** PTY is attached at a time — selecting a different session closes
the current PTY and opens a new one. The PTY child is `ssh -t <host> tmux
attach` (remote) or `tmux attach` (local). Output is buffered and `pty_drain`
is polled by the frontend on a self-rescheduling adaptive loop; the buffer is
capped to bound memory. This is a known race surface — see the hardening review.

### 2.4 Why a hand-rolled terminal

The terminal is **not** xterm.js. xterm's renderer failed to repaint reliably
in the WKWebView Tauri uses, so the terminal is a hand-rolled ANSI screen
buffer: `src/lib/ansi.ts` parses the byte stream into a grid; `TerminalView.svelte`
renders it with dirty-row diffing.

## 3. Frontend (`src/`)

A vanilla Svelte 5 SPA (no SvelteKit). `App.svelte` is a three-pane resizable
layout: **Sidebar** (project/worktree/session tree), **Details** (center —
metadata + actions for the selected session or project), **TerminalView**
(the attached PTY).

### 3.1 Stores as runes

Each `src/lib/*.ts` store (`hosts`, `sessions`, `projects`, `accounts`) holds
app state as Svelte 5 runes. The pattern:

- **Bootstrap** — on startup, `bootstrapX()` fetches the full list once.
- **Event-driven patching** — backend mutations emit row events;
  `subscribeToRowEvents` (`events.ts`) routes each to `mergeOne` / `removeOne`,
  which patch the store **in place** instead of re-fetching the whole list.
- **Optimistic merge** — mutation wrappers (`newSession`, `killSession`, …)
  also patch the store from the command's return value immediately, so the UI
  updates without waiting for the event round trip.

### 3.2 Per-session UI memory

Layout is partly per-session: the center pane width is remembered per
`(host, tmux_name)` key (`session_ui.ts` + `localStorage`). Global layout
(sidebar width, collapsed states) lives in `prefs.ts`. Theme is in `theme.ts`.

## 4. Data model

SQLite at the platform appdata dir (`ProjectDirs` for `sk.rlt.claude-fleet`),
e.g. `~/.local/share/claude-fleet/state.db` on Linux,
`~/Library/Application Support/…` on macOS. Schema is built by sequential
migrations `001`–`007` in `src-tauri/migrations/`, tracked in `schema_version`.

| Table | Purpose |
|---|---|
| `hosts` | One row per registered host (incl. `local`). `ssh_alias`, `reachable`, `claude_version`, `tmux_version`, `hidden`, `account_uuid`. |
| `accounts` | Normalized Claude accounts (`uuid` PK) — email, org, seat tier. Auto-populated from each host's `~/.claude.json` `oauthAccount` during probe. |
| `projects` | Discovered repos — `(owner, repo)` unique, `base_path`, `last_session_at`. |
| `worktrees` | Git worktrees per project — `name`, `path`, `branch`. |
| `sessions` | Tmux sessions across all hosts. `kind` (`work`/`review`), `reviews_session_id` (self-FK link to the reviewed session), `worktree_key`, `account_uuid`. |
| `handoffs` | Reserved by the original spec — handoff is not implemented. |
| `settings` | Key/value app settings. |

Key invariants:

- **Account preservation** — a session's `account_uuid` is captured only when
  the row is first discovered. Re-probe never rewrites it, so the UI can show
  which account a session was created under even after the host re-auths.
- **Reconcile is per-host and transactional** — `apply_host_reconcile` wraps a
  host's whole write-burst (probe update + session upserts + delete-not-in) in
  one transaction (one fsync), and emits events only **after** commit. A
  mid-burst failure rolls back and emits nothing for that host, never aborting
  other hosts.
- **`Store` is behind a `std::sync::Mutex`** — never hold the guard across an
  `.await`. Probes run off-lock; only the write-burst takes the lock.
- **`reviews_session_id` is `ON DELETE SET NULL`** — a review is itself a
  session; if its source session is deleted, the review survives with a null
  link rather than failing the FK and aborting reconcile.

## 5. IPC surface

All commands return `Result<T, IpcError>`; the frontend unwraps via
`result.ts`. `IpcError` carries an `E_*` code (`E_PROBE`, `E_GIT_SETUP`,
`E_TMUX`, `E_LOCK`, `E_NOTFOUND`, `E_INVALID`, `E_CANCELLED`, …).

| Command | Effect |
|---|---|
| `health_check` | App/DB health snapshot. |
| `list_projects` / `refresh_projects` | Read / rescan the local project tree. |
| `list_sessions` | Full multi-host reconcile — probes every visible host in parallel, returns all sessions. |
| `related_sessions` | Other sessions sharing a project + worktree. |
| `new_session` | Create a tmux session (auto-clones the repo / adds a worktree on a remote host if missing). Cancellable. |
| `kill_session` / `rename_session` / `restart_session` | Mutate one session, then per-host reconcile. |
| `send_prompt` | `tmux send-keys` literal text + Enter into a session. |
| `spawn_review` | Spawn a `kind=review` session in the source's worktree, seeded with a review prompt. |
| `discover_hosts` | List `~/.ssh/config` aliases (for the Add-host picker). |
| `list_hosts` / `list_accounts` | Read registered hosts / accounts. |
| `add_host` / `remove_host` / `hide_host` | Manage registered hosts. |
| `probe_host` / `probe_ssh_alias` | Probe a host for reachability + tmux/claude versions + account. |
| `pty_open` / `pty_write` / `pty_resize` / `pty_drain` / `pty_close` | Drive the single embedded terminal. |
| `cancel_command` | Cancel an in-flight cancellable command by `call_id`. |

## 6. Event flow

```
backend mutation → Store write (transaction) → commit
  → EventBus.emit_change(RowChange)  [AFTER commit only]
  → tauri emit  →  events.ts subscribeToRowEvents
  → mergeOne / removeOne  →  store rune updates  →  UI repaints
```

`RowChange` covers host/session/project/worktree/account upserts and deletes.
Tests use `NoopEventBus` (silent) or `RecordingEventBus` (captures emits).

## 7. Operation lifecycles

### New session (remote, new worktree)

1. `validate` host alias, tmux name, git ref.
2. Register a `CancellationToken` under the frontend's `call_id`.
3. `remote_home` → derive `~/projects/github.com/<owner>/<repo>`.
4. `ensure_remote_project` — `git clone` if `.git` missing (idempotent,
   cancellable).
5. `worktree_add_script` — create the branch + worktree, return its abs path.
6. `tmux new_session` in that cwd (the pane runs `cl --continue || cl || bash`).
7. `reconcile_one_host` — re-probe just that host, write transactionally.
8. Return the new `SessionRow`; the frontend optimistically merges it.

### Attach

1. Frontend selects a session → `pty_close` any current PTY, then `pty_open`.
2. The PTY child is `ssh -t <host> tmux attach -t <name>` (or local `tmux
   attach`).
3. Frontend polls `pty_drain` on an adaptive loop; bytes feed `ansi.ts`.
4. Keystrokes → `pty_write`; resize → `pty_resize`.

### Reviews

`spawn_review` reuses the new-session + reconcile + send-prompt machinery: it
spawns a tmux session in the source session's worktree, tags it `kind=review`
with `reviews_session_id` pointing at the source, waits for `cl`'s REPL prompt,
then seeds the (editable) review prompt via `send-keys`. Seeding is soft-fail —
if `cl` wasn't ready the session still exists and the user can type manually.

## 8. Security notes

- Every value interpolated into a remote command string **must** be shell-quoted
  (`shell::quote`). Quoting stops command injection but not `..` path traversal,
  so path components are *also* run through `validate::path_component`.
- The app never reads or stores Claude credentials — it reads only the
  `oauthAccount` *metadata* object (email/org/tier) from `~/.claude.json`.
- SSH auth is entirely the user's existing agent + `~/.ssh/config`. No new auth
  layer.
- Shell-quoting currently lives in **four** duplicated copies — consolidating
  them is a tracked cleanup. See `docs/specs/2026-05-21-hardening-review.md` for
  the full open-issues list.
