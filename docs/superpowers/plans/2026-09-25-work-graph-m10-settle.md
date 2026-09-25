# Work graph M10: settle, prove, and reach the phone (plan)

**Date:** 2026-09-25
**Roadmap:** `../2026-09-24-work-graph-roadmap.md` (M10 is new; add it there when this plan lands)
**Design:** `../specs/2026-09-24-work-graph-design.md` §0
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md`, plus the M9.3 / M9.6 / M9.7 review of 2026-09-25 (its blocking findings are fixed on `claude/cloud-fleet-work-graph-m9-fixes`, not here)
**Depends on (landed on `main`):** M0–M9 (#260, #262–#268), M8.1–M8.5 on fleet-mobile `main` (#32)
**Depends on (not yet merged):** the M9 fixes branch `claude/cloud-fleet-work-graph-m9-fixes`. M10.1 starts only after it merges, because both touch `tickets.rs` and `agent_handover.rs`.

**Repos:**
- `claude-fleet`: M10.1–M10.4 and M10.6.
- `fleet-mobile`: M10.5.

## Goal

> M1–M9 built the work graph in nine fast milestones. Every status block
> still says "the manual acceptance is still to do", the end-to-end script
> barely touches work, the phone stops at M8, and the reviews left
> should-fix items. M10 ships no big new idea. It makes what exists
> **trustworthy and complete**: fix the leftovers, prove it end to end, give
> the phone the M9 moments, and close the small gaps each milestone wrote
> down as "not done".

**Value line:** a new hub with Jira and GitHub connected passes one scripted
end-to-end run (link, detect, start, multi-start, handover, tidy) and one
written manual checklist. The phone shows *Today* and the ticket card.
Nothing in the roadmap says "not done" without a decision behind it.

## Facts this plan builds on (verified 2026-09-25 at `main` 649e486)

- **The end-to-end script.** `scripts/hub-e2e.sh` mentions `work` in 6 lines and exercises no `work` / `work_link` / `work_admin` action end to end. The work graph is tested only through unit and integration tests with `FakeSsh` and fixtures.
- **Manual acceptance.** No acceptance has been run. M3–M9 each list their own checklist in their plans.
- **Should-fix findings not taken by the fixes branch.**
  - M9.6 (`service/trackers/tickets.rs`):
    - The starts run one after another, and start prompts are typed only after the loop. Past the Lifecycle cap, the siblings already started get no prompt.
    - An early `?` inside the loop discards the siblings that already started.
    - A session that spawned but failed to link is reported as `failed`, and a retry then starts a duplicate.
    - The ticket is re-resolved for every project.
    - `project_ids: []` silently becomes a single start.
    - The confirm gate runs before the argument validation.
  - M9.3 (`service/work/agent_handover.rs`):
    - A Stop from an earlier turn can settle the request as `handover_missing`.
    - The nonce and marker lines leak into the `turn_done` detail and the `progress` journal row.
    - The UI never shows written / missing.
  - Isolation matrix (`mcp/tools/tests_isolation.rs`): the `handover` and multi-start rows assert only "not refused". Neither asserts a per-caller outcome.
  - `store/timeline.rs`: a doc comment is misplaced and now attaches to `newest_session_event_of`.
- **Gaps each milestone recorded as "not done".**
  - M4.6, the opt-in classification nudge (plan: `plans/…-m4-detection.md` §M4.6).
  - The SessionStart measurement on a remote host. The local numbers are 8–13 ms up and ~2,012 ms at the cap (M4 plan). D5 is still the user's.
  - M5:
    - Filters for tracker / status category / assignee / has-session / archived exist in the predicate (`rowMatches`) but have no controls.
    - The phone does not show orgs.
  - M9.1: Today's **Stale** section jumps to the session. It does not open M7's Tidy-up sheet (`TodayView.svelte:179`), although M7 is now on `main`.
- **The phone (fleet-mobile `main` 9b9212e).**
  - It discovers features with `tools/list`: `HubCapabilities` keeps each tool's `action` enum (`net/HubCapabilities.kt`).
  - Work UI: `WorkChip`, `WorkSheet`, `TicketsSheet`, `SessionWorkViewModel`, `TicketsViewModel`.
  - It has none of M9: no `today`, no `card`, no `handover`, no `project_ids`.
  - `MAX_HUB_CONTRACT = 4`. The hub already serves `today` and `card` in the `work` action enum, so the phone can gate on them **without a contract bump**.
- **Replay ring.** `events.rs` `REPLAY_RING = 512`, shared by every event kind (roadmap *Risks*). No measurement has been made with trackers syncing.

## Design decisions

1. **No new MCP tool, no contract bump, no new migration unless M10.6 needs one.** New behaviour is a `work` / `work_link` action or parameter. Every new action gets an isolation-matrix row.
2. **The e2e runs against fakes, not real trackers.** `hub-e2e.sh` gets a local fake tracker (a tiny HTTP server serving Jira-Cloud-shaped JSON on loopback). This is allowed only through a test-only `extra_ca` / loopback override that release builds refuse. Real Jira stays in the manual checklist.
3. **The phone reads; it does not become a second desktop.**
   - The phone gets *Today* (read-only, plus Copy standup to the share sheet).
   - It gets the ticket card, read-only. "Insert into composer" becomes "Copy".
   - Handover and multi-start stay desktop-only unless D15 says otherwise.
4. **Every leftover either gets fixed or gets a decision.** None stays as a silent "not done".

## Tasks

### M10.0: this plan
Commit this file. Add M10 to the roadmap's milestones, critical path and *Revisions*.

### M10.1: review leftovers (claude-fleet) — implement after the M9 fixes merge
- **M9.6 multi-start:**
  - Type each sibling's start prompt right after it starts.
  - Run the spawns with bounded parallelism (`buffer_unordered(3)`), or check the remaining Lifecycle budget before each start and report the rest as `skipped: deadline`.
  - A per-project error goes into `failed` and the loop continues (no `?`).
  - Spawned-but-unlinked is reported as `started` with a `warning`, and the per-project guard also counts live unlinked sessions on the same branch.
  - Resolve the ticket once.
  - `project_ids: []` returns `E_INVALID`.
  - Validate before the confirm gate.
- **M9.3 handover:**
  - Bind the request to the turn it started. Store `turn_seq` / `last_stop_at` with the nonce, and ignore Stops of earlier turns.
  - Strip the marker block from `turn_done` / `progress`.
  - The card shows *written* / *missing* / *failed* from the newest `handover_*` event.
- **Isolation matrix:** assert per caller for `handover` and multi-start:
  - master gets a cross-org refusal per project, and success with `force_cross_org`;
  - host A gets its own org only;
  - host B gets `E_FORBIDDEN` from the host fence;
  - readonly is refused.
  - Use the fixture ids instead of literals.
- **Nits:**
  - Move the misplaced doc comment in `timeline.rs`.
  - `multiStartNote` failures become a warning toast.
  - A cross-org failure offers *Start anyway* (re-call with `force_cross_org`).
- **Tests first** for each item. `cargo test --workspace`, `pnpm test`.

### M10.2: end-to-end work graph (claude-fleet, `scripts/hub-e2e.sh`)
- A loopback fake tracker (Jira Cloud search + issue + changelog shapes), behind the test-only override from decision 2.
- Scenarios, each a numbered step in the script's existing style:
  1. `work_admin` connect; sync; `work { tickets }`.
  2. `work_link start` on a real tmux host; the branch `{key}-{slug}`; the brief typed; the link confirmed.
  3. Detection: a prompt mentioning a second key yields `work_suggested`; confirm and reject.
  4. Multi-start with two projects; duplicate guard; one failure itemised.
  5. `work_link handover` with a scripted fake Claude that answers with the markers (the Stop hook path); the note appears in the next brief, fenced.
  6. A ticket moves to done; `work { tidy }` lists it; `tidy_apply` with a nonce safe-kills the clean session and refuses the dirty one.
  7. A per-host token on host B sees none of it (the org boundary over the wire, including `/events`).
- `ci-local.sh --hub-e2e` runs it. CI stays opt-in, as today.

### M10.3: the manual acceptance, written once (docs)
- `docs/work-graph-acceptance.md` gathers the M3–M9 checklists into one ordered run:
  1. no tracker;
  2. Jira Cloud;
  3. GitHub via `gh`;
  4. two orgs on one hub;
  5. phone;
  6. operator.
- Each step says what to click and what must be true.
- Record the result of the user's run in the roadmap's *Revisions*, including date, hub version and failures. **The user runs it; agents do not claim it.**

### M10.4: close the recorded gaps (claude-fleet)
- **Today → Tidy-up:** the Stale section's action opens M7's Tidy-up sheet, pre-filtered to those links. Test in `TodayView.test.ts`.
- **Filter chrome (M5 "not done"):** tracker / status category / assignee / has-session / archived as chips in the sidebar's filter popover, wired to the existing `rowMatches`. No backend change.
- **Remote SessionStart measurement (M4 "not done"):** run the M4.5 measurement from a remote host through the hub and write the numbers into the M4 plan. Then put D5 to the user again with those numbers.
- **M4.6 classification nudge:** build only if D14 says yes. Otherwise mark it "decided against" in the roadmap.

### M10.5: the phone gets the M9 moments (fleet-mobile)
- Gate on `HubCapabilities`: `work` action enum contains `today` / `card`.
- **Today screen:**
  - in progress, waiting on you, shipped today, stale;
  - tap to open the session;
  - *Share standup* shows the system share sheet with the same plain text the desktop copies. Use one formatter; port `today.ts`'s standup text with a shared fixture test.
- **Ticket card:**
  - in `SessionScreen` under the work chip: acceptance criteria and the link;
  - *Copy*, never *Send*;
  - the text is shown as untrusted: plain text, no links auto-opened.
- **Orgs:** the org colour bar on rows and the scope filter, if the hub's rows carry `org_id` (they do since M5).
- Readonly tokens see all of this (reads only).
- No contract bump.
- Tests: view-model tests with recorded hub JSON; `CiWorkflowTest` unchanged.
- Docs: `docs/` in fleet-mobile.

### M10.6: replay-ring pressure (claude-fleet) — measure first
- Measure the frames per minute by kind on a hub syncing two trackers (a test with the fake tracker and a synthetic 200-item board), and how long 512 slots last.
- If sessions fall out of the ring in under 5 minutes during sync:
  - coalesce `work` frames per item within one sync pass;
  - otherwise leave it and record the numbers.
- No per-kind ring unless the numbers demand it. That would be a design change: put it to the user.

## Acceptance (manual)
This is M10.3's document: run once by the user on a real hub with Jira Cloud and GitHub, a second org, a paired phone, and the operator.

## Risks
- **The fake tracker override leaking into release builds.** Gate it behind `cfg(any(test, feature = "e2e"))`, and add a release-build test that refuses the override.
- **Parallel spawns (M10.1) racing the per-host SSH ControlMaster.** Keep the parallelism low (3) and reuse the existing per-host semaphore if one applies.
- **The phone formatter drifting from the desktop's.** Use one shared fixture file of Today inputs and expected standup text, tested on both sides.
- **The tool budget:** M10.1 adds no parameters. If M10.4 adds none either, `BUDGET_BYTES` stays at 70,865 (the fixes branch may move it).

## Decisions (defaults if unanswered)

| # | Question | Options | Default |
|---|---|---|---|
| D5 | (re-asked with remote numbers) SessionStart work context on by default? | on · off | off until the remote numbers are under ~300 ms p95 |
| D14 | Build M4.6, the opt-in classification nudge? | build (off by default) · decided against | built, off by default (#273) |
| D15 | Handover and multi-start on the phone? | read-only M9 only · also the actions | read-only only (Today + card) |
| D16 | Run `hub-e2e` in GitHub CI (not only locally)? | local opt-in · CI on `main` pushes | local opt-in, as today |

## Order and parallelism
```
M9 fixes ─> M10.1 ─┐
M10.2 (needs M10.1's multi-start semantics for scenario 4) ─> M10.3
M10.4 ─────────────┤   (independent)
M10.5 (fleet-mobile, independent of all claude-fleet tasks)
M10.6 (independent; reuses M10.2's fake tracker)
```

## Revisions
- 2026-09-25: first version.
- 2026-09-25: **M10.4 done** on `claude/cloud-fleet-work-graph-m10`
  (frontend, a script and docs; no new tool, action, Tauri command,
  migration or contract bump; the tool budget is untouched).
  - **Today → Tidy-up.** The Stale section's *Tidy up · n* already existed
    on `main` (the M9 follow-up on M7: it ticked the stale candidates but
    the sheet listed every candidate). A request now **narrows** the sheet
    to the requested sessions, with "From Today's Stale · n of m" and
    *Show all*; the pill's own opening, and a request none of whose
    sessions is still a candidate, show everything. The per-row jump is
    unchanged. `tidy_apply` stays Routed / blocked in hub-client mode as
    before (`hubActionBlocked`).
  - **Filter chrome.** A "⚑ work" pill in the sidebar's triage row opens
    chip rows: tracker (only with two or more trackers), status category,
    *mine*, *hide archived*, and — in work mode only — has-session (*any*,
    *with session*, *past only*). Persisted as `sidebar.work-filters`, like
    the host and scope filters; composed with needs-you; a focused
    suggestion is past them. Everything goes through `rowMatches`
    (`work_filters.ts` only builds rows): a session's tracker is its key's
    owning tracker (`trackerForKey`), its status and archive come from
    `SessionRow.work`, past links are archived and never live and have no
    status (a status filter hides them). **Deviation:** *mine* is not an
    assignee name — `SessionRow.work` carries no assignees — but the hub's
    own `mine` view (`work { tickets, view: mine }`, already Routed; up to
    200 items, assigned to you and not done), read while the chip is on.
  - **Remote SessionStart measurement.** **Deviation:** not measured —
    the build environment has no remote host, and no number is invented.
    `scripts/measure-session-start.sh --ssh <host>` runs the exact
    installed synchronous command on the host in the four cases of the
    M4 table plus "hub stopped, tunnel alive" (`--hub-stopped`), and
    prints Markdown rows (min / median / p95 / max, and curl's exit code,
    with a warning when a case did not behave as named). The M4 plan has
    a placeholder table "to be measured by the user". D5 stays open.
  - **M4.6.** Landed on `main` separately (#273, OFF behind
    `work.classify_nudge`); D14 therefore reads *built, off by default*.
    D14–D16 are in the roadmap's decisions table.
