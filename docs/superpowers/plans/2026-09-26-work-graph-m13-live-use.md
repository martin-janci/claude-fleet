# Work graph M13: live use (plan)

**Date:** 2026-09-26
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`. M13 is new; M13.0 adds it there.
**Depends on:** M0–M12 on `main` (v0.3.0 and later), and the acceptance script `docs/work-graph-acceptance.md` (M10.3, #313).
**Inputs:**
- `../reviews/2026-09-26-work-graph-decisions-revisited.md` (M12.6);
- #318, the first fix that came from real use.

## Goal

> M0–M12 built the work graph, proved it in CI and wrote it down. M13 is
> the move from building to **operating**:
> - the owner's acceptance run becomes fixes and decisions;
> - the failures real use has started to show become visible in health;
> - the decided-against list gets the usage evidence M12.6 could not cite;
> - the roadmap says what is true.
>
> Anything a decision turns into a "yes" is built here, one small task
> each. Anything that stays "no" stays unbuilt.

**Value line:**
- The roadmap and CLAUDE.md match `main`.
- A tracker that silently skips items reads as `degraded`, not `ok`.
- `work_admin { usage }` shows how the work graph is actually used, without anything leaving the machine.
- Every "Found issues" row of the owner's run is fixed, decided, or ticketed.
- Every open decision has an answer backed by evidence.

## Facts this plan builds on (verified 2026-09-26 at `main` 74b7cab)

**The roadmap is out of date:**
- It has no **M11** section. M12's section only points at M11's plan.
- The per-milestone **"Not done" lines** of M1, M2, M5, M6 and M7 still list things that have landed: "Name this work…", the transcript probe, GHES, the per-tracker metrics, `idle_unlinked`, the phone, Today. M11.6 was meant to rewrite them and never did.
- **M10's status** still says "M10.1–M10.3, M10.5 and M10.6 are not started". All of them have landed.
- **The decisions table**:
  - It stops at D16. D7 is only in the M5 plan; D17–D20 are only in the M11 plan; D21–D23 are only in the M12 plan. D8 was never used.
  - D16 still says the work-graph e2e leg "is skipped" in CI. Since #314 it runs on every PR (`WBIN`, 188 checks), and a missing `WBIN` fails when `CI=true`.
- **CLAUDE.md**'s status paragraph ends at M9.

**The phone:**
- fleet-mobile's default branch has *Ask for a handover* (`WorkActions.kt`, `SessionWorkViewModel.kt`, `WorkSheet.kt`).
- This answers the M12.6 question "is M8.6.3 on fleet-mobile `main`?": it is.

**Tracker sync after #318:**
- One failing item no longer rolls back its batch. The item is skipped and retried next pass; its view's watermark does not move.
- The count lives only in `TrackerPass.failed` and one log line per pass. It is not in M11.4's `SyncMetrics` (`service/trackers/sync.rs`).
- So M12.4's roll-up (`service/health.rs`, `tracker_health_level`) reads **ok** for a tracker that skips the same item on every pass, and the item never reaches the cache.

**Evidence for decisions:**
- The M12.6 review could cite no usage, because none is recorded in a form anyone can read.
- The data already exists in the store:
  - link states and their `source` (`work_links`);
  - handover and resume events (`session_events`, the `handover_*` kinds);
  - brief deliveries and `compact_summary` rows (`work_journal`);
  - tidy outcomes (`gc_tidied`, `tidy_kept`);
  - confirm / reject of suggestions.
- Nothing aggregates it.

**Numbers:**
- Migrations run through **060** (059 and 060 came with the hub-latency work in v0.3.0). The next is 061.
- `BUDGET_BYTES` is whatever `main` measures; every task here re-measures.

**Open decisions:** D3, D5, D10, D15 and D20 all say "decide after M10.3". D13 and D17 stay decided against (M12.6); M13 does not reopen them.

## Design decisions

1. **Real use leads.**
   - The acceptance run and live bug reports come before new features.
   - A decision-gated task starts only after the user answers "yes" in the decisions table. It does not start from a default.
2. **Visible before clever.**
   - A partial failure is reported before anything tries to heal it.
   - The health roll-up stays cached and never calls a tracker (M12 design decision 3).
3. **Usage is counted from what is already stored, and never sent anywhere.**
   - No new table, no telemetry, no network.
   - `work_admin { usage }` aggregates existing rows over a window.
   - It is master-only, and it holds counts only: never titles, keys or paths.
4. **One decision, one small PR.**
   - Each "yes" is built as the *smallest safe version* the M12.6 review already describes. That review is the spec; this plan does not restate it.
   - Each such PR carries its isolation rows, its budget measurement, and its guide and settings-table update.

## Tasks

### M13.0: this plan and the roadmap truth pass (docs)

- **Commit this file.**
- **Roadmap:**
  - add an **M11** section (between M10 and M12) and an **M13** section;
  - rewrite every stale "Not done" line to *done (Mn.x)*, *decided against (Dn)* or *waits on the user (Dn)*;
  - correct **M10's status**;
  - complete the **decisions table**: add D7 and D17–D23 with their current answers, note that D8 is unused, and correct D16 (the leg runs in CI since #314).
- **Record** in the roadmap's D15 row and in the M12.6 review's *Revisions* that fleet-mobile ships handover (M8.6.3).
- **CLAUDE.md:** bring the status paragraph up to M12, one sentence per milestone, in the existing style. Name `docs/work-graph.md` and `docs/work-graph-acceptance.md`.
- Docs only. `work_settings_are_in_the_user_guide` must still pass.

### M13.1: Partial sync failures are visible (from #318)

- **Metrics.**
  - `SyncMetrics` gains, with `#[serde(default)]`:
    - `items_failed`: the last pass;
    - `consecutive_partial`: passes in a row with `items_failed > 0`.
  - The sync fills both from `TrackerPass.failed`.
  - `work_admin { status }`, Settings → Work and `fleet-hub tracker status` show them.
