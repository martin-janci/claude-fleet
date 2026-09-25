# Work graph M8: the phone (plan)

**Date:** 2026-09-24
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M8
**Design:** `../specs/2026-09-24-work-graph-design.md` §0 (work exists before any tracker; hybrid grouping)
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md`, C19 (gate on the hub's tools, not the contract) and the UX review (the phone is a pager)
**Depends on:** M1b (`SessionRow.work`, `work` / `work_link`), M2 (resume), M3 (tickets, `start`)
**Uses when present:**
- M4: `work_suggested`, `confirm` / `reject`
- M5: org scope on client tokens
- M7: tidy and reopened

**Repos:**
- `fleet-mobile` (Kotlin Multiplatform): almost all of the work.
- `claude-fleet`: two small hub-side changes.

## Goal

> From the phone you can see what each session is working on, triage by
> work, confirm or reject a guessed link, and start or resume work on a
> ticket. The phone never becomes a second brain: it shows what the hub
> decided and calls the same tools the desktop does.

**Value line:** a push says *PAY-7 is waiting on you*. The phone opens on the
PAY-7 group; you answer, then pick PAY-9 from *My work* and tap **Start here**.

## Facts this plan builds on (verified 2026-09-24)

**fleet-mobile at `origin/main` 5752abf**

- **Transport.** All calls go through `net/HubClient.kt` `call(tool, args, …)`.
  - Ordinary calls go to `/mcp/json`.
  - `new_session` is in `LIFECYCLE_TOOLS` and goes to `/mcp` (SSE framing, a 330 s deadline).
  - There is **no `tools/list` discovery**. Additive features are gated on the hub's version string (`semverAtLeast(hubVersion, HUB_VERSION_KEYS)` in `ui/Blocked.kt`).
- **Contract.** `MAX_HUB_CONTRACT = 4` (`net/HubContract.kt`). The contract revision comes from the `/events` `ready` frame, and a mismatch refuses the whole connection. **M8 must not bump the contract.**
- **Session model.** `model/SessionRow.kt` parses 28 fields with `ignoreUnknownKeys = true`. It has `tags` and **no `work`**.
- **Events.** `SNAPSHOT_EVENT_KINDS = ["session","host","project"]` (`data/FleetSnapshot.kt`).
  - Rows are replaced whole by `id`.
  - A merge keeps `isController` from the existing row, because event payloads do not carry it.
  - `ProjectRow.kt` warns against modelling fields that an event payload cannot carry.
- **Grouping.** `ui/SessionsViewModel.kt` `groupSessions(…)` is a pure function: host, then project, then recency.
  - `FilterRow` has a *needs attention* chip and a *host* chip.
- **New session.** `ui/NewSessionViewModel.kt` (PR #31) has a reachable-host picker and a project picker, and calls `new_session` in the fleet scope.
- **Permissions.** `Credentials.canWrite` (`mode == "full"`) gates every write in the UI. The rule is *never call a tool the token cannot use*.
- **Allowed tools.** `jvmTest/.../ToolsTheAppMayCallTest.kt` scans `HubClient.kt` for `call("name"` and fails on any tool outside `permitted`.

**claude-fleet at the M4 head**

- `list_sessions { view: "phone" }` keeps `PHONE_SESSION_FIELDS` (`mcp/tools/views.rs`). It already contains `work`, but **not `work_suggested`** (M4).
- **`work`** is `Access::Client`, readonly. **`work_link`** is `Access::Client`, not readonly, `Deadline::Lifecycle` (`start` and `resume` create sessions).
  - `work` actions: `links | context | resume_plan | purge_impact | tickets | lookup | trackers`.
  - `work_link` actions: `link | reject | unlink | resume | start | confirm | trust_project`.
  - `action` is a **free string** in the schema, so a client cannot discover which actions exist.
- **`tools/list`** is filtered per caller (`present::visible_to`). A readonly token does not see `work_link`.
- **`WorkSummary`** has these fields: `link_id`, `item_id?`, `key?`, `title`, `source`, `state`, `strength?`, `rule?`, `preselected`, `suggestions`, `status_category?`, `status_name?`, `url?` and `unavailable`. Absent optional fields are skipped.
- **Events.**
  - `/events` ignores unknown `?kinds=` values, so asking for `work` from an older hub is harmless.
  - `work:item`, `work:tracker` and `work:tracker_removed` frames exist (M3). A host-bound stream never gets them; a paired client token does.
- **Primary work changes.** A change to a session's primary work arrives as `session:updated`, so the list needs no `work` subscription. Only the ticket list does.

## Design decisions

1. **Gate on tools, not on the contract (C19).** On each `ready` frame the phone calls `tools/list` once (plain MCP, same auth) and keeps the set of tool names for that connection.
   - `work` present: show chips and grouping.
   - `work_link` present: show Confirm, Reject, Start and Resume. This also covers a readonly token for free, because the hub hides the tool.
   - **Action-level gating:** the hub's `action` is a free string.
     - Preferred: M8.0 turns it into a schema `enum`, measured against `BUDGET_BYTES`. The phone then reads the enum.
     - Fallback for an older hub: a call answered `E_INVALID` with "unknown … action" marks that action absent for the connection and hides its button.
   - The version-string gate stays only for hubs so old they fail `tools/list`.
2. **No new wire shapes.**
   - The phone reads `work` / `work_suggested` from session rows and the existing `work` action replies.
   - It never re-derives a work key. `work_keys.ts` stays desktop-only, because the hub now stamps `work` on the row. A row without `work` sits in its project group, as on the desktop (hybrid grouping, no "Unclassified").
3. **Event safety.** `work` / `work_suggested` are on every `session:updated` payload (they come from `SESSION_COLUMNS`), so a whole-row replace is correct.
   - Test this against a real store-row fixture, not just the phone view. `is_controller` taught that lesson.
   - If a payload ever lacks them, the merge keeps the old values, as it does for `isController`.
4. **Starting work is a lifecycle call.** `work_link` joins `LIFECYCLE_TOOLS`: `/mcp`, SSE framing, the 330 s deadline, and the fleet scope, like `new_session`. Backing out never cancels a start.
5. **Resume on the phone is host-picker only.**
   - No brief editing; the hub's default brief is used.
   - A live session for the key means **Jump**, never a second session. The hub refuses one with `E_EXISTS` naming the session, and the phone opens that session.
6. **Pager first.** The ticket list is a sheet from the Sessions tab, not a fourth tab. *My work* is a filter chip. The tab set does not change.

## Tasks

Each task is one reviewable PR. Hub tasks run in the **Worker** environment (cargo). Phone tasks run in `fleet-mobile`: `./gradlew :shared:jvmTest` as the fast gate, and `./gradlew build` as CI runs it.

### M8.0: hub side (claude-fleet)

- Add `work_suggested` to `PHONE_SESSION_FIELDS`, with a doc line naming its reader (the Confirm / *Not this* chip). Update the pinning tests in `mcp/tools/tests.rs`.
- Make the `action` parameter of `work` and `work_link` a schema `enum`, generated from the same list `parsed_action` matches, so the two cannot drift.
  - Measure the change and record it at `BUDGET_BYTES`.
  - `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
  - If the budget cost is refused, skip this; the phone's fallback covers it.
- Test: `tools/list` for a readonly client token lists `work` and **not** `work_link`. This pins the gate the phone relies on.
- No `CONTRACT_REVISION` bump.

### M8.1: model and transport (fleet-mobile, `shared`)

- **Model.** Add `model/WorkSummary.kt` (`@Serializable`, every field defaulted) and `SessionRow.work` / `workSuggested`.
  - Add `status_category` → a `StatusCategory` enum with an `Unknown` variant, because new wire enums need one.
- **Tool discovery.** `HubClient.toolNames()` calls `tools/list` and returns the set of names plus each tool's `action` enum when the schema has one.
  - `FleetRepository` calls it on every `ready`.
  - It exposes `StateFlow<HubCapabilities>` (`work`, `workLink`, `actions: Map<String, Set<String>>`).
- **Typed wrappers** in `HubClient.kt`:

  | Wrapper | Call |
  |---|---|
  | `workTickets(view)` | `work { action: tickets, view }` |
  | `workLookup(keyOrUrl)` | `work { action: lookup }` |
  | `workResumePlan(key)` | `work { action: resume_plan }` |
  | `confirmWork(sessionId, linkId)` | `work_link { action: confirm }` |
  | `rejectWork(sessionId, linkId)` | `work_link { action: reject }` |
  | `startWork(key, hostAlias, projectId?)` | `work_link { action: start }` |
  | `resumeWork(key, mode = "last", hostAlias?)` | `work_link { action: resume }` |

  `work_link` joins `LIFECYCLE_TOOLS`.
- **Allowed tools.** `ToolsTheAppMayCallTest`: add `work` and `work_link` to `permitted`. `work_admin` goes into `forbidden`, because it is master-only and the phone must never name it.
- **Events.** Add `"work"` to `SNAPSHOT_EVENT_KINDS`.
  - `FleetSnapshot.applying` handles `work:item` (upsert into a `tickets` cache keyed by `item_id`) and `work:tracker_removed` (mark that tracker's tickets unavailable).
  - The paired hub test pins the filter and the applier to the same list.
- **Tests.**
  - `HubClientTest` for each wrapper, with argument shape and the lifecycle mount.
  - `SessionRowTest` with a full store-row payload carrying `work` and `work_suggested`, plus one without them.
  - `FleetSnapshotTest`: a `session:updated` without `work` keeps the old `work` (decision 3), and `work:item` upserts.
  - `HubContractTest`: no change.

### M8.2: sessions list (fleet-mobile)

- **Grouping by work.** `groupSessions` gains `byWork: Boolean`. With it on, a host's project groups become:
  - work groups: key + title, a status dot, and the *needs attention* count first;
  - then project groups for the rest.

  It stays pure, with table tests mirroring the desktop's `buildSessionsByWork` cases: one key across two projects; a row with a suggestion only, which stays in its project; a key with an unavailable ticket.
- **`FilterRow`.**
  - A **by work** toggle, persisted in `Prefs`.
  - A **My work** chip: sessions whose `work.item_id` is in the cached `tickets(view = "mine")`. It is hidden when the hub has no tracker (empty `trackers`).
- **Row.** `SessionRowItem` shows a key chip. A suggestion shows a dotted outline, and an unavailable ticket shows strike-through. The desktop vocabulary is in the M4 docs.
- **Tests.**
  - `SessionsViewModelTest` for grouping and filters.
  - Extend the Compose device test `SessionsFilterLayoutTest` for the new chips at 320 dp width.

### M8.3: session screen (fleet-mobile)

- A ticket chip in `SessionBar`: key, status and title. Tap opens a small sheet with:
  - the title, status and URL, opened in the browser;
  - **Why**: `rule` and `source`, as plain words;
  - for a suggestion: **Confirm** and **Not this**, shown only when `canWrite` and `work_link` is present with those actions;
  - **Clear**, which is `work_link unlink`, under the same gate.
- The overflow menu gains **Set work…**, which accepts a key or pasted URL, runs `workLookup` and then `link`.
- **Tests.** `SessionViewModelTest`:
  - Confirm and Reject call the right `link_id`.
  - A readonly token sees no buttons.
  - An `E_INVALID` "unknown action" hides that action for the connection.

### M8.4: tickets sheet, Start here and Resume (fleet-mobile)

- A **Tickets** sheet opens from the Sessions tab's app bar.
  - Sections: *My work*, *Current sprint*, *Recent*, served by `work tickets` from the hub cache.
  - A search field calls `workLookup` for a key or pasted URL.
- **Per ticket:**
  - If the ticket has a live session: **Open**, which jumps to it.
  - Otherwise **Start here** reuses the New Session host and project pickers, with the project prefilled from the hub's default. Unlike M3's desktop dialog, there is no ticket brief preview.
  - If the ticket has past work (from `resume_plan`): **Resume**, with a host picker only.
  - On `E_EXISTS` the phone opens the named session.
- **Navigation.** `Screen.NewSession` gains `ticketKey?`, and `Navigator.created` opens the session exactly as for a plain new session.
- **Tests.**
  - `NewSessionViewModelTest`: ticket mode, the default project, and `E_AMBIGUOUS` offering candidates.
  - A Jump on `E_EXISTS`.
  - Resume uses the `last` mode and passes the host override.

### M8.5: docs

- **fleet-mobile.**
  - README "What it does": add work.
  - `skills/fleet-mobile-repo/SKILL.md`: the tool-gating rule (tools/list, action enum, E_INVALID fallback) and the fact that `work` / `work_link` are permitted.
  - The design doc appendix.
- **claude-fleet.** `docs/hub.md` → *Pair a phone*: what the phone can do with work. Update the roadmap's M8 status and Revisions.

### M8.6: the phone catches up (M4.6, M5, M9.1–M9.3)

Added 2026-09-25, after M8.1–M8.5 landed and the hub moved on. Four parts,
each gated on the hub's `tools/list` like the rest of M8. No contract bump.

- **M8.6.0, hub side (claude-fleet).** `org_id` joins `PHONE_SESSION_FIELDS`.
  - It is needed for a session with no work, whose org nothing else carries.
    Each `WorkSummary` already carries its own `org_id`.
  - Update the view pin and put an org on the real-key fixture. `/events`
    frames are whole rows, so they already carry it.
- **M8.6.1, M4.6 on the phone.**
  - `workWhy` learns `agent_inferred`, with the desktop's words:
    "suggested by Claude when asked" (or "named …" once confirmed), plus the
    rule, and no strength suffix.
  - The chip, the ring and Confirm / Not this need nothing new: an
    `agent_inferred` link is only ever a pre-selected suggestion (R11).
  - Unknown strengths (`inferred` today) are shown as text, never refused.
- **M8.6.2, Today (M9.1).**
  - A **Today** sheet opens from the Sessions app bar when the hub lists
    `work today`. It calls `work { action: today, since: <local midnight> }`.
  - Sections in the desktop's order: *Waiting on me*, *In progress*,
    *Shipped*, *Stale*.
  - Tapping a session opens it. It re-reads on open, on pull, and (debounced)
    when a `session:*` or `work:item` frame arrives while it is open.
  - **Copy standup** copies the desktop's `standupText`, ported with its
    test cases by name: `bucketOf`, `sessionPhrase`, `groupLabel`, and the
    scoping rule.
  - Readonly and full tokens alike.
- **M8.6.3, past work and handover (M2, M9.2, M9.3).**
  - Tapping a ticket in the Tickets sheet also reads `work card` (cache
    only): the acceptance criteria, or the excerpt when there are none.
  - It lists the resume plan's `candidates` as **past work**: name, host,
    when it ended, branch, PR, and the number of conversations.
  - The session screen's work sheet gains **Ask for a handover**
    (`work_link handover`). It is shown only when all of these hold:
    - a full token;
    - the hub lists the action;
    - the session has a primary key;
    - the session is running.
  - The answer is asynchronous. The sheet follows the session's
    `handover_requested` / `handover_written` / `handover_missing` /
    `handover_send_failed` timeline frames and says which one arrived.
    `E_NOT_ALIVE`, `E_EXISTS` and `E_INVALID` are said in words.
- **M8.6.4, orgs (M5).**
  - When the hub lists `work orgs`, it is read once per connection (like
    *My work*) into id → name and colour.
  - A row's org is its own `org_id`, else its work's `org_id`. A ticket's org
    is its tracker's `org_id`.
  - With two or more orgs in view:
    - work group headings and ticket rows carry an org label;
    - the Sessions tab gains an org filter chip, which also scopes Today.
  - A client token sees every org (`OrgScope::All`), so this is a view
    filter, not a fence.
- **Tests.**
  - `standupText` / `bucketOf` / scoping cases, by name.
  - `workWhy` for `agent_inferred`.
  - Today refresh and gating.
  - Card and past work parsing, from hub-shaped fixtures.
  - The handover button's gates and its timeline states.
  - Org resolution (row, then work) and the filter.
  - `ToolsTheAppMayCallTest` is unchanged: `work` and `work_link` only.

## Acceptance (manual: a hub with Jira connected and a paired phone)

1. **Chips.** A session on `pay-7-refund` shows a **PAY-7** chip on the phone. By work groups it under PAY-7 with the title.
2. **Suggestion.** A session given a ticket URL in its prompt shows a suggested chip. **Not this** removes it, and it never comes back (M4 R9). With a readonly token the buttons are absent.
3. **Start.** *My work* → PAY-9 → **Start here** on host *hetzner* creates the session, opens it, and shows the chip. Starting it again jumps to it instead.
4. **Resume.** Kill the PAY-9 session on the desktop. The phone's PAY-9 now shows **Resume**, which recreates it on the chosen host.
5. **Old hub.** Against a hub without `work`, the phone shows no chips, sheet or chip filters, and nothing errors.
6. **Isolation (with M5).** A phone paired with an org-scoped token sees only that org's tickets and work groups.

7. **Today (M8.6).** The Today sheet lists the same groups as the desktop's
   Today view at the same moment. **Copy standup** pastes the same text.
8. **Handover (M8.6).** With a full token, **Ask for a handover** on a linked,
   idle session ends in "Handover written" once the turn stops. The note
   then shows in the desktop's context. With a readonly token the button is
   absent.
9. **Orgs (M8.6).** With two orgs configured, the org chip narrows Sessions
   and Today to one org. A session with no work still lands in its org.

## Risks

| Risk | Mitigation |
|---|---|
| A `session:updated` payload without `work` blanks the chip | Payloads come from `SESSION_COLUMNS`, and a fixture-based test pins this; the merge keeps the old value (decision 3) |
| Contract bump locks out phones | None needed; gating is by tools and actions (C19) |
| Starting work is slow (worktree creation) | Lifecycle mount and deadline, run in the fleet scope, like `new_session` |
| The phone drifting from the desktop's grouping rules | Table tests reuse the desktop's `buildSessionsByWork` cases by name |
| A readonly token showing write buttons | Gated twice: `canWrite` and the hub's `tools/list` |

## Decisions (defaults if unanswered)

| # | Question | Default |
|---|---|---|
| new | Make `work` / `work_link` `action` a schema enum (≈ +300 B budget)? | Yes; the phone falls back to `E_INVALID` probing without it |
| new | Tickets as a sheet or a fourth tab? | Sheet; the phone stays a pager |
| new | Edit the brief on the phone? | No; the hub's default brief (desktop keeps editing) |

## Revisions

- **2026-09-24, M8.0 landed** on `claude/cloud-fleet-work-graph-m8`
  (stacked on M4). Verified with `cargo fmt`, `clippy -D warnings`
  (workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub; only the
  four chmod tests that fail as root on `main` fail) and `pnpm check` /
  `pnpm test`.
  1. **`work_suggested`** is in `PHONE_SESSION_FIELDS`, its doc bullet
     naming the reader (the suggested chip's Confirm / *Not this*). The
     pinning test's fixture now carries a suggestion: the field is skipped
     when there is none, so without one the test could not tell it from a
     misspelling.
  2. **The action enum landed**, cheaper than the ≈ +300 B estimate:
     **+66 B** (68,385 → 68,451; `BUDGET_BYTES` 68,551), because the two
     doc lines that listed the actions by hand were cut to "Default links."
     / "The decision." rather than kept beside the enum (every parameter
     must keep a description). The source is one table per tool in
     `service/work/mod.rs` (`WORK_ACTIONS`, `WORK_LINK_ACTIONS`): the
     parsers (`WorkArgs::parsed_action`, the new
     `WorkLinkArgs::parsed_action` / `WorkLinkAction`) look names up in it,
     the schema's `enum` is generated from it (`schemars(schema_with)`),
     and the MCP `work_link` dispatch and `service::work::work_link` now
     match on the enum instead of strings. Tests: every table entry parses
     to its variant and every variant is in the table (an exhaustive match
     makes a new action fail to compile until it is named), the schema's
     enum equals the table, and the refusal still names every action.
     An unknown action still answers `E_INVALID` "unknown … action" (the
     hub does not validate against the schema), so decision 1's fallback
     for an older hub is unchanged. `control-api-reference.md` did not
     change (it does not render parameters).
  3. **The gate test** (`a_client_token_is_served_work_and_work_link_by_mode_and_never_work_admin`):
     a readonly client token is served `work` and not `work_link`, a full
     one both, neither ever `work_admin`; and the served `action` enums
     equal the tables.
  4. **Correction to decision 3 (for M8.1).** An absent `work` /
     `work_suggested` on a row MEANS none, on both paths: `list_sessions`
     answers null-stripped, `/events` strips nulls before broadcasting
     (`events.rs`), and `work_suggested` is `skip_serializing_if` besides.
     So a `session:updated` without them is a session whose link was
     cleared or whose suggestion was decided — the phone must replace the
     row whole for these two fields and must NOT keep the old value the way
     it keeps `isController` (which is absent because events never carry
     it). `FleetSnapshotTest` should pin "an update without `work` clears
     the chip", the opposite of the task text above.
  5. No `CONTRACT_REVISION` bump, no new tool, no verdict change.
- **2026-09-24, M8.1 landed** in fleet-mobile on
  `claude/cloud-fleet-work-graph-m8` (45f140a), stacked on `main` 5752abf.
  Verified with `./gradlew :shared:jvmTest` (all green) and the iOS main and
  test compiles (`compileKotlinIosArm64`, `compileTestKotlinIosSimulatorArm64`,
  no `e:` lines); `./gradlew build` was not run (no Android SDK in the
  container). Deviations from the task text, and why:
  1. **Decision 3 as corrected above**: an update without `work` /
     `work_suggested` clears them; `FleetSnapshotTest` pins that against
     a row captured from the hub's serializer, and pins the opposite for
     `work:item`'s non-columns (`live_session_ids`, `views`), which ARE
     carried over, as `isController` is.
  2. **`HubCapabilities`** also records actions the hub refused as unknown
     (`forgetAction`, `HubError.isUnknownAction`), so M8.3's fallback is a
     one-liner; `known` stays false for a hub that cannot answer
     `tools/list`, which never fails the connection (a 401 still routes to
     Pair). `tools/list` runs alongside the re-list and on a resumed stream.
  3. **Wrappers beyond the table**: `workTrackers` (M8.2's "My work" chip
     hides without a tracker), `unlinkWork` and `linkWork` (M8.3's Clear
     and Set work…). All `work_link` calls ride the lifecycle mount.
  4. **`HubError.Tool.details`** carries the refusal's structured details:
     `existingSessionId` for `E_EXISTS` (M8.4's Jump), candidates for
     `E_AMBIGUOUS`. A details object that would repeat the token is dropped.
  5. **The ticket cache** lives in `FleetSnapshot.tickets`, seeded by a
     screen (`FleetState.remember`) and kept current by `work:item`; a
     re-list keeps it (there is no all-tickets call worth making on every
     reconnect).
- **2026-09-24, merged over M5 (#264).** M5 had grown its own action tables
  in `service/work/mod.rs` — `WORK_ACTIONS` with `scopes` / `orgs` /
  `org_suggestions`, `WORK_LINK_ACTIONS` as plain names that the org
  isolation matrix and the desktop's routed commands check — and threaded
  the org scope through every `work_link` branch. The merge keeps all of
  that as M5 wrote it and generates M8.0's schema enums from those tables;
  M8.0's `WorkLinkAction` enum and its string-to-enum dispatch refactor were
  dropped rather than re-threaded through M5's fences (no behaviour depended
  on them). The enums cost +72 B over M5 (69,171; `BUDGET_BYTES` 69,271).
- **2026-09-24, M8.2 landed** in fleet-mobile on the same branch (42c2868),
  stacked on M8.1. Verified with the full `./gradlew build` (0 `e:` lines;
  with an Android SDK installed in the container, so M8.1 is now
  build-verified too) and `:androidApp:compileDebugAndroidTestKotlin`; the
  extended `SessionsFilterLayoutTest` itself runs only in CI's emulator job.
  Deviations and choices:
  1. **Work groups are per host**, inside each host group and before its
     projects, as this plan says — unlike the desktop, whose work groups
     span hosts. The table tests are named after `buildSessionsByWork`'s
     cases, with "filters rows but still reports every keyed session"
     becoming "filters rows and groups per host".
  2. **Order:** groups with someone waiting first (the phone's one
     severity), then the most recently active (the desktop's tie order),
     then the key.
  3. **The heading** takes status and title from the ticket cache when it
     has the item, since a `work:item` frame does not restamp session rows.
  4. **"My work"** loads through a new `WorkActions` seam (trackers, then
     `tickets(mine)`) when the hub serves `work`, and again on a pull to
     refresh; the answer seeds the ticket cache for M8.4. A failure keeps
     what was known. The chip appears only once "mine" has answered.
  5. **The row chip** follows design §0.3.1: solid when a person or agent
     linked it (or `strength: explicit`), a ring when the hub linked it by
     itself, dashed with `?` for a suggestion (only on a row without a
     link), struck through when unavailable; hidden under a work group's
     own heading.
- **2026-09-24, M8.1–M8.5 merged** into fleet-mobile `main` as
  martin-janci/fleet-mobile#32 (merge `9b9212e`; CI green on JVM, the
  Android emulator and the iOS simulator). It was built in parallel with the
  M8.1 / M8.2 entries above, which describe fleet-mobile's
  `claude/cloud-fleet-work-graph-m8` branch; that branch is superseded and was
  not merged. #32 follows the corrected decision 3 (an update without `work` /
  `work_suggested` clears them). Its own departures from the task text are in
  the fleet-mobile design appendix: the session screen's work lives in a
  `SessionWorkViewModel`; Start here leaves the project to the hub unless one
  is picked (`E_AMBIGUOUS` narrows to the candidates); the resume plan is read
  when a ticket is tapped; *My work* is read once per connection.
- **2026-09-25, M8.6 planned and M8.6.0 landed** on
  `claude/cloud-fleet-work-graph-m8`. The phone catches up with M4.6
  (`agent_inferred`), M5 (orgs) and M9.1–M9.3 (Today, the ticket card, past
  work, handover on demand). The hub side is one field: `org_id` in
  `PHONE_SESSION_FIELDS`, with no contract bump and no budget change (the
  view is not part of the tool schema). A survey of the client-token surface
  found:
  1. Client tokens are never org-scoped.
  2. `today`, `card`, `orgs`, `scopes`, `resume_plan` and `links` are all
     `work` reads, open to readonly tokens.
  3. `handover` is `work_link`, so it is full-token only and not
     confirm-gated.
  4. There is no journal action. Past work is the resume plan's candidates,
     and the journal reaches a phone only inside `context`.
  5. No `work:link` / `work:journal` event exists. Link changes arrive as
     `session:updated`, and handover progress as `session:event`.
