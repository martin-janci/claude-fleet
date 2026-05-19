# claude-fleet — Design

**Status:** Draft
**Owner:** Martin Janči (martin-janci)
**Date:** 2026-05-19
**Repo:** `martin-janci/claude-fleet` (to be created at `~/projects/github.com/martin-janci/claude-fleet` via `proj-clean new`)

## 1. Summary

`claude-fleet` is a native cross-platform desktop app for managing long-lived Claude Code sessions running in tmux across multiple machines (mac, mefistos, hetzner). It replaces the current bash-script-based workflow (`bin/bin/dev` + `bin/bin/claude-handoff`) with a Tauri-based GUI that embeds terminals, mirrors sessions live across hosts, and snapshots scrollback for hand-off between machines.

The app does **not** replace `cl` (the claude CLI). `cl` keeps running inside tmux on whichever host owns each session. `claude-fleet` is a session orchestrator and control panel — a polished `dev` + `claude-handoff` with a UI, embedded terminals, and live cross-host mirroring.

## 2. Goals

- **Single pane of glass** for all active Claude sessions across `mac` / `mefistos` / `hetzner`.
- **Embedded terminals** — attach to any session inside the app, no separate Terminal.app window required.
- **Cross-host handoff** with three selectable modes:
  - **Live mirror** (default) — source and destination attached concurrently via tmux's native multi-client behavior.
  - **Scrollback snapshot** — capture the source pane content, replay as a read-only header on the destination, then resume `cl --continue`.
  - **State only** — `claude-handoff push` + fresh `cl --continue` on destination, no terminal carryover.
- **First-class git worktrees** — each worktree is a sibling row under its project; each can host its own session.
- **Project discovery** via filesystem scan of `~/projects/github.com/{owner}/{repo}`.
- **Multiplatform** — macOS (primary) + Linux (mefistos/hetzner). Windows is a non-goal but should not be precluded.

## 3. Non-goals for v0.5

- Native Anthropic API integration — Claude is consumed via the existing `cl` CLI inside tmux.
- Custom session engine — tmux remains the persistence layer.
- Native rsync — sync is via shell-out to `claude-handoff`.
- Plugin / skill management UI — handled by existing dotfiles tooling.
- Mobile or web versions.
- Sharing sessions with other users.

## 4. Background

Today the user manages remote Claude sessions via:

| Tool | Role |
|---|---|
| `bin/bin/dev` | Bash; opens/lists/kills `tmux new-session -d` running `cl` on remote hosts via SSH. Prints attach command for user to paste in their own terminal. |
| `bin/bin/claude-handoff` | Bash; rsyncs `~/.claude/projects/` (sessions + auto-memory) and `~/.claude/skills/` between paired hosts (mac ↔ mefistos). |
| `dev-session` skill | Documents how Claude Code orchestrates `dev`. |
| `claude-handoff` skill | Documents how Claude Code orchestrates `claude-handoff`. |
| `worktree` skill | Generic git worktree management. |

Pain points addressed by `claude-fleet`:

- Terminal attach is manual (paste an `ssh -t … attach` into a separate terminal).
- No visibility across hosts in one view — you have to `dev list` and parse text.
- No "carry the terminal state with me" when switching machines mid-task.
- Worktrees require remembering naming conventions (`dev-<repo>--<wt>`).

## 5. Architecture

### 5.1 Decisions

| Topic | Decision | Rationale |
|---|---|---|
| Form factor | Native desktop GUI | User wants OpenClaw-like layout; matches "wrapper for consoles". |
| Tech stack | Rust + Tauri 2, Svelte frontend, xterm.js terminal | Best ratio of polish (web frontend) to footprint (system webview, not bundled Chromium). |
| Topology | SSH peer (same app on every host) | No daemon to install/keep alive. Tmux already handles persistence. |
| Tmux relationship | Visible wrapper — tmux session names surfaced in UI | Interop with existing `dev` script and ad-hoc `ssh + tmux attach` from a regular terminal. |
| Sync engine | Shell-out to existing `claude-handoff` script | Reuses a tool the user already trusts; saves weeks of reimplementation. |
| PTY | Embedded via `portable-pty`, streamed to xterm.js over Tauri channel | Battle-tested approach (Wezterm, Warp, Hyper use the same shape). |
| Storage | SQLite (`rusqlite`) at platform appdata dir | Simple, embedded, well-supported in Tauri. |
| Session source-of-truth | Tmux on the originating host | Tmux is already the durable layer; the app is a view onto it. |
| Frontend framework | Svelte (vanilla Svelte SPA, no SvelteKit) | Small bundle, less ceremony than React, plays well with Tauri. |
| SSH auth | System SSH agent + `~/.ssh/config` aliases | No new auth layer; reuses the user's existing key/agent setup. |
| Project discovery | Scan `~/projects/github.com/{owner}/{repo}` (configurable in Settings) | Matches the user's `proj-clean` convention. |
| Conflict resolution | Accept `claude-handoff`'s newer-file-wins per-file | Punts to existing tool; v0.5 doesn't try to be smarter. |

