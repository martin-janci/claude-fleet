# M1 ↔ `origin/main` integration scout report

**Branch** `m1-integrate` · **merged** `origin/main` (77 commits, `65fd5001..cfd22efc`,
including `chore(release): v0.4.6`) into M1's `89073391` (T1–T13).
**Merge base** `65fd5001`.

What main brought that matters to M1: **Assets M4 changesets** (migration 094, the
`changesets` tool), **file downloads** (migration 095, the `send_file` /
`list_downloads` / `remove_download` tools, the `GET /downloads/<id>` route, four
desktop commands, a `download:changed` row event, its own `CONTRACT_REVISION` bump
to 7), **`scripts/verify.sh`** as the one verification command, and a
**build-and-test performance effort** that restructured two test files M1 had
conflicts in.

---

## Decisions taken on review

Three decisions were taken on this merge after the first pass, and they are the
reason the download fence reads the way it does. Recorded here because the code
carries the *verdict* and this carries the *argument*.

### Q2 — `send_file` is the `own` tier, not `Read`
My first pass used `sees_session_row` (≈ `Reach::Read`) on the ground that it
"matches `repo_file`". **That precedent does not transfer: `repo_file` is confined
to the worktree and `send_file` is not.**

`service/downloads.rs::stat_script` resolves a RELATIVE path against the session's
`pane_current_path`, but an ABSOLUTE path is accepted as given — `parse_stat`'s
success condition is `path.starts_with('/')`, with no canonicalisation against a
root and no `starts_with(worktree)` check. So
`send_file { session_id, path: "/home/<user>/.claude/.credentials.json" }` reads
that file off the session's host and copies it to the caller's devices.

At `Read` that is reachable by a **watcher**. An unconstrained read of the owner's
host is a subset of what a terminal gives, and spec §4.3 invariant 5 — *sharing
never confers a terminal* — exists precisely to refuse that class. So **only the
session's owner may send a file**: `may_own`, not `sees_session_row`.

The alternative was considered and **not** taken: confine the path to the worktree
and keep `Read`. That is the better product answer and it would make the
`repo_file` precedent true — but it redesigns main's feature rather than fencing
it. Main's authors may well have meant an owner to be able to send `/var/log/...`
off their own host, and that use survives intact under `own`. **Confining the path
is a follow-up for whoever owns downloads; if they do it, `Read` becomes
defensible and this tier should be revisited.** That sentence is written next to
`downloads::visible`, so the next person inherits the reasoning and not just the
verdict.

`list_downloads` and `remove_download` were judged on what they actually touch,
and **the same argument applies to both**: `list_downloads`' rows carry the
absolute path, so it is the index into those bytes, and `remove_download` destroys
the owner's copy. All three are the `own` tier, and `GET /downloads/<id>` — which
serves the bytes themselves — with them.

The per-host token is the one caller that does **not** go through `may_own`, and it
has to be: a session's own Claude is how main's headline use works
(`whoami` → `send_file`), and `may_own` deliberately excludes the pane proof so
that no agent can reach the owner tier. Its arm is §4.4 clauses 1 and 2 through
`sees_session_row` — its own host, and either an unclaimed row or the one pane the
request proves — which is strictly tighter than main's host-only fence.

### Q1 — a download whose session row is gone: org-only stands
Kept as shipped. Side B (a `person` column on `downloads`) is a migration into
main's own feature, and M1 is not the milestone to add it.

It also **matters less** under Q2's answer, and that is what makes the residue
bounded rather than merely acknowledged: if only the owner (or the session's own
Claude) can send a file, every download that could ever reach the dead-session arm
was created by the owner's own act, never by a grantee. A person's own device keeps
seeing files it was sent; nobody inherits a file a grantee extracted, because a
grantee can extract none. The `ORG_HALF_SITES` note says this.

