# Work graph M12: ship and operate (plan)

**Date:** 2026-09-25
**Roadmap:** `../2026-09-24-work-graph-roadmap.md` (M12 is new; add it there when this plan lands)
**Depends on:** M0–M10 on `main`, M11 (the long tail) and its branches landing first. M12.1 and M12.3 read the final migration list, which may include 057 from M11.

## Goal

> M0–M11 built the work graph and closed its gaps. M12 makes it something
> you can **ship, upgrade into, run for months and explain**. That means:
> - an upgrade path proven on real data;
> - bounded growth of the new tables;
> - trackers visible in the fleet's health;
> - a user guide;
> - a deliberate second look at the ideas that were decided against, now
>   with usage behind the decision.

**Value line:**
- A user on v0.2.38 (before the work graph) upgrades to the release, and every migration from 045 onward runs on their real database in seconds.
- `fleet_health` says *"Jira (acme): last sync failed 3× — token expired"*.
- A year of journals and a 5,000-item board don't slow the sidebar.
- `docs/work-graph.md` explains the whole feature in one page.

## Facts this plan builds on (verified 2026-09-25 at `main` d11025c)

- **Migrations.**
  - `crates/fleet-core/migrations/` runs through **056** (`056_classify_nudge.sql`); M11 may add 057.
  - The work graph added 045–053 and 056. 054/055 are peer links, which is not work graph.
  - There is no test that runs the whole chain on a realistic, pre-work-graph database.
- **Retention.**
  - The only retention sweep in the store is for retired participants (`RETIRED_RETENTION_SECS`, 7 days).
  - The work journal, `tracker_items`, ended `work_links`, timeline events (handover, nudge, tidy) and `conversations` grow without bound.
- **Health.**
  - `service/health.rs` (`fleet_health`) rolls up sessions and hosts from cached reconcile state.
  - It does not mention trackers, their sync errors, or detection backlog.
  - M11.4 adds per-tracker in-memory sync metrics, which `fleet_health` can read.
- **Docs.**
  - `docs/concepts.md` mentions work or trackers 4 times.
  - `getting-started.md`, `troubleshooting.md` and `README.md` never do.
  - There is no user guide. The knowledge sits in the roadmap, the plans and `control-api.md`.
- **CI.**
  - `scripts/hub-e2e.sh` already runs in CI in the `hub-headless` job (`.github/workflows/ci.yml:107–147`), with log upload.
  - D16's default ("local opt-in") is therefore stale. M10.2's work-graph scenarios run in CI once they land.
- **Decided against, pending real use:**
  - D3 write-back;
  - D10 dead-session summaries;
  - D13 webhooks;
  - D15 / D20 phone read-only;
  - D17 acli.

## Design decisions

1. **Upgrades are tested on shapes, not on anyone's data.**
   - A generator builds a v0.2.38-shaped database: hundreds of sessions, conversations and a long timeline.
   - The migration chain runs on it in a test, with a time budget.
   - No real user database is ever committed.
2. **Retention never deletes what the UI still points at.**
   - A sweep only removes rows that are all of these: ended or done, older than the retention window, and not referenced by any live link, open item or pinned note.
   - Windows are settings with conservative defaults, and there is an explicit "keep forever" (0).
3. **Health is cached, never live.** `fleet_health` reads the in-memory sync state and the store. It never calls a tracker or a host, the same rule the existing roll-up follows.
4. **Revisiting a decision needs evidence.** M12.6 writes a short note per decided-against item, each saying:
   - what usage showed;
   - what the smallest safe version would be;
   - a recommendation.

   The user decides. Nothing gets built in M12 from that list.

## Tasks

### M12.0: this plan
Commit this file. Add M12 to the roadmap's milestones, critical path and *Revisions*.

### M12.1: Upgrade path, proven
- **Generator.** A test-only generator (`store/testgen.rs` or `tests/`) builds a database at the pre-work-graph schema, 044 plus peer links as they were:
  - 500 sessions;
  - 5,000 timeline events;
  - 2,000 conversations;
  - 20 hosts.
- **Chain test.** It runs every migration from there to the latest in order and asserts:
  - completion under a time budget (for example 5 s on CI);
  - `PRAGMA integrity_check` is `ok`;
  - the M0.3 participant backfill and M5 `org_id` derivation give the expected counts;
  - a second run is a no-op.
- **Downgrade.** An older binary refuses a newer database with a clear message. Verify the existing guard and test it.
- **Docs.** A `docs/RELEASING.md` note: which release first carries the work graph, and what to back up before upgrading.

### M12.2: Scale
- **Seeded benchmark tests (as asserts, not criterion)** for:
  - `list_sessions` with work fields (`session_org_sql!`, `work`, `work_suggested`);
  - `work { today }`;
  - `work { tickets }`;
  - the tidy planner;
  - the resolver.
- **Data sizes:** 2,000 sessions, 20,000 links, 5,000 tracker items, 50,000 journal rows.
- **Budgets:** p95 under fixed thresholds on the CI runner class. Record measured numbers in the plan.
- **Fixes:** any query that fails its budget gets an index (new migration) or a query rewrite, and a test pins it.
- **Frontend:** group-by-work and `rowMatches` over 2,000 rows in Vitest, under a budget.

### M12.3: Retention
- **Sweep.** A retention sweep in the GC tick, behind settings:
  - `work.retention.journal_days` (default 365);
  - `work.retention.tracker_items_days` for done items no link points at (default 180);
  - `work.retention.timeline_work_events_days` (default 180).