### 5.2 Component diagram

```
┌─────────────────────────────────────────────┐
│              Svelte frontend                │
│  ┌──────────┐ ┌──────────┐ ┌─────────────┐  │
│  │ Sidebar  │ │ Session  │ │  Terminal   │  │
│  │  (tree)  │ │  Center  │ │ (xterm.js)  │  │
│  └──────────┘ └──────────┘ └─────────────┘  │
└───────────────────────┬─────────────────────┘
                  Tauri IPC + events
┌───────────────────────┴─────────────────────┐
│              Rust (Tauri backend)           │
│                                             │
│  commands/  ssh.rs  tmux.rs  pty.rs         │
│  sync.rs    git.rs  store.rs                │
└───────┬──────────────────┬──────────────────┘
        │                  │
   system `ssh`        portable-pty
   ~/.ssh/config           │
        │                  │
        ▼                  ▼
 ┌──────────────┐   ┌──────────────────────┐
 │ remote tmux  │   │ local PTY running    │
 │   + cl       │   │   ssh -t <h>         │
 └──────────────┘   │   tmux attach …      │
                    └──────────────────────┘
```

### 5.3 Rust modules

```
src-tauri/src/
├── main.rs                # Tauri app setup, register commands + emitters
├── commands/
│   ├── sessions.rs        # list / create / attach / kill / handoff / freeze
│   ├── projects.rs        # discover / refresh
│   ├── hosts.rs           # ping, list, settings
│   └── pty.rs             # open / write / resize / close
├── ssh.rs                 # std::process wrapper around system `ssh`
├── tmux.rs                # tmux command builders + porcelain parsers
├── pty.rs                 # portable-pty wrapper + Tauri channel for bytes
├── sync.rs                # claude-handoff shell-out (push/pull/status)
├── git.rs                 # git worktree list --porcelain parser
└── store.rs               # rusqlite + migrations
```

### 5.4 Svelte modules

```
src/
├── App.svelte                  # 3-pane layout, resizable splits
├── lib/
│   ├── Sidebar.svelte          # tree + filter pills + search
│   ├── SessionView.svelte      # center pane, action buttons, handoff log
│   ├── Terminal.svelte         # xterm.js + PTY channel
│   ├── HandoffDialog.svelte    # destination + mode picker
│   ├── NewSessionDialog.svelte # project / worktree / host picker
│   └── SettingsView.svelte     # paths, hosts, defaults, theme
└── stores/
    ├── sessions.ts             # session list + selected
    ├── hosts.ts                # host status + reachability
    └── projects.ts             # project tree, recency filter
```

## 6. Data model

SQLite at `~/Library/Application Support/claude-fleet/state.db` (macOS) and `~/.local/share/claude-fleet/state.db` (Linux). Schema:

```sql
CREATE TABLE hosts (
  alias            TEXT PRIMARY KEY,
  last_pinged_at   INTEGER,
  reachable        INTEGER NOT NULL DEFAULT 0,
  claude_version   TEXT,
  tmux_version     TEXT,
  hidden           INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE projects (
  id               INTEGER PRIMARY KEY,
  owner            TEXT NOT NULL,
  repo             TEXT NOT NULL,
  base_path        TEXT NOT NULL,
  last_session_at  INTEGER,
  UNIQUE (owner, repo)
);

CREATE TABLE worktrees (
  id           INTEGER PRIMARY KEY,
  project_id   INTEGER NOT NULL REFERENCES projects(id),
  name         TEXT NOT NULL,
  path         TEXT NOT NULL,
  branch       TEXT,
  UNIQUE (project_id, name)
);

CREATE TABLE sessions (
  id                  INTEGER PRIMARY KEY,
  tmux_name           TEXT NOT NULL,
  host_alias          TEXT NOT NULL REFERENCES hosts(alias),
  project_id          INTEGER REFERENCES projects(id),
  worktree_id         INTEGER REFERENCES worktrees(id),
  created_at          INTEGER NOT NULL,
  last_activity_at    INTEGER NOT NULL,
  status              TEXT NOT NULL,    -- running | frozen | orphan
  frozen_scrollback   TEXT,             -- ANSI-preserving capture
  notes               TEXT,
  UNIQUE (host_alias, tmux_name)
);

CREATE TABLE handoffs (
  id            INTEGER PRIMARY KEY,
  session_id    INTEGER NOT NULL REFERENCES sessions(id),
  from_host     TEXT NOT NULL,
  to_host       TEXT NOT NULL,
  mode          TEXT NOT NULL,          -- mirror | scrollback | state
  started_at    INTEGER NOT NULL,
  finished_at   INTEGER,
  status        TEXT NOT NULL,          -- pending | success | error
  error         TEXT
);

CREATE TABLE settings (
  key    TEXT PRIMARY KEY,
  value  TEXT NOT NULL
);
```

