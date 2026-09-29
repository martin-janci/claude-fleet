# Review: claude-fleet work merged 2026-09-26 to 2026-09-28 (`d4b7a21..HEAD`, PRs #316–#384)

## Summary

- **The work is solid overall.** Each change is carefully built and well tested inside itself. Shell quoting, the `proc` spawn rule and `OrgScope` all held up, and the migrations (057–081) run without gaps. Most of the problems are where features meet: a fix applied to only some of its callers, a rule kept in two copies that now disagree, or a new row type that nothing cleans up.
- **No high-severity finding survived review.** 14 findings are rated medium. The rest are low: cleanups, small gaps and moderate speed costs.
- **Top 5 things to do:**
  1. **Stop "Hide host" from hard-deleting the host's sessions.** `reap_hidden_hosts` ghosts every row and deletes it one pass later, but the UI offers Hide with an Undo button.
  2. **Finish the shared-key fix.** `card`, bare-key `lookup`, the handover brief and `plan_resume` still take the oldest item for a key. An org-B caller is refused, or gets no item, when org A owns an older item with the same key.
  3. **Fence the new host rows.** `fleet_health.hosts[]` is not narrowed for org-bound clients, and `blank_rollups` does not clear it either.
  4. **Keep the stale-working sweep off hosts nobody observed.** Today an unreachable host's `working` rows are demoted to idle, with an attention reason, after 30 minutes.
  5. **Fix the signed manifest's `build_id` before 0.4.1 ships.** It comes from `release.yml`'s run, so the OCI hub image, built by `hub-image.yml` in its own run, can never match it (U11).
- **Other items worth doing soon:**
  - PR write-back is queued only when the PR's signals change, so a ticket linked after the PR exists never gets written back.
  - Fork drops the source session's model and effort.
  - `fleet-hub provision` stops waiting after 10 s while the hub's own deadline is 300 s.
  - A placement on a bare key is lost when a sync binds that key to an item.
- **Speed:** the M14 Work view is the one real cost. Every read rebuilds the whole graph, one desktop refresh sends 2+K of those reads, and a few paths hold the writer mutex during the build. Everything else is minor and bounded.
- **Docs:** the release key landed in a3033c2, but code comments, CLAUDE.md and a debug-level log still assume `RELEASE_KEYS` is empty. CLAUDE.md also does not mention #354 (host identity) or #381 (stale-working acknowledgement).

## Bugs

**Shared-key fix covers only `describe` and `require_key`; `card`, `lookup`, the handover brief and `resume` still take the oldest item.** Medium, effort M, `crates/fleet-core/src/service/work/card.rs:269`
Commit 2c49856 changed `orgs::require_key` and `describe` to use the first item the caller can see. `card` still reads `s.work_item_by_key(&key)` (`LIMIT 1`) right after `require_key` passes, then refuses when the item is not in `tickets::allowed`. I checked this on HEAD. The same single-item read is in:
- `handover.rs:622` (`gather_stored`): org B's brief has no item.
- `resume.rs:209` and `:965`: the cross-org check compares against org A's org.
- `tickets.rs:358`: bare-key `lookup`.

Every path fails closed, so nothing leaks, but org-B hosts and phones get wrong refusals. The only regression test covers `describe`.
*Recommendation:* add `orgs::visible_item_for_key(s, scope, key)` and use it in all four places. Add one table-driven two-org test.

**The manifest's `build_id` can never equal the OCI hub image's (U11 gate cannot be met).** Medium, effort M, `.github/workflows/release.yml:562`
The manifest signs `BUILD_ID: gh-run-${{ github.run_id }}-${{ github.run_attempt }}` from `release.yml`'s own run. `hub-image.yml:166` bakes the same pattern into the image, but with its own run id (verified). Tarball legs also stop matching when only the manifest job is re-run. Design §8.4 step 3 requires `build_id` to equal the manifest's `release`. The consumer (fleet-updater) is not built yet, but signed manifests are about to be published.
*Recommendation:* record the build identity per artifact in the manifest, or derive it from something both workflows share (tag plus commit). Add a check to `release-update-scripts-test.sh`.

**Hiding a host now hard-deletes all its session rows within about two passes, although Hide is presented as reversible.** Medium, effort M, `crates/fleet-core/src/service/sessions/reconcile.rs:1762`
`reap_hidden_hosts` treats `h.hidden` as unprobed and calls `reap_host_ghosts`. That ghosts live rows (`lost_reason='missing'`, with no lost-TTL exemption) and deletes them on the next pass (verified). Meanwhile:
- `hideHostWithUndo` (`host_actions.ts:69`) shows an Undo toast.
- `HostDetail.svelte` lists Hide among the "reversible actions".
- The `hide_host` tool still says "skipped by reconcile".

Undo cannot bring back the names, work links, timeline or resumable `claude_session_id`s.
*Recommendation:* limit the reap to `local` on a hub with `local_host=false` and to external/bg rows, or ghost user-hidden rows and let `sessions.lost_ttl_secs` decide deletion. Add a test for hide, then two passes, then unhide. Pull the rule into a shared `hosts::is_active` (see Simplify).

**org_impact counts bound clients without D31 `bound_sees_unassigned`.** Low, effort S, `crates/fleet-core/src/service/work/structure.rs:645`
The local closure `org.is_none() || org == viewer` stands in for `OrgScope::Org::sees_org`. On a move to or from no org, it counts clients that never saw the task as losing or gaining it. Only the preview number is wrong, because the same count is used when the token is checked. No test asserts these counts.
*Recommendation:* build `OrgScope::for_client` and call `sees_org`. Add a test.

