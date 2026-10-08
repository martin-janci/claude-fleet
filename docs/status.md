# Status

What is landed, what is built but off by default, and what waits on the
owner, by feature area — with the spec or plan behind each. Moved out of
`CLAUDE.md` (which every session loads) on 2026-10-06. When a feature lands or
a default flips, update its paragraph here; when a paragraph stops being true,
delete it rather than adding a correction after it.

Iterations 1–4a are landed (multi-host, accounts, cross-host sessions, prompt
transfer, async/events rework), plus the MCP control API, background sessions,
the background reconcile tick, fleet_health roll-up, and the persistent session
event timeline (session_history). Handoff from the original spec is replaced by
`move_session` (Move to host…) and Freeze is descoped, per
`docs/adr/0001-descope-freeze-ship-move.md`. `move_session` now CARRIES
uncommitted and unpushed work plus small git-ignored files to the target
instead of refusing a dirty or unpushed source (`strict: true` restores the
ADR 0001 refusals), and the session's Claude directory and project memory
(slice 2 spec `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`),
per `docs/adr/0002-move-carries-work-as-is.md` and
`docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`. The move's UI
is the Transfer sheet (terminal-header chip + `moves.ts`, live steps from the
`move:progress` event), per
`docs/superpowers/specs/2026-09-20-transfer-sheet-design.md`.

Multi-account (docs/accounts.md): Hosts shows each host's `/login` from
`$CLAUDE_CONFIG_DIR` and flags credential variables that outrank it
(migration 111). **Login profiles** are landed for launching: `new_session
{ profile }` and the New session dialog run a session under
`~/.claude-profiles/<name>` (its own `/login`, everything else shared with
`~/.claude`), and `restart_session { profile }` resumes a session under
another login (migration 113; desktop: session details → Login). Each
pass reads a host's profiles and their logins (migration 114), attributes a
profile session to its profile's account and polls that account's usage
through the profile. Not built: a hub vault for setup-tokens and API keys,
and automatic switching when an account hits its limit.

