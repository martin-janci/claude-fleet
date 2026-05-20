# claude-fleet — iteration 2 handoff prompt

You're picking up the `claude-fleet` project on **mefistos** to design and build iteration 2 of the multi-host SSH feature: the **account model**.

## Repository

- Path: `~/projects/github.com/martin-janci/claude-fleet`
- Branch: `main` (just pushed iter 1 commits; pull latest before starting)
- Stack: Rust + Tauri 2 backend, Svelte 5 (runes) frontend, SQLite via rusqlite

## What iteration 1 delivered (already merged)

SSH multi-host foundations. The app can now register hosts from `~/.ssh/config`, list/create/kill/rename/restart tmux sessions on any host (local or remote), and attach the embedded terminal over SSH. Key pieces:

- `src-tauri/src/ssh_config.rs` — pure parser
- `src-tauri/src/ssh.rs` — ControlMaster client
- `src-tauri/src/tmux.rs` — `TmuxExec` trait + `LocalTmux` + `RemoteTmux`
- `src-tauri/src/commands/hosts.rs` — discover/list/add/probe/remove/hide
- `src-tauri/src/commands/sessions.rs` — multi-host reconcile + auto-clone remote repos + worktree-add
- `src-tauri/src/pty.rs` — branches on host_alias, `ssh -tt + bash -lc` for remote
- `src/lib/hosts.ts`, `Sidebar.svelte`, `SettingsDialog.svelte`, `AddHostPicker.svelte`, `NewSessionDialog.svelte` host picker, `TerminalView.svelte` reconnect banner

67 Rust + 114 vitest tests pass. Iter 1 spec: `docs/specs/2026-05-20-multi-host-foundations-design.md`. Iter 1 plan: `docs/plans/2026-05-20-multi-host-foundations.md`. Final code review captured several follow-ups inline in the plan's risks section.

## Iteration 2 scope — account model

The user wants the concept of "claude accounts" to be a first-class entity, distinct from hosts. The motivating use case:

- Mode A: **Different claude accounts on different machines** (e.g., personal claude on the laptop, work claude on mefistos). Each host has exactly one account associated.
- Mode B: **Same claude account on multiple machines** (e.g., a single account that exists on both mac and mefistos, possibly with shared `~/.claude` via syncthing or with separate logged-in sessions). Multiple hosts share one account.

The user said (in the original Slovak brainstorming dialog):
> "dva mody. ked budem mat sve rozne konta na dvoch masinach a jedno a to iste konto claude na dvoch masinach. Pamataj si session id pre kazde konto, ked napriklad budem riesit tu istu worktree."

The "session id per account per worktree" memory is **iteration 3**, not iter 2. Iter 2 is just the account data model and configuration UX.

## Your job

Follow the same disciplined workflow iter 1 used:

1. **Brainstorm** (use `superpowers:brainstorming` skill). Refine scope by asking the user 1 question at a time. Likely areas to probe:
   - How is an "account" identified? OAuth token? Email? Free-text label the user picks? (claude code stores credentials in `~/.claude/.credentials.json` — possibly read claude_account_id from there if accessible, or just take a user-provided label)
   - When the user adds a host, do they pick from existing accounts or always create new? What does Mode B (one account, many hosts) look like in UI?
   - How does the user view "which account is on which host"? Settings dialog new tab? Inline in host pill?
   - Per-host probe: should `probe_host` also detect the logged-in claude account (e.g., `claude /api/whoami`) and auto-attach to the right account row?
   - Default behavior if a host has no account assigned (treat as anonymous, refuse to attach, prompt user)?
   - Schema: new `accounts` table with FK from hosts? Or `account_label` column on hosts directly (simpler, no normalized accounts table needed until iter 3 cross-references)?

2. **Write a spec** to `docs/specs/2026-MM-DD-account-model-design.md` mirroring the iter 1 spec structure (goal, scope, architecture, data model, UX, error handling, test plan, slices, risks).

3. **Write a plan** to `docs/plans/2026-MM-DD-account-model.md` with bite-sized tasks following iter 1's template (each task: file paths, complete code, test code, commands, commit message).

4. **Execute via `superpowers:subagent-driven-development`** with two-stage review per task.

## Important constraints

- **Don't break iter 1.** All 67 Rust + 114 vitest tests must continue to pass.
- **Schema migrations** go in `src-tauri/migrations/`. Migration 002 already exists (hosts.ssh_alias). Use 003 for any account-related schema changes. Bump the constant assertion in `commands/health.rs` if you bump `schema_version`.
- **Iter 1 follow-ups are NOT in scope** for iter 2. The plan reviewer flagged: extract shared `shell_quote` util, `LOCAL_HOST_ALIAS` const, on_exit hook timing, 3-state host pill, confirm-on-remove. Some are easy quick fixes — apply them if they touch files iter 2 already changes, but don't open separate branches for them.
- **Live host available**: `mefistos` is reachable via key-based SSH (you're literally running on it). For end-to-end testing, your "remote" is the user's laptop (alias `mac` in their ssh config), but credentials may not work bidirectionally — confirm with the user before assuming you can ssh back.
- **No commits unless explicitly approved.** Follow the user's CLAUDE.md instruction: ask before each commit, or batch a clear "commit this slice" request from the user.

## Notes on environment

- claude on mefistos: 2.1.144 (matches `claude --version`)
- tmux on mefistos: 3.6a
- Linuxbrew available at `/home/linuxbrew/.linuxbrew/bin/`
- Shell: zsh
- `~/bin/cl` wrapper: `exec claude --dangerously-skip-permissions "$@"`

## Style

- Slovak/casual mix is fine if user starts that way; default to English otherwise.
- Verify before asserting (use Bash, Read, etc. — don't guess file contents from training data).
- Use `AskUserQuestion` for branching design decisions. One question at a time.
- When dispatching subagents, give them self-contained briefs (paste full task text, don't make them re-read the plan).

Good luck. Start by reading `docs/specs/2026-05-20-multi-host-foundations-design.md` and `docs/plans/2026-05-20-multi-host-foundations.md` to ground yourself, then kick off the brainstorming skill.