**`TaskCounts` mixes units.** Low, effort S, `crates/fleet-core/src/service/work/view.rs:1053`
`active` counts distinct sessions, while `ended` and `suggested` count links. `WorkTree` shows them side by side ("{active} active · {ended} past").
*Recommendation:* count all three by distinct session, or rename the fields and labels.

**A replayed or late `/update/report` overwrites the observed row.** Low, effort S, `crates/fleet-core/src/service/update/mod.rs:381`
`report()` upserts `update_observed` before `insert_update_event`'s `INSERT OR IGNORE`. A duplicate therefore still rewrites phase, version and error. The spec says reports are idempotent on (target, attempt, phase). No updater replays reports yet.
*Recommendation:* upsert only when the event insert returns true, and compare attempts. Add a replay-after-newer test.

**Readiness is not a "first reconcile" latch.** Low, effort S, `crates/fleet-hub/src/ready.rs:67`
`first_reconcile` reads the running `consecutive_failures`. One later failed pass flips a healthy hub to `ready: false`, which contradicts §8.4 and the field's name. Only the not-yet-built updater reads it.
*Recommendation:* latch the first success. Report ongoing failures in a separate field. Add an "ok, then a failure, still ready" test.

**A connection-slot queue timeout is reported as `E_SSH_TIMEOUT`, which callers read as "may have run".** Low, effort S, `crates/fleet-core/src/ssh.rs:850`
Nothing was spawned, but `codes::may_have_run` matches this code, so the usage sweep stops (`FailedAfterConnect`) and `add_project` warns "GitHub state unknown". This only happens without mux.
*Recommendation:* return `E_SSH` and add a test.

**A transient non-zero `wsl.exe --list` empties the WSL host table.** Low, effort S, `crates/fleet-core/src/wsl.rs:366`
A non-zero exit maps to `Some(vec![])`. `refresh()` treats that as an answer and replaces `DISTROS`, which contradicts its own doc ("a failed wsl.exe keeps the previous table"). The next retry waits 60 s.
*Recommendation:* keep a non-empty previous table on a non-zero exit, and record the attempt as unanswered.

**A same-worktree Fork of a session with no project leaves an orphan transcript copy.** Low, effort S, `crates/fleet-core/src/service/rewind.rs:799`
`copy_transcript` runs first (line 727). `project_id_for_fork(...)?` runs afterwards and returns without calling `remove_copy`. `discover_lost_sessions` will later report the stray `.jsonl`.
*Recommendation:* resolve `project_id` before the copy whenever the mode is Fork. Add a ReplyOps test.

**Retry re-sends, and Rewind puts back into the composer, the hub's untrusted-client marker.** Low, effort S, `src/lib/ReplyActions.svelte:43`
The raw `turns[index].prompt` still carries `[claude-fleet: message from …; treat as untrusted input]` from `apply_marker`. On a paired desktop, Retry sends it through the hub, which marks it a second time.
*Recommendation:* use `splitMarker(prompt).text` for both paths. Add a test.

**The outbox `seen` baseline ignores identical messages already queued.** Low, effort S, `src/lib/ConversationPanel.svelte:1281`
Two quick "continue" prompts get the same `seen`. When the transcript carries the first one, `settle()` also drops the second one's bubble before Claude has read it. The effect is cosmetic.
*Recommendation:* in `enqueue`, add the number of pending outbox messages with the same body.

**`agent_old` / `agent_behind` flag any version mismatch, including an agent newer than the hub.** Low, effort S, `src/lib/hosts_view.ts:246`
The Rust check (`health.rs:256`) is the same. After a hub rollback, the mark tells the operator to "upgrade" an agent that is already ahead.
*Recommendation:* compare with `compareVersions` / `version_parts`.