- **Health.**
  - `tracker_health_level` reads **degraded** when the last pass had `items_failed > 0`.
  - It reads **failing** after `TRACKER_FAILING_AFTER` consecutive partial passes, because the same item is stuck. The Reconnect Attention item (D22) must not fire for that case: a partial failure is not a credential problem. It gets its own wording, "Sync skipping items".
  - `last_error` carries the defused reason of the last failed item.
- **Guide and docs.** `docs/work-graph.md` → *Trackers in fleet health* and `troubleshooting.md` gain a "sync skips items" entry.
- **Tests.**
  - The roll-up with a partial pass, then a clean pass (back to `ok`).
  - N partial passes in a row give `failing`.
  - The Attention wording.
  - Isolation: a per-host token still sees only its org's trackers.
- No migration, no new tool, no contract bump.

### M13.2: Usage summary (evidence for the decisions, D24)

- **Action.** `work_admin { action: usage, days? }` (default 30, 1..=365), master-only, read-only. Over the window it returns counts only:
  - **links:** started / confirmed / suggested → confirmed, rejected, expired; `source` breakdown;
  - **detection:** suggestions shown, confirmed and rejected; the median time to a decision;
  - **handover:** requested, written, refused (`busy`), failed;
  - **resume:** by mode (`last` / fresh with brief); probe outcomes (present / absent / unknown);
  - **briefs delivered;** `compact_summary` rows harvested;
  - **tidy:** suggested per reason, applied, kept, auto-tidied;
  - **multi-start:** runs and repos per run;
  - **trackers:** passes, failures, items failed (from M13.1), per tracker **id**, not name.
- **Shape.** No titles, keys, paths or free text; ids and counts only. One SQL block per group, over existing tables, sharing M12.2's scale fixture for a budget test.
- **Surfaces.**
  - Desktop: Settings → Work → *Usage* (a read-only table, Copy as text).
  - `fleet-hub work usage`.
- **Isolation.** A master-only row in the isolation matrix. Per-host and client tokens are refused.
- **Budget.** Re-measure `BUDGET_BYTES` (one action) and regenerate the reference. The Tauri command gets a verdict row (Routed).
- **Acceptance link.** `docs/work-graph-acceptance.md` gains a step: "paste `work_admin { usage }` into the run record". That gives the next decision revisit real numbers.

### M13.3: Acceptance triage (waits on the owner's run)

**Start:** when the filled-in `docs/work-graph-acceptance.md` is committed.

For every **Found issues** row:
- **blocker:** fix at once, one PR per issue, with a regression test (unit test or an e2e check in `hub-e2e.sh`, which runs in CI);
- **major:** fix in M13, or ticket it with a reason;
- **minor / docs:** batch them into one PR;
- **not a bug:** say why in the row.

