# Work graph M4: smarter detection and explanations (plan)

**Date:** 2026-09-24
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M4.
**Design:** `../specs/2026-09-24-work-graph-design.md` §0.3 (resolution rules).
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md`: C9–C15, the detection review (§5, ideas 1–9), and the UX review (suggestion chip, batch review).
**Depends on:** M1b (links, `work_link`, `SessionRow.work`).
**Uses when present:** M3 (tracker key prefixes, URL-to-tracker mapping), M2 (journal, `handover` rows for the loop guard).
**Works without them:** branch keys and URLs still resolve with no tracker.

## Goal

> Most sessions get linked to the right work without anyone touching them,
> every link can say *why*, a wrong one is undone in one keystroke, and a
> rejection is never proposed again.

**Value line:** start working on `abc-123-login`, paste a ticket URL, or open a PR with
`Fixes #42`, and the session's work chip appears by itself. Hovering it
explains the source; `n` rejects it for good.

## Principles for M4 (from design §0.3)

1. **State signals are current, not accumulated (C10).** The current branch and
   the current PR head are *state*. Only their present value is a candidate.
   When it changes, the auto link that signal made ENDS (it is not contested).
   Manual, `started` and `agent` links are never ended by a state change.
2. **Event signals suggest, with evidence.** Event signals are a URL, a key in a
   prompt, or a commit trailer. They produce `suggested` links, except a
   single strong URL, which is confirmed with an Undo.
3. **Decisions are final.** `confirmed` and `rejected` come from a person or an
   explicit agent declaration. A rejected (participant, target) pair is never
   re-suggested.
4. **Tiers, not scores.** `explicit > strong > weak`, with an ordinal rank inside a
   tier. Nothing is learned and nothing is probabilistic. An LLM nudge
   (M4.6) is opt-in and only proposes.
5. **Conversations are windows.** `/clear` is the natural boundary between
   tasks. A link records the conversation it was decided in. Weak
   suggestions decay at the next boundary unless they are seen again.
6. **No new signal log.** The earlier review dropped a separate
   `work_observations` table. State signals are re-derived from columns.
   Event signals write a `suggested` link carrying denormalised evidence
   directly (C13). An unknown key becomes a `ref_key` link that M3's sync
   binds later.

## Tasks

Each task is one reviewable PR in the **Worker** environment. The usual
checks apply, plus `REGEN_*` whenever tools or verdicts change.

### M4.1 One recogniser, two languages, one fixture

- **Rust side.** Add `crates/fleet-core/src/work/recognize.rs`:
  - `recognize(text, ctx) -> Vec<Match { kind: key|url|repo_issue, key?, tracker_id?, span, upper_written }>`.
  - Keys use the M1a rules (C11): an upper-case key is taken as written. A
    lower-case key is accepted only with a letters-only, non-denied prefix,
    and prefixes are **restricted to known tracker prefixes when any tracker
    exists**. A `.` after the number means a version, not a key.
  - URLs use the patterns from the detection review: Jira `/browse/KEY`,
    `?selectedIssue=`, Linear `/issue/KEY`, Asana `/0/<p>/<task>` and
    `/1/<ws>/project/<p>/task/<t>`, GitHub `/<o>/<r>/issues/<n>`. The URL
    host names the tracker when one is configured, which settles the
    "two Jira instances, same key" case.
  - A bare `#123` resolves only against the session's own `owner/repo`, and
    only as a weak match.
- **Shared fixture.** Add `crates/fleet-core/src/work/testdata/recognize_cases.json`
  (input, context, expected matches). Both `cargo test` and a vitest
  (`src/lib/work_keys.test.ts`) run it, so the TypeScript fallback and the Rust
  recogniser can never drift.
- **TypeScript side.** `work_keys.ts` gains `extractTicketRefs` (URLs and `#n`)
  so the M1a fallback and the dialog agree with the backend.

### M4.2 Signals (backend)

1. **Live branch, for free.** `context::refresh` (`service/context.rs:80-105`)
   already reads the transcript tail after every Stop, and every JSONL line
   carries `gitBranch`. Store it with migration 049:
   `sessions.current_branch TEXT, current_branch_at INTEGER`.
   - These columns are **not** in reconcile's `ON CONFLICT` list.
   - bg and external rows are covered, and `gh` is not needed.
   - Fallback: `worktrees.branch` via `worktree_id`.
2. **PR probe fields.** In `service/outcome.rs`, extend the `gh pr view --json`
   list with `headRefName,title,body,closingIssuesReferences`. Keep
   `body` ≤4k and parse it, never store it. Append
   `git log --format=%B @{u}..HEAD | head -200` to the same probe script to
   read commit trailers (`Refs:`, `Fixes`, `Jira:`, `Closes #n`). This costs no
   extra SSH round trip.
   - Caveats from the specs: the probe exits early without `gh`, is capped at
     12 sessions, and has a 300 s TTL.
