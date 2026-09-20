# Post-merge review — Transfer sheet + Conversations background work

Date: 2026-09-20 · Reviewed at `24cead3` (`main`, in sync with `origin/main`)

A whole-branch review of the two features that landed on 2026-09-20:

- `feature/transfer-ui` (`8356217`) — the Transfer sheet, 6185 lines
- `feature/conversations-notifications-background-ebdffa` (`94cbebd`) — background
  work in the Conversations tab, 4691 lines

Both merged straight to `main` as local merge commits rather than through a PR, so
CI ran only after the push. It passed.

## Baseline

Verified locally at `24cead3`:

| Check | Result |
|---|---|
| `npx vitest run` | 2075 passed, 115 files |
| `npx svelte-check` | 0 errors, 0 warnings |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt --all --check` | clean |
| `cargo test --workspace` | 2765 passed — **on the second run**; see F1 |

## Findings

Ordered by the cost of leaving them alone.

### F1 — A wall-clock assertion makes the full suite intermittently red

`crates/fleet-core/src/agent/registry.rs:486`

`a_request_with_no_connection_is_offline_immediately` asserts the call returns in
under 250 ms. The first full-workspace run failed with:

```
waited 305.552666ms before reporting an offline agent
```

It passes in 0.00 s in isolation (3/3 runs) and passed on a second full run. The
bound measures scheduler latency on a loaded box, not the code's behaviour.

The second-order cost is the real one: `cargo test` stops at the first failing
target, so that run **never executed the `claude-fleet` lib tests** — `verdict_gen`,
the hub contract and the routing table. A flake here silently skips a whole
verification layer.

The requirement the test names is "returns now, not after the 60 s budget". A 5 s
bound separates those by 12× and cannot flake.

### F2 — A resumed agent that failed and then succeeded reads `failed` in the thread and `done` in the switcher

`crates/fleet-core/src/service/transcript.rs:949` and `:961`

`join_notifications` applies a call's notifications in transcript order. `result`,
`ended_at` and `done` are **assigned** — last report wins, as the module's own
comment says. `error` is **accumulated** with `|=`, so it can never be cleared.

The frontend's `statusFromReports` (`src/lib/conversation.ts:383`) uses
last-non-null-wins and answers `done`. Verified against the real store:

```
SWITCHER STATUS = done | RESULT = fixed it
```

So a `failed` → `completed` agent renders its successful report under a red `failed`
word in `SubagentBlock`, while the switcher row and `BackgroundDetail` one click
away both say `done`.

This is the ordinary retry path — the transcript's own `<note>` says "a
task-notification fires each time this agent stops". Every existing resumed-agent
test (Rust `the_last_notification_wins_when_an_agent_is_resumed`, and three in
`conversation.test.ts`) uses `completed` twice, so no test covers a mixed sequence.

**Decision:** the newest notification is authoritative for `error` exactly as it
already is for `result`, `ended_at` and `done`. A notification arriving at all is
proof the launch succeeded, so there is no launch-time error worth preserving.

### F3 — `new_bg_session` accepts `requester_session_id` with no validation

`crates/fleet-core/src/mcp/tools/session_ops.rs:351`

`dispatch_task` guards the identical field and says why
(`crates/fleet-core/src/mcp/tools/orchestration.rs:209`):

> The requester (when given) must exist and, for a per-host caller, live on that
> host — otherwise any agent could file tasks as anyone.

`new_bg_session` gained the same parameter in `a5169d0` with none of that. It runs
`require_host` for the *new* session's host, then `stamp_bg_row`
(`crates/fleet-core/src/service/bg_sessions.rs:278`) writes `set_parent_session_id`
straight through. `parent_session_id` is a plain `INTEGER` with no foreign key
(`crates/fleet-core/migrations/020_tasks_and_turns.sql:24`).

Consequences: a per-host token scoped to host A can parent its new background
session to any session on host B, which then lists it in *that* session's Background
switcher; and a nonexistent id is stored silently.

**Decision:** the same `resolve_target_row` call the sibling tool makes.

### F4 — `BackgroundDetail` can render an empty body

`src/lib/BackgroundDetail.svelte:47`, `:80-88`

`nothing` requires `history.length === 0`, but its `{:else if nothing}` arm is last
in the chain. An entry whose single report carries a null `summary` *and* a null
`result` satisfies none of the four arms, so the body renders nothing at all — one
click after a switcher row that said the task reported.

**Decision:** the chain ends in an unconditional `{:else}`. The two cases get
different sentences, because "has not reported back yet" is false for a report that
arrived carrying nothing.

### F5 — `requester_session_id` is invisible to the only caller that can set it

`crates/fleet-core/src/mcp/tools/session_ops.rs:344-349`

The `#[tool]` description never mentions the parameter;
`docs/control-api-reference.md:162` lists the bare name. `Caller`
(`crates/fleet-core/src/mcp/auth.rs:59`) carries no session identity, so an agent
must pass the id explicitly — and nothing tells it to. The desktop passes `null` by
design ("nobody asked for it from inside a session").