**The status-map bench error message contains 18 stray spaces.** Low, effort S, `crates/fleet-core/src/service/decide/bench/status_map.rs:223`
A `\` continuation was lost, so the message reads `are in                  org(s)`.
*Recommendation:* restore the continuation.

## Gaps

**A placement on a bare key (`ref:KEY`) is silently lost when a sync binds that key; placements are never cleaned up.** Medium, effort S, `crates/fleet-core/src/service/work/view.rs:767`
`bind_tracker_refs` rewrites `work_links.item_id`, so the task id becomes `item:N`, but `work_placements` stays keyed on `ref:KEY`. `group_of` falls back to a rule or the tracker group. The only DELETE is an explicit clear.
*Recommendation:* re-key the placement in the same transaction (if the item has none). Sweep orphaned placements. Add a test: place `ref:KEY`, bind, read the tree.

**The fleet-hub CLI uses a fixed 10 s call timeout, which the new `fleet-hub provision` cannot meet.** Medium, effort S, `crates/fleet-hub/src/pair.rs:41`
`provision_hosts` has `Deadline::Lifecycle` (300 s). The CLI gives up after 10 s and reports failure while the hub carries on (verified). Only `tracker test` got the 120 s constant. The desktop already uses `tool_deadline(tool) + CALL_MARGIN`.
*Recommendation:* derive the CLI timeout the same way and drop the per-site constants.

**`fleet_health.hosts[]` is not scoped for org-bound clients or per-host tokens.** Medium, effort S, `crates/fleet-core/src/service/health.rs:721`
`scope_to_org` re-derives counts and usage over `hosts_in_scope` but never filters `h.hosts` (verified). `blank_rollups` does not clear it either. An org-bound client, including one whose scope failed to read, sees every host's disk, load and agent/claude versions. `list_hosts` already exposes the aliases; the extra telemetry is new.
*Recommendation:* `h.hosts.retain(visible)` in `scope_to_org`, clear it in `blank_rollups`, and assert it in the isolation tests.

**The stale-working sweep demotes rows on an unreachable or unprobed host.** Medium, effort S, `crates/fleet-core/src/store/sessions.rs:1447`
`age_out_stale_working` filters only on the row's own stamps, with no reachability or `last_reconciled_at` guard (verified). It also runs after skipped or failed passes (`tick.rs:205`). After 1800 s offline, every `working` row becomes idle with a `stale_working` attention reason, a timeline entry and a fresh `idle_since`.
*Recommendation:* add `AND COALESCE(last_reconciled_at,0) >= ?2`. Add an unreachable-host test.

**Fork does not carry the source's model and effort.** Medium, effort S, `crates/fleet-core/src/service/rewind.rs:833`
The fork spawns with `model: None, effort: None` (verified). Recreate, restart, repair and move all use `stored_launch` (81bfab0), and rewind-in-place keeps the settings through `restart_session`. A fork of an opus/high session opens on the host default and stores `(None, None)`.
*Recommendation:* read `stored_launch(&s, sess.id)` under the snapshot lock and pass it through. Add a test that the fork's `session_launch` equals the source's.

**PR write-back is queued only when PR signals change, so a link made after the PR never writes.** Medium, effort M, `crates/fleet-core/src/service/sessions/reconcile.rs:995`
`on_pr`'s only caller runs inside `set_pr_signals(...) == Some` (verified). In the usual order, PR first and then a person links the ticket, nothing is queued until the PR's state changes (for example at merge). Turning the setting on later queues nothing for PRs already open.
*Recommendation:* also call `on_pr` when a person-source link is confirmed on a session with a `pr_url`, and when the setting is enabled (enqueue is idempotent). Add a link-after-PR test.

**Four new Work-view dialogs have no component tests, including the org-boundary impact-token flow.** Medium, effort M, `src/lib/WorkOrgDialog.svelte:92`
No test covers WorkOrgDialog (token-only send, stale-review drop, E_CONFLICT re-review), WorkRuleEditor ("save only after a preview of exactly this draft"), WorkPlaceDialog (`ruleDraft`) or `ruleWire`. The backend still enforces the boundary.
*Recommendation:* add component tests with the mocked-invoke harness `WorkReview.test.ts` uses, and a unit test for `ruleWire`.

**The `catalog_admin` tool gate has no tool-level test.** Medium, effort M, `crates/fleet-core/src/mcp/tools/assets.rs:325`
The store predicate is tested. Nothing drives `call_tool` as a per-host, un-granted, readonly, org-bound or later-un-granted caller. Nothing tests the nested `apply_sync` confirm nonce or `call_id` reset, or `admin::run`'s refusal of hostile names and paths.
*Recommendation:* add a caller-matrix test, an `apply_sync` nonce test, and a table test for `../x` inputs.

**`StatusMapTrigger` marks a tracker as run for 24 h even when the gate refused it.** Low, effort S, `crates/fleet-core/src/service/decide/status_map.rs:1213`
`last.insert` happens before `propose_for_tracker`. The digest hashes only org, consent and sections, so fixing `unassigned`, the key, the breaker or the budget waits `RUN_EVERY_SECS`. This bites most on first-time setup.
*Recommendation:* record `last` only after a run gets past the gate, or fold `unassigned` and the key status into the digest. Add a test.

**`work_unlinks`: the doc cites migration 066 (it is 070), and `item_id` has no index for its cascade.** Low, effort S, `crates/fleet-core/src/store/work_detect.rs:496`
The table stays tiny, so the missing index is hygiene only.
*Recommendation:* fix the comment and add a partial index in the next migration.

**The usage report does not count M13's own features (summaries, write-backs).** Low, effort S, `crates/fleet-core/src/store/work_usage.rs:224`
The query filters `kind IN ('handover','compact_summary')`. Journal kinds `summary` and `write_back` are neither counted nor listed in `UNRECORDED`.
*Recommendation:* add both counts, the TS type and the docs row.

**Stale doc text: summary hooks flag and the removed `fleet-hub tracker webhook`.** Low, effort S, `crates/fleet-core/src/service/work/summary.rs:17`
The module doc says `{"hooks":{}}`, but the code (correctly) uses `disableAllHooks`. `fleet-hub/src/decide.rs:6` and `docs/hub.md` still refer to `tracker webhook`, which #330 removed.
*Recommendation:* fix the text and point at `tracker set-credential`.

**The release key is trusted now, but docs, comments and the log level still say "no key yet".** Low, effort S, `crates/fleet-core/src/service/update/mod.rs:829`
a3033c2 added the key. Still describing the old state:
- the `keys.rs` module doc ("Empty until…");
- CLAUDE.md:458;
- the `update-channels.yml` guard;
- the refresh tick's "Debug, not warn" comment. The tick logs real signature, rollback or transport failures at debug only.

The key test (`the_compiled_in_keys_decode`) asserts nothing.
*Recommendation:* warn on refresh failure (rate-limited). Update `keys.rs`, CLAUDE.md and `docs/updates.md`. Assert the key count and distinct key ids.

**`release-update-scripts-test.sh` now always skips its `release-key.sh` checks.** Low, effort S, `scripts/release-update-scripts-test.sh:142`
It copies the real `keys.rs`, which now names a key, so CI no longer exercises the key write, the secret on stdin, or the refusal to rotate.
*Recommendation:* write an empty fixture instead of copying.

**Changing `update.track` (or the interval) does not refresh the channel.** Low, effort S, `crates/fleet-core/src/service/update/mod.rs:665`
`load_cached` has no row for a new track, so every target reads `unknown` until the next tick (up to 6 h). A new interval also waits out the current sleep. `update_admin refresh` works around it.
*Recommendation:* wake the tick through a `Notify` on `update.*` writes, or refresh inline on a cache miss. Document the behaviour.

**`update_observed` rows and per-target pins are never removed for revoked clients, deleted hosts or merged aliases.** Low, effort S, `crates/fleet-core/src/service/update/mod.rs:479`
`status()` counts ghost targets forever. `validate_target` pins any `client:<n>`.
*Recommendation:* delete or re-home the rows in `revoke_client_token`, `delete_host` and `merge_host_alias`. Validate that the target exists.

**The `update_events` log is written but never read, and has no per-target cap.** Low, effort S, `crates/fleet-core/src/store/update.rs:191`
It is staged for S4b but not marked as such. Any reporter can add unlimited distinct attempts within the retention window.
*Recommendation:* expose it or document it as S4b's, and cap rows per target.

**`update.track` offers `nightly`, which is not published.** Low, effort S, `crates/fleet-core/src/service/settings.rs:292`
Choosing it leaves every target `unknown`, and the settings row gives no warning.
*Recommendation:* drop it until S2b, or label it.

**Settings → Updates shows only on a standalone desktop; a paired desktop has no Updates note.** Low, effort S, `src/lib/SettingsDialog.svelte:1659`
*Recommendation:* add an `update-remote-section` that points at the hub's `update_admin`, and label the standalone rows as having no effect until S3.

**`FLEET_UPDATE_E2E_KEYS` is documented as serving `hub-e2e.sh`, but nothing uses it.** Low, effort S, `crates/fleet-core/src/service/update/mod.rs:50`
*Recommendation:* reword it as S4b's planned section U, or add a minimal e2e leg.

**`record_hub_self` is untested and carries the previous phase and error across a version change.** Low, effort S, `crates/fleet-core/src/service/update/mod.rs:773`
*Recommendation:* make it `pub(crate)`, test the first start and a restart, and decide in a test whether a new version clears `phase`/`last_error`.

**`/events` judges `context_full` at 85, not `health.context_red_pct`.** Low, effort S, `crates/fleet-core/src/events.rs:720`
*Recommendation:* keep the threshold in an atomic on the bus, set at startup and on settings writes.

**Retry's re-send goes around the outbox.** Low, effort S, `src/lib/ReplyActions.svelte:108`
It breaks the "one sender per session" rule, shows no delivery bubble or receipts, and duplicates the failure path.
*Recommendation:* enqueue into the outbox after `waitForReplQuiet`.

**Chips load only at startup, although the doc says "after a hub reconnect".** Low, effort S, `src/lib/composer_presets.ts:80`
After a phone edits the chips, the desktop's next edit gets E_CONFLICT and is replaced (with a notice).
*Recommendation:* reload in the gap handler and when the chip editor opens, or emit a change event.

**`rotate_host_token` drops the WSL "hooks cannot reach the desktop" warning.** Low, effort S, `src-tauri/src/commands/mcp.rs:299`
*Recommendation:* return the warning in the rotate result and show it.

**Add host never re-detects WSL distributions.** Low, effort S, `crates/fleet-core/src/service/hosts.rs:21`
A new distribution needs a restart (this is documented), and the picker is empty during the startup detection.
*Recommendation:* make `discover_hosts` async, wait for a pending detection, and re-detect when due.

**`merge_host_alias` leaves host-keyed data behind.** Low, effort S, `crates/fleet-core/src/store/hosts_accounts.rs:485`
`asset_inventory`, `org_rules.host_alias` and `hosts.org_id` are left alone.
*Recommendation:* move the inventory rows, rewrite `org_rules`, and copy `org_id` when `into` has none.

**The health line ignores sample age.** Low, effort S, `src/lib/hosts_view.ts:313`
`void now;`. `disk_low` keeps firing on stale samples.
*Recommendation:* show "sampled X ago" and gate `disk_low` on freshness.

**Desktop `merge_host` Tauri command has no caller.** Low, effort M, `src-tauri/src/commands/hosts.rs:96`
This is deliberate (`noUiControl` allowlist, operator task), and a standalone desktop refuses `local` anyway.
*Recommendation:* drop the command or keep it knowingly. Low priority.

**WSL routing decisions are untested.** Low, effort M, `src-tauri/src/pty.rs:432`
The PTY `wsl.exe` branch, the tunnel skip and the two provision skips have no tests. `wsl::attach_argv` itself is tested.
*Recommendation:* seed the distro table in tests and assert that no tunnel is started.

**Per-view selection and expansion persistence in `work_view.ts` is untested.** Low, effort S, `src/lib/work_view.ts:1111`
*Recommendation:* add a view-switch round-trip test.

**CLAUDE.md status omits #354 (host identity and health) and #381 (stale-working acknowledgement).** Low, effort S, `CLAUDE.md:258`
*Recommendation:* add one paragraph each, citing migrations 076–078 and 080–081 and the plans.

## Speed

**One Work view refresh does 2+K full graph loads, fired by any work-touching session event.** Medium, effort M, `src/lib/WorkTree.svelte:284`
Each debounced tick (500 ms, capped at 3 s) runs `workTree` for the first page, one `work_tree` loop per open section, and `workReview({limit:1})`. Other mounted readers (views, rules, task detail, SessionTasks, review) re-read too. The kind of change carried by `work:changed` is thrown away (`noteWorkChanged`), and the tree reloads even while the Review tab is showing. Each call is a full `Graph::load` plus `all_tasks`, at about 600–875 ms p95 at scale.
*Recommendation:* return the review total and requested sections from one tree call. Carry change kinds so views and rules re-read only on `view`/`rule`/`resync`. Skip the tree while Review is showing. Consider caching the built tasks per work/session generation.

**`task` / `session_tasks` hold the Store mutex through the whole build; `Rules` / `OrgImpact` use the writer; the desktop has no read pool.** Low, effort S, `crates/fleet-core/src/service/work/view.rs:1586`
`tree` and `review` release the lock after the load; `task` and `session_tasks` keep it through `build_tasks`/`to_task`. `WorkAction::Rules` and `OrgImpact` pass `&self.store` in spite of the comment above them. Hub reads mostly go through the read pool.
*Recommendation:* scope the lock as `tree` does, and route those two actions through `reader()`.

**`place` / `assign_org` load the whole graph twice under the writer lock.** Low, effort M, `crates/fleet-core/src/service/work/structure.rs:204`
*Recommendation:* at minimum, do the second load after dropping the guard, or return only the new version.

**Every tree page runs `to_task` for every visible task before filtering and paging.** Low, effort M, `crates/fleet-core/src/service/work/view.rs:1409`
Roughly 20k TaskLinks are built per page at scale, to return 400. `needs_attention_with` also runs twice per link.
*Recommendation:* do a cheap summary pass, then build TaskLinks only for the page.

**Work view reads load and JSON-parse every item's `meta`, including descriptions, on every call.** Low, effort S, `crates/fleet-core/src/store/work_view.rs:179`
*Recommendation:* select only the `json_extract` fields the tree needs, and fetch the description only in `task()`.

**SessionTasks (and WorkTaskDetail) re-read twice per work change.** Low, effort S, `src/lib/SessionTasks.svelte:78`
The `workSig` effect and the debounced tick both fire on the same event.
*Recommendation:* keep one trigger.

**`session_tasks` runs a second link query and a linear find per link.** Low, effort S, `crates/fleet-core/src/service/work/view.rs:1747`
*Recommendation:* filter `g.links` by `session_id` and drop `work_view_session_links`.

**Sidebar `archivedHidden` rebuilds both session groupings a second time on every `$sessions` change.** Low, effort M, `src/lib/Sidebar.svelte:291`
This is also a drifting copy of the list's rules (see Simplify).
*Recommendation:* return 0 early when nothing is archived; better, count inside the existing groupings.

**`/metrics` decodes every session row under the writer mutex on each scrape.** Low, effort S, `crates/fleet-core/src/mcp/metrics.rs:159`
The `context_red_pct` read it does is never exported.
*Recommendation:* use the read pool and a `GROUP BY` count, and drop the dead read.

**`fleet_health` for an org-bound client reads the fleet twice, with N+1 per tracker and per host.** Low, effort M, `crates/fleet-core/src/service/health.rs:721`
*Recommendation:* scope in a single pass, and use `GROUP BY tracker_id` and `host_alias IN (…)`.

**`describe`/`card`/`lookup` load every item's org to check one item.** Low, effort S, `crates/fleet-core/src/service/trackers/tickets.rs:95`
*Recommendation:* add a per-item `item_visible(scope, s, &item)` predicate.

**Reconcile does three per-session point queries while holding the host's full rows; #381 added `stale_demoted`.** Low, effort S, `crates/fleet-core/src/service/sessions/reconcile.rs:764`
*Recommendation:* build a map from `list_sessions_for_host`, and read the demoted set in one query.

**Every authenticated hook takes an extra lock and writes `hosts.last_hook_at` unconditionally.** Low, effort S, `crates/fleet-core/src/service/hooks.rs:45`
*Recommendation:* throttle the write in SQL (`< ?1 - 60`) inside the hook's main lock.

**Commands for known WSL hosts wait on any running detection.** Low, effort S, `crates/fleet-core/src/wsl.rs:284`
A stale alias triggers a re-detection every 60 s.
*Recommendation:* return at once when `distro_for(alias).is_some()`.

**The no-mux slot wait doubles the wall clock, and long operations share the reconcile probe's four slots.** Low, effort M, `crates/fleet-core/src/ssh.rs:741`
*Recommendation:* use one deadline over the slot wait plus the child, skip the cap for WSL, and reserve a permit for probes. Document the cap in `docs/windows.md`.

## Simplify

**`archivedHidden` re-implements the list's search and past-link visibility rules inline, and has already drifted once.** Medium, effort S, `src/lib/Sidebar.svelte:325`
It copies `matchesSearch`, `pastVisible`/`pastPassesWorkFilters` and the past-only search. 86d2eb9 was already a drift fix.
*Recommendation:* parametrise the existing helpers, or fold this into the single-pass speed fix above.

**Two copies of the locked-down `claude -p` script and its tag parser, already drifting.** Medium, effort M, `crates/fleet-core/src/service/decide/haiku.rs:229`
`haiku.rs` and `summary.rs` share the isolation flags (`disableAllHooks`, `--tools ''`, `--strict-mcp-config`, `--no-session-persistence`) and the wrapper shell. Each test pins only its own copy. The parsers differ: haiku uses first-tag-wins, while summary uses `rposition`, so a reply line starting with `fleet-summary=` is read as the verdict.
*Recommendation:* one `service::claude_print` helper holding the flags, the wrapper and a first-tag-wins parser. At minimum, switch summary to first-tag-wins.

**Host health rules exist in Rust and TS and already differ.** Medium, effort M, `src/lib/hosts_view.ts:238`
- Freshness: Rust hard-codes 86 400 s and never reads `health.version_max_age_secs`.
- Claude version: TS flags any older version, while Rust needs more than `claude_max_behind` releases behind.
- Disk: `diskMeter` hard-codes 90/95 and drives the sidebar dot, while `disk_low` uses the setting.
- Hooks: TS judges hook health from `last_stop_at`, Rust from `last_hook_at`.
- Settings: a paired desktop never loads `$fleetSettings`, so it uses the defaults.

The TS `Health` type ignores `hosts[]` entirely.
*Recommendation:* render the flags from `fleet_health.hosts[]` and keep only the token/hook/provision marks in TS.

**Retention batch loop copied three times, and the outbox is missing from the status dry run.** Low, effort S, `crates/fleet-core/src/service/work/retention.rs:240`
`sweep_capped` (per table), the `tracker_writes` loop and `decide::sweep_runs` repeat one loop. Because `tracker_writes` is not a `RetentionTable`, `work_admin status` never reports it.
*Recommendation:* add `RetentionTable::TrackerWrites` and one `sweep_batches` helper.

**The fleet-hub CLI files copy `now()`, `db_path()` and the read-only open.** Low, effort S, `crates/fleet-hub/src/decide.rs:372`
`bench.rs`, `decide.rs` and `census.rs` each have them, plus `ready::unix_now` and `tick::unix_now`, although `fleet_core::store::now_unix` exists and says it is there for fleet-hub.
*Recommendation:* use `now_unix`, and move `db_path` and `open_read_only` into one shared module.

**Bench Jev call loop and metric helpers are duplicated.** Low, effort M, `crates/fleet-core/src/service/decide/bench/work_link.rs:1229`
`f3` shadows the exported `bench::f3`, `f2` exists twice, `ratio` equals `pct`+`round3`, and there is a read-back `get_decision_run` only for latency and cost.
*Recommendation:* put latency, tokens and cost on `DecisionOutcome`, and share the helpers in `bench/mod.rs`.

**Person-source list has three copies.** Low, effort S, `crates/fleet-core/src/store/bench_work_link.rs:12`
`BENCH_PERSON_SOURCES`, `PERSON_SOURCES` and an SQL literal.
*Recommendation:* use `PERSON_SOURCES` and build the `IN` list from it.

**Decide mode vocabulary has three copies with no test tying them.** Low, effort S, `crates/fleet-core/src/service/settings.rs:287`
*Recommendation:* define `FeatureMode::ALL` and derive the others from it, or add a tie test like the fallback one.

**Two diacritic folds.** Low, effort S, `crates/fleet-core/src/service/nl/mod.rs:349`
`nl::fold` is a subset of `bm25::fold_char`. The narrower `is_ticket_key` is intentional.
*Recommendation:* keep one fold in `nl`, and have bm25 import it.

**Link state, session name and org derivation are re-implemented in `impact_of` and `brief_of`.** Low, effort S, `crates/fleet-core/src/service/work/structure.rs:586`
*Recommendation:* make `Graph::state_of` and `session_org` `pub(crate)`, and add `link_name` and `task_kind` helpers.

**`workSig` re-implements "needs you" partially.** Low, effort S, `src/lib/work_view.ts:1221`
It misses `context_full`, `stale_working`, `ci_failing` and lifecycle, so the tree's dot and sort go stale.
*Recommendation:* use `attention.ts`'s `needsYou`.

**Constant `GroupRef.editable`, an unreachable match arm, an evidence serde round-trip, and double `check_filters`.** Low, effort S, `crates/fleet-core/src/service/work/view.rs:1903`
*Recommendation:* remove them.

**Dead frontend exports.** Low, effort S, `src/lib/work_view.ts:1208`
`needsFullReload` (and the `events.ts` comment describing it), `filtersToQuery`/`filtersFromQuery`, `tasksShowingSession`, `noticeText`, `isWorkFilters` (`work_filters.ts:97`), and `transcriptCarries`/`PendingPrompt` (`conversation.ts:751`) are used only by tests.
*Recommendation:* delete them and move the tests onto the live code.

**The rule-draft builder is copied in two components, and its regex disagrees with the server's `key_prefix`.** Low, effort S, `src/lib/WorkPlaceDialog.svelte:90`
*Recommendation:* one `ruleDraftFor` in `work_view.ts`, taking the prefix from the key group's label.

**Compare-and-set check and `WorkChanged` emit repeated in `store/work_view.rs`.** Low, effort S, `crates/fleet-core/src/store/work_view.rs:436`
*Recommendation:* a `check_version` helper plus `WorkChanged::rule`/`view` constructors.

**Third provider-name table in the TS.** Low, effort S, `src/lib/work_view.ts:952`
*Recommendation:* reuse `providerShort`, or add a `short` field to `PROVIDERS`.

**The usage text form is copied by hand in TS.** Low, effort S, `src/lib/work_usage.ts:58`
*Recommendation:* return `lines` from the command, or pin both sides to one fixture.

**Test-only `Store::cached_description`.** Low, effort S, `crates/fleet-core/src/store/work_describe.rs:25`
*Recommendation:* mark it `#[cfg(test)]`.

