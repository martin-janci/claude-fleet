# Work graph M2: resume and work memory (plan)

**Date:** 2026-09-24
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M2.
**Design:** `../specs/2026-09-24-work-graph-design.md` §0.6.
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md` (C5–C7, C14, C16; detection §5 journal).
**Depends on:** M1b (migration 046 `work_links` with its end snapshot, `store/work.rs`, `SessionRow.work`, and MCP `work` / `work_link`).

## Goal

> "Pick up ABC-123 where I left off": even weeks after its sessions were
> killed, fleet shows the past work and resumes it with the context intact,
> and never types into a dialog or destroys anything.

**Value line:** from a work group, one click either continues the last Claude
conversation on its host and worktree, or starts a fresh session with a
handover brief.

## What already exists (reuse, do not rebuild)

| Need | Existing piece |
|---|---|
| Resume a conversation | `new_session { resume_claude_session_id }` (`service/sessions/lifecycle.rs:328,414-423`); `recreate_session`; `discover_lost_sessions` (`LostCandidate.git_branch`) |
| Past sessions as data | `work_links.ended_at` + `snap_*` (host, tmux, label, project, worktree, branch, PR, **all** claude ids), written by migration 046's trigger |
| Claude's own summaries | `ConvItem::Compact { summary }` in `service/transcript.rs:447,944-953` (the `isCompactSummary` entry, ≤20k chars) |
| Turn-level progress | the `turn_done` detail (the first 200 chars of `last_assistant_message`, `service/hooks.rs:738`) |
| Hook points | `apply_post_compact_hook` (`hooks.rs:675`), `apply_session_end_hook`, Stop; `record_kill`; the ghost reap |
| Context injection | inbox → `additionalContext` on UserPromptSubmit / Stop (`service/delivery.rs`, 8000 chars / 200 lines, whole messages only) |
| Safe prompt sending | `wait_for_repl_ready`, `stuck_kind=trust_prompt` detection (`pane_intel.rs:389`), `prompt_submit_seq` (042) |
| Untrusted text | `mark_untrusted` / `UNTRUSTED_END` (`mcp/guard.rs:1213`) |

## Design decisions for M2

1. **The journal is keyed by conversation, not by session or link.**
   `work_journal.claude_session_id` is the durable join: every link's
   `snap_claude_ids` (or the live session's conversations) names the
   conversations that belong to a work key. A later relink or reject
   reassigns history without rewriting it, and a swept participant loses
   nothing.
2. **Journal every fleet session, linked or not, within caps.** Work is often
   linked after the fact (a key typed later, a tracker connected later), and
   M1b's links bind retroactively only if the history is there. Caps per
   conversation: 1 `conversation` row, the last 5 `progress` rows, the last 3
   `compact_summary` rows. Global retention is `work.journal_days`
   (default 90, `0` = keep forever).
3. **Deterministic first.** The handover is a template over stored facts plus
   one git probe. No LLM call in M2. Summaries written by Claude itself (its
   compaction summary) are harvested, not generated.
4. **Pull beats push.** The brief injected at start stays short, at most
   4000 chars. The full context is a `work` read (`action: context`) that the
   agent can call.
5. **Never type into an unknown pane.** The brief travels as a message from
   the **hub** participant through `additionalContext`. Only a short start
   prompt is typed, and only when the REPL is ready and
   `stuck_kind != trust_prompt`. Delivery is confirmed by `prompt_submit_seq`.
6. **Resume never destroys or duplicates.**
   - A key with a live session offers *Jump*, not a second resume.
   - When the worktree is gone (for example after a safe kill), resume
     recreates it from the snapshot branch (`base_branch = snap_branch`,
     since the branch was pushed) instead of failing.
   - When the transcript is gone (purge, or a host that cannot be reached),
     *continue* is disabled with the reason, and *fresh with brief* still
     works.

## Tasks

Each task is one reviewable commit or small PR. Run fmt, clippy
`-D warnings`, the touched crates' tests, `pnpm check` and `pnpm test` for
each, plus `REGEN_DOCS` / `REGEN_HUB_VERDICTS` whenever tools or verdicts
change. Work in the **Worker** environment, where cargo reaches crates.io.

### M2.1: journal storage and harvest (backend)

- **Migration 047 `work_journal`:**
  `id, claude_session_id TEXT NOT NULL, participant_id REFERENCES participants ON DELETE SET NULL, at, kind, source, body, meta`,
  plus the indexes `(claude_session_id, kind, at DESC)` and `(at)`.
  - `kind` ∈ `conversation | progress | compact_summary | outcome | note`.
  - `source` ∈ `hook | transcript | probe | agent | fleet`.
- **`store/work_journal.rs`:**
  - `append_journal` enforces the per-conversation caps in the same
    transaction and upserts the single `conversation` row.
  - `journal_for_conversations(&[id])` and `journal_for_key(key)`. The key
    lookup joins live links (through participant → conversations) and ended
    links (`json_each(snap_claude_ids)`).
- **Harvest points:**
  - **Stop:** a `progress` row from the `turn_done` detail. Skip empty
    details and exact repeats of the previous one.
  - **PostCompact:** read the transcript tail off-lock, the same way
    `context::refresh` does, and store a `compact_summary` row from the last
    `ConvItem::Compact.summary`, capped at 12k chars.
  - **SessionEnd, `record_kill` and the ghost reap:** upsert the
    `conversation` row with first prompt, turns, compactions, span and
    `end_reason`. This runs **before** the cascade (C5): `conversations` rows
    die with the session.
- **GC:** a sweep for rows older than `work.journal_days`. It goes in
  `gc.rs`'s ungated sweeps, next to the cursor sweep.
- **Tests:**
  - the caps;
  - the journal survives `delete_session`, move and sweep;
  - `journal_for_key` through both live and ended links;
  - harvest with fake hooks and a transcript fixture holding an
    `isCompactSummary` entry.

### M2.2: carry rules (backend, small)

All of these come from review C7.

- **Resume auto-carry:** when a session starts on a `claude_session_id` that
  appears in an **ended** link's `snap_claude_ids` (SessionStart with
  `source=resume`, or `new_session` with `resume_claude_session_id`), relink
  it to the same target with `source='resumed'`. A matching live link means
  the work is already carried, so do nothing.
- **`keep_source` fork:** copy the source's confirmed links to the target
  with `source='forked'`.
- **Inheritance:** review sessions (`reviews_session_id`) and task workers
  (`parent_session_id`) inherit the parent's primary link, with `role`
  `review` or `worker`. This adds a `role` column to `work_links` (migration
  047 is shared with M2.1).
- **Purge warning:** `purge_project` reports which work keys lose resumable
  conversations. The link is marked `resumable=0` and the UI disables
  *continue*.
- **Tests:** a resumed id re-attaches; a fork copies; reviews and workers
  inherit; a purge marks.

### M2.3: handover brief (backend, pure plus one probe)

- **`service/work/handover.rs`:**
  - `build_handover(input) -> String` is pure and snapshot-tested.
  - `gather_handover(key)` assembles the input: item, live and ended links,
    journal, and one git probe on the target host (branch, head sha, ahead
    and pushed, dirty, `diff --stat` top 15, last 10 subjects). It reuses
    `repo_read` / move `probe.rs` patterns with `GIT_OPTIONAL_LOCKS=0`.
- **Template** (design review §5), always under 4000 chars. The full version
  is served by `work { action: context }`:
  ```
  # ABC-123 — <title> [<status>] <url?>
  Prior work: <n> sessions / <m> conversations, last active <date> on <host>
  Branch <b> @ <sha> (<k> ahead, pushed: y/n) · PR <url> CI <state>
  Worktree <host:path> (present|removed) · uncommitted: y/n
  Recent commits / changed files (capped)
  Timeline: conv1 "<first prompt>" (12 turns) → conv2 …; last: "<progress>"
  Latest summary (<compaction>, <date>): <≤2000 chars>
  Verify the git state before acting; this summary may be stale.
  ```
- **Wrapping:** journal and summary text sit inside `mark_untrusted` fences,
  with fleet-authored lines outside them. This covers C14 and the injection
  finding.
- **Delivery:**
  - Add `Store::insert_system_message(to_session, body, kind)` with the
    **hub** participant as sender. First check that
    `session_messages.from_session_id` accepts NULL; if not, it needs a
    guarded migration.
  - It is delivered on the next UserPromptSubmit or Stop through the
    existing packer.
  - `kind='handover'` is excluded from key detection (the loop guard,
    ahead of M4).
- **Tests:**
  - template snapshots: full, a bare key, no git, a removed worktree, an
    oversized summary;
  - the untrusted fencing;
  - `insert_system_message` delivery through `pack`.

### M2.4: `resume_work` over MCP and Tauri

- **Action:** `work_link { action: resume, key | link_id, mode: last|brief|fresh, host_alias? }`.
  It goes on the existing grouped tool (budget C21); no new tool. The
  service `service/work/resume.rs` handles it:
  - it picks the most recent ended link, or the one given;
  - it resolves the host (the snapshot's host if reachable, else
    `host_alias`, else an error that names the choices);
  - it resolves the project and worktree (the existing worktree, or
    `new_worktree = snap_worktree` with `base_branch = snap_branch`);
  - `last` calls `new_session { resume_claude_session_id = last(snap_claude_ids) }`;
    `brief` and `fresh` call a plain `new_session`, and `brief` then enqueues
    the handover and sends the short start prompt safely (decision 5);
  - it links the new session (`source='resumed'`) and returns the row.
- **Resume plan:** `work { action: resume_plan, key }` returns the candidates
  and which modes are possible and why. The UI renders it without guessing.
- **Wiring:** Tauri `resume_work` / `work_resume_plan` are **Routed**. Then
  `REGEN_HUB_VERDICTS`, `REGEN_DOCS`, and the skill / control-api doc lines.
- **Tests:**
  - the service against `FakeSsh` for: host reachable or not, worktree
    present or removed, transcript present or purged;
  - routing tests;
  - a guard policy row.

### M2.5: UI (frontend)

- **Past work in groups:**
  - `work.ts` loads ended links for the visible keys (`work { key }`), plus
    **recent past-only keys** (ended within `work.recent_days`, default 14,
    no live session) so reopened work has somewhere to show.
  - In Group-by-Work they render as ghost rows (snapshot label · host ·
    branch · "ended 3d ago") under a collapsed `Done · n` inside the group.
    A key that has only past work gets a group of its own, collapsed.
- **Resume ▾ split button** on the ghost row and the group header:
  - The default is *Continue last conversation*. Alternatives: *Fresh with
    brief* (opens an **editable preview** of the brief) and *Fresh*.
  - Disabled modes show their reason from `resume_plan`.
  - The tooltip names the host and branch it will land on.
- **New session dialog duplicate guard:** a key with **only past** work shows
  "ABC-123 has previous work (2 sessions, last Tue) · Resume" instead of
  "already running".
- **Purge confirmation** lists the work keys that lose resumable
  conversations.
- **Tests:** `work.ts` loaders; Sidebar with past-only groups and collapsed
  Done; the Resume menu's disabled reasons; the dialog guard variant; the
  preview edit reaching the call.

### M2.6: docs and roadmap

- Record the resume and journal semantics in the design §0.6 revision.
- Add the `work` / `work_link` actions to `docs/control-api.md` and
  `skills/claude-fleet-control/SKILL.md`: an in-session agent can call
  `work { action: context }` when it is picking up work.
- Update the roadmap M2 status and Revisions.

## Acceptance (end-to-end, manual on a real fleet)

1. **Continue after a kill:**
   - Start `ABC-123 Fix login` on host A, work two turns, `/compact`, then
     kill the session.
   - The group shows it under *Done*. *Continue* opens it on host A in the
     same worktree, with the same conversation.
2. **Fresh with brief after the worktree is gone:**
   - Safe-kill the session, which removes the worktree.
   - *Fresh with brief* recreates the worktree from the pushed branch.
   - The brief arrives with the first prompt and names the branch, the PR
     and the compaction summary. Nothing is typed before the trust dialog is
     answered.
3. **Move:** move a linked session to host B. The link stays live and the
   journal keeps growing under the same key.
4. **Resume from outside fleet:** resume a conversation with
   `claude --resume <id>` outside fleet's buttons. The link re-attaches
   automatically (`resumed`).
5. **Purge:** purge the project and confirm the warning.
6. **Older hub:** against a hub without these tools the UI hides Resume and
   explains why (tool-list gating, C19).

## Risks

| Risk | Mitigation |
|---|---|
| Journal growth | Caps per conversation, retention sweep, tail reads only |
| Stale brief misleads the agent | "Verify the git state" line; a live git probe at brief time; timestamps on every section |
| Brief crowds the inbox (shared 8000-char cap) | ≤4000 chars; `pack` keeps whole messages; overflow reaches the agent via `work { action: context }` |
| Resume on the wrong host or path | `resume_plan` names the host, path and branch before anything runs; host/path resolution tested with FakeSsh |
| Prompt injection through summaries | `mark_untrusted` fences; fleet text outside them |
| Second brain in hub-client mode | Harvest runs only in hooks (hub side); the desktop only routes |

## Open questions (with defaults)

1. **Journal retention:** default 90 days. Should linked work be kept longer?
   Proposal: never sweep journal rows of conversations referenced by a
   **confirmed** link.
2. **Journal unlinked sessions?** Default yes, within the caps. The
   alternative is to journal only once a link exists, which loses history
   that is linked after the fact.
3. **Auto-carry on resume:** default on. It is always visible and reversible
   with *Not this*.
4. **Brief default:** should *Fresh with brief* show the preview every time,
   or only the first time? Default: every time, editable (the FAB spec's rule:
   no invisible prompts).
