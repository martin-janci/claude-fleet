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
- 2026-09-25: M12.1 landed. `store::testgen` builds the pre-work-graph database from the historical files 001–044. That is v0.2.37, not v0.2.38: v0.2.38 already shipped 045–055, peer links included, so 054/055 run after the work graph and belong to the chain. `store::schema::tests_upgrade` runs every later migration (045–057, read from `MIGRATIONS`). Measured: ~120 ms in debug and ~44 ms in release, against a 5 s budget. No migration was slow or wrong on the generated data. There was no downgrade guard: M12.1 adds one in `migrate()`. It is a behaviour change: an older build now refuses a newer database instead of running against it. The `docs/RELEASING.md` section *Upgrading into the work graph* covers the upgrade.
- 2026-09-25: **M12.2 (scale) landed**, with its numbers:
  - **Fixture.** `store/scale_fixture.rs` (test-only, seeded splitmix64):
    20 hosts, 3 orgs (4 placement rules, one org isolating), 40 projects,
    2,000 sessions with worktrees and participants, 3 trackers with 5,000
    items, 20,000 work links (≈4,000 live in every state, the rest ended on
    4,000 retired participants), 50,000 journal rows of every kind, 4,000
    conversations, 40,000 timeline events. Built in ≈3.6 s by plain SQL in one
    transaction. It is independent of M12.1's `store::testgen`, which was
    built in parallel.
  - **Budget tests.** `service/work/scale_tests.rs` (plain `cargo test`;
    `-- --nocapture` prints p50/p95). Every call is timed over 11 runs, and its
    statements are traced (`rusqlite`'s `trace` feature, dev-only) and put
    through `EXPLAIN QUERY PLAN`. A call fails on any `SCAN` of `work_links`,
    `work_items`, `work_journal`, `session_events`, `conversations` or
    `participants`, unless the scan walks a partial index. The shape check is
    exact, and the time budgets are 5–10× the measured p95.
  - **Measured.** p50 / p95 in ms, unoptimised test build, the development
    container, after the fixes:

    | Call | p50 | p95 | budget |
    |---|---|---|---|
    | `list_sessions` (All, 2,000 rows with `work` / `work_suggested` / `org_id`) | 163 | 165 | 1,500 |
    | `list_sessions` (per-host token h01, filter + redact) | 163 | 166 | 1,500 |
    | `list_sessions_for_host` (100 rows) | 7.4 | 8.3 | 300 |
    | `work { today }` (All) | 241 | 262 | 2,000 |
    | `work { today }` (All, since 30 days) | 273 | 308 | 2,000 |
    | `work { today }` (per host) | 271 | 295 | 2,000 |
    | `work { tickets }` (page of 50) | 80 | 85 | 1,000 |
    | `work { tickets, limit: 200 }` | 136 | 154 | 1,500 |
    | `work { tickets, view: mine }` | 135 | 140 | 1,000 |
    | `work { tickets, query: <key> }` | 69 | 70 | 1,000 |
    | `work { tickets }` (per host) | 121 | 122 | 1,500 |
    | `work { tidy }` (read + plan, all candidates) | 181 | 214 | 2,000 |
    | `plan_tidy` alone (pure, 2,000 sessions) | 3.7 | 4.7 | 500 |
    | `resolve_session` (busiest session, 40+ live links) | 0.4 | 0.4 | 200 |
    | `on_prompt` (loop guard + recognise + resolve) | 7.7 | 8.8 | 200 |
    | `resolve` (pure, 60 candidates × 60 links) | 0.3 | 0.3 | 50 |
    | `recent_ended_work_links` (7 days / a year, 200) | 1.7 | 1.8 | 100 |

  - **Fixes.** Before / after p95 on the same fixture:
    - `recent_ended_work_links`: `SCAN work_links` plus a temp B-tree sort
      (it also runs inside `work { today }`). Fixed by
      `idx_work_links_ended ON work_links(ended_at) WHERE ended_at IS NOT
      NULL`. p95 8.3 → 1.8 ms (7 days) and 12.4 → 1.7 ms (a year).
    - The prompt trigger's loop guard (`recent_handover_bodies`):
      `SCAN work_journal` (50,000 rows) on every UserPromptSubmit, because
      047's partial index covers only undelivered handovers. Fixed by
      `idx_work_journal_participant_handover ON work_journal(participant_id)
      WHERE kind = 'handover'`. `on_prompt` p95 18.2 → 8.8 ms.
    - `live_work_sessions_for_key`, called once per ticket by
      `work { tickets }`, joined `work_items` to match the key. SQLite then
      walked every live link per ticket: no full scan, but O(tickets × live
      links). Rewritten to `item_id IN (SELECT id FROM work_items WHERE key =
      ?)`, so the OR has an index on both arms (`idx_work_links_ref`,
      `idx_work_links_item`). Same rows. `tickets` p95: page of 50,
      243 → 85 ms; 200, 618 → 154 ms; `mine`, 619 → 140 ms; per host,
      246 → 122 ms.
    - `scale_plans_pin_the_m12_fixes` pins all three by plan shape.
  - **Migration 058** (`058_work_graph_scale_indexes.sql`) holds the two
    indexes. It is index-only, `IF NOT EXISTS`, and makes no row changes.
    It is numbered after 057, the highest on `main` at push time.
  - **Not changed (within budget, noted for later).**
    - `list_all_sessions` is ≈165 ms for 2,000 rows in a debug build: the
      per-row work subqueries all SEARCH by index, and the cost is the JSON
      building. `today` and `tidy` inherit it.
    - `tracker_items` reads every item of every tracker (5,000 rows plus
      their JSON meta) for `today` and `tickets`. A `done since` filter in
      SQL is the next step if a larger board needs it.
  - **Frontend.** `src/lib/work_scale.test.ts` (Vitest, jsdom, seeded
    mulberry32, 2,000 rows), p95 on the same container:
    - `buildSessionsByWork` + `sortWorkGroups` under the work filters:
      4.6 ms (budget 250);
    - `rowMatches` × 7 filter combinations: 3.6 ms (100);
    - `sessionWorkRow` × 2,000: 3.0 ms (150);
    - `scopeToday` + `standupText` over a 200-group digest: 4.8 ms (150).

    Each also asserts shape: the key, scope, predicate and severity
    callbacks run once per row, never per row × group, and the output
    matches a direct reading of the fixture.
- 2026-09-25: M12.3 landed on `claude/cloud-fleet-work-graph-m12-retention`. Settings only (D23), no migration. The three `work.retention.*` windows (D21) replace M2's `work.journal_days`. While the new journal key is unset, an old `0` still means forever, and an old window only counts if it is longer than 365 days (it was chosen when confirmed work was kept forever). Protected sets per table are in `store/work_retention.rs`: journal rows of open conversations, live-linked sessions and work not done or still live-linked; undelivered handovers and handovers to a live session; done tickets any link or bare ref names, and ancestors of kept tickets; the newest work event of each kind per session, by both orders its readers use (`at`, `id` for a pending handover; `MAX(id)` for M11.3's `tidy_kept` keep, which is swept like the other tidy events once superseded). There is no pinned-note concept, so notes follow the journal rules. M11.4's `work_admin { status }` now answers `{ trackers, retention }`: the sync metrics, plus rows, the dry run and the last sweep; `fleet-hub tracker status` prints both. `sweep_now` is new. Both are master-only. The sweep deletes at most 2,000 rows per table per tick, 200 per lock. The Settings → Limits → Retention UI is standalone-only, and its two commands are `LocalOnly`. The tool surface measures 55,635 B (+20 over main at merge time); `BUDGET_BYTES` is 55,735 (measured plus 100). At M12.2's scale (`scale_retention_status_and_sweep_batches`, its own fixture copy, two years on, debug build): `status` p95 329 ms over separate short locks; 2,704 of 50,000 journal rows and 18 of 5,000 items swept, each equal to the dry run; the slowest batch, which is how long the lock is held, 186 ms (budget 1,000).
- 2026-09-26: **M12.4 (trackers in fleet health) landed** on `claude/cloud-fleet-work-graph-m12-health`. `fleet_health` gains `trackers` (`service::health::TrackersHealth`, `#[serde(default)]`, additive, no `CONTRACT_REVISION` bump, no new tool): per tracker `health` (`ok` / `degraded` / `failing`), the stored `state`, `consecutive_failures`, `last_error`, `last_success_at` (the stored `last_sync_at`), `last_pass_at`, `org_id` / `org_name`; plus `failing`, `degraded`, `detection_backlog` and `detection_backlog_days`. It reads M11.4's in-memory `SyncMetrics` (which gain `consecutive_failures`, reset by a pass that ends ok) and the store, never a tracker or a host (design decision 3). Levels: `auth_failed`, `captcha` and `unconfigured` are `failing` at once (the sync stops polling them, so no count would grow), as are 3 failed passes in a row (`TRACKER_FAILING_AFTER`); transient or unknown states and fewer failures are `degraded`. The error is redacted, defused, one line, at most 300 characters, and fenced as untrusted (`fence_untrusted`) for every MCP caller; the desktop strips the fence to show it. The detection backlog counts live suggestions older than `DETECTION_BACKLOG_DAYS` (7, a constant — no new setting) on live sessions, minus a weak one beside a confirmed primary (the row hides it too). Isolation: a per-host token sees only its own org's trackers (an unassigned host only unassigned ones — stricter than `sees_org`, since a tracker's name and error describe another team's setup) and its own host's backlog; `tests_isolation::fleet_healths_tracker_roll_up_is_fenced_by_org` runs it for all six callers. Desktop: the footer shows a `trackers: …` line (click: Settings → Work), and D22's Attention item — `TrackerAttention.svelte` in the attention strip — shows ONE "Reconnect Jira (acme)" per failing tracker, deduplicated by id, opening Settings scrolled to Work (`openSettingsAt`); a degraded tracker raises none. The roll-up is seeded at startup and re-read every 60 s. The tool surface measures 55,804 B (+169); `BUDGET_BYTES` is 55,904.
- 2026-09-26: **M12.6 written** (docs only): `docs/superpowers/reviews/2026-09-26-work-graph-decisions-revisited.md`. It was written before the user's M10.3 run: `docs/work-graph-acceptance.md` does not exist yet and no usage is recorded, so it says so and recommends keeping every decision until that evidence exists. It also names what evidence would change each one. Findings:
  - D15's "read-only phone" no longer matches M8/M8.6, where a full token gets Confirm / Not this, Start / Resume and *Ask for a handover*. The note suggests rewording it to "no desktop-sized flows on the phone".
  - D20 (name work on the phone) reuses `work_link { action: "name" }`, so it is the cheapest to turn to yes.
  - The roadmap's D16 is corrected: `hub-e2e` already runs in CI's `hub-headless` job, and only the work-graph leg (hub W, needs the `e2e` build) is local.