**Update service helpers.** Low, effort S, `crates/fleet-core/src/service/update/mod.rs:376`
- serde round-trips stand in for enum parse/format (add `UpdatePhase::as_str` and `Component::FromStr`).
- `update::setting` duplicates `settings::get_string` (add `settings::get_i64`).
- `update_route::refuses_peer` repeats `identity()`.

*Recommendation:* make those three changes.

**Hand-rolled `civil_from_days` again in `utc_stamp`.** Low, effort S, `crates/fleet-hub/src/serve.rs:697`
*Recommendation:* build it from `usage::day_string` (fleet-hub already uses it) or `fleet_update::time`.

**The four-step `fleet-release` toolchain setup is repeated in three release jobs.** Low, effort S, `.github/workflows/update-channels.yml:64`
*Recommendation:* a local composite action.

**`stale_demoted_at` is kept off `SessionRow`, so five call sites carry a `demoted` bool and two accessors exist.** Low, effort M, `crates/fleet-core/src/service/tasks.rs:258`
*Recommendation:* add it as a `#[serde(skip)]` field. This also removes the reconcile N+1 above.

**The attention classifier is written twice with no shared fixture.** Low, effort S, `src/lib/attention.ts:204`
*Recommendation:* add `testdata/attention_cases.json`, read by both a Rust test and a Vitest test, as `quiet_statuses.json` is.