3. **Prompt signal.** The UserPromptSubmit handler (`service/hooks.rs:784`) runs
   `recognize` over the **full** prompt. It stores only matches: matched text
   ≤80 chars, a ±40-char snippet (redactable) and the conversation id.
   - **Loop guard (C14):** skip text fleet itself injected, meaning a prompt
     equal to the last fleet-sent `last_prompt`, or any delivered `handover`
     row or brief.
   - **Dump guard:** more than 3 distinct keys in one prompt means a reference
     list. All of them are weak and none is pre-selected.
   - **First prompt of a conversation:** a sole key or URL there is
     pre-selected. It is still a suggestion unless it is a URL.
4. **Agent declaration.** This already exists as `work_link { source: agent }`
   (M1b.2). Also accept an optional `work_item` param on `set_friendly_name`,
   **only if** the budget allows; otherwise the skill calls `work_link`. The
   `fleet-friendly-name` skill gets one line: "if you know the ticket, declare
   it". It fires on the same deterministic triggers (first prompt, after
   `/clear`, the heartbeat).

### M4.3 Resolver (backend, pure)

- Add `crates/fleet-core/src/work/resolve.rs` with
  `resolve(input) -> Vec<LinkChange>`.
  - `input` holds the current state signals (branch, PR head, closing refs),
    the new event matches, the existing live links with their state, source,
    evidence and window, the trackers' prefixes, and the per-project trust
    flag.
  - `LinkChange` is one of `Create{suggested|confirmed}`, `End{reason}`,
    `Promote`, `Decay`, or `Touch` (seen again).
- **Rules**, numbered so evidence can name them:

  | Rule | Condition | Outcome |
  |---|---|---|
  | R1 | a manual `confirmed` / `rejected` decision | final; the resolver never touches it |
  | R2 | `explicit` (`started`, `agent`) | confirmed |
  | R3 | exactly one `strong` state candidate (branch key, PR head key, PR closing ref) in a repo whose project is trusted | confirmed, `auto`, with Undo |
  | R3b | the same candidate in an untrusted project | a pre-selected suggestion |
  | R4 | several `strong` candidates | all suggested, none primary |
  | R5 | a ticket URL in a prompt | strong; confirmed only when it is the sole candidate in the first prompt of the conversation, otherwise suggested |
  | R6 | a weak candidate (prompt key, trailer, `#n`) | suggested; decays at the next conversation boundary unless seen again |
  | R7 | the value of a state signal changes | End the auto link that signal created (`reason: branch_changed`); manual, started and agent links stay |
  | R8 | the same key in two trackers | never automatic; a suggestion with both candidates |
  | R9 | a rejected (participant, target) pair | never re-suggested, from any signal |
  | R10 | review and worker sessions | inherit the parent's primary (from M2.2; listed here for completeness) |

- **Primary link:** explicit beats strong beats the most recent decision. The
  user can change it.
- **Windows.** Migration 049 adds these columns to `work_links`:
  `claude_session_id`, `strength`, `rule`, and `evidence` (a JSON array). The
  `state` column now also takes `suggested`.
  - A `/clear`, i.e. SessionEnd(clear) followed by SessionStart(clear),
    closes the window. The latest conversation's resolution decides the
    primary link.
  - Past windows stay as history in the journal (M2).
- **Triggers**, all on the hub side with the store lock held briefly:
  - after a UserPromptSubmit hook,
  - after a Stop hook (when the branch is refreshed),
  - after a PR probe result,
  - after a tracker sync binds keys (M3),
  - after a manual decision.
  Each run is idempotent and emits `session_updated` only on a real change.
- **Tests.** Table-driven tests over `resolve` covering:
  - conflicts; rejection stickiness; a branch change ending an auto link but
    never a manual one;
  - two trackers sharing a key; a key that becomes known late;
  - the dump guard; the loop guard; decay at a boundary;
  - trusted vs untrusted projects; primary selection.

  Integration tests drive the hook, the probe and `FakeSsh` through to the
  links.

### M4.4 Explanations and correction (API + UI)

- **API.**
  - `work_link` gains `confirm` and `reject` for an existing suggestion by
    `link_id`, and `trust_project {project_id, on}`. Trust is stored as a
    JSON set in a `work.trusted_branch_projects` setting (a typed-registry
    `Kind` like `PathMap`).
  - `SessionRow.work` summary gains `state`, `strength`, `rule`, and
    `suggestions: n`.
  - `work { session_id }` returns the full evidence.
- **Chip states** in `SessionRowItem`:
  - solid means confirmed;
  - a small dot means auto (R3/R5), plus a toast "Linked blue-sirius →
    ABC-123 (branch) · Undo";
  - dashed with `?` means a suggestion.
- **Popover:** evidence lines such as "branch `abc-123-login` since 09:05 ·
  R3" and "mentioned ABC-99 in a prompt at 10:12 (reference)". Actions:
  - `Confirm` (↵)
  - `Not this` (⌫, sticky)
  - `Pick another…`, which opens ⌘K in ticket/key mode
  - a checkbox "Trust branch keys in this repo"
- **Batch review.** An Attention entry "5 link suggestions · Review" opens a
  sheet with j/k to move and y/n to decide. It reuses the Sidebar
  `selectedIds` and bulk patterns.
- **Keyboard on a focused row:** `l` links or picks, `y` / `n` decide the
  top suggestion.