- **Scope.** It deletes only what design decision 2 allows, in batches with a row cap per tick. It never holds the Store mutex for long: one batch per lock.
- **Per org.** It respects an org override if M5's pattern fits (an `orgs.retention` JSON field needs a migration; decide in the task and prefer a setting-only first cut).
- **Visibility.** `work_admin { action: status }` shows row counts per table and the last sweep.
- **Tests.**
  - Every protected row survives.
  - The batch cap holds.
  - "Keep forever" (0) deletes nothing.
  - Isolation: the sweep is hub-internal, and no tool triggers it except an admin `work_admin { action: sweep_now }`, which is Master-only and has an isolation row.

### M12.4: Trackers in fleet health
- **Roll-up.** `fleet_health` gains a `trackers` roll-up:
  - per tracker: ok / degraded / failing, consecutive failures, last error (defused, capped) and last success;
  - fleet-wide: the detection backlog, meaning suggestions older than N days awaiting a decision.
- **Readers.**
  - The desktop's health surfaces show it.
  - The phone reads it only if `fleet_health` is already in its tool list and the field is additive (`#[serde(default)]`, no contract bump).
  - A per-host token sees only its org's trackers.
- **Attention.** A failing tracker (for example an expired token) raises one Attention item on the desktop, "Reconnect Jira (acme)", which links to Settings → Work.
- **Tests.** Roll-up, isolation, and a Vitest test for the attention item.

### M12.5: The user guide
- **`docs/work-graph.md`**, one page covering:
  - what "work" is;
  - linking and detection;
  - trackers and the per-provider setup (Jira Cloud/DC, GitHub/GHES, Asana, Linear);
  - start, multi-start and resume;
  - handover;
  - Today and standup;
  - tidy-up and auto-tidy;
  - orgs and isolation;
  - the phone;
  - the operator and confirmations.

  It also lists every setting with its default, with screenshots or placeholders marked for the user.
- **Other docs.**
  - `getting-started.md`: a short "connect a tracker" step.
  - `troubleshooting.md`: sync failures, the replay-ring `lagged` behaviour after a first sync, and "why is this session linked to X?" (the evidence popover).
  - `concepts.md`: a work section.
- **Links.** `control-api.md` stays the tool reference, and the guide links to it rather than repeating it.

### M12.6: Revisit the decided-against list (docs only)
- `docs/superpowers/reviews/<date>-work-graph-decisions-revisited.md`. For D3, D10, D13, D15/D20 and D17 it records:
  - what the acceptance run (M10.3) and use showed;
  - the smallest safe version;
  - the risk;
  - a recommendation.
- Also correct D16 in the roadmap: hub-e2e already runs in CI.
- The user updates the decisions table. Any "yes" becomes its own milestone (M13+), not part of M12.

## Acceptance (manual)
- Upgrade a real older install (the user's own) after a backup.
- Watch `fleet_health` while revoking a tracker token.
- Set retention to 1 day on a test hub and confirm linked work survives.
- Read `docs/work-graph.md` cold and follow it to connect a tracker.

## Risks
- **Retention deleting something still needed.** Mitigations:
  - conservative defaults;
  - the protection rules in design decision 2, tested row by row;
  - a dry-run count shown in `work_admin status` before the first real sweep.
- **Benchmarks flaky on CI runners.** Budgets have generous margins, and a failure prints the measured number. Assert on algorithmic shape (index use via `EXPLAIN QUERY PLAN`) where time is too noisy.
- **Migration test data drifting from real shapes.** Build the generator from the historical migration files themselves (schema at 044), not from today's structs.
- **The guide going stale.** Link it from each plan's *Revisions* template. A docs check fails CI if a `work.*` setting in code is missing from the guide's table (a simple grep test).

## Decisions (defaults if unanswered)

| # | Question | Options | Default |
|---|---|---|---|
| D21 | Retention defaults: journal / done items / work timeline events | 365 / 180 / 180 days · keep forever | 365 / 180 / 180; 0 = forever |
| D22 | Should a failing tracker raise an Attention item (not only a health row)? | yes · health only | yes, one per tracker, deduplicated |
| D23 | Per-org retention override (needs a migration) | now · later | later: settings-only first |
| D16 (fix) | hub-e2e in CI | already on | record as done |

## Order and parallelism
```
M11 lands ─> M12.1 (reads the final migration list)
M12.2 ─────┐
M12.3 ─────┤ independent; M12.3 uses M12.1's generator for its tests
M12.4 ─────┘ (reads M11.4's sync metrics)
M12.5        after M12.3/M12.4 (documents their settings)
M12.6        docs; after the user's M10.3 acceptance run
```

## Revisions
- 2026-09-25: first version.
- 2026-09-25: M12.3 landed on `claude/cloud-fleet-work-graph-m12-retention`. Settings only (D23), no migration. The three `work.retention.*` windows (D21) replace M2's `work.journal_days`. While the new journal key is unset, an old `0` still means forever, and an old window only counts if it is longer than 365 days (it was chosen when confirmed work was kept forever). Protected sets per table are in `store/work_retention.rs`: journal rows of open conversations, live-linked sessions and work not done or still live-linked; undelivered handovers and handovers to a live session; done tickets any link or bare ref names, and ancestors of kept tickets; the newest work event of each kind per session. There is no pinned-note concept, so notes follow the journal rules. M11.4's `work_admin { status }` now answers `{ trackers, retention }`: the sync metrics, plus rows, the dry run and the last sweep; `fleet-hub tracker status` prints both. `sweep_now` is new. Both are master-only. The sweep deletes at most 2,000 rows per table per tick, 200 per lock. The Settings → Limits → Retention UI is standalone-only, and its two commands are `LocalOnly`. The tool surface is +21 B over M11.4 (54,955), inside `BUDGET_BYTES`.