**`read_external_grace_cutoff` copies `read_lost_ttl_cutoff` and a literal 3600.** Low, effort S, `crates/fleet-core/src/service/sessions/reconcile.rs:86`
*Recommendation:* use `settings::get_secs` with one helper.

**`reap_hidden_hosts` re-implements `active_hosts` inverted, with a literal `"local"`.** Low, effort S, `crates/fleet-core/src/service/sessions/reconcile.rs:1766`
*Recommendation:* a shared `hosts::is_active`.

**Migration doc comments carry pre-renumber numbers.** Low, effort S, `crates/fleet-core/src/store/schema.rs:59`
072/073/074 should read 076/077/078 in nine places (`schema.rs`, `rows.rs`, `hosts_accounts.rs`), and the 072 comment sits on the wrong function.
*Recommendation:* correct them.

**Outbox body built in three places; `relabel_conversation` SQL has lost continuations and uses a raw transaction.** Low, effort S, `src/lib/outbox.ts:322`, `crates/fleet-core/src/store/conversations.rs:354`
*Recommendation:* export `outboxBody` (or compute `seen` in `enqueue`), and switch to `in_savepoint`.

**`catalog/admin.rs` duplicates the `author.rs` validators and 29 hand-written action names.** Low, effort S, `crates/fleet-core/src/service/catalog/admin.rs:190`
*Recommendation:* share one `pub(crate)` validator, and keep the pre-checks only where the callee does not check.