Then:
- The decisions table is updated from the run's *Evidence for the decisions* section and the `usage` output (M13.2). Each changed row cites the run.
- The M12.6 review gains a *Revisions* line: "M10.3 ran on <date>", with what changed.
- M13.3 is **done** when every row is closed and the table carries the run's answers.

### M13.4: Decision-gated builds (each only on a "yes")

Each item starts only when the user writes "yes" in the decisions table. Each one builds the M12.6 review's **smallest safe version** for that decision, in the listed order (cheapest first):

| # | Decision | Where | Scope |
|---|---|---|---|
| M13.4a | D20: name local work on the phone | fleet-mobile only | "Name this work…" and rename in the phone's work sheet, full token only, gated on the hub listing `name`. No hub change. |
| M13.4b | D5: SessionStart context on by default | claude-fleet | Only if the remote p95 from `scripts/measure-session-start.sh` is under ~300 ms. Flip the default of `work.session_start_context`, update the guide's settings table, and record the numbers in the M4 plan. |
| M13.4c | D10: summarise a dead session, on demand | claude-fleet (+ phone via D15) | `work_link { action: summarize }`, run by `claude -p --resume --fork-session` with **no tools allowed** (a test proves it), fleet's hooks off, `logging::redact` plus the untrusted fence, and the M11.2 transcript probe first. A new `work.summary_model` setting. The operator is confirm-gated; a hub refuses it. |
| M13.4d | D15: multi-start on the phone | fleet-mobile only | A project multi-select with a confirm sheet (count and org label). Cross-org is refused in words; the app never silently retries with `force_cross_org`. |
| M13.4e | D3: PR remote link to Jira | claude-fleet | Only the idempotent remote link (`globalId`), only for `manual` / `started` links, only to the link's own org's tracker. An outbox table (**migration 061**, with a retention rule under M12.3), a per-tracker opt-in, and every write refused for per-host tokens. |

The review's scope, isolation, M9.7, budget, contract and phone notes for each decision are binding. Anything wider needs a new decision.

### M13.5: Close-out

- **Roadmap.** Mark the work graph *operating*: new work arrives as issues or small plans against `docs/work-graph.md`, not as milestones (D26).
- **CHANGELOG.** The next release's section names what M13 changed for users. Cutting the release stays the owner's step (`scripts/release.sh`).

## Acceptance (manual)

- The owner's M10.3 run is committed, filled in, including the `usage` output.
- Force one tracker item to fail on every pass (the #318 test fixture, or a malformed item in a test tracker). Health must read `degraded`, then `failing`, with "Sync skipping items" and not "Reconnect".
- For every decision the user answered "yes": its M13.4 item is on `main`, and `docs/work-graph.md` describes it.

## Risks

- **Usage counts could reveal work to the wrong person.** Mitigations:
  - the action is master-only;
  - it returns counts and ids only;
  - an isolation row pins it;
  - no key, title or path is ever included.
- **"Degraded" noise.** A tracker with one permanently bad item would read degraded forever. That is intended: the item never syncs. The guide says how to find it (the log line names the view, and M13.1's `last_error` names the reason).
- **Decision creep.** A "yes" could grow beyond the M12.6 smallest version. Design decision 4 and the table above bound it; anything more is a new decision.
- **The acceptance run may not happen soon.** M13.0–M13.2 do not depend on it; M13.3 and M13.4 wait. The plan does not assume a date.

## Decisions (defaults if unanswered)

| # | Question | Options | Default |
|---|---|---|---|
| D24 | Build the usage summary (`work_admin { usage }`)? | yes (master-only, counts only, local) · no | yes |
| D25 | May a partial sync failure alone make a tracker `failing` (after N passes)? | yes · degraded only | yes, with its own Attention wording (not "Reconnect") |
| D26 | After M13, is the work graph "operating" (issues, not milestones)? | yes · keep milestones | yes |
| D3, D5, D10, D15, D20 | See the roadmap's table and the M12.6 review | per decision | unchanged until the M10.3 run; each "yes" is an M13.4 item |

## Order and parallelism

```
M13.0 (docs) ──────────────┐
M13.1 (partial failures) ──┤ independent; each re-measures BUDGET_BYTES if it touches tool text
M13.2 (usage, D24) ────────┘ (M13.1 before M13.2 if both touch work_admin; they merge serially)
M13.3   after the owner's M10.3 run
M13.4a–e  each after its decision's "yes"; a/d are fleet-mobile, b/c/e claude-fleet
M13.5   last
```

## Revisions

- 2026-09-26: first version.
