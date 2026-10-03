# `origin/main` merged into `m1-merge-main`

Branch `m1-merge-main`, branched from `mellow-virgo` at its single WIP commit
`85905c33` ("wip(m1): multi-user foundations"). `mellow-virgo` itself was not
touched. Merge base `77653006`; `origin/main` was 203 commits ahead, 261 files
changed, +47,995/−1,344.

## Summary

- **22 conflict hunks in 17 files**, all resolved on their merits; nothing taken
  wholesale from one side except `store/mod.rs` (main's test-store template, a
  pure build-performance change with no M1 content).
- **M1's migrations 086–089 were renumbered 094–097.** `main` had shipped
  086–093 meanwhile. Forced, not chosen; the SQL bodies are unchanged because
  the two sets touch disjoint tables. **One open question for you**: a database
  an earlier M1 build already opened will skip main's 086–093, and I did not
  add a repair arm. See [the collision](#the-migration-number-collision).
- **The merge introduced no new deterministic test failure.** Proven by running
  the same `fleet-core` binary on `85905c33` (M1 alone) and diffing the failure
  sets: M1 4480/11, merged 4875/8, and every non-flake failure appears in both.
- **Five pre-existing M1 failures remain**, four of them fences reading more
  permissively than their own tests demand. They fail identically on M1 before
  the merge. I did not invent answers for them — they are M1's design calls.
- `cargo fmt --all --check`, `cargo fleet-check`, `cargo fleet-lint`, `pnpm
  check` (658 files, 0 errors) are all clean. `pnpm test` 3892/3893, the one a
  5 s timeout that passes alone.
- **A bug of my own, found and fixed**: the renumber missed the version each
  migration records *inside its SQL*, so M1's four migrations silently never
  ran. [Recorded at the end](#one-bug-of-my-own-recorded-because-it-was-nearly-invisible).

---

The merge produced **22 conflict hunks in 17 files** — and one conflict git
could not see, which is the most important thing in this report:
[the migration-number collision](#the-migration-number-collision).

Neither `CLAUDE.md` nor any build-configuration file conflicted: **the M1
commit does not touch them at all** (`git diff --name-only 77653006 HEAD` hits
no `CLAUDE.md`, `.cargo/`, `Cargo.*`, `rust-toolchain.toml`, `.githooks/` or
workflow path). Main's versions therefore arrived whole, with no decision to
make — including the toolchain pin to 1.99.0, the `fleet-*` cargo aliases, the
rlib-only desktop library and the SQLite build flags.

---

## The migration-number collision

**M1's four migration scripts were renumbered 086–089 → 094–097.**

`origin/main` shipped eight migrations in the meantime:

| main | | M1 (as written) | → renumbered |
|---|---|---|---|
| 086 | `shared_work_context` | 086 `people` | **094** |
| 087 | `inventory_flags` | 087 `session_owner` | **095** |
| 088 | `guides` | 088 `session_grants` | **096** |
| 089 | `host_harnesses` | 089 `tasks_detach` | **097** |
| 090–093 | `catalogs`, `catalog_ids`, `host_provision_warning`, `catalog_access` | | |

Two scripts cannot both be 086, so this was forced rather than chosen. The
renumber is the whole of the change: **the SQL bodies are untouched**, because
the two sets alter disjoint tables — M1 writes `people`, `sessions`,
`session_grants`, `tasks`; main writes `work_items`, `asset_inventory`,
`guide_proposals`, `hosts`, `catalogs`, `host_layers`, `client_catalog_grants`,
`host_catalogs`. Nothing had to be rewritten to compose.

M1's 095 re-issues `sessions_row_version_bump` (as 082 did before it) and, now
ordered after main's 091, is still the newest re-issue — which is what running
them in order would have left.

What moved with the numbers: `git mv` of the four files; the four `MIGRATIONS`
rows in `store/schema.rs` (now listed after main's 086–093); the `SEED_AT`
constants in M1's three re-run tests (85→93, 86→94, 87→95) and the
`store_at_version(86)`→`(94)` in the backfill test; four test function names;
and ~139 comment / `expect()` references across 31 files. Those references
were classified one at a time — M1's by their subject (people, person, owner,
visibility, grant, `detached_at`), main's by theirs (work items, asset
inventory, guides, harnesses, catalogs) — and every changed line was reviewed,
because both sets now own a number in 086–089.

`LATEST_SCHEMA_VERSION` and `known_schema_version()` are both derived from
`MIGRATIONS`, so neither needed a hand edit.

### ⚠ Open question for M1's owner: no repair arm

A database that an **earlier M1 build already opened** recorded versions 86–89
for M1's scripts. `migrate()` only offers `version > MAX(schema_version)`, so
on that database **main's 086–093 will never run**, and main's own guards will
then record 094–097 without their statements. The schema ends up missing
`work_items`' shared-context columns, `guide_proposals`, `hosts.harnesses`,
`hosts.provision_warning` and the whole catalogs set.

This is precisely the collision `Store::repair_skipped_main_migrations`
(`store/schema.rs`) already handles **twice** — for the conversations branch
(034/036) and for the hub-ops branch (064–068). The repo has a settled
convention for it.

**I did not add a third repair arm.** It is a real code-and-test change about
which databases exist in the wild, and only you know whether any `mellow-virgo`
build has opened a database that matters (a dev box, the hub, a phone's paired
desktop). A pointer to this decision is written in the comment above M1's rows
in `MIGRATIONS`. If the answer is "yes, repair it", the shape is: detect each
of main's 086–093 by its artefact (main already has the predicates —
`work_items_has_origin`, `asset_inventory_has_fleet_owned`,
`hosts_has_harnesses`, `hosts_has_provision_warning`,
`asset_inventory_has_catalog_id`) and run the missing script when the recorded
version is past it. If the answer is "no database to save", the renumber alone
is complete and correct.

---

## Every conflicted file, and which side I took

### Both sides added adjacent, independent things — took both (10 files)

Six files are the same conflict: M1 appends `unclaimed_sessions` to a `HostRow`
literal, main appends `provision_warning` and `harnesses`. Both fields are
real; the literal needs all three.

| File | Resolution |
|---|---|
| `service/account_usage.rs` | both — M1's `unclaimed_sessions`, main's `provision_warning` / `harnesses` |
| `service/account_usage_poll.rs` | both, same |
| `service/health.rs` | both, same |
| `service/hosts.rs` | both, same |
| `service/onboarding.rs` | both, same |
| `store/reconcile.rs` | both, same |
| `store/rows.rs` | both. Two hunks: the `HostRow` field declarations, and the row reader — main reads SQL columns 23/24, M1's field is computed per request and stays `None`, so there is no index to renumber |
| `src-tauri/backend/tests_contract.rs` | both — the golden sample needs every field name on the wire |
| `src/lib/hosts.ts` | both — the TS mirror of the same three fields |
| `fleet-hub/src/pair.rs` | both test blocks. The concatenation dropped M1's closing brace (its hunk ended mid-`fn`); restored |

### Took main

| File | Resolution |
|---|---|
| `store/mod.rs` | **main**, wholesale. M1 kept `open_in_memory` building a store by hand; main delegates to `open_with_bus_in_memory`, which copies a once-per-process migrated template instead of running every migration per test store. That is a deliberate build-performance change with no M1 content in it, and the field set is identical on both sides. (Main's `migrated_template_copy` then needed M1's `owner_intent` field — see the fixes below.) |

### Took both, by hand

| File | Resolution |
|---|---|
| `fleet-hub/src/main.rs` | M1's `ClientCmd::BindPerson` / `UnbindPerson` arms **plus** main's `Grant` / `Ungrant` arms, which grew a `catalog` field. Not either side's text: M1's arms are new commands, main's are the same commands with a new parameter |
| `service/trackers/tickets.rs` | main added a `with_native_defaults` step to `start_work`; M1 had renamed the function's `scope: &OrgScope` parameter to `view: &ViewScope`. `with_native_defaults` takes only the org half, so the merged line is `with_native_defaults(store, args, &view.org)?` followed by M1's `plan_start(store, args, view, net)` |
| `service/work/view.rs` | three hunks. (1) `Graph` keeps M1's `hidden_sessions` / `hidden_links` **and** main's `job_states` / `open_proposals`. (2) M1 split `load` into `load` / `load_for` / `build`; main's new `items` and `open_proposals` computation moved into `build`, and the literal uses M1's `links` local (which `hidden_links` reads) rather than a second `work_view_links()` query. (3) M1's `link_hidden` method **and** main's `item_org` parent fallback, with main's doc comment, which is the one that describes the merged body |
| `mcp/tools/tests.rs` | two hunks. The `BUDGET_BYTES` measurement history is main's — it is the repo's running record — with M1's line appended and the constant **re-measured** (below). The second hunk is two test blocks appended at the same place: both kept, and a dropped closing brace restored |
| `service/provision.rs` | three hunks, and the only place where both sides were load-bearing prose about the same mechanism. Main added the `ag` launcher to `fingerprint()`'s inputs and documented it; M1 had documented, at length, that the fingerprint deliberately does **not** cover the `~/.claude.json` MCP entry where its `X-Fleet-Pane` header lives, and why adding it would be worse than the gap. Both statements are true of the merged code and both are kept. M1's pinning test is kept too — and **extended**: it asserted that no fingerprint *input* carries the pane header, over a hard-coded list of inputs, and `AG_FILES` has just become an input, so the list now chains the ag bodies. (The naive concatenation had put that pin inside main's unrelated `provision_ag_failure_is_a_warning` test; moved back into the fingerprint test where M1 had it.) |
| `store/schema.rs` | both sets of `MIGRATIONS` rows — main's 086–093 first, then M1's renumbered 094–097, with a comment recording the renumber and pointing at the open question above |

---

## Changes to M1's own code, to satisfy main

Everything here is M1's side bending to main, not the reverse.

### Compile errors

1. **`service/work/view.rs` — `scope.sees_row` → `scope.sees_row_org_only`
   (2 call sites in main's new `native_work`).** M1 renamed
   `OrgScope::sees_row` and tightened its `Org` arm; the old name is gone, so
   main's new code had to adopt the new one. `native_work` is reached only from
   `task`, which builds the graph with `Graph::load_for`, so `g.sessions` has
   already had every person-invisible row removed — these two calls are the org
   half alone, which is what the new name says.

2. **`mcp/tools/orchestration.rs` — main's new `propose` arm calls
   `resolve_target_row` with 5 arguments; M1 gave it a 6th, `reach: Reach`.**
   I chose **`Reach::Drive`**, consistent with every other `work` action that
   writes a row about one session (`decide_batch` and the `work_link` decision
   gate both do, and say why). The proposal is *stored in that session's name*,
   so a caller who may only watch a session must not be able to put words in its
   mouth. `Reach::Read` would have been enough for the name and host the code
   reads, which is why this was a judgement call rather than a lookup — it is
   flagged again at the bottom.

3. **`store/mod.rs` — `owner_intent` added to main's `migrated_template_copy`
   store literal.** Main wrote the literal; M1 added the field.

4. **`service/work/view_tests.rs` (7 sites) and
   `service/trackers/tests_tickets.rs` (1 site)** — main's new tests pass a
   bare `&OrgScope` to `task` / `resolve_start`, which now take M1's
   `&ViewScope`. Each is wrapped in the `vs(…)` helper that file already
   defines for exactly this (`tests_tickets.rs`'s builds a real `ViewScope`
   through the one permitted constructor rather than `ViewScope::internal`, so
   the org matrices keep asserting what they were written to assert).

### M1's registries, which caught main's new code

These are not build errors — they are M1's own acceptance gates refusing to
let new code through unclassified, and they worked.

5. **`scope_guard_tests::every_scope_guard_is_classified` — 5 new rows in
   `SCOPE_GUARDS`**, for main's `work/local.rs` `create_task` (#0, #1),
   `propose` (#0), `decide` (#0) and `view.rs` `native_work` (#0). All five are
   `Verdict::OrgBoundary`, and the test requires the sentence "This is the org
   boundary, not a privacy fence" **at the guard**, so it is written there too,
   with the reason in each case. They are org-boundary and not privacy fences
   because every one asks about a **work item** — a standalone task has no
   links and no sessions at all; the parent checks ask whether the parent item
   exists for this caller, and an item's key and title are work data that
   survive the person fence exactly as `task_visible`'s do; `decide` refuses
   every scoped caller outright, so no row is reached to fence.

6. **`scope_guard_tests::a_row_that_names_call_sites_names_all_of_them` —
   `local_item_visible`'s row undercounted its callers again.** Main added a
   fourth, `visible_parent`. Named, with what it asks.

7. **`scope_guard_tests::every_org_only_session_predicate_call_names_its_person_half`
   — 2 new rows in `ORG_HALF_SITES`** for `native_work`'s two
   `sees_row_org_only` calls, plus the person predicate named **in the code**:
   the table alone does not satisfy this test by design, so `native_work`'s doc
   comment now says that every session it reads comes out of a `g.sessions`
   that `Graph::load_for` emptied into `hidden_sessions`.

8. **`the_served_definition_budget_stays_bounded` — `BUDGET_BYTES` re-measured
   to 71_014.** The merged master tool surface measures **70,914 bytes**:
   main's 70,623 plus **291 bytes** of M1 prose (`pair_client { person }` and
   its sentences in `pair_client` / `list_clients`, and M1 T6's sentence on
   `list_hosts` about `unclaimed_sessions`). Set to measurement + the
   customary 100 bytes of headroom, as the test's own message instructs.

Nothing else in M1 needed changing. In particular the desktop crate becoming
an **rlib only** required no change: `src-tauri`'s tests build and run as
before.

---

## What on `main` CHANGES M1's design, not just its build

Read this section before continuing M1.

1. **The migration list — the collision above.** M1 has live work in
   `store/schema.rs`; main touched it in 15 commits. The numbers moved and the
   repair arm is an open decision.

2. **Two new MCP tools M1's person gate has never seen:** `guide`
   (`Access::Client`, not readonly — a host's session proposes a guide) and
   `set_host_harnesses` (`Access::Master`). Main also widened `catalog_admin`
   and `import_assets` (per-catalog grants, `NOT_FOR_HOST_TOKENS`,
   `may_admin_catalog`). M1's choke points — `require_person_sees`, the person
   half of `ViewScope` — were written against the tool set as it was. Each of
   these needs the same question asked of it that M1 asked of every other tool:
   whose rows can it reach, and which predicate finishes the fence.

3. **`work` / `work_link` grew four actions** — `create`, `propose`, `accept`,
   `reject`-a-proposal (`service/work/local.rs`, shared work context). M1's
   `work`-action gate tables had to grow with them: the five `SCOPE_GUARDS`
   rows above, and the `Reach` for `propose`. Main did add the
   `tests_isolation.rs` matrix rows, so the org half is covered; the **person**
   half of a proposal is now exactly one decision — the `Reach::Drive` I chose
   in `orchestration.rs` — and it deserves your eye.

4. **`service/work/view.rs`'s `Graph` is no longer only M1's.** It now carries
   main's `job_states` and `open_proposals`, and `item_org` gained a parent
   fallback for native subtasks. Any new projection off `Graph` must still ask
   `link_hidden` first, which is unchanged; but the graph is now a shared
   surface and the merge had to interleave main's computation into M1's
   `build`.

5. **`mcp/guard.rs` changed in 5 commits and `verdicts.rs` in 6**, adding
   `accept_work_proposal`, `create_work_task`, `reject_work_proposal`,
   `decide_guide`, `list_guides`, `remove_guide`, `guide`,
   `catalog_set_host_harnesses` and `catalog_admin` rows. The hub contract
   golden (`hub_contract.golden.json`) merged cleanly and
   `tests_contract` / `tests_routing` pass, so routing is consistent — but
   these are nine new desktop commands whose M1 verdict nobody has stated.

**Not** changed, and worth knowing because M1 has work there:
`crates/fleet-core/src/events.rs`, `mcp/events_route.rs` and `mcp/pairing.rs`
were untouched by main (0 commits each). The event bus and the pairing path
are M1's alone.

---

## The validation commands a future agent on this branch should use

Quoted from main's `CLAUDE.md`, "Build & test → Validation ladder (use this,
in this order)". These replace any hand-rolled loop; the aliases are in
`.cargo/config.toml`.

```bash
# 1. after every edit (fleet-core edit ≈ 20 s; = rust-analyzer's own check)
cargo fleet-check                       # check --workspace --all-targets
pnpm check                              # frontend edits: svelte-check
# 2. the tests of what you touched (module path filter; fleet-core module ≈ 40 s)
cargo fleet-test -- service::health     # test --workspace --lib --bins -- <filter>
pnpm exec vitest run src/lib/foo.test.ts
# 3. before committing (also what .githooks/pre-commit runs)
cargo fmt --all --check
cargo fleet-lint                        # clippy --workspace --all-targets -- -D warnings
# 4. before pushing / marking a PR ready (≈ 20 min; offload it on a small box)
cargo test --workspace                  # full suite, what CI runs
scripts/ci-local.sh                     # everything in CI order; --rust-only / --frontend-only / --hub-e2e
```

Main states the rules behind them, and they are the ones that retire the
branch's older advice:

> - Do not run `cargo build` to see whether something compiles; `cargo
>   fleet-check` answers that 2–6× faster. Build only when you need a binary.
> - Do not narrow with `-p <crate>` in the inner loop; narrow with a test
>   filter. Keep `-p` for the cases below that need a binary or a different
>   feature set.

So `cargo test -p fleet-core` and `cargo build` in an inner loop — what M1's
own notes suggested — are both out: each `-p` selection makes cargo compile
another copy of fleet-core and its dependency graph.

The regeneration commands also moved to the aliases:

```bash
REGEN_DOCS=1 cargo fleet-test -- reference_is_current
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen
REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current
REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current
```

Main's own flake list, which now applies to this branch:

> Known Rust flakes — timing-sensitive, so they fail on a loaded box; re-run
> alone before blaming your change:
> `rewind::tests::the_removal_script_leaves_a_tree_a_live_pane_is_in`,
> `work::scale_tests::*`, the `CHAIN_BUDGET` migration tests in
> `store/schema/tests_upgrade.rs`, and `service::add_project`.


---

## Step 4: the verbatim results

### A note on how these were run

This host could not run `cargo test --workspace` as one process. Three
attempts were killed by the machine's low-memory watchdog (the box carries
other claude-fleet sessions; load average was 16–33 on 12 cores throughout,
and the kills land on the largest process). The suite was therefore run **per
test binary**, against the binaries `cargo test --workspace --no-run` built —
the same code, the same feature set, the same selection, just not in one
process. Where a run is split or re-run alone, it says so.

### `cargo fmt --all --check`

```
$ cargo fmt --all --check
$ echo $?
0
```

Clean, no output.

### `cargo fleet-check`

```
$ cargo fleet-check
    Finished `dev` profile [unoptimized + debuginfo] target(s)
$ echo $?
0
```

Clean. (Cold, with the new 1.99.0 toolchain, the first run took 10m02s.)

### `cargo fleet-lint`

```
$ cargo fleet-lint
    Finished `dev` profile [unoptimized + debuginfo] target(s)
$ echo $?
0
```

Clean — no warnings, under `-D warnings`.

### `cargo test --workspace`

Per binary, as explained above.

```
fleet_core        test result: FAILED. 4875 passed; 8 failed; 3 ignored; 0 measured; 0 filtered out; finished in 611.42s
fleet-hub         test result: ok. 164 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 153.11s
claude_fleet_lib  test result: ok. 381 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out  (377 in one run + the 4 slow routing tests below, run separately)
fleet_agent       test result: ok. 107 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 18.37s
fleet_proto       test result: ok.  44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
frames            test result: ok.  45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.34s
fleet_update      test result: ok.  37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
local_host_guard  test result: ok.   1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.49s
wire              test result: ok.   1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
decide_cases      test result: ok.   1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
fleet-agent       test result: ok.   0 passed; 0 failed (no tests in that target)
fleet-release     test result: ok.   0 passed; 0 failed (no tests in that target)
claude-fleet      test result: ok.   0 passed; 0 failed (no tests in that target)
```

**The four slow `src-tauri` routing tests**, which the watchdog kept cutting
off, were run on their own and all pass:

```
$ claude_fleet_lib --test-threads=4 --exact \
    backend::routing::tests::a_configured_but_unavailable_hub_refuses_every_routed_command \
    backend::routing::tests::a_hub_with_a_skewed_wire_contract_refuses_every_routed_command \
    backend::routing::tests::every_routed_mutation_names_its_tool_and_arguments \
    backend::routing::tests::every_routed_read_names_its_tool_and_arguments
test backend::routing::tests::every_routed_read_names_its_tool_and_arguments ... ok
test backend::routing::tests::every_routed_mutation_names_its_tool_and_arguments ... ok
test backend::routing::tests::a_configured_but_unavailable_hub_refuses_every_routed_command ... ok
test backend::routing::tests::a_hub_with_a_skewed_wire_contract_refuses_every_routed_command ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 377 filtered out; finished in 1274.64s
```

21 minutes for four tests, and that is inherent rather than a merge effect:
each iterates every routed command (~214) and builds a **fresh on-disk store**
per command, so each one runs the migration chain ~214 times — and the chain
is now 97 long instead of 85, main's 8 plus M1's 4. They matter here because
they are what proves main's nine new desktop commands route or refuse
consistently with M1's tree. (Main's `migrated_template_copy` optimisation
covers `open_in_memory` only; this path is on disk.)

**`fleet_core`'s 8 failures, and what they are.** I did not want to assert
"pre-existing" from reading code, so I built `85905c33` — M1 alone, before the
merge — in a second worktree and ran the same binary there:

```
M1 baseline (85905c33)  test result: FAILED. 4480 passed; 11 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2707.75s
merged (this branch)    test result: FAILED. 4875 passed;  8 failed; 3 ignored; 0 measured; 0 filtered out; finished in 611.42s
```

Diffing the two failure sets:

```
--- NEW in merged (i.e. candidate regressions) ---
store::schema::tests_upgrade::opening_a_pre_work_graph_file_upgrades_it_within_budget

--- present in M1, absent in merged ---
service::work::scale_tests::scale_list_sessions_with_work_fields
service::work::scale_tests::scale_recent_ended_work_links
service::work::scale_tests::scale_resolver
service::work::scale_tests::scale_usage_summary

--- in BOTH (pre-existing M1 WIP) ---
service::view_scope::tests::only_caller_view_scope_constructs_a_view_scope
service::work::card::tests::the_lift_is_fenced_by_org_scope
service::work::scale_tests::scale_work_today
service::work::scale_tests::scale_work_view
service::work::today::tests::today_reads_the_store_and_a_host_scope_reads_only_its_host
service::work::view::tests::d31_decides_whether_a_bound_client_sees_unassigned_work
store::reconcile::tests::renaming_onto_a_lost_sessions_name_is_refused_and_keeps_it
```

**The merge introduced no new deterministic failure.** The single entry in the
"new" column is a `CHAIN_BUDGET` latency test, which main's own `CLAUDE.md`
names as a known flake on a loaded box; re-run alone it passes:

```
$ cargo fleet-test -- store::schema::tests_upgrade::opening_a_pre_work_graph_file_upgrades_it_within_budget
test store::schema::tests_upgrade::opening_a_pre_work_graph_file_upgrades_it_within_budget ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4885 filtered out; finished in 2.01s
```

The `scale_tests` moving in BOTH directions between the two runs is the same
effect from the other side — they are latency budgets, and which of them trips
depends on what else the box is doing, not on the code. Per your instruction I
have neither treated them as regressions nor touched them.

**The five that fail in both are M1's own unfinished business**, and they are
worth your attention because four of them are fences reading more permissively
than their tests demand. Identical assertion output on M1 and on the merge:

| Test | M1 and merged both say |
|---|---|
| `only_caller_view_scope_constructs_a_view_scope` | `src/service/health.rs: calls ViewScope::for_caller` — M1's own rule is that only `Caller::view_scope` in `mcp/auth.rs` may build one; `health.rs`'s parity test calls it directly (M1's line, `health.rs:1554` here / `1544` on M1) |
| `the_lift_is_fenced_by_org_scope` | `left: Some("in_progress")`, `right: Some("todo")` — "org A must not see org B's session lift this to in_progress" |
| `today_reads_the_store_and_a_host_scope_reads_only_its_host` | `left: ["h", "h2"]`, `right: ["h"]` |
| `d31_decides_whether_a_bound_client_sees_unassigned_work` | `called Result::unwrap_err() on an Ok value: SessionTasks { session_id: 3, org_id: None, primary_link_id: None, links: [] }` |
| `renaming_onto_a_lost_sessions_name_is_refused_and_keeps_it` | `dev-b belongs to a lost session; restore it with restore_host_sessions or dismiss it first` |

I deliberately did **not** "fix" these. Each is a statement about M1's own
privacy model — whether an org-scoped reader may see another org's live
session lift an item, what a host scope's Today contains, whether a bound
client gets `E_*` for unassigned work — and inventing an answer would be
writing M1's design rather than merging into it. They are the same four the
`scope_guard` registries are built to force a decision about.

### `pnpm install --frozen-lockfile && pnpm check && pnpm test`

```
$ pnpm install --frozen-lockfile
Done in 1m 1.8s using pnpm v10.34.5

$ pnpm check
> svelte-check --tsconfig ./tsconfig.json
COMPLETED 658 FILES 0 ERRORS 0 WARNINGS 0 FILES_WITH_PROBLEMS

$ pnpm test
 Test Files  1 failed | 210 passed (211)
      Tests  1 failed | 3892 passed (3893)
   Duration  394.69s
```

The one failure is a load artifact, and it says so itself:

```
FAIL  src/lib/pages/PageView.test.ts > PageView — every generated page >
      renders every field it places, tab by tab, with the registry label and help
Error: Test timed out in 5000ms.
```

Re-run alone:

```
$ pnpm exec vitest run src/lib/pages/PageView.test.ts
 Test Files  1 passed (1)
      Tests  18 passed (18)
   Duration  8.60s
```

(An earlier `pnpm test` run, started while a cargo build held the CPU, also
reported 2722/2722 tests passing with 11 vitest *worker* timeouts — the same
contention, in its other shape.)

### The registries and generated artifacts, separately

Because these are the gates most likely to object to a merge, each was run on
its own:

```
$ cargo fleet-test -- scope_guard_tests::
test scope_guard_tests::the_guard_scan_finds_guards_and_skips_test_code_and_comments ... ok
test scope_guard_tests::every_org_only_session_predicate_call_names_its_person_half ... ok
test scope_guard_tests::every_scope_guard_is_classified ... ok
test scope_guard_tests::a_row_that_names_call_sites_names_all_of_them ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 4882 filtered out; finished in 97.12s

$ cargo fleet-test -- every_session_addressed_tool_declares_its_reach the_served_definition_budget_stays_bounded reference_is_current
test mcp::doc_gen::tests::reference_is_current ... ok
test mcp::tools::tests::the_served_definition_budget_stays_bounded ... ok
test mcp::tools::tests::every_session_addressed_tool_declares_its_reach ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 4883 filtered out; finished in 0.33s

$ cargo fleet-test -- settings_docs_are_current page_docs_are_current
test service::settings_doc_gen::tests::settings_docs_are_current ... ok
test pages::tests::page_docs_are_current ... ok

$ cargo fleet-test -- verdict_gen
test result: ok. 20 passed; 0 failed   (all backend::verdict_gen::tests)

$ cargo fleet-test -- store::schema::
test result: FAILED. 91 passed; 1 failed   (the CHAIN_BUDGET flake above; 48 before the fix below)
```

No generated artifact needed regenerating: `docs/control-api-reference.md`,
`src/lib/hub_verdicts.generated.json` + `docs/hub.md`'s refusal table,
`docs/settings-reference.md` and the page docs were all already current after
the merge.

---

## One bug of my own, recorded because it was nearly invisible

The first full `fleet_core` run after the renumber failed **56** tests, 48 of
them in `store::schema`, with `left: 93, right: 97`.

Each migration records its own version *inside its SQL*
(`INSERT OR IGNORE INTO schema_version (version) VALUES (94);` is the last
statement of `094_people.sql`). `git mv` renamed the files and I updated
`MIGRATIONS`, but the four SQL bodies still said `VALUES (86)` … `(89)`. The
effect was quiet and bad: `migrate()` recorded 89 and then stopped offering
94–97, so **M1's four migrations never ran** — no `people`, no
`sessions.visibility`, no `owner_person_id`, no `session_grants` — and the
person fences had no columns to read. That is why the first run's failures
reached far past `store::schema` into `card`, `today`, `view` and `reconcile`.

Fixed in the four SQL bodies, together with their own stale cross-references
(`095_session_owner.sql`'s "Migration 086 gave the hub its people" → 094, and
so on). `store::schema` went from 48 failures to 1. Recorded here because a
renumber that updates the filename and the table but not the SQL body leaves a
schema that is silently short of four migrations, and only the version
assertions catch it.