The headless `fleet-hub` daemon, `fleet-agent` for hosts the hub cannot reach
over SSH, paired-client access for phones/browsers, and hub-client mode
(pairing the desktop itself to a hub) are landed; see `docs/hub.md`. Their
live acceptance (#156) is recorded in `docs/hub-acceptance.md`: the desktop
half is partly observed, the TLS, phone and agent-host steps wait on the owner. Since contract revision 5 a hub client adds
projects through the hub (`add_project` / `list_github_repos` tools), per
`docs/superpowers/specs/2026-09-27-hub-add-project-design.md`. Host
reboot handling is landed in both halves, per
`docs/superpowers/specs/2026-09-17-host-reboot-session-survival-design.md` and
`docs/superpowers/plans/2026-09-19-host-reboot-recovery.md`: **survival**
(sessions are recovered by boot identity rather than declared lost on a
restart, migration 036 + `lost_reason`) and **recovery** —
`restore_host_sessions` batch-resumes a host's lost sessions over
`recreate_session`, `discover_lost_sessions` scans a host's Claude transcripts
for conversations fleet has no row for, and `new_session` takes a
`resume_claude_session_id`; the UI for both is in `HostDetail`.

The work graph's M0–M3 are landed (work links anchored on participants,
group-by-work, and M2's work journal, carry rules, handover brief and resume
— `work` / `work_link` actions, `service/work/`; M3's trackers: Jira Cloud
read-only over `fleet-core::net`, the sync tick, `work_admin`, tickets /
lookup / start — `service/trackers/`, `store/trackers.rs`,
`store/tracker_items.rs`); read
`docs/superpowers/2026-09-24-work-graph-roadmap.md` before touching them.
The user guide is `docs/work-graph.md`: update it with any change a user
sees. Its `work.*` settings table is generated (see *Settings metadata* in
`docs/architecture.md`).
Tracker secrets are read ONLY by `Store::resolve_tracker_credential`.
Work graph M4 (detection) is landed: one recogniser in Rust and TS over a
shared fixture (`service/work/recognize.rs`, `src/lib/work_keys.ts`), the
pure resolver (`service/work/resolve.rs`, rules R1–R9 and R3u; state signals are
current, rejections are final), `detect.rs` wiring the prompt / Stop / PR
probe / sync triggers, migration 049, `SessionRow.work_suggested` (a guess
never groups a session), and the chip / popover / batch review UI. The
SessionStart context (M4.5) is built but OFF behind
`work.session_start_context` (decision D5; the remote-host numbers are the
user's to take with `scripts/measure-session-start.sh`). M4.6, the opt-in classification
nudge, is landed OFF behind `work.classify_nudge`: after three turns with no
link and 1–5 candidates in the host's scope, one UserPromptSubmit per
conversation carries a ≤400-char note after the mail
(`service/work/nudge.rs`, migration 056 `conversations.classify_nudged_at`; migration 057 re-issues the read-cursor delete trigger that a rewrite of 044 left out of some databases);
Claude's answer, `work_link { source: agent_inferred }`, is only ever a
pre-selected suggestion (rule R11, strength `inferred`).
Work graph M5 (organisations) is landed: migration 050 (`orgs`, text-keyed
`org_rules`, `hosts.org_id`, `work_links.snap_org_id`), `SessionRow.org_id`
(SQL, `session_org_sql!`, held equal to `store::org_of_session`), and the
org BOUNDARY for per-host tokens — `service::orgs::OrgScope`, made only by
`Caller::org_scope`, filters every work read in the service layer, and
`call_tool` redacts session rows' work in everything a host receives. M3's
per-host ticket fence is kept (composed with the org). Cross-org links need
`force_cross_org` for every caller; `isolate_sessions` (D7) is per org, off.
Any new `work` / `work_link` / `work_admin` action needs a row in the
isolation matrix (`mcp/tools/tests_isolation.rs`), which fails otherwise.
Work graph M6 (more providers) is landed: GitHub Issues (through `gh` on a
host, `transport = via_cli:<host>`, no token in fleet), Asana (`asana:<gid>`
keys, events-API sync tokens, the section map), Linear, and Jira Data Center
(admin-fenced site: exact host, resolve-then-refuse loopback / link-local,
optional `extra_ca`), all behind the `TrackerProvider` trait and the
`HttpTransport` seam (`net/via_host.rs` has `gh` and host-side `curl`, the
credential only ever on stdin), migration 051 (`tracker_views.sync_mark`,
`trackers.settings`). A new provider must pass the conformance suite
(`service/trackers/conformance.rs`, `conformance_suite!`) and
`tests_isolation_providers.rs`.
Work graph M7 (self-cleaning lifecycle) is landed: the pure planner
`service/gc/tidy.rs` (reasons, hard-coded protections), migrations 052
(UI-only archive, snooze / never per link, `sessions.last_touch_at`,
`work_items.reopened_at`) and 053 (`orgs.auto_tidy`), `work { tidy | reopened }` and
`work_link { archive | unarchive | snooze | never | dismiss | tidy_apply }`,
and auto-tidy in the GC sweep behind `work.auto_tidy` (OFF; safe kill only),
overridable per org. Tidy-up suggests; it never kills a dirty tree except
through safe kill, and a per-host token sees only its host's and org's
candidates.
Work graph M9.1 / M9.2 are landed: the Today view (`work { action: today }`,
Details' empty state and ⌘⇧T, a plain-text Copy standup) and the ticket
context card (`work { action: card }`, acceptance criteria from the cache,
Insert into composer with the hub-fenced `composer_text` — never sent).
M9.7 (the operator's starts and kills always confirmed; refused on a hub),
M9.3 (agent-written handover on demand, `work_link { action: handover }`)
and M9.6 (multi-repo start, `work_link start { project_ids }`) are landed
too. Write-back (D3), dead-session summaries (D10) and webhooks (D13) were
decided against; the user said yes to D3, D10 and D20 on 2026-09-27, as
M13.4e, M13.4c and M13.4a of M13. D13 stays no: its build (M13.4f) reached
`main` and was removed again (migration 062 stays, 064 drops its table;
build notes:
`docs/superpowers/plans/2026-09-26-work-graph-m13-decided-yes.md`).
M13.4c (D10) is landed (#327, fixed in #331):
`work_link { action: summarize }`, on demand only, one tool-less
print-mode fork on the session's own host (`service/work/summary.rs`,
`work.summary_model`), stored as a journal `summary` and fenced in the
brief.
M13.4e (D3) is landed too (#327, fixed in #332; tests #334): the PR
remote link on Jira only, per tracker `settings.write_back.pr_remote_link` (off), queued by the PR probe
for `manual` / `started` confirmed links in the tracker's own org, through
the outbox `tracker_writes` (migration 061) drained by the sync pass
(`service/trackers/write_back.rs`, `TrackerProvider::write`).
Work graph M10 (`docs/superpowers/plans/2026-09-25-work-graph-m10-settle.md`):
M10.4 is landed — Today's Stale opens Tidy-up narrowed to those sessions,
and the M5.5 filters (tracker / status / mine / has-session / archived)
have chips under the sidebar's "⚑ work" pill (`work_filters.ts`, through
`rowMatches`). The rest of M10 is landed too: the M9 review leftovers, the
work graph end to end in `scripts/hub-e2e.sh` against a loopback fake
tracker (the `e2e` feature; CI builds it as `WBIN` and fails without it),
the replay-ring numbers, and the phone's Today / ticket card; the written
acceptance run, `docs/work-graph-acceptance.md`, waits on the owner.
Work graph M11 (the long tail) is landed: "Name this work…" (`work_link
{ action: name }`, `work { local_items }`), resume probes the transcript,
tidy reason `idle_unlinked` (`work.tidy_idle_unlinked_days`, never
auto-tidied), GitHub Enterprise and per-tracker `SyncMetrics`
(`work_admin { status }`), and the tool budget paid back.
Work graph M12 (ship and operate) is landed: the upgrade test and
downgrade guard (`store::testgen`), the scale fixture and budget tests
(`service/work/scale_tests.rs`, migration 058), the `work.retention.*`
windows (`store/work_retention.rs`), trackers in `fleet_health` with a
Reconnect Attention item, and the review of the decided-against list
(`docs/superpowers/reviews/2026-09-26-work-graph-decisions-revisited.md`).
M13 (live use, `docs/superpowers/plans/2026-09-26-work-graph-m13-live-use.md`)
is closed (M13.5, #337): M13.1 (partial sync failures, #320), M13.2
(`work_admin { usage }`, #323 / #324), M13.4c and M13.4e above are on
`main`; M13.4a (D20) and M13.4d (D15, multi-start) are on fleet-mobile
(#51, #50). The work graph is *operating* (D26): new work is issues and
small plans. Two items stay open, waiting on the owner: the acceptance run
and its triage (M13.3), and D5 (M13.4b). Open decisions are the roadmap's table, and a decision-gated
feature starts only on the user's "yes".
Work graph M14 (the Work view: org → group → task → every session, and a
phone paired to one org) is the one milestone after it (roadmap D36): plan
`docs/superpowers/plans/2026-09-27-work-graph-m14-work-view.md`, design
`docs/superpowers/specs/2026-09-27-work-view-design.md`. M14.1a–d (the
backend: `work { tree | task | session_tasks | review | rules | … }` in
`service/work/view.rs`, `work_link { set_primary | place | assign_org | … }`
in `service/work/structure.rs`, migrations 066–067, org-bound clients as
`OrgScope::Org`, compare-and-set with `E_CONFLICT`, the desktop commands
and `work:changed`) and M14.2–M14.4 (the desktop Work view — `WorkTree`,
`WorkTaskDetail`, `WorkReview`, the rules / place / org dialogs, state in
`src/lib/work_view.ts` — and fleet-mobile's *My work*) are landed; the
desktop re-reads on `onWorkChanged` and the `workChanged` tick in
`work.ts`. `scripts/hub-e2e.sh` hub W section 10 runs the contract on a
real hub. M14.2 / M14.3 landed in #349 (fixes #357, #359, #361, #365) and
M14.4 as one PR, fleet-mobile#54; M14.5's docs are on `main`, so only the
owner's Part R run is open, and *Assign org…* / *Make a rule…* stay
desktop-only (owner, 2026-09-28; M14's D31–D36 and Jev's D31–D47 share
numbers, so write "M14-D3x" / "Jev-D3x").

Sprints and releases (design
`docs/superpowers/specs/2026-09-28-sprints-releases-epics-design.md`): the
backend is landed — migration 108 (`work_buckets`, `work_bucket_items`,
`work_bucket_refs`), `store::work_buckets` (one current sprint per item,
history kept on removal and close, adoption from a tracker's sprint or
version and its withdrawal), `Caps.versions` with Jira `fixVersions`,
GitHub milestones and Linear project milestones (E8's default), and the MCP
actions (`work { buckets | bucket }`, `work_link { bucket_add |
bucket_remove }`, six `work_admin` bucket actions). The board (§6c) is
built as a first cut: the Work view's *Board* button opens `WorkBoard` over
the terminal, To do / Doing / Done from one `work_tree` read with the Work
view's filters, a native card dragged (pointer events, or ← →) to set its
status through `set_work_status` → `work_link { set_status }`, a tracker's
card refused on the card (E11), each card with its live session and host.
A native task is edited (title, description, status, assignees) from its
card's ✎ or E, the List row's ✎ and the task page's *Edit*:
`EditTaskDialog` writes `edit_work_item` → `work_link { edit }` and
`set_work_status`; a tracker's ticket stays its tracker's to edit.
It is not yet scoped to a sprint: the Work view's Sprint / Release axis and
bulk assignment are not built, nor are epics for local items (phase 4). E9–E11 run on their
defaults.

The Jev evaluation (TypeSafe's decision model as an optional reader for
closed-set decisions) has started with a local language census: `fleet-hub
census languages` over `service::nl` (cargo feature `nl-detect`, lingua, ON
only in fleet-hub — the models add ~45 MB, kept there by D47). The decision
envelope is built and OFF (Jev spec D35–D37; the roadmap's D31–D36 are other decisions): `service::decide` (`gate` / `decide`,
`DecisionBackend`, `jev.rs` fenced to api.typesafe.ai), `decide.*` settings,
per-org consent `orgs.jev_allowed` (migration 068), the record
`decision_runs` + key `decision_secrets` (069; the key is read ONLY by
`Store::resolve_decision_credential`, never raw text in a run), `fleet-hub
decide`; guide `docs/decisions.md` (its `decide.*` settings table is generated). The first use case, J3 `status_map`, is built (shadow / assist
only, off): the Asana probe keeps `config.unmapped_sections` /
`project_sections`, `service/decide/status_map.rs` asks one Choice per
unclassified section after a clean sync (`StatusMapTrigger`, daily), and
`fleet-hub decide proposals` lists what a person applies with `fleet-hub
tracker section-map`; follow-ups are recorded in `work_admin update`.
Assist is usable one proposal at a time (`status_map::decide_proposal`,
by run id: apply / apply_as through `work_admin update`, reject → the
follow-up only, hidden until a new answer): Settings → Trackers on a
standalone desktop (`status_map_proposals` / `decide_status_map_proposal`,
`LocalOnly` when paired) and `fleet-hub decide proposals apply|reject`.
Phase 0 (offline) is built for J1 `work_link` and J3: `fleet-hub decide bench
work-link | status-map` (`service/decide/bench/`: BM25, leakage guard, time
split, calibration, the test map's acceptance lines, D39 `--export-unlinked`
/ `--labels`) with the `claude -p haiku` baseline (D33,
`service/decide/haiku.rs`: a named host of the SAME org only, prompt on
stdin). J1 has no live adapter: it waits on its acceptance lines.
Their diagnostics are built too (evidence, never an acceptance line):
`--perturb` (dataset C, `bench/perturb.rs`; J3 in `status_map_robust.rs`,
J1 in `work_link_robust.rs`), J3's paired languages (dataset B, `pair` ids,
`--paired-fixture`), `--floor-sweep` and `--question-set` (drafts in
`service/testdata/decide/questions/`, dev only), and `fleet_health.decide`
(`service::decide::health`, *degraded* per test map §7; the desktop's *Jev
degraded* Attention item). Label
hygiene (D34) is built: an agent never overturns a person's rejection,
`store::Decider` records `agent` / `agent_started` vs `manual` / `started`
(`PERSON_SOURCES` gate write-back, auto-trust and person counts), and a
person's Clear work holds against the unchanged branch / PR (R9u, migration
070 `work_unlinks`). The test map is
`docs/superpowers/specs/2026-09-27-jev-test-map.md`.
Decisions D31–D47 and what is still open
are in `docs/superpowers/specs/2026-09-27-jev-language-census-design.md`.
Five more use cases, K1–K5, were accepted by the owner on 2026-10-07 (test
map §5): K1 `start_project` next, K5 with the picker's phase 2, K2 and K3 as
shadow slots in Mode B and the mission loop, K4 last. K1 is built, off
(`decide.jev.start_project`, `service/decide/start_project.rs`): the start
popover pre-selects Jev's repository in assist; K2–K5 are not built.

Task → session (spec `docs/superpowers/specs/2026-10-06-task-to-session-flow-design.md`,
recommendations TS1–TS14 accepted 2026-10-07): A0, A1, A2 and C0 are landed, A3 in part.
The operator never accepts or rejects a proposal and is never a detection
or classification-nudge subject (`operator::is_operator_session`,
`detect::subject_state`). `work_link { preview_start }`
(`preview_start_work`, `tickets::preview_start`) answers a `StartPreview`:
the plan or what is `missing`, candidates, the brief and every conflict as
data; `parallel: true` starts beside a live session in a `<slug>-N`
checkout. The Work tab's `WorkButton` / `StartPopover` (`start_preview.ts`)
read it. The preview is its own action, never a flag on `start`: an older
hub answers it `E_INVALID` (the desktop then starts as before) instead of
ignoring an unknown field and starting a session (`wire_contract.rs`'s
`dry_run` lesson). A2: `work_link { switch }` (`switch_session_work`,
`Store::switch_session_work`) ends one link (`end_reason = 'switched'`)
and takes the primary in one compare-and-set; `ack_live: false` on `link`
/ `switch` refuses `E_EXISTS` with `details.live_elsewhere[]`
(`work::check_live_elsewhere`; absent checks nothing); `work { tickets,
include_local }` adds own tasks. Link windows (P-7) need no migration:
`work_journal.rs`'s `WINDOW_LO` / `WINDOW_HI` bound a link's journal by the
switches around it, and a link no switch touched reads as before. The
desktop's attach picker (`AttachPicker.svelte`, `attach.ts`) and Work on
task… (`SessionTasks.svelte`) read them. Not yet: the new task's brief
waiting for the next prompt (J3), own tasks in ⌘K.
C0: the operator is born with the `fleet-brainstorm` skill
(`skills/fleet-brainstorm/SKILL.md`, written to its directory's
`.claude/skills/` by `operator::operator_files`, so no catalog sync is
needed): Diverge → Converge → Decide → Plan, each stage closed by the
person; the plan becomes a task (`work_link create`, the plan in `notes`)
with proposed subtasks a person accepts. An operator born before C0 gets
it at its next birth. Not yet: plan rows, decision rows, the worker
catalog copy (C1).
A3 (part): a start writes its steps to the new session's timeline
(P-5): `start_spawned` (detail: `tickets::StartSpawned` JSON),
`worktree_ready`, `repl_ready` (`seed::REPL_READY`) and `handover_started`,
which is the spec's `brief_sent`. The Work button shows them as a strip
(`StartProgressStrip.svelte`, `start_progress.ts`) with "Waiting for you"
on a trust dialog and **Cancel start** until the brief is in.
`work_link { abandon_start }` (`work::abandon`, P-6, `Reach::Own`, confirmed
like a kill) refuses `E_DIRTY` unless the newest `start_spawned` made the
session's checkout and `git status` and the branch's own commits are both
empty; then it records `start_abandoned`, kills the session, runs `git
worktree remove` (no `--force`) and `git branch -d`. Not yet in A3: J7
multi-repo in the popover, the empty states.

Orchestration projects (spec
`docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md`,
recommendations O1–O12 accepted 2026-10-07): O0 is built.
`work_link { run, item_id, role? }` (`service/work/run.rs`) starts an
attempt at an existing item through the start path (its own worktree, the
brief) and tracks it as a `tasks` row naming the item, attempt and role
(migration 110); its first prompt carries the done marker, and no mirror
item is made. One open attempt per (item, role): a second run answers it.
Per-host tokens are refused; the operator's run is confirmed like a start.
O1 is built: missions ("Mission" in the UI, `orchestration_projects` in the
store, migration 115) with a root task, member tasks
(`work_items.orchestration_project_id`, one mission per item, 30 per
mission), a repo allow-list and a capped event log (`store/orchestration.rs`).
`work { missions | mission }` and `work_link { mission_save | mission_state |
mission_repo | mission_item | mission_delete }` (`service/work/missions.rs`)
fence by the mission's org first, then its owner or the org's members (only
an org admin changes one); per-host and peer tokens are refused. Seven routed
desktop commands (`commands/missions.rs`) back the Work view's Missions tab.
Projects carry no org, so the repo allow-list is not org-checked yet. The
phone does not show missions yet. O2 is built: `work_item_deps` and
`work_items.held_at` (migration 116), drawn by `work_link { dep | hold }`
(cycles and cross-org edges refused in the store, `store/item_deps.rs`);
`service/work/graph.rs` derives each member's state (ready, waiting,
blocked, running, failed, held, …) and wave on every read and gives an
active mission its `running | blocked | waiting` phase. An agent proposes a
plan with `work_link { propose_tree }`, whose proposals join the parent's
mission; a person takes it with `accept_many` and can `undo_accept` within
10 minutes while nothing has touched it. The Missions tab lists tasks by
wave. O3 is built: a run's prompt asks for a fenced JSON report after its
done marker, stored as `tasks.result_json` (the worker's word), and when the
run finishes fleet reads the commits and changed files from git in the
worker's checkout into `tasks.evidence_json` (migration 117,
`service/work/report.rs`). An item's `done_when` lines are typed (`ci[:check]`,
`review`, `test[:command]`, `person` or free text) and checked on every read
(`service/work/verify.rs`): CI from a fresh PR reading on the checkout's own
commit, review and test from a separate reviewer's or tester's run, and the
rest by a person's recorded check (`work_link { verify }`, a person's act).
The Missions tab shows each task's last attempt and Verified / Unverified.
O4–O8 are built: each active mission has a loop (`service/work/orchestrate/`)
that the hub runs, or a standalone desktop, on a 20 s tick, one mission at a
time under a lease (migration 118). Its deterministic steps (`steps.rs`) run
ready tasks within `policy.max_parallel`, retry a failure once with the error
in the prompt, ask a person when the same error comes back, start review and
test runs for `review` / `test:` lines, close a verified task and complete a
finite mission whose checks all hold. A locked `claude -p` planner
(`planner.rs`, no tools, worker text fenced as untrusted, all or nothing,
at most `policy.max_planner_runs_per_hour`) breaks a goal down and decides
what a failure needs; its commands become cards in the mission's confirm
queue. `git merge-tree` checks the finished branches against each other and
proposes a resolve task for a conflict (`integrate.rs`). What the loop does by
itself is the least of the mission's level, `orchestrator.max_level`
(default 1: a person presses every step) and a person's grant
(`work_link { mission_grant }`: level, hours, budget, hosts); brakes pause the
mission on a spent budget or no progress, and `missions_pause_all` pauses
every mission and ends its grants. `orchestrator.enabled` is the kill switch.
A continuous mission wakes on `policy.wake_every_secs`; a finished run, a
member's status moving on the tracker and a worker's PR checks moving each
wake its mission. The Missions tab has Start wave, the next steps, the
cards, the grant and Pause all; the phone (fleet-mobile) has the list, the
steps, the cards and Pause all. A mission's worker is refused a person's
step (`gh pr merge` / `ready`, a push to `main` / `master`, a tracker CLI)
by fleet's `PreToolUse(Bash)` guard (`orchestrate/guard.rs`): a shell
prefilter that asks the hub only about those commands and lets the command
run when the hub does not answer. Hosts get it on re-provisioning
(`provision_stale`); branch protection on the remote stays the backstop.

Reply actions are landed (#338): Copy, Quote, Retry, Fork here and Rewind
here under each reply; Fork, Rewind and Retry are one operation,
`rewind_conversation` (`service/rewind.rs`), which copies the transcript up
to the anchor into a new conversation and never changes the original.
Retry (the client's rewind + `send_prompt`) is offered only when
`ConvTurn.prompt_partial` is false. A rewind is refused unless the session
is quiet (live pane probe first) and without an anchor; a failed restart
reverts the binding (`Store::revert_rebind`) and removes the copy. Fork into
a NEW worktree (`new_worktree`, the Fork sheet's default) creates the
worktree first — a fresh branch at the source's HEAD, uncommitted changes
not carried — then writes the copy under its `pwd -P`, then starts in it;
a failure after the worktree removes the copy, the tree, the branch and
the row. Spec
`docs/superpowers/specs/2026-09-26-reply-actions-design.md`.

Local workspace sync, Phase 1, is built (spec
`docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md`): a
session's worktree bound to a folder on the desktop's machine and kept in
step both ways by a three-way engine over the existing SSH layer
(`service/local_sync/`, migration 109; the Local workspace card in session
details and a dot on the session row). A link belongs to the worktree
(host, owner/repo, worktree key), not the session. Writes are guarded on
both sides, two sides that changed differently become a conflict (Keep local
/ Keep remote, or by hand), and a folder that lost most of its files pauses
the link instead of deleting the other side. A desktop paired with a hub
syncs too, over its own SSH, with the session and project read from the hub;
hosts reached through `fleet-agent` are refused (no stdin). Phases 2 and 3 are built too (spec
`docs/superpowers/specs/2026-10-07-local-workspace-handoff-design.md`,
migration 112): Open in VS Code / IntelliJ / a terminal / the file manager,
a per-path log of which side changed what ("7 local changes"), Review
changes (git status and diff on the host, Commit, Discard), Ask AI about the
changes, Take over / Hand back to AI, Compare / Keep both / Ask AI to
resolve on a conflict, and an overview of every link with Clean up stale.
Symlinks sync as links (their target, never followed; not on a Windows
desktop). Not yet: a filesystem watcher, a three-way merge editor.

Chat forms, part 1 (spec 2026-10-07-chat-forms-design.md): the ask tool, fleet.form/1, the form card in the Conversation panel, secrets to host files (migration 119; guide `docs/forms.md`). Contract revision 9: the desktop and its hub ship together. Part 2 (forms in guide steps) is not started; the fleet-mobile card is its own plan.

Session state machine hardening (plan A, #343) is landed: a `working` row
with no activity for `reconcile.stale_working_secs` turns `idle` with
`stale_working_at` (migration 065); a StopFailure reads as failed; one
threshold, `health.context_red_pct`, drives `context_full`; the `oom`
playbook is capped by `playbooks.oom_max_attempts`; `gc.external_lost_ttl_secs`
ages out lost external rows. Attention reasons `stop_failed`,
`context_full`, `stale_working`, `ci_failing`. Plan
`docs/superpowers/plans/2026-09-27-session-state-machine.md`.

The stale-working acknowledgement (#381) is landed, per
`docs/superpowers/plans/2026-09-28-stale-working-acknowledge.md`: the tick
runs `Store::expire_stale_working`, which lifts the `stale_working_at`
stamp once the row is working / blocked again or older than
`reconcile.stale_working_ttl_secs`; an attach (`touch_session`) clears it
too. Migration 080 adds `stale_demoted_at`, the reconcile veto's own
memory: cleared by a hook, a pane that shows a live turn, or the row being
`working` / `blocked` again — never by an attach or the TTL — and while it
is set a turn-over check asks the pane (`store::trusted_status`). It is a
`#[serde(skip)]` `SessionRow` field: off the wire, and a change to it alone
emits nothing. Migration 081 adds `pane_working_at`, so a long tool call
whose spinner is on screen is not stale. The sweep judges only rows a
reconcile pass observed within the window (`last_reconciled_at`), so an
unreachable or unprobed host's `working` rows are never demoted.

Hub ops and accounting (plan D, #344) is landed: `fleet_health.hub`
(uptime, reconcile timing), process gauges on `/metrics`, a transcript's
first read booked as `backfill` apart from the day's live cost (migration
071 re-keys `usage_daily` by `(day, host_alias, backfill)`), and
`deploy/hub/backup.sh` / `upgrade.sh` and the `behind-proxy` compose; see
`docs/hub.md` → *Backups* / *Upgrade with the script*, plan
`docs/superpowers/plans/2026-09-27-hub-ops-accounting.md`.

Org administration (spec
`docs/superpowers/specs/2026-10-06-org-administration-design.md`) is landed
in four phases: A, the org overview in Settings → Organisations; B, the hub
tool `org_admin` (`service/org_admin.rs`) for devices and people; C,
per-org settings (`Spec::per_org`, migration 106 `org_settings`) and spend
and budgets (`usage_daily_org`, `service/org_spend.rs`,
`fleet_health.org_budgets`); D, members and roles (multi-user M2, migration
107 `org_members`). In D a device's org follows its person's memberships —
`store::effective_device`, applied to the auth rows
(`Store::auth_client_tokens`) and the `/events` re-check, never to listings
— and `org_admin` is `Access::Device`, deciding the caller's authority
itself (`org_admin::Authority`: the hub owner's device for the fleet, an
org admin's for that org; `check` refuses the rest). Team sharing is a grant
to an org (`session_grants.org_id`), reaching only members whose
`shares_since` is not after the grant. No admin reads a member's private
session.

Host identity and health (#354) is landed, per
`docs/superpowers/plans/2026-09-27-host-identity-health.md`: migrations
076 (`hosts.claude_version_at`), 077 (the health sample — disk / load /
mem / uptime, `health_at`, `last_hook_at`, `agent_version`) and 078
(`provision_fingerprint` / `provisioned_at`). The reconcile probe reads
versions every `VERSIONS_REFRESH_SECS` (6 h) and the health sample every
pass, which rides `host:pinged`; `fleet_health.hosts[]` (`disk_low` /
`claude_behind` / `agent_behind` / `hooks_silent`) is judged against
`health.version_max_age_secs`, `health.disk_low_pct`,
`health.claude_max_behind` and `health.hooks_silent_secs`. One rule,
`service::hosts::active_hosts`, picks the hosts of every host loop: a
hidden host is skipped by reconcile, not reaped. `merge_host` /
`fleet-hub host merge <from> <into>` retires a renamed alias (its
worktrees, sessions, usage, asset inventory, org rules and org move to the
target); a provisioning records its content fingerprint, so an older one
reads `provision_stale` (`fleet-hub provision --host <alias>
--content-only` refreshes it); `forget_project` drops a project row, and
`refresh_projects` drops rows that vanished.

Conversation event tracking is landed end to end (migration 037
`conversations` table; `SessionStart`/`PreCompact`/`PostCompact` hooks;
`/clear`, `/resume` and compaction tracked as conversation switches;
`session_conversations` API; the Conversations UI panel), per
`docs/superpowers/specs/2026-09-18-conversation-events-design.md`.

The desktop builds for Windows as a **client** (plan
`docs/superpowers/plans/2026-09-27-windows-desktop.md`, user guide
`docs/windows.md`): no `local` host (`retire_local_host`, as on a hub with
`hub.local_host=false`), no ssh multiplexing (`ssh::mux_supported`, off
there), the home/cache dirs through `fleet_core::home` only, and the hub
token in Credential Manager. Unix-only code and tests stay `#[cfg(unix)]`
(for a test module: a `#[cfg(unix)]` line above a bare `#[cfg(test)]`, the
form `no_eprintln_tests` recognises); in CI, `rust-windows` keeps the tests
and the Windows leg of `clippy` keeps the lints green there. `fleet-agent`
and `fleet-hub` stay Unix-only.
On Windows a WSL distribution is a host (`fleet_core::wsl`, alias
`wsl-<name>`): `SshClient::remote_command` and the PTY attach run it through
`wsl.exe … sh -c` instead of `ssh`, and it gets no reverse tunnel. The `ssh`
program is `ssh::default_ssh_binary()` everywhere (probes, PTY, tunnels):
`CLAUDE_FLEET_SSH`, else the Windows OpenSSH, else PATH. The Windows
bundle ships Microsoft's ConPTY (`conpty.dll`/`OpenConsole.exe`, which
portable-pty prefers to the built-in one) via `scripts/fetch-conpty.sh`
(pinned version + SHA-256) and `--config src-tauri/tauri.conpty.conf.json`
in release.yml and ci.yml; plain dev builds use the system ConPTY.

Hub↔hub federation (cycle 3) is landed: two `fleet-hub` daemons link with
`fleet-hub pair --mode peer` / `peer add|list|remove`, a dialer supervisor
and a `peer_exchange` listener carry messages both ways by fleet address,
and `fleet_health.peer_links_down` reports a link in trouble, per
`docs/superpowers/specs/2026-09-24-hub-federation-design.md`.

Application updates: design
`docs/superpowers/specs/2026-09-28-update-channel-design.md` (with
fleet-mobile's `docs/superpowers/specs/2026-09-28-mobile-update-adapter.md`).
The Hub is the policy authority and the release key (minisign) the content
authority. The manifest is two signed documents, a per-release manifest plus
a per-track channel doc on the `update-channels` branch. The `/update` wire is
frozen and exempt from `E_HUB_CONTRACT`. `fleet-updater` rolls the hub
container back, including the DB restore. **S1 is landed:**
`crates/fleet-update` (Tauri-free, no fleet-core dependency; version-exempt
like fleet-core) holds the manifest / channel types, `verify` (`verify_target`
is the one check before any install), the pure `decide()` over the shared
fixture `tests/decide_cases.json`, `UpdatePhase`, and `UpdateChannel` with
`GitUpdateChannel` / `HubUpdateChannel`. **S5 is landed too:**
- `fleet-hub backup [--prefix|--to] --json` (`store::backup`, a
  read-only `VACUUM INTO`, never migrates);
- `fleet-hub healthcheck --ready --json`, which reads the readiness file
  `serve` rewrites every 5 s (`fleet-hub/src/ready.rs`, `<data
  dir>/run/ready.json`), so `/healthz` stays unversioned;
- the build identity from `crates/fleet-hub/build.rs` (`FLEET_GIT_SHA` /
  `FLEET_BUILD_ID`, passed by `release.yml` and the `hub-image.yml` build
  args).

**S4a (the hub side) is landed:**
- migration 079 (`update_desired`, `update_observed`, `update_events`, and
  `update_docs`, the signed-document cache, re-verified on every read);
- `service/update/` (`check` / `report` / `status` / `pin` / `refresh`,
  plus the refresh tick in `fleet-hub serve`, which records `hub:self`);
- `POST /update/check` and `POST /update/report` (`mcp/update_route.rs`,
  behind `authorize`; the caller's identity comes from its token);
- `TokenMode::Updater` (`fleet-hub pair --mode updater`): `/update/*` only,
  refused by every tool, `/events` and `/report`;
- the tools `update_status` (client, read-only; a scoped caller sees only
  itself) and `update_admin` (master only);
- the `update.*` settings, with their Settings → Updates rows, and the user
  guide `docs/updates.md` (every `update.*` setting must be in its table).

**S2 (publishing) is landed:** release.yml's `manifest` job signs
`release-manifest.json` from 0.4.1 (`scripts/release-manifest.sh`, the
windows from the shipped `fleet-hub compat`; `verify-release` requires it
through the `manifest` leg of `release-assets.sh`), its `channel` job and
`update-channels.yml` write the signed `stable` / `beta` channels on the
orphan branch `update-channels` (`scripts/update-channels.sh`, the
`fleet-release` bin of fleet-update), and every document is checked
against `keys.rs` before it leaves the runner
(`scripts/release-update-scripts-test.sh`, CI hub-headless). See
`docs/RELEASING.md` → *Update manifest and channels*.

**Half of S4b is landed:** `X-Fleet-Client` (`fleet_update::client_header`)
recorded into `update_observed` on `last_seen_at`'s once-a-minute beat in
`authorize`; the `update:changed` row event (kind `update`, ids only, in
`HOST_BOUND_HIDDEN_KINDS`); `fleet_health.updates` (`service::update::health`:
`update_required`, `update_failed`, `update_rolled_back`, `rollback_failed`,
`channel_stale`); and `update_status { target }`, the design's
`update_check_for`. Left: the per-target `update:decision` push, hub-e2e
section U, rollouts (S9).

**S3 is landed:** `service::update::git_check` (Git mode: a `GitCheck`
from the hub's own settings, pin and last-seen sequence) and `fleet-hub
update check [--track] [--json]`, which reads the published channel and
prints what this build should run, verified; it installs nothing.

Trusted keys are `fleet_update::keys::RELEASE_KEYS`: the owner's release
key (made on the owner's machine only, `scripts/release-key.sh`) is trusted
since a3033c2 / #384 (v0.4.1); the secret half is only the
`RELEASE_SIGNING_KEY` repository secret and the owner's backup. The first
channel was published with v0.4.1 (2026-09-28); `stable` / `beta` list every
release since. `FLEET_UPDATE_E2E_KEYS`
(read by `e2e` builds only) is reserved for S4b's hub-e2e section U;
nothing uses it yet. `update.track` offers `stable` / `beta` only until S2b
publishes `nightly` (a stored `nightly` resolves to `stable`). S2b
(nightly), the rest of S4b and S6–S9 are not built; the
other §13 questions wait on the owner.