**WSL settle and command preparation repeated at every `ssh.rs` entry; `pty_open` waits after `openpty`.** Low, effort S, `crates/fleet-core/src/ssh.rs:486`, `src-tauri/src/pty.rs:629`
*Recommendation:* one `prepare()` helper, one wait loop and a `wsl_prefix` helper. Move the settle above `openpty`, drop the redundant `child_gone_at` reset, and word the exit note per program.

## Suggested follow-up PRs

1. **Host lifecycle and health fence.** Covers:
   - Hidden-host reap (limit it, plus the hide/unhide test) and the `hosts::is_active` helper.
   - `fleet_health.hosts[]` scoping plus the isolation assert.
   - `merge_host_alias` leftovers.
   - `agent_behind` direction and health-line sample age.
   - Migration comment numbers and the CLAUDE.md paragraph for #354.
2. **Session state and reply actions.** Covers:
   - The stale-working reachability guard and `/events` threshold.
   - Fork carrying model and effort.
   - `stale_demoted_at` on `SessionRow` (removes the reconcile N+1).
   - Orphan fork copy, the Retry marker strip, and Retry through the outbox.
   - Outbox `seen` duplicates and the `outboxBody` helper.
   - Chip reload.
   - The #381 CLAUDE.md paragraph.
3. **Work graph cross-org and write-back.** Covers:
   - `visible_item_for_key` in card, lookup, handover and resume.
   - `on_pr` on a person link or setting enable.
   - The per-item `tickets::item_visible`.
   - The `TrackerWrites` retention variant and `sweep_batches`.
   - Usage counts for summaries and write-backs.
   - Stale summary and webhook docs.