The feature is plumbed end to end and will never fire. `dispatch_task`'s description
does mention its copy.

**Constraint:** `the_served_definition_budget_stays_bounded`
(`crates/fleet-core/src/mcp/tools/tests.rs:2122`) caps the served surface at 56,000
bytes. The added clause must be one slim sentence.

### F6 — The git step's detail hides carried dirty work

`crates/fleet-core/src/service/move_session/progress.rs:108`

```rust
pub(super) fn git_detail(commits: u32, dirty: usize) -> String {
    if commits == 0 && dirty == 0 { "nothing to carry".to_string() }
    else { count(commits as usize, "commit") }
}
```

`git_detail(0, 2)` → `"0 commits"`, pinned by the test at `:248`.
`CarryReport.commits` is *unpushed commits only*
(`crates/fleet-core/src/service/move_session/carry.rs:121`); dirty work travels
separately as `dirty_entries`. So a move carrying five uncommitted files and no
unpushed commits reports `git · 0 commits` — the exact behaviour ADR 0002 exists to
add. `dirty` is read only for the zero check, which is the smell.

**Decision:** the detail names both, omitting whichever is zero.

### F7 — The pinned substring is weaker than the one the frontend matches

`src/lib/moveErrors.ts:98` matches `'is not idle'`. The backend test at
`crates/fleet-core/src/service/move_session/mod.rs:4982` asserts only
`contains("not idle")`.

The sibling coupling is pinned correctly: `moveErrors.ts:96` matches
`'already in progress'` and `mod.rs:5193` asserts that exact phrase.

So a reword from "is not idle" to "was not idle" keeps the backend test green while
the frontend silently falls back to showing the raw backend sentence instead of
"wait for the turn to finish". `moveErrors.test.ts:47` hardcodes its own copy of the
message, so it would not catch it either.

**Decision:** tighten the existing assertion to the exact substring the frontend
depends on. No new test.

### F8 — An unknown status is `failed` in the switcher and benign in the thread

`src/lib/conversation.ts:383`

`statusFromReports` sends anything outside `completed`/`stopped` to `failed`. Its two
siblings default the other way: `notificationTone` sends an unrecognised status to
`info`, `notificationMark` to `✓`. A status the parser has not seen therefore shows a
green tick in the thread and a red `failed` in the switcher.

**Decision:** name the failing values explicitly (`failed`, `killed`) and default to
`done`, so all three functions agree.

### F9 — `running` has no visual distinction

`src/lib/ConversationPanel.svelte:1583-1593`, `src/lib/BackgroundDetail.svelte`

In both places only `failed` and `stopped` get a colour; `running`, `done` and `idle`
are all `--fg-muted`. Commit `24cead3`'s own message calls the switcher "the one view
meant to answer what is still live" — sorting carries that alone, with nothing to see.

### F10 — The thread and the switcher disagree about an unreported background agent

`src/lib/SubagentBlock.svelte:30`

`statusWord` is `null` for an unfinished block outside the live turn, and the duration
reads "no result". The switcher lists the same call as `running`
(`transcriptBackground` keeps a not-done subagent with no reports, and
`statusFromReports` answers `running` for an empty report list).

The component already knows better: `onOpen` is passed *precisely* when the switcher
holds an entry for this call.

## Simplifications

- **S1** — `STATUS_WORD` (`src/lib/BackgroundDetail.svelte:21`) is an identity map:
  five keys each mapping to their own name. `{entry.status}` is the same string.
- **S2** — `newest()` (`BackgroundDetail.svelte:34`) duplicates `lastNonNull()`
  (`conversation.ts:399`) — the same walk-backwards-for-non-null, one of them
  module-private.
- **S3** — the notification row is written twice in `ConversationPanel.svelte`
  (`:1262-1280`), a clickable `<button>` and a plain `<div>` with three identical
  spans each.

## Repository hygiene

- **H1** — `.claude/` is untracked and not ignored, holding **14 GB** across 16
  worktrees. `.gitignore:19` covers only `.claude/settings.local.json`, so
  `git status` shows `?? .claude/` and a `git add -A` would try to stage all of it.
  `.mcp.json` and `.ignore` are untracked too (`.mcp.json` checked — no secrets, just
  the graft server).
- **H2** — `origin/feature/design-index-fixes-ffd2d0` is dead: its single commit
  `3747407` is byte-identical to `3d918b5`, already on `main`. Twelve more remote
  branches are 3–4 months stale. Locally, 16 worktrees are registered, several for
  branches that merged months ago.
- **H3** — 42 commits since `v0.2.26`; two complete features unreleased.

## Out of scope

The features themselves are carefully built. `moves.ts`'s result-vs-events race
handling and `join_notifications`' two-pass borrow split are both well reasoned, and
the comments explain the *why*. Nothing here argues with their design.