- **Group-by-Work.** Suggestions do **not** move a session into a group; only
  confirmed links do. A suggested session shows its chip under its project.
  Nothing jumps around on a guess.
- **Tests:** chip-state rendering, popover actions with the right `invoke`
  args, the batch sheet keyboard, the Undo toast, and a grouping test showing
  a suggestion does not regroup.

### M4.5 SessionStart context (gated by a measurement, decision D5)

- **Measure first.** Time a synchronous SessionStart command hook with
  `curl --connect-timeout 1 -m 2 … || true`, against a hub that is up and
  against one that is down, on a local and a remote host. Record the numbers
  in this plan.
- If the numbers are acceptable, `hooks_install.rs` switches SessionStart from
  `async` with `-o /dev/null` to synchronous with stdout passed through
  (C15). `mcp/hooks.rs` then returns
  `hookSpecificOutput.additionalContext` for sources `startup`, `resume`
  and `compact`:
  - the linked item's title, status, URL and branch, plus the M2 handover
    (≤4000 chars, inside `mark_untrusted`);
  - **not** on `clear`, which provisionally closes the window.
- First verify against the pinned Claude Code version that SessionStart
  command hooks honour `additionalContext`, the same way the 8000-char cap
  was measured. This behaviour is behind the setting
  `work.session_start_context`, default **off** until measured.
- **Tests:** a hooks_install golden, a hook response shape test, and
  source-by-source behaviour.

### M4.6 Opt-in classification nudge (optional, last)

- **When it fires.** Only when all of these hold:
  - the setting `work.classify_nudge` is on;
  - the session's repo maps to a tracker (M3) or has local items;
  - there is no link after 3 prompts;
  - there are ≤5 candidates in scope ("My work" plus recent local items);
  - it has not fired yet in this conversation.
- **What it sends.** ≤400 chars of `additionalContext` on UserPromptSubmit,
  packed **after** inbox mail:
  "If this work is one of: ABC-1 (title)…, call `work_link {action: link, source: agent}`. Otherwise ignore; don't ask the user."
- **Result.** Recorded as `agent_inferred`, a new source whose tier sits
  between strong and weak. It is shown as a pre-selected suggestion, never
  auto-confirmed.
- **Never:** a Stop `block`, or a prompt typed into the pane.

### M4.7 Docs and roadmap

- Design §0.3 revision: the final rule table and the chip vocabulary.
- `docs/control-api.md` and the control skill: the agent declaration and the
  evidence reads.
- Roadmap status.

## Acceptance (manual)

1. **Branch auto-link, then a branch change.**
   - In a trusted project, create the branch `abc-123-login`. Within one
     Stop, the chip shows `ABC-123` with the auto dot and an Undo toast.
   - Checking out `abc-130-other` ends that link and creates `ABC-130`.
   - A manually linked key survives both.
2. **Prompt reference.** A first prompt of "see ABC-99 for context, fixing the
   retry bug" produces a dashed `?` suggestion for `ABC-99`, not a group.
   `Not this` means it never comes back in this session.
3. **URL.** A first prompt that is only a Jira URL gives a confirmed link. The
   URL host settles which tracker it belongs to.
4. **`/clear`.** After `/clear`, a new task's key becomes primary, and the
   previous link is history in the journal.
5. **PR.** A PR with `Fixes #42` links the GitHub issue once the probe runs,
   provided GitHub Issues exists as a tracker (M6); otherwise the ref is kept
   unresolved.
6. **Batch review.** After connecting Jira with 10 sessions that mention keys,
   the Attention entry lets you confirm or reject all of them from the
   keyboard in under a minute.
7. **Loop guard.** A resumed session's handover brief mentions three keys, and
   none of them become suggestions.

## Risks

| Risk | Mitigation |
|---|---|
| False positives annoy the user | Suggestions never regroup; auto-links only happen in trusted repos, always come with Undo, and a rejection is sticky; prefixes are restricted to known trackers |
| Chip flapping when switching branches | R7 ends only the auto link its own signal created; the primary changes only on a real state change; the UI shows the change as a toast with Undo |
| Prompt privacy | Prompts are never stored; only matches and a redactable snippet; the snippet can be turned off (`work.evidence_snippets`, default on) |
| Hook latency (SessionStart) | Gated by a measurement and a setting; `-m 2 \|\| true` |
| Budget for the MCP tool descriptions | Everything goes through actions on `work` / `work_link`; no new tools |
| The resolver runs too often | It runs only on the listed triggers, is idempotent, and emits only on change |

## Decisions (defaults if unanswered)

| # | Question | Default |
|---|---|---|
| D5 | Is a synchronous SessionStart hook acceptable (up to 2 s when the hub is down)? | Measure in M4.5; off until then |
| roadmap 5 | Auto-confirm a single strong branch key? | Yes, but only in trusted projects. A project becomes trusted by the popover checkbox, or automatically after 3 confirmed branch links in it (visible, reversible) |
| new | Store prompt evidence snippets? | Yes, ±40 chars, with a setting to turn them off |
| new | Classification nudge? | Off; opt-in per fleet |
