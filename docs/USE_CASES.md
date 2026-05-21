# claude-fleet — Use Cases

What `claude-fleet` is for, who uses it, and step-by-step walkthroughs of the
workflows it supports. For *how it is built* see
[ARCHITECTURE.md](ARCHITECTURE.md).

## Who this is for

A developer who runs long-lived [Claude Code](https://claude.com/claude-code)
sessions on more than one machine — a laptop, a home server, a cloud box — and
wants one window onto all of them instead of juggling SSH tabs and remembering
tmux session names.

The app does **not** replace the `cl` / `claude` CLI. Claude keeps running
inside `tmux` on whichever host owns each session; `claude-fleet` is the
control panel around it: discover, create, attach, prompt, kill.

## The problem it solves

Before `claude-fleet`, managing remote Claude sessions meant:

- `ssh`-ing into each host and running `tmux ls` to see what is alive.
- Memorising tmux naming conventions (`dev-<owner>-<repo>--<worktree>`).
- Opening a separate terminal window per `ssh -t … tmux attach`.
- No single view of which sessions exist, where, or how recently they were
  touched.
- No quick way to fire the same prompt at several sessions at once.

`claude-fleet` collapses all of that into a single three-pane desktop app:
a project/session tree, a details/actions pane, and an embedded terminal.

## Core concepts

| Term | Meaning |
|---|---|
| **Host** | A machine running `tmux` + `claude`. Always includes `local`; remote hosts come from `~/.ssh/config`. |
| **Project** | A repo discovered under `~/projects/github.com/<owner>/<repo>`. |
| **Worktree** | A git worktree of a project — each can host its own session. |
| **Session** | A `tmux` session running `cl`. Has a `kind`: `work` (normal) or `review`. |
| **Account** | The Claude account a host is logged into (email / org / seat tier), auto-detected from `~/.claude.json`. |
| **Reconcile** | The app re-probing hosts over SSH and syncing its view to reality. |

---

## Use case 1 — Survey every session across every machine

**Goal:** open the app and immediately see all active Claude work, everywhere.

1. Launch the app. On startup it reconciles: probes every visible host in
   parallel over SSH and lists their tmux sessions.
2. The **Sidebar** shows a tree — project → worktree → session — merging local
   and remote. Each session row shows its host, last activity, and status.
3. Unreachable hosts keep their last-known sessions, rendered dimmed, so a host
   being briefly offline never makes its work disappear.

**Why it matters:** one glance replaces N `ssh + tmux ls` round trips.

---

## Use case 2 — Start a new session on any host

**Goal:** spin up Claude on a project, on whichever machine you want it to run.

1. Open the **New Session** dialog.
2. Pick the **project**, optionally a **worktree** (existing or brand-new), and
   the **target host**.
3. The backend:
   - For a **remote** host where the repo isn't cloned yet, `git clone`s it to
     `~/projects/github.com/<owner>/<repo>` (idempotent — skipped if present).
   - For a **new worktree**, creates the branch + worktree under
     `.worktrees/` or `.claude/worktrees/`.
   - Creates the tmux session; the pane runs `cl --continue || cl || bash`, so
     Claude resumes prior context if there is any.
4. The session appears in the sidebar; optionally it auto-attaches.

A long `git clone` can be cancelled from the UI mid-flight.

**Why it matters:** "run Claude on this repo, on the big server" is two clicks
— no manual clone, no manual tmux, no path-juggling.

---

## Use case 3 — Attach an embedded terminal

**Goal:** interact with a running session without leaving the app.

1. Select a session in the sidebar.
2. The **Terminal** pane attaches a live PTY (`ssh -t <host> tmux attach`
   for remote, `tmux attach` for local) and streams its output.
3. Type directly into the terminal — keystrokes flow back to the session.
4. Selecting a different session swaps the terminal to it (only one PTY is
   attached at a time).

The terminal is a hand-rolled ANSI renderer, not xterm.js — see
[ARCHITECTURE.md §2.4](ARCHITECTURE.md) for why.

**Why it matters:** no separate Terminal.app windows; the session you are
reading about is the session you are typing into.

---

## Use case 4 — Push a prompt without attaching

**Goal:** send an instruction to a session (or several) quickly.

1. Use the **Prompt Composer** for the selected session.
2. The backend sends the text via `tmux send-keys` (literal text, then Enter)
   — the prompt lands in Claude's REPL exactly as typed.
3. The same prompt can be sent to multiple sessions, so a single instruction
   ("run the tests and report back") can fan out across a fleet.

**Why it matters:** drive a session without watching it; broadcast a prompt to
a whole batch of sessions at once.

---

## Use case 5 — Spawn a review of a session's work

**Goal:** get a fresh Claude to review what another session has been doing.

1. Trigger **Review** on a work session.
2. The backend spawns a *new* tmux session in the **same worktree** as the
   source, tagged `kind=review` and linked back to the source.
3. Once `cl`'s prompt is ready, it is seeded with an editable multi-pass review
   prompt; the review runs in its own embedded terminal.
4. The review session is linked to its source in the UI, so the relationship
   is visible.

**Why it matters:** a one-click "have Claude review this branch" that runs in
the right directory with the right context, without any manual setup.

---

## Use case 6 — Manage hosts and accounts

**Goal:** control which machines the fleet spans and see which account each
runs under.

1. **Add a host** — pick an alias from `~/.ssh/config`. The app probes it first
   (reachability, tmux + claude versions, logged-in account) and only persists
   it if reachable.
2. **Re-probe** a host any time to refresh status; an unreachable host is
   marked, not deleted.
3. **Hide** hosts you don't want in the tree (their sessions stop being probed
   but last-known state is kept).
4. Each host shows its **account** — email, organization, seat tier — detected
   from the remote `~/.claude.json` `oauthAccount`. No credentials are ever
   read or stored.
5. A session remembers the account it was *created* under even if the host
   later re-authenticates into a different one.

**Why it matters:** know at a glance which Claude account (and org/tier) any
piece of work is consuming — useful when juggling personal and org accounts.

---

## Use case 7 — Housekeeping: rename, restart, kill

- **Rename** — give a tmux session a clearer name.
- **Restart** — restart the session's process (e.g. after a `cl` crash) without
  losing the tmux session.
- **Kill** — tear down a finished session; it disappears from every connected
  view on the next reconcile.

Every mutation triggers a per-host reconcile and emits a row event, so the UI
updates in place without a full refresh.

---

## What it deliberately does not do

From the original design's non-goals (still accurate):

- **No Anthropic API client** — Claude is consumed only through `cl` in tmux.
- **No custom session engine** — tmux remains the persistence layer.
- **Handoff / Freeze** (carrying terminal state between machines) were specced
  but are **not implemented**.
- **No session sharing** between different users.
- **No web or mobile** version.

See [specs/2026-05-19-claude-fleet-design.md](specs/2026-05-19-claude-fleet-design.md)
§3 and §11 for the full scope boundary.