### D2 — the download route got what `/events` got
`only_the_owner_reaches_a_private_sessions_download` in
`crates/fleet-core/src/service/downloads_tests.rs` asks `visible`, `open_ready`
(the bytes the route streams) and `remove` as a `watch` grantee, a `drive`
grantee, another person, the session's own host token and a client bound to
another org — five refusals — and as the owner and `ViewScope::internal()`, which
both succeed. Under Q2's answer a watcher is refused the **bytes** as well as the
send, and the test holds that at the gate rather than at the route's early
`host_alias` return, so deleting that early return would still not open the route.

---

## CONFLICTS

13 files conflicted. None was taken wholesale from one side.

| File | Took | Why |
|---|---|---|
| `crates/fleet-core/src/wire_contract.rs` | **both, renumbered** | Both sides bumped `CONTRACT_REVISION` 6 → 7 independently — main for file downloads, M1 for the five sharing tools. Main's revision-7 paragraph stays as **7**; M1's becomes the new **revision 8**, and `CONTRACT_REVISION = 8`. |
| `src-tauri/src/backend/contract.rs` | **both, renumbered** | Same collision on the client side. Main's "Raised to 7 … file downloads" paragraph kept; M1's reworded as "Raised to 8 for revision 8"; `MIN_HUB_CONTRACT = MAX_HUB_CONTRACT = 8`; the `ContractFit` doc's "the real bounds are `7..=7` today" became `8..=8`. |
| `crates/fleet-core/src/events.rs` | **both (union)** | Two new row-event names and kinds, one per side. `EVENT_NAMES` is now 28 (`download:changed`, `grant:changed`), `EVENT_KINDS` 16 (`download`, `grant`). The `RowChange` variants themselves auto-merged. |
| `src/lib/events.ts` | **both (union)** | Seven hunks, all "one side added a handler, the other added another": `onDownloadsChanged` + `onGrantChanged`, both `RowEvent` union arms, both accumulators, both `case` arms, both flush calls, both `wanted` flags, both `sub(...)` lines. One hand fix after the union: the `RowEvent` union's terminating `;` had to move from `download:changed` to the now-last `grant:changed` arm. |
| `src/App.svelte` | **both (union)** | `onDownloadsChanged: noteDownloadsChanged` and `onGrantChanged: applyGrantChanges` in the same handler object. |
| `crates/fleet-core/src/mcp/guard.rs` | **both (union)** | `NOT_FOR_HOST_TOKENS`: main added `changesets`, `list_downloads`, `remove_download`; M1 added the five sharing tools. Both doc paragraphs kept, main's first (it is the older class), M1's as the second reason. |
| `crates/fleet-core/src/mcp/tools/mod.rs` | **both (union)** | `tool_router()` sums `downloads_router()` **and** `sharing_router()`. |
| `crates/fleet-core/src/mcp/tools/params.rs` | **both (union)** | `RemoveDownloadParams` and M1's four sharing param structs. The shared closing `}` had to be restored for `RemoveDownloadParams`. |
| `crates/fleet-core/src/mcp/events_route.rs` | **both (union)** | `HOST_BOUND_HIDDEN_KINDS` is now `["work", "settings", "update", "grant", "download"]`, with both sentences. |
| `crates/fleet-core/src/mcp/tools/tests.rs` | **both + remeasured** | `include_str!("downloads.rs")` and `include_str!("sharing.rs")` both; served tool count 108 (main) / 110 (M1) → **114**; `BUDGET_BYTES` re-stacked from main's measurement chain plus M1's +2,051 (see VALIDATION for the measured value). |
| `crates/fleet-core/src/store/schema.rs` | **both, M1 renumbered** | `MIGRATIONS`: main's 094 / 095 first, then M1's four as **096–099**. See MIGRATIONS. |
| `crates/fleet-core/src/service/work/scale_tests.rs` | **main's structure, M1's types** | This is the one conflict where main's side was *mechanically* different: main's suite cut (`e1c0b0d3`) split one `scale_work_view` test into six `#[test]`s inside `mod scale_work_view` and changed `fixture()` to hand out a per-test `copy_for_test()` instead of a shared `Mutex` guard. M1's side had only re-typed the scope from `OrgScope` to `ViewScope`. Resolved by taking **main's whole module** and re-applying M1's scope substitutions to it (`&OrgScope::All` → `&ViewScope::internal()`, `&host` / `&bound` → `ViewScope::internal().with_org(…)`). Per the rule: how we test is main's. |
| `docs/hub.md` | **regenerated** | The one conflicting line is the generated verdict-count sentence ("Of the N commands, …"). Placeholder written, then produced by `REGEN_HUB_VERDICTS`. |

