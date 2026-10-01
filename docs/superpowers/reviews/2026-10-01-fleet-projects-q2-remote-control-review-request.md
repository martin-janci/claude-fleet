# Independent check: is Remote Control a usable bridge for Fleet Projects?

You are reviewing a claim made by another agent. **Your job is to try to break it**,
not to confirm it. If it survives, say so plainly; if it does not, say exactly which
part fails and what the evidence is. Do not be agreeable.

## Before you start — where everything is

The spec under review is **not on `main`**. It lives only on a feature branch, so a
fresh clone of the default branch will not contain it.

| What | Where |
|---|---|
| Repository | `martin-janci/claude-fleet` |
| Branch holding the spec | `claude/youthful-heisenberg-lnexvr` — read its **head**, not a pinned commit |
| The spec | [docs/superpowers/specs/2026-10-01-fleet-projects-design.md](https://github.com/martin-janci/claude-fleet/blob/claude/youthful-heisenberg-lnexvr/docs/superpowers/specs/2026-10-01-fleet-projects-design.md) |
| This review request | `docs/superpowers/reviews/` on the same branch |

To get it:

```bash
git clone --branch claude/youthful-heisenberg-lnexvr https://github.com/martin-janci/claude-fleet.git
# or, in an existing clone:
git fetch origin claude/youthful-heisenberg-lnexvr && git checkout claude/youthful-heisenberg-lnexvr
```

If you cannot clone, read the files through the GitHub API or the blob URL above.
Should that URL 404 because the branch name contains slashes, use a commit form
instead: `.../blob/<sha>/<path>`, taking `<sha>` from the branch head (it was
`2b2b7138` when this request was written, and the spec may have moved forward
since — prefer the head).

**Which files are where** — this matters, because two of the documents named below
are also not on `main`:

- Everything under `crates/`, `src-tauri/`, `tools/`, `CLAUDE.md`, and every
  `docs/superpowers/specs/2026-09-*` document **is on `main`** and also on this
  branch. Reading them from either is fine.
- `docs/superpowers/specs/2026-10-01-fleet-projects-design.md` and the review
  requests exist **only on `claude/youthful-heisenberg-lnexvr`**.
- `docs/superpowers/plans/2026-09-30-assets-m2-sync.md` and
  `crates/fleet-core/migrations/091_catalog_ids.sql` exist **only on the branch
  `feat/assets-m2-sync`**, which is open pull request
  [martin-janci/claude-fleet#416](https://github.com/martin-janci/claude-fleet/pull/416).
  Read them from that branch or from the pull request.

If the repository is unreachable to you entirely, say so rather than guessing —
but note that the vendor-documentation half of this check (the claims about what
the Claude Code CLI and its docs do or do not support) stands on its own and can
still be answered.

## Context you need

The repository `martin-janci/claude-fleet` is a Tauri desktop app + `fleet-hub`
daemon that manages long-lived Claude Code sessions running in **tmux on remote
hosts over SSH**. Read `CLAUDE.md` first for orientation.

A design spec was added at
`docs/superpowers/specs/2026-10-01-fleet-projects-design.md`. Read its §2 F1, §7
and §16 Q2. The claim under review is **§16 Q2 / decision P12**:

> Remote Control (`claude remote-control` in server mode) should be built as phase
> FP7, and it delivers more of the design's goal R6 than cloud execution (FP6),
> because:
> 1. the docs' own limitations section says to run it in `tmux`/`screen` on a
>    remote machine to survive SSH disconnect — which is exactly Fleet's
>    architecture;
> 2. `--spawn worktree` gives each on-demand session its own git worktree, which
>    matches Fleet's worktree model;
> 3. server mode is how a thread of a **native Claude Project** executes on a
>    machine the operator controls;
> 4. because such a thread is ordinary local Claude Code on that host, Fleet's
>    catalog assets, hooks, the repo's `CLAUDE.md` and the spec's §6 project
>    header all apply to it — whereas a **cloud** thread gets none of them;
> 5. Fleet already discovers sessions it did not start as `kind='external'` rows
>    parsed from `claude agents --json` (`crates/fleet-core/src/claude_agents.rs`),
>    so the observation channel may already exist.

## What to verify

Check each numbered claim against **primary sources**: the live vendor docs at
https://code.claude.com/docs/en/remote-control , https://code.claude.com/docs/en/claude-projects ,
https://code.claude.com/docs/en/cli-reference , and the actual code in this repo.
Quote what you find. Where the docs are silent, say "unverified" rather than
inferring.

Then attack the design, not just the facts:

1. **The observation question — the most important one in this review.** Does
   `claude agents --json` actually list the sessions a `claude remote-control`
   server serves? If not, what is left — is reading `~/.claude/projects/*.jsonl`
   enough for Fleet to show status, and what does Fleet's `claude_agents.rs` /
   reconcile do with a session it cannot see in that listing?
   This answer is load-bearing **twice**: it decides whether Fleet can show the
   thread's status, and — per the claim-4 correction in point 5 — whether the
   thread can receive a Project's context at all, since that is delivered against
   a `SessionRow` that only exists if the listing produced one. If the answer is
   no, say plainly that decision P12 ("FP7 beats FP6") should change.
   Already established by a run on the owner's machine (spec §16, 2026-10-01):
   the rows carry `id, kind, name, pid, sessionId, cwd, startedAt, state, status`,
   `kind` is only `interactive` or `background`, and **nothing marks a Remote
   Control session** — so "listed" alone may not be enough to tell one apart, and
   you should say how Fleet could distinguish them (pid? cwd? the server's own
   pid as parent?).
2. **Ownership collisions.** Fleet owns tmux panes and assumes it started what it
   manages. A remote-control server creates sessions Fleet did not start, in
   worktrees Fleet may or may not know about. Find the concrete places this
   breaks: reconcile, the GC sweep (`gc.external_lost_ttl_secs`), worktree
   bookkeeping, `move_session`, rewind, and the stale-working veto. Does
   `--spawn worktree` put worktrees where Fleet expects them
   (`<repo>/.claude/worktrees/<name>`) or somewhere that confuses `refresh_projects`?
3. **The launcher constraint.** The docs say a global `claude` flag placed before
   `remote-control` is refused when dropping it would change what the sessions can
   do. Fleet launches through the `ag` launcher (`tools/ag/`, see
   `docs/superpowers/specs/2026-09-29-multi-harness-agents-design.md`). Does `ag`
   wrap flags in a way that would make `ag claude remote-control` refuse to start?
4. **Supervision.** Server mode exits after roughly 10 minutes of network outage.
   Does Fleet have a supervision path that would restart it, or is that new work?
5. **Claim 4 is already known to be partly wrong — confirm or extend the
   correction.** The spec's §16 Q2 now carries a correction found before this
   request went out, and your job is to check it rather than rediscover it:
   Fleet's hooks are installed **user-level** in the host's
   `~/.claude/settings.json` (`hooks_install.rs`), so they do fire for a
   Remote Control thread; but Fleet's *row-keyed* context (the mail, the handover
   brief, the §6 project header) only reaches it once an `external` row exists,
   and `X-Fleet-Pane` is `$TMUX_PANE`, which `resolve_hook_row`
   (`crates/fleet-core/src/service/hooks.rs`) tries **before** the session id — so
   a server sharing a pane with a Fleet session misattributes every one of its
   sessions' hooks to that row.
   Verify that reading of `resolve_hook_row`, `rebind_eligible`,
   `find_session_by_pane` and `agent_row_name`. Then push further: is the proposed
   mitigation (the server in its own tmux session) actually sufficient, or does
   something else — `reconcile`, `trusted_status`, `pane_working_at`, the
   stale-working veto, conversation tracking — still cross the wires? Is there a
   case where `rebind_eligible` *does* let the row's id move (an awaiting-rebind
   row, a stopped row, an ended conversation, a `SessionStart(clear)`) and a
   Remote Control session could therefore capture a Fleet row's identity?
6. **The comparison.** Is "FP7 beats FP6" defensible, or does it depend on the
   operator already using native Claude Projects — which are Pro/Max only, one
   user, and unshareable? Argue the other side.

## How to answer

- A verdict per numbered claim: **confirmed / weaker than stated / wrong /
  unverified**, each with the quote or `file:line` it rests on.
- The answer to question 1, stated as plainly as you can get it.
- The three strongest reasons **not** to build FP7, in priority order.
- Any correction the spec needs, as the exact replacement sentence.

Do not edit the repository. Do not run `claude --cloud`, `claude remote-control`,
or anything else that creates a session, a cloud session or a pull request — this
check is read-only.