4. **Work view backend.** Covers:
   - Placement re-key on bind plus the orphan sweep.
   - `org_impact` through `OrgScope`.
   - Lock scoping in `task`/`session_tasks` and `reader()` for Rules/OrgImpact.
   - The single graph load in `place`/`assign_org`.
   - The lazy `meta` load and the `to_task` summary pass.
   - `session_tasks` filter, `TaskCounts` units, shared state/name helpers, dead arms.
5. **Work view frontend.** Covers:
   - Change kinds on `work:changed`, a batched tree plus sections plus review count, and no tree reload under Review.
   - The single SessionTasks trigger.
   - `archivedHidden` sharing helpers, with an early return.
   - `workSig` via `needsYou`.
   - Dead exports, the shared `ruleDraftFor`, and the provider table.
   - Dialog and view-persistence tests.
6. **Update channel hardening, before 0.4.1 manifests.** Covers:
   - Per-artifact `build_id`.
   - Key docs and warn-level refresh failures, and the key-test fixture.
   - Refresh on a track change.
   - Observed/pin cleanup on revoke, delete and merge.
   - Replay-safe `report`.
   - `nightly` label, paired-desktop Updates note, `update_events` and `E2E_KEYS` wording, `record_hub_self` tests.
   - Enum parse/format helpers, the peer gate, `get_i64`, the composite CI action.
7. **Hub ops, CLI and WSL.** Covers:
   - CLI timeout from `tool_deadline`.
   - `/metrics` via the read pool.
   - Readiness latch.
   - `catalog_admin` gate tests and shared validators.
   - Single-pass health scoping and the hook-stamp throttle.
   - Shared fleet-hub `now`/`db_path`/`open_read_only`/`utc_stamp`.
   - Slot timeout as `E_SSH`; no wait for known WSL aliases.
   - Keep the WSL table on a failed list; rotate warning; Add-host re-detect.
   - Slot deadline and probe permit; WSL routing tests; ssh `prepare()` cleanup.