One file was wrongly caught by my own renumber sweep and restored from main:
`docs/superpowers/plans/2026-10-02-assets-m4-changesets.md` (its "migration 094" is
*changesets*, main's, not M1's).

---

## M1 CHANGES

Everything below is a change to M1's own code (or to a shape M1 owns) that main
forced. **This is the list to re-apply.**

### 1. Contract revision 7 → 8 (three symbols, three doc paragraphs)
Main shipped revision 7 for file downloads before M1 pushed. M1 owns **8**:

* `crates/fleet-core/src/wire_contract.rs` — `CONTRACT_REVISION: u32 = 8`; M1's
  revision history entry renumbered to `- **8**` and its "A revision-6 hub serves
  none of the five" → "A revision-7 hub …".
* `src-tauri/src/backend/contract.rs` — `MIN_HUB_CONTRACT = 8`,
  `MAX_HUB_CONTRACT = 8`; M1's two doc paragraphs renumbered ("Raised to 8 for
  revision 8", "Raised to 8 with revision 8"), and "A revision-6 hub serves none of
  them" → "A revision-7 hub".
* `ContractFit`'s doc: `7..=7` → `8..=8`.
* `src-tauri/src/backend/hub_contract.golden.json` — `"revision": 8` (regenerated).

### 2. Migrations 094–097 → **096–099** (second renumber)
See MIGRATIONS for the file renames. The code side:

* `crates/fleet-core/src/store/schema.rs` — the four `MIGRATIONS` entries'
  `version:` and `include_str!` paths; the `Migration::plain(96, …)` →
  `Migration::plain(98, …)`; the renumber comment above main's 086 block rewritten
  (it described one renumber, now describes two, and states that **no repair arm is
  needed**).
* Three M1 migration test functions renamed, with their seed versions:
  * `migration_094_on_a_populated_v93_database_is_safe_to_rerun` →
    `migration_096_on_a_populated_v95_database_is_safe_to_rerun`, `SEED_AT = 95`
  * `migration_095_on_a_populated_v94_database_is_safe_to_rerun` →
    `migration_097_on_a_populated_v96_database_is_safe_to_rerun`, `SEED_AT = 96`
  * `migration_096_on_a_populated_database_is_safe_to_rerun` →
    `migration_098_on_a_populated_database_is_safe_to_rerun`, `SEED_AT = 97`