Notes:

- `frozen_scrollback` is the output of `tmux capture-pane -p -e -S -2000`. ANSI escapes are preserved and replayed verbatim into xterm.js on the destination.
- `status = orphan` means the tmux session on the host no longer exists but the DB row hasn't been cleaned (e.g., remote host rebooted). Reconciled on the next refresh.
- Times are Unix epoch seconds.

## 7. UI structure

### 7.1 Layout

Three resizable panes, left to right:

**Sidebar (~25% width)**
- Search box at top (filters across tmux name, project name, branch).
- Recency filter pills: `All` `Today` `7d` `30d`.
- Tree: project → worktrees → sessions.
- Each session row: tmux name (mono), host label, last-activity (relative), status dot.
- Row context menu (right-click): attach, kill, handoff, freeze, copy attach command.

**Center pane (~30% width)**
- When no session selected: project summary (path, host, last activity, worktree count).
- When session selected:
  - Header: `<owner>/<repo>` + worktree (if any) + host alias.
  - Metadata: tmux name, created-at, last-activity, status.
  - Action buttons: **Attach** (focuses terminal), **Send to…** (handoff dialog), **Kill**, **Freeze**.
  - Handoff history: last N transfers involving this session.

**Terminal pane (~45% width)**
- Tabs across top: one per attached PTY.
- Body: xterm.js, full-bleed.
- Footer: PTY status (alive/dead), connection latency to remote host (if remote).

### 7.2 Status bar (bottom)

- Host reachability dots: `mac●` `mefistos●` `hetzner●` (green/yellow/red).
- Active session count.
- Background-task indicator (sync in progress, ping, etc.).

## 8. Cross-host operations

### 8.1 New session

1. User picks project + (optionally) worktree + target host.
2. App computes tmux name:
   - No worktree: `dev-<owner>-<repo>`
   - With worktree: `dev-<owner>-<repo>--<worktree>`
3. Executes: `ssh <host> "tmux new-session -d -s <name> -c <path> 'cl --continue || cl || bash'"`.
4. Inserts row in `sessions` with `status='running'`.
5. Optionally auto-attach by opening a PTY tab.

### 8.2 Attach

For both local and remote sessions:

1. Open new tab in terminal pane.
2. Spawn local PTY running:
   - Local: `tmux attach -t <session>`
   - Remote: `ssh -t <host> 'tmux attach -t <session>'`
3. xterm.js subscribes to PTY byte stream via a Tauri binary channel (one channel per PTY).
4. Keystrokes from xterm.js flow back to PTY stdin.
5. xterm.js resize events trigger `winsize` ioctl on the PTY.

### 8.3 Handoff

User clicks **Send to…**, picks destination host + mode:

