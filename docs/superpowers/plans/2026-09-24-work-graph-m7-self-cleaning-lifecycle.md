# Work graph M7: self-cleaning lifecycle (plan)

**Date:** 2026-09-24
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M7
**Design:** `../specs/2026-09-24-work-graph-design.md` §0.6 (lifecycle: archive = link `ended_at`, UI-only by default) and §H (the six lifecycle layers)
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md`, the UX review (Tidy-up sheet, reopened badge) and the devil's-advocate review (M7 = hints, never auto-kill)
**Depends on:**
- M1b (links)
- M2 (ended links, snapshots, `resumable`, Resume)
- M3 (item status and `status_changed_at`)

**Uses when present:** M4 (PR merged, via the probe), M5 (per-org policy).

## Goal

> The sidebar stays about current work without the user curating it, and
> nothing useful is ever destroyed automatically. Fleet *suggests* cleaning
> up; the user confirms in one sheet. Done work collapses out of the way.
> Work that comes back (a reopened ticket) announces itself with its history.

**Value line:**
- A "Tidy up · 6" entry appears only when there is something to tidy.
- One sheet, preselected, with safe defaults, clears the day's finished work.
- A reopened ticket shows "reopened · 2 past sessions · Resume".

## What exists and what M7 changes

| Piece | Today | M7 |
|---|---|---|
| `service/gc.rs` | opt-in (`gc.enabled`, default off) **idle killer** per session kind (`bg` / `shell` / `work` TTLs); `plan()` → `Planned { action: Kill \| InspectThenKill }`; dirty worktrees go through safe-kill | The same planner gains **work-aware reasons** and a **suggest-only** output. The killer's behaviour is unchanged unless the user opts into auto-tidy (below). It never kills a session linked to in-progress work |
| `attention.ts` | triage buckets (`waiting`, `stuck`, `failed`, `done_unread`, `lifecycle`, `idle_long`, …) | Adds a **non-needs-you** "Tidy up" entry and a **reopened** bucket |
| Archive | = link `ended_at` (M1b trigger); ended links render as ghost rows under *Done* (M2.5) | Adds a **UI-only archive** of a *live* session: it collapses into Done and tmux keeps running |

## The six lifecycle layers (design §H)

M7 touches only the first two rows automatically, and only on suggestion.

| Layer | M7 may… | Never |
|---|---|---|
| work link | end it (archive) when the session is killed; mark a live session archived (UI-only) | delete it |
| sidebar visibility | collapse archived or done work | hide needs-you rows |
| tmux session | kill it via **safe kill**, only on the user's confirm or with opt-in auto-tidy | kill a dirty worktree without the safe path |
| Claude conversation / transcript | — | delete |
| worktree / branch | — (safe kill removes the worktree only after Claude committed and pushed) | delete an unpushed branch |
| journal / history | — | delete (retention is M2's setting) |

## Design decisions

1. **Suggest first.** Tidy-up candidates are computed on every GC sweep and on
   tracker-status or PR changes, then **shown**. Nothing is acted on
   without a confirm, unless the user has turned on `work.auto_tidy` **per
   org** (M5) or globally.
2. **Reasons are explicit and ranked.** Every candidate carries one primary
   reason and optional secondary ones. The sheet groups by reason.

   | Reason | Rule | Default action |
   |---|---|---|
   | `done_idle` | the linked item is `done` for ≥ `work.tidy_done_days` (default 2) **and** the session has been idle ≥ `work.tidy_idle_hours` (default 4) | safe kill (archive) |
   | `pr_merged_idle` | the session's PR is merged (M4 probe) and it has been idle ≥ `tidy_idle_hours` | safe kill |
   | `not_planned` | the item was resolved as won't-do or duplicate, and the session is idle | safe kill |
   | `duplicate_worktree` | ≥2 live sessions on the same `(host, project, worktree_key)` and at least one idle ≥ `tidy_idle_hours` | kill the idlest; keep the most recent |
   | `ghost_expiring` | a resumable ghost within 24 h of `sessions.lost_ttl_secs` | **Resume** or *let expire* (no kill; the row is already gone) |
   | `idle_unlinked` | no work link and idle ≥ `gc.work_idle_secs` (the existing GC rule) | UI-only archive (tmux stays) |

3. **Protections that no rule or setting overrides:**
   - never a session whose linked item is `in_progress`;
   - never a session that is `working`, `blocked`, stuck or needs-you;
   - never the controller or operator session;
   - never a session touched by the user in the last hour (a prompt or an
     attach);
   - never a `bg` agent with open tasks.

   All of these are unit-tested in the planner.
4. **Archive vs kill are distinct actions.**
   - *Archive* (UI-only) sets `work_links.archived_at` on the live link. The
     session collapses into its group's Done section and tmux keeps running.
     A new prompt or attach un-archives it automatically.
   - *Safe kill* uses the existing safe-kill path. Claude commits and pushes,
     and the worktree is removed only if that succeeds. Then the M1b
     trigger ends the link with its snapshot.
   - *Snooze 7 d* hides the candidate. *Never for this work* writes a
     per-link flag.
5. **Reopen is an event, not a guess.** A tracker transition out of `done`,
   per `status_changed_at` in M3's sync (or a local item toggled back),
   records `reopened` in the journal. The Attention *reopened* bucket stays
   until the user resumes or dismisses it.

## Tasks

Each task is one reviewable PR in the **Worker** environment, with the usual
checks.

### M7.1: planner (backend, pure)

- In `service/gc.rs`, add `plan_tidy(input) -> Vec<TidyCandidate>` next to
  the existing `plan()`. It is pure, and its input is:
  - session rows (with `work`, `idle_since`, `claude_status`, `stuck_kind`,
    and the last user-touch time);
  - the linked items' status, resolution and `status_changed_at`;
  - the PR merged state;
  - the settings, and the snooze and never flags.
- `TidyCandidate { session_id | link_id, reason, secondary: Vec<reason>, action: Archive|SafeKill|Kill|ResumeOrExpire, since }`.
- The existing idle killer is untouched. When `work.auto_tidy` is on,
  `maybe_sweep` executes only the `SafeKill` and `Archive` candidates of
  **allowed reasons** (default: `done_idle`, `pr_merged_idle`), through
  the same executor. It writes `gc_tidied` timeline events and the journal.
- **Tests:** a table with one row per reason, one row per protection, the
  secondary-reason merge, and auto-tidy executing only allowed reasons.
  Idempotence: nothing is suggested twice after a snooze.

### M7.2: storage and API (backend)

- **Migration:** in `work_links`, add `archived_at`, `tidy_snoozed_until` and
  `tidy_never` (a guarded ALTER).
- A trigger or service hook clears `archived_at` on the next
  UserPromptSubmit, attach or prompt_sent for that session.
- **`work` read:** `tidy { org? }` returns the candidates with reasons and a
  preview (host, branch, dirty or unpushed state if the last probe knows,
  PR, item status).
- **`work_link` actions** (no new tool, keeping within budget):
  - `archive`, `unarchive`;
  - `tidy_apply { items: [{target, action}] }`, a batch that runs the
    existing safe-kill and kill paths and reports per item;
  - `snooze { days }`, `never`.
  - Routed verdicts.
- **Reopen:** M3's sync (and local item edits) write a journal row
  `status_change` with `reopened: true`. `work { action: reopened }` lists
  the work items that are open again and have past links.
- **Settings** in the typed registry:
  - `work.tidy_done_days` (2)
  - `work.tidy_idle_hours` (4)
  - `work.auto_tidy` (false)
  - `work.auto_tidy_reasons` (a Choice set)

  Per-org overrides arrive with M5 (`orgs.auto_tidy`).
- **Tests:**
  - `tidy_apply` against FakeSsh: a dirty worktree takes the safe path; a
    clean one is killed; a failure is reported per item and does not abort
    the batch;
  - un-archive on a prompt;
  - the reopened listing.

### M7.3: UI (frontend)

- **Attention:**
  - "Tidy up · n" appears only when n > 0. It does not count toward
    needs-you, and has a neutral colour.
  - "Reopened · n" gets the accent colour and opens that work group,
    expanded.
- **Tidy-up sheet** (a modal list):
  - Grouped by reason. Each row shows the session label, host, branch,
    item status, PR, idle time, and a dirty/unpushed warning when known.
  - Preselected checkboxes. A per-row action dropdown offers *Safe kill*
    (the default for kill reasons), *Archive only*, *Snooze 7 d* and
    *Never for this work*.
  - The footer reads "Tidy 6 · Cancel". Keyboard: j/k to move, space to
    toggle, ↵ to apply.
  - It reuses Sidebar's `selectedIds` and bulk patterns where it fits.
- **Group-by-Work:**
  - Archived live sessions move to the group's collapsed *Done* with an
    "archived" chip. One click un-archives.
  - A group whose item is `done` shows a muted header.
  - A group whose item was **reopened** shows the badge "reopened · 2 past
    sessions" and a **Resume ▾** split button (from M2.5).
- **Settings → Work → Lifecycle:** the thresholds, the auto-tidy toggle and
  allowed reasons (with a plain-language warning), and "Show what auto-tidy
  would do" (a dry run of the current candidates).
- **Tests:**
  - Attention counts and colours;
  - sheet preselection, actions and keyboard;
  - un-archive on click;
  - the reopened badge and Resume entry;
  - the dry-run preview.

### M7.4: docs and roadmap

- `docs/concepts.md` gets a *Lifecycle* section with the six layers and what
  fleet may and may not do.
- `docs/hub.md` covers the settings and auto-tidy.
- Update the roadmap status and Revisions.

## Acceptance (manual)

1. **Done → tidy.**
   1. Mark a linked ticket Done.
   2. After `tidy_done_days`, with the session idle `tidy_idle_hours`,
      "Tidy up · 1" appears.
   3. Applying it asks Claude to commit and push (safe kill), then ends the
      link with its snapshot.
   4. The session appears under Done, and Resume works (M2).
2. **Protections.** None of these ever appears as a candidate:
   - a session on an in-progress ticket;
   - one that is working or blocked;
   - one the user prompted 10 minutes ago;
   - the controller.
3. **Archive only.** Archiving a live session collapses it without killing
   tmux. The next prompt to it brings it back.
4. **Duplicate worktree.** Two sessions on one worktree, one idle for a day,
   suggest killing the idle one and keep the other.
5. **Reopen.** Moving the Done ticket back to In Progress in Jira raises
   "Reopened · 1". The group shows past sessions and Resume.
6. **Auto-tidy.** Enabled for `done_idle` only, the next sweep safe-kills the
   candidate. A `pr_merged_idle` candidate is still only suggested.
7. **Safety check.** After every scenario, no branch with unpushed commits
   has been deleted, no transcript is missing, and no journal row is lost.

## Risks

| Risk | Mitigation |
|---|---|
| Killing work the user still wanted | Suggest-only by default; protections are hard-coded and tested; safe kill commits and pushes first; the snapshot makes Resume possible |
| Noise ("Tidy up" always shows something) | Thresholds are conservative; snooze and never; the entry hides at 0; it does not count as needs-you |
| Status drift (tracker says done, work continues) | `in_progress` protection covers only the tracker; the "touched in the last hour" and "working" protections cover the reality |
| Auto-tidy surprises | Off by default; per-org opt-in; restricted reasons; dry-run preview; every action is written to the timeline and journal |

## Decisions (defaults if unanswered)

| # | Question | Default |
|---|---|---|
| D2 | Can "done" ever kill a live session automatically? | Only with `work.auto_tidy` on (per org), for `done_idle` and `pr_merged_idle`, via safe kill; off by default |
| new | What does archiving a live session mean? | UI-only (collapse); tmux keeps running; a prompt or attach un-archives |
| new | Default thresholds | done ≥ 2 days + idle ≥ 4 hours |

## Revisions

- **2026-09-24, M7 landed** on `claude/cloud-fleet-work-graph-m7` (stacked
  on M4, built in parallel with M5 and M6): M7.1 (the planner), M7.2
  (storage and API), M7.3 (UI), M7.4 (docs). Verified with `cargo fmt`,
  `clippy -D warnings` (workspace), `cargo test` (fleet-core, claude-fleet,
  fleet-hub; only the four chmod tests that fail as root fail), `pnpm check`
  / `pnpm test` and `scripts/hub-e2e.sh` (102/102). Deviations, and why:
  1. **Where the code lives.** The planner is `service/gc/tidy.rs` (a child
     of `gc`, reusing its idle reference) with its table in
     `gc/tidy/tests.rs`; the storage `store/work_tidy.rs`; the one entry
     both transports and the sweep share `service/work/tidy.rs`.
  2. **Auto-tidy execution moved to M7.2.** It needs the snooze / never
     flags and the gathered input, which are storage. M7.1 is the pure
     planner plus `auto_selection` (safe kill / archive of an allowed
     reason, only with auto-tidy on). The sweep runs it after the idle
     killer, skipping what the killer acted on; off, it reads one setting
     and returns (regression test: the killer's behaviour and report are
     unchanged). `GcReport` gained `tidied` (`serde(default)`).
  3. **`idle_unlinked` is not a reason.** Archive is a flag on a work link
     (§0.6); an unlinked session has none to carry it, and a session-level
     archive would be the flag design §H argues against. The idle killer
     (`gc.enabled`) still covers idle unlinked sessions.
  4. **`ghost_expiring` is for linked, resumable ghosts only** (a snooze
     needs a link to live on; the host's restore list covers the rest), and
     never kills. The sheet offers Resume, Snooze or Never for it.
  5. **Kills.** `kill` and `safe_kill` both inspect a work session's own
     worktree (dirty, unpushed or uninspectable ⇒ the safe-kill path), so a
     person's plain "Kill" never bypasses it. A worktree another live work
     session shares is only plain-killed — a safe-remove would delete a tree
     in use; a plain tmux kill leaves every file — and the planner
     downgrades such a candidate's action to `kill`. `bg` / `shell` /
     `review` rows are plain-killed, as the idle killer does; a review is
     never a duplicate of its source. A work row with no tracked worktree
     is offered Archive only.
  6. **Protections, as built:** in-progress (any live confirmed link, not
     only the primary), `working` / `blocked` / `failed` (a needs-you
     bucket), any `stuck_kind` (incl. `trust_prompt`), a pending dialog,
     the controller, the operator, a touch within the hour, a `bg` agent
     that is the worker or requester of a queued / running task; also a
     safe kill in flight and `external` rows. Each has a planner test, and
     `tidy_apply` re-checks them at apply time for every destructive item.
  7. **The touch is `sessions.last_touch_at`** (migration 050, outside
     reconcile's `ON CONFLICT`), stamped by the UserPromptSubmit hook, a
     prompt sent through fleet, and an attach. `pty_open` reads no
     `state.db` (it is `SameInBoth`), so the desktop sends the attach as
     `work_link { action: unarchive }` (Tauri `unarchive_session_work`),
     which stamps the touch and un-archives; an automatic reconnect is not a
     touch. Any prompt counts, fleet-dispatched ones too: over-protection
     is the safe side.
  8. **Reopen.** `work_items.reopened_at` plus a `reopened` journal kind
     (instead of `status_change` with `reopened: true`; M3's `status_change`
     rows are still written): on the live sessions' conversations, else on
     the newest past session's last conversation. Only a transition from
     `done` to `todo` / `in_progress` counts, once; done again clears it.
     `work { reopened }` lists open-again items with past sessions until a
     session links to the item after the reopen (resumed), it is done again,
     or `work_link { action: dismiss, item_id }` (a new action) clears it.
     Local items have no status edit yet, so only tracker transitions
     reopen.
  9. **Merged PRs**: `state` joined the probe's full field list and
     `PrSignals.state`; a host whose `gh` answers only the basic fields
     gives no merged signal (no guess).
  10. **`tidy_apply` is tested against the injectable `GcExec`** (the idle
      killer's executor) rather than `FakeSsh`: `kill_session` and
      `safe_kill_session` take a concrete `SshClient`. The batch test covers
      a dirty tree (safe path), a clean one (killed), a failing kill
      (reported, the batch goes on), a protected session (refused, never
      touched), a snooze and an unknown session.
  11. **Confirmation.** `work_link` is now `confirm: true` in the policy
      table (its annotation says `destructiveHint`); the gate runs only for
      a `tidy_apply` batch that contains a kill, as `work_admin` gates only
      `remove`. `confirm_nonce` is the new parameter for it.
  12. **API and budget.** `work { tidy | reopened }`, `work_link { archive |
      unarchive | snooze | never | dismiss | tidy_apply }`; three new
      parameters (`days`, `items`, `confirm_nonce`); the action field docs
      now point at the description. `BUDGET_BYTES` 68,485 → 69,124
      (measured 69,024, +639). Wire enums have `Unknown`; new fields are
      `serde(default)`; no contract bump. Eight Tauri commands, all
      `Routed` (159 in the verdict table). A per-host token sees and
      applies only its own host's candidates, reads reopened work only when
      its newest past session ran there, and cannot dismiss.
  13. **Settings.** `work.auto_tidy_reasons` needed a new `Kind::ChoiceSet`
      (a comma-separated subset, stored in the set's order); it may name
      only `done_idle`, `pr_merged_idle` and `not_planned` — a duplicate is
      only ever plain-killed and a ghost never killed, so neither can be
      automatic. `work.tidy_done_days` is 1–365, `work.tidy_idle_hours`
      1–720.
  14. **UI.** The two entries are pills in the attention strip under the
      sidebar filters (as M4's link review; `Attention.svelte` is a watcher
      with no entries). The sheet is inline, not a modal. "Reopened · n"
      opens a list with the badge, Resume and Dismiss rather than scrolling
      to the group; the group header carries the same badge and Resume. An
      archived session that needs you stays in the live list. The sheet
      does not show a dirty / unpushed warning per row: nothing probes that
      ahead of the inspection; each safe-kill row says "commits & pushes
      first" instead. `selectedIds` / the bulk bar were not reused: the
      sheet's rows are candidates, not sidebar rows.
  15. **Not done:** the phone (M8) and the manual acceptance on a real
      fleet. (The per-org override landed after M5, below.)
- **2026-09-24, M5 merged into M7** (a merge, no rebase; the user's call:
  "add the per-org override once M5 lands"). Changes:
  1. **Migrations renumbered.** M5 took 050 (`050_orgs.sql`), so M7's is
     `051_work_lifecycle.sql` (same content, same guard), and the org column
     is its own `052_org_auto_tidy.sql` with its own guard: a database
     that already has 051's columns but an `orgs` table made later (M5's
     own "existing rows" test rebuilds it) still gets the column.
  2. **Per-org override.** `orgs.auto_tidy` (NULL inherit, 0 off, 1 on),
     set by `work_admin { add_org | update_org, auto_tidy: on | off |
     inherit }` (master only), `fleet-hub org set <id> --auto-tidy …`, and
     the Organisations settings' select (read-only when paired). The
     planner decides per session (`TidyConfig::auto_for(row.org_id)`); the
     sweep runs when auto-tidy is on globally or for any org. Reasons and
     thresholds stay fleet-wide. `BUDGET_BYTES` 69,859 (measured 69,759:
     M5's 69,099 plus M7's 639 plus the `auto_tidy` parameter).
  3. **Org scope.** `work { tidy }`, `work_link { tidy_apply }` and
     `work { reopened }` run under M5's `OrgScope`: a per-host token sees
     and applies only its own host's candidates of an org it sees (a
     session outside reads as `session N not found`, per item); a
     candidate of its own session linked to another org's ticket loses its
     key, item status and link id; snooze / never cannot reach another
     org's link (through `tidy_apply` or `work_link`); reopened work needs
     its newest past session on the token's host and its item in the
     token's org. The eight actions have isolation-matrix rows (M5's rule),
     and `WORK_ACTIONS` / `WORK_LINK_ACTIONS` / `ROUTED_WORK_COMMANDS` list
     them. `SessionRow.work` carries both `org_id` and `archived_at`.
  4. **UI.** The Tidy-up pill and sheet follow the sidebar's scope
     selector (a candidate shows when its session is in the chosen scope);
     169 commands in the verdict table.
  Verified again: `cargo fmt`, `clippy -D warnings` (workspace), `cargo
  test` (fleet-core, claude-fleet, fleet-hub; only the four root chmod
  tests fail), `pnpm check` / `pnpm test` (2779), `scripts/hub-e2e.sh`
  (102/102).
- **2026-09-25, M6 (main, #266) merged into M7** (a merge, no rebase). M6's
  `051_tracker_providers.sql` stays 051; M7's migrations are renumbered to
  `052_work_lifecycle.sql` and `053_org_auto_tidy.sql` (same content and
  guards, their `schema_version` rows 52 and 53). A store at 51 with M6's
  schema migrates up to 53
  (`migrations_052_053_add_lifecycle_columns_after_051_and_rerun_safely`).
  `work_admin`'s audit line carries both M6's `transport` / `settings` and
  M7's `auto_tidy`; the tool budget was re-measured.