* Every prose reference renumbered across 24 files — the forms `migration 09N`,
  `migrations 09N`, `Migration 09N`, `09N's`, `"09N mints one"`. Files touched:
  `migrations/045_session_participants.sql`, `migrations/096_people.sql`,
  `mcp/auth.rs`, `mcp/events_route.rs`, `mcp/mod.rs`, `mcp/settings.rs`,
  `mcp/token_cache.rs`, `mcp/tools/orchestration.rs`, `mcp/tools/session_ops.rs`,
  `mcp/tools/support.rs`, `mcp/tools/tests.rs`, `mcp/tools/tests_isolation.rs`,
  `service/orgs_tests.rs`, `service/sessions/lifecycle.rs`, `service/tasks.rs`,
  `service/view_scope.rs`, `service/view_scope_tests.rs`, `service/work/resume.rs`,
  `service/work/summary/tests.rs`, `store/people.rs`, `store/read_pool.rs`,
  `store/rows.rs`, `store/scale_fixture.rs`, `store/schema.rs`,
  `store/schema/tests_upgrade.rs`, `store/session_grants.rs`, `store/sessions.rs`,
  `store/tasks.rs`, `crates/fleet-hub/src/serve.rs`,
  `src-tauri/src/commands/sessions.rs`, `src-tauri/src/backend/tests_routing.rs`.
  **Deliberately NOT touched:** `crates/fleet-core/src/events.rs` (its
  "migration 095" is downloads), `crates/fleet-core/src/store/downloads.rs`,
  `schema.rs`'s `migration_94_creates_changesets_items_and_verdicts`, and
  `docs/superpowers/plans/2026-10-02-assets-m4-changesets.md`.

### 3. The person gate had to be extended over main's file downloads, at the `own` tier
This is the only place where M1's *semantics* had to grow, and it was not optional:
`service/downloads.rs` called `OrgScope::sees_row` and `OrgScope::sees_session`,
**which M1 T6 renamed to `sees_row_org_only` / `sees_session_org_only`**, so main's
code did not compile on M1's branch at all. A rename alone would have left a hole
M1 exists to close, and `scope_guard_tests::every_org_only_session_predicate_call_names_its_person_half`
would have failed for a call site with no person half.

The tier is **`own`**, not `Read`. See *Decisions taken on review* below for why.

* `crates/fleet-core/src/service/downloads.rs` — `visible`, `send`, `list`,
  `visible_row`, `remove` and `open_ready` now take `&ViewScope` instead of
  `&OrgScope`.
  * `visible` additionally takes `&Store` (the shape
    `trackers::tickets::item_visible` already uses) and asks
    **`ViewScope::may_own`** on the *session the file came out of*; when that
    session row is gone it falls back to
    `scope.org.sees_session_org_only(…)` with `DownloadRow.org_id`, which main
    recorded for exactly that case.
  * `send` has two arms: a **person** must pass `may_own`; a **per-host token**
    (the session's own Claude — main's headline use, `whoami` gives it its
    `session_id`) passes §4.4 clauses 1 and 2 through `sees_session_row`, which
    `may_own` deliberately excludes because the pane proof must never reach that
    tier. That arm is strictly tighter than main's host-only fence.
* `crates/fleet-core/src/mcp/tools/downloads.rs` — all three tools
  `caller.org_scope(&s)` → `caller.view_scope(&s)`.
* `crates/fleet-core/src/mcp/downloads_route.rs` — `GET /downloads/<id>` likewise,
  with a module doc saying this is the **second route M1 fences** (`/events` was
  the first) and the only one that serves file bytes.
* `src-tauri/src/commands/downloads.rs` — the four desktop commands
  `&OrgScope::All` → `&ViewScope::internal()`, matching every other local-mode
  command (`hosts.rs`, `tasks.rs`, `work.rs`, `trackers.rs`).
* `crates/fleet-core/src/scope_guard_tests.rs` — one new `ORG_HALF_SITES` row for
  the surviving org-only call in `downloads::visible`, naming `may_own` in the
  other match arm as its person half and recording why the residue is bounded.
* `crates/fleet-core/src/service/downloads_tests.rs` — the existing
  `a_host_sees_its_own_files_and_an_org_its_own` adapted to a
  `ViewScope::internal().with_org(host)`, plus a **new test**
  `only_the_owner_reaches_a_private_sessions_download`: one private session owned
  by `ada`, one download of `/home/ada/.claude/.credentials.json` out of it, and
  `visible` / `open_ready` / `remove` asked by a `watch` grantee, a `drive`
  grantee, another person, the session's own host token and a client bound to
  another org — all five refused, owner and `ViewScope::internal()` allowed.