**Live mirror (default):**
- Run `claude-handoff push --to <dest>` in background (best-effort; doesn't block attach).
- Open a second PTY tab attached via `ssh -t` to the same tmux session on the source host.
- Tmux's native multi-client support means both attached clients see the same content live.
- Either end can type; both see the result.

**Scrollback snapshot:**
- `ssh <src> 'tmux capture-pane -p -e -S -2000 -t <session>'` → store in `sessions.frozen_scrollback`.
- `claude-handoff push --to <dest>`.
- Ship the scrollback text to dest (`scp` to `/tmp/`).
- `ssh <dest> "tmux new-session -d -s <name> -c <path> 'cat /tmp/scrollback-<id>; echo ---; cl --continue || cl'"`.
- User attaches on dest; sees the frozen preamble followed by a live, resumed Claude shell.

**State only:**
- `claude-handoff push --to <dest>`.
- `ssh <dest> "tmux new-session -d -s <name> -c <path> 'cl --continue'"`.
- User attaches; clean shell, Claude resumes from the JSONL.

After a successful handoff, the user can optionally:
- **Move semantics:** kill the source tmux session.
- **Copy semantics (default):** leave source running. Both hosts now have a session for the same project.

All handoffs append a row to `handoffs`.

### 8.4 Freeze

Mark a session as frozen for later inspection without killing tmux:

- `ssh <host> 'tmux capture-pane -p -e -S -2000 -t <session>'` → `frozen_scrollback`.
- Optionally: actually kill tmux on disk (configurable; default = keep tmux).
- Sidebar row shows a snowflake icon, last-activity becomes "frozen at <time>".
- Clicking attach on a frozen session opens the scrollback in xterm.js as a read-only buffer.

## 9. Phased delivery

### Phase 1 — Bootstrap & UI shell (~1–2 weeks)

- Cargo + Tauri 2 project scaffolded.
- Svelte frontend, 3-pane resizable layout.
- Theme (light/dark, follow OS).
- SQLite schema + migrations module.
- Empty states everywhere.
- CI: `cargo check`, `cargo clippy`, `pnpm build` on push.
- README: build + run instructions.

**Exit criteria:** `cargo tauri dev` opens an empty 3-pane app on macOS.

### Phase 2 — Local discovery & project tree (~1–2 weeks)

- Scan `~/projects/github.com/{owner}/{repo}` → populate `projects`.
- Parse `git worktree list --porcelain` per project → populate `worktrees`.
- List local tmux sessions: `tmux list-sessions -F …` → populate `sessions`.
- Sidebar tree, recency filter pills, search box.
- "New session" dialog (local host only) → `tmux new-session -d …`.
- Kill button.
- Auto-refresh on focus.

**Exit criteria:** start the app, see your projects, create a tmux+claude session, see it in sidebar, kill it.

### Phase 3 — Embedded terminal (~2 weeks)

- `portable-pty` integration.
- xterm.js wired to PTY via Tauri binary channel.
- Attach to local tmux session.
- Input forwarding, resize handling.
- Multiple tabs in terminal pane.
- Detach / reattach without losing buffer (cached scrollback in xterm.js).

**Exit criteria:** click a local session, see Claude inside the embedded terminal, type to it, watch it respond.

### Phase 4 — Cross-host: discovery + remote attach (~2–3 weeks)

- Import host aliases from `~/.ssh/config` (filter to known set; user can hide/show in Settings).
- Reachability ping in background, status dots in status bar.
- Remote tmux discovery: `ssh <host> 'tmux list-sessions -F …'`.
- Remote attach: PTY runs `ssh -t <host> 'tmux attach -t <session>'`.
- Sidebar shows mixed local + remote sessions, host label per row.
- New-session wizard supports remote target.

**Exit criteria:** from mac, see mefistos sessions in sidebar, attach to one, interact with Claude running on mefistos.

### Phase 5 — Handoff & sync (~2–3 weeks)

- Handoff dialog: destination host + mode (mirror / scrollback / state-only).
- Live mirror via multi-client tmux attach.
- Scrollback capture + replay.
- `claude-handoff push` shell-out wired up.
- Optional "move" semantics (kill source after handoff).
- Handoff log view (history per session, global history).
- Settings UI: paths, hosts, default sync mode, theme.
- Freeze action.

**Exit criteria:** create a session on mac, hit "send to mefistos" with mirror mode, see content in both at once. Try scrollback mode and state-only too. Freeze a session, see the snowflake.

### Post-v0.5 (optional, prioritized later)

- Native sync engine (replace `claude-handoff` shell-out with `russh` + delta).
- Plugin / skill awareness.
- Multi-pane terminals per session (tmux window/pane navigation).
- Keyboard shortcuts deepening.
- Status bar widget.
- Packaging: signed DMG (macOS), AppImage + `.deb` (Linux).
- Auto-update.

## 10. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Tmux multi-client live mirror has unexpected behavior (e.g. window-size conflicts) | Phase 5 dedicates a small spike to validate before building UI on top. Fallback: scrollback mode. |
| `claude-handoff` shell-out is slow on first sync of a large `~/.claude/projects/` | Run async with progress bar; don't block UI. Document expected first-sync time. |
| `portable-pty` quirks across mac/linux | Stick to known-good patterns from Wezterm/Warp; do not exotic-flag the PTY. |
| Webview performance with 5+ embedded xterm.js instances | xterm.js handles this well in practice (every modern terminal app does it). Worst case: limit visible terminals to 4 tabs. |
| Tauri 2 stability on Linux for mefistos | Test on mefistos early (Phase 1 exit). |

## 11. Out of scope for v0.5

- Sharing sessions with other users.
- Web UI or mobile.
- Anthropic API client / chat UI.
- Custom session engine (replacing tmux).
- Native sync engine (`claude-handoff` reimplementation).
- Windows support (not precluded, just not tested).

## 12. References

- `bin/bin/dev` — current shell-based dev session opener.
- `bin/bin/claude-handoff` — current rsync wrapper.
- `claude/.claude/skills/dev-session/skill.md` — skill doc.
- `claude/.claude/skills/claude-handoff/skill.md` — skill doc.
- Tauri 2: https://v2.tauri.app/
- portable-pty: https://crates.io/crates/portable-pty
- xterm.js: https://xtermjs.org/
- tmux multi-client: `man tmux` → "ATTACHING TO SESSIONS".