8. **Decide / Jev tidy-up.** Covers:
   - `StatusMapTrigger` recording only after the gate.
   - The shared `claude_print` helper with first-tag-wins (also fixes summary).
   - One person-source list and one mode list.
   - Shared bench helpers and outcome metrics; the string continuation fix.
   - The `work_unlinks` doc and index; one diacritic fold.

## Area notes

**decide-jev.** The Jev envelope, record, J3 assist, phase-0 benchmarks and D34 label hygiene are careful work. Nothing runs by default, the key has one reader, and no lock is held across a call. No correctness bugs. The costs are copy-paste between the benchmarks and the new CLI files, parallel vocabularies with no tie tests, and the 24 h gated-run latch in `StatusMapTrigger`.

**work-view-backend.** M14.1a–d is a large, well-fenced projection with isolation rows and scale budgets. Every read loads the whole graph by design. Some writes and reads do that under the writer mutex, a few twice. `org_impact` re-implements visibility. A bare-key placement is lost on bind. Several derivations are copied between modules and languages.

**work-view-frontend.** The pure model (`work_view.ts`, `work_filters.ts`, pref migration) is well tested, and the stale-answer guards are correct. The cost is in fan-out: every `workChanged` re-reads everything and drops the change kind. `archivedHidden` duplicates the sidebar's grouping and rules. Some dead exports remain.

**work-graph-m13.** Usage counts, summaries, the Jira write-back outbox, webhook removal, the describe cache and prompt filtering are each well built. The problems sit between them: the shared-key fix skipped four callers, write-back triggers only on PR signal changes, the outbox sweep bypasses `RetentionTable`, and the usage report misses M13's own features.

**update-channel.** S1, S2, S4a and S5 are high quality: cache reads re-verified, token-derived identity, careful CI. The seams are weak: the OCI `build_id` cannot match, the key text is stale, observed rows are never cleaned or protected from replays, and a track switch waits a tick. There is also a copied peer gate and date routine.

**session-state.** Plan A and #381 are guarded, change-only SQL with thorough tests. The sweep judges rows on hosts nobody observed. Keeping `stale_demoted_at` off the row costs extra queries and a bool at five call sites. Fork misses the model/effort carry, and `/events` uses a different context threshold.

**conversation-rewind.** Reply actions, the outbox and chips are carefully built, with quoted scripts and step-by-step undo. The remaining issues: an orphan copy in one fork ordering, the marker line re-used on Retry, Retry bypassing the outbox, chips never reloaded, `seen` ignoring queued duplicates, and small leftovers.

**hub-ops.** The usage backfill split and `backup.sh`/`upgrade.sh` are well reasoned and tested. The pieces around them are weaker: unscoped `hosts[]`, `/metrics` on the writer mutex, a fixed 10 s CLI timeout, blocking git in `catalog_admin`, a readiness flag that is not a latch, and duplicated backup, date and validator code.

**windows-wsl.** WSL hosts, one ssh program, the mux-less cap, ConPTY and Credential Manager are cleanly split out and tested. The interactions are weaker: slot timeouts read as "may have run", known aliases wait on detections, a failed list wipes the table, and long operations share probe slots.

**host-identity.** The store work for #354 (merge transaction, guarded ALTERs) is careful. Around it, the hidden-host reaper turns a reversible Hide into deletion, `hosts[]` is unscoped, the health rules are split between Rust and TS and disagree, the merge leaves some host data behind, and the migration comments are misnumbered.

**x-speed.** New tables are well indexed and the ticks are set-based. The real cost is the M14 Work view read path: full graph rebuilds, some under the mutex, multiplied by the desktop's fan-out, and no scale test for the multi-read refresh. Everything else is small per-row or per-hook overhead.

**x-duplication.** The house rules (quoting, `proc`, `OrgScope`) hold. Duplication sits a level below them: the two `claude -p` builders (already drifting), three retention loops, compare-and-set checks, the rule draft, CLI helpers and provider tables.

**x-docs-gaps.** The mechanical guards (migrations, settings tables, verdict count, isolation matrix) all pass. The gaps are in the newest update work (stale key text, the skipped key test, the settings UI, track refresh, cleanup), the fork model carry, the host-mark rule drift, and CLAUDE.md missing #354 and #381.

**x-tests.** Rust behaviour is broadly covered with scenario-named tests. The gaps are at the seams: four untested Work-view dialogs, the `catalog_admin` gate, WSL routing skips, `record_hub_self`, a key test that asserts nothing, and view-switch persistence. No no-op tests were found.

## Method

- **Analysts:** 14 in total. 10 covered areas (decide-jev, work-view backend and frontend, work-graph M13, update channel, session state, conversation/rewind, hub ops, Windows/WSL, host identity). 4 were cross-cutting lenses: speed, duplication, docs and gaps, tests.
- **Challenge step:** every finding was challenged by one skeptic, or by three with a majority vote for high severity. Many severities were adjusted in that step. 28 findings were refuted and dropped, and the rest are reported here with duplicates across areas merged.
- **Static reading only:** no cargo builds or tests were run.
- **My own checks:** I re-checked the top findings against HEAD before writing: the `card.rs` single-item read, the `reap_hidden_hosts` filter, the fork's `model: None`, the CLI's 10 s timeout, the workflow `build_id` sources, the missing reachability guard in the stale sweep, and `on_pr`'s single caller.