### 4. The frontend halves sit side by side
`src/lib/events.ts` and `src/App.svelte` now carry both `download:changed` and
`grant:changed`. No M1 frontend logic changed; only the union had to be hand-closed
(the `;` on the `RowEvent` union).

### 5. `CLAUDE.md`'s known-flake paragraph (M1's own addition) corrected
Main's `e1c0b0d3` removed the *serialisation* of `work::scale_tests::*` (each test
now gets its own fixture copy) but not their wall-clock budgets, and `dea92962`
lifted the file-based upgrade test's budget **on Windows only**. The paragraph now
says both, so M1 stops attributing the wrong thing to the wrong fix.

### 6. `scale_tests.rs` structure
See CONFLICTS — M1's `scale_work_view` single test is gone; the six `#[test]`s in
`mod scale_work_view` are main's, carrying M1's `ViewScope` types.

---

## MIGRATIONS

**Yes, they collided again.** Main shipped two migrations in the window:

* `094_changesets.sql` (Assets M4)
* `095_downloads.sql` (file downloads)

M1's four, which had already been renumbered once (086–089 → 094–097), are now:

| was | **is now** | file |
|---|---|---|
| 086 → 094 | **096** | `crates/fleet-core/migrations/096_people.sql` |
| 087 → 095 | **097** | `crates/fleet-core/migrations/097_session_owner.sql` |
| 088 → 096 | **098** | `crates/fleet-core/migrations/098_session_grants.sql` |
| 089 → 097 | **099** | `crates/fleet-core/migrations/099_tasks_detach.sql` |

The renames are `git mv`s; **not one byte of SQL changed**. The three sets touch
disjoint tables — main's 094/095 are `changesets` / `changeset_items` /
`asset_triage_verdicts` / `downloads`; M1's are `people`, `client_tokens`,
`sessions`, `session_grants`, `tasks` — so renumbering is the whole of it.
`LATEST_SCHEMA_VERSION` is derived (`MIGRATIONS[len-1].version`) and
`migrations_are_contiguous_from_one` is satisfied: 1..=99 with no gap and no
duplicate.

**No `repair_skipped_main_migrations` arm is needed, and none was added.** The
previous scout left this open; it is answered: no `fleet-hub` binary was ever built
from an M1 worktree, and the branch was first pushed on 2026-10-04, so no database
anywhere has ever recorded 86–89 or 94–97 for M1's scripts. The renumber alone is
correct. The comment above main's 086 block in `store/schema.rs` now says this in
the source, replacing the "M1 has no repair arm yet" note.

---

## VALIDATION

The repo now prescribes **`scripts/verify.sh`** (main's `cfd22efc`). CLAUDE.md's
"Validation ladder" section is the authority:

```
1. cargo fleet-fast-check        # inner loop, ~15 s
   pnpm check
2. scripts/verify.sh             # before a commit (fmt, lint, the touched tests)
3. scripts/verify.sh full        # before a push (= ci-local.sh, narrowed)
```

Results, verbatim, in the order run:

<!--VALIDATION-->

---

## ADOPT

1. **`scripts/verify.sh` replaces M1's hand-rolled ladder — adopt it.** It is the
   repo's own command now, it reads the diff against the merge base and runs each
   needed check **once** in the ladder's single package selection, and
   `scripts/verify.sh full` is `ci-local.sh` narrowed to the touched jobs. There is
   also `scripts/verify.sh --dry-run` to print the plan and `scripts/verify-test.sh`
   for the script's own tests. The pre-commit hook was reworked to not duplicate it.
2. **`cargo fleet-fast-check` / `cargo fleet-check` / `cargo fleet-lint` are the
   only selections to use.** CLAUDE.md now spells out why: any other selection
   (`-p <crate>`, bare `cargo build`, `cargo check` without `--all-targets`) makes
   cargo compile a second copy of fleet-core's whole dependency graph. `fleet-lint`
   ⊇ `fleet-check`, so running both after an edit wastes ~22 s.
3. **`TMPDIR=/dev/shm/...` is still needed and is now written down** — that
   paragraph is M1's own and survived the merge; main does not have it. Keep it.
4. **The upgrade-test "flake" is NOT fixed for M1's box.** `dea92962` added
   `FILE_OPEN_BUDGET` = 30 s **on Windows only**; on Linux
   `opening_a_pre_work_graph_file_upgrades_it_within_budget` and both in-memory
   chains still hold `CHAIN_BUDGET` = 5 s. So M1 may keep treating a Linux
   `CHAIN_BUDGET` failure under load as environmental — but must not cite main's
   commit as the fix, and must not dismiss it on Windows any more.
5. **`work::scale_tests::*` no longer queue.** `e1c0b0d3` gave each test its own
   `copy_for_test()` of the fixture (and did the same for the desktop crate's
   `store()`, 33.3 s → 3.2 s). Their 3,000 ms budgets are unchanged, so they stay
   load-sensitive — but "they serialise" is no longer true and should stop being
   the explanation.
6. **A test that reads a file outside its crate must read it at run time**
   (`repo_files::read`), never `include_str!` — CLAUDE.md's new rule, with a
   measured reason (~26 s of recompilation per edit vs ~0.4 s). M1 has several
   cross-file tests (`hub_verdicts.generated.json`, `src/lib/*.ts` mirrors);
   check they follow it.
7. **New CI jobs**: `changes` (skips build/test for prose-only PRs), a separate
   `clippy`, and `windows-bundle`. macOS and Windows clippy now run in parallel
   with their tests.

---

## CHANGES M1's DESIGN

Each of these is a surface main added that M1's own machinery has an opinion about.

### D1 — Three new session-addressed MCP tools the person gate had never seen
`send_file`, `list_downloads`, `remove_download`
(`crates/fleet-core/src/mcp/tools/downloads.rs`), all `Access::Client`, all
originally `caller.org_scope(&s)`. `send_file { session_id, path }` reads a file
**off the session's host at an unconstrained absolute path** and copies it to the
caller's devices; before this merge that meant any client in the org could pull a
file out of a session private to somebody else. **Closed at the `own` tier** (M1
CHANGES §3, decision Q2). **Work left for M1:** nothing mandatory — but if anyone
ever confines `send_file`'s path to the session's worktree, revisit the tier
(`Reach::Read` becomes defensible then, and the `repo_file` precedent becomes
true). The reasoning is written next to `visible` so it is inherited, not
rediscovered.

### D2 — A second HTTP route to fence — **done, with a test**
`crates/fleet-core/src/mcp/downloads_route.rs::handle_download` serves
`GET /downloads/<id>` behind `authorize`. M1 had fenced exactly one route,
`/events`, per frame. This one streams **file bytes**. It now goes through
`view_scope`, and `only_the_owner_reaches_a_private_sessions_download` holds the
three callers the review asked for — a watcher, a per-host token and an org-bound
client — against `open_ready`, the function the route delegates to. That is
deliberately a level BELOW the route: `handle_download` refuses a `host_alias`
caller on its first lines, and testing the gate rather than the early return means
deleting the early return would still not open the route. The module doc says so.

### D3 — A new `Client` tool on the host-token refusal list
`changesets` (`crates/fleet-core/src/mcp/tools/assets.rs`), plus
`list_downloads` / `remove_download`, joined `NOT_FOR_HOST_TOKENS` for main's own
reasons. M1's five sharing tools are in the same list for a *different* reason (a
per-host token proves no person). Both paragraphs are now in the doc; if M1 later
turns that list into two lists, this is the seam.

`changesets` itself names no session — it is fenced to the master or an unbound
full person device via `caller.is_person_device() && caller.mode == TokenMode::Full`
— so the person gate has nothing to add. But note it leans on
`Caller::is_person_device`, which is **M1's own** predicate: main is now a consumer
of M1's auth vocabulary in a path M1 did not write.

### D4 — Four new routed desktop commands
`list_downloads`, `send_file`, `remove_download`, `save_download`
(`src-tauri/src/commands/downloads.rs`, rows in `backend/verdicts.rs`). All four
carry verdicts already; `save_download` routes to `list_downloads` and then streams
`GET /downloads/<id>`. M1 owes nothing to the verdict table here, but the local-mode
scope had to become `ViewScope::internal()` (M1 CHANGES §3).

### D5 — A new row event and kind
`download:changed` / kind `download`, in `HOST_BOUND_HIDDEN_KINDS` beside M1's
`grant`. `EVENT_NAMES` is 28 and `EVENT_KINDS` 16. M1's replay refusal and per-frame
fence in `events_route.rs` now have one more kind to be right about; the kind is
hidden from host- and org-bound streams, which is the same answer M1 gave `grant`.

### D6 — Two new migrations and a changed `schema.rs`
094/095 above, plus `store/changesets.rs`, `store/downloads.rs`, new rows in the
schema's table list, and `SyncImpact.cards`. M1's `store/schema.rs` neighbours
(the `already_applied` guards, the renumber comment, `repair_skipped_main_migrations`)
all live in the same file and conflicted; resolved as described.

### D7 — `CONTRACT_REVISION` is no longer M1's to set alone
Main bumped it for a feature of its own. M1's bump is now 8 and the two reasons are
stacked in the revision history. Any further main merge before M1 lands may do this
a third time: **the number is the first thing to check.**

### D8 — New settings
`downloads.max_file_mb`, `downloads.max_total_mb`, `downloads.keep_secs`,
`catalog.auto`, `catalog.auto_push`, with their Settings pages and generated docs
(`docs/settings-reference.md`, `docs/page-catalog.json`,
`src/lib/pages/registry.generated.json`). All merged clean; listed so M1 knows the
settings surface moved.

### D9 — `Store::client_has_any_catalog_grant`
New in `store/clients.rs` for the `changesets` pre-check. Reads `client_tokens`
joined to `client_catalog_grants` under `LIVE_GRANT_ELIGIBLE` — a table M1's
`people` migration also alters (`client_tokens.person_id` and its auth-epoch
trigger). No conflict, but they are the same rows.

### D10 — The GC sweep gained a stage
`service::gc` now calls `service::downloads::sweep` and reports
`swept_downloads`. Unscoped, correctly (it is the GC, not a caller).

---

## OPEN QUESTIONS

### Q1, Q2 and D2 are decided
See *Decisions taken on review* above. In short: `send_file`, `list_downloads`,
`remove_download` and `GET /downloads/<id>` are the **`own` tier**; a download
whose session row is gone keeps the **org-only** answer, and that residue is
bounded by the tier above it; the route has its test.

### Q3 — one follow-up, for whoever owns downloads
**Confine `send_file`'s path to the session's worktree.** `parse_stat` accepts any
absolute path (`path.starts_with('/')`) with no root to canonicalise against, which
is the whole reason the tier had to be `own` rather than `Read`. If the path is
confined, a `watch` grantee saving `out/report.pdf` from a session they can already
read in `FileViewer` becomes defensible again, and `downloads::visible` should move
back to `sees_session_row`. This is deliberately NOT done here: it changes what
main's feature does, not who may call it.

A second, smaller one: `FileViewer.svelte`'s "⤓ Send to downloads" button is now
backend-refused for a non-owner, so it will show an error rather than being hidden.
Gating it on `accessOf(session)` is a one-line UI improvement M1's frontend half
could take.

### Q4 — The verdict-count sentence in `docs/hub.md`
It is generated, and the generator is the authority; I wrote a placeholder and let
`REGEN_HUB_VERDICTS` produce the real numbers. If a future merge conflicts there
again, do not hand-resolve it — regenerate.
