# Status

What is landed, what is built but off by default, and what waits on the
owner, by feature area — with the spec or plan behind each. Moved out of
`CLAUDE.md` (which every session loads) on 2026-10-06. When a feature lands or
a default flips, update its paragraph here; when a paragraph stops being true,
delete it rather than adding a correction after it.

The Orbit Fleet redesign (plan
`docs/ux/2026-10-08-orbit-fleet-redesign/transition-plan.md`, parity
contract `docs/redesign/parity.md`) is landing step by step, and its New
layout is built but OFF by default: Settings → Appearance → Layout picks
Classic or New (`ui.layout`, default `classic`, `src/lib/prefs.ts`). New has
a rail (`src/lib/rail.ts`: Control, Inbox, Sessions, Work, Automation,
Accounts, Toolkit, Settings), the session inspector, and Get started with a
first-run tour (10.5). Control is the operator's place: its Chat and Today
tabs, the Views panel (9.4, 9.5), confirmations as cards (9.2), "Sent to a
session" and "Sent to a mission" receipts (9.3, migration 140), tasks in
Control (9.6) and the Library (9.7, `library_items`, migration 143, the
`library` tool). New became the default in step 7.6 (#657) and Classic
was removed in step 13.1 (#699). The redesign's backend is not behind
the switch: M2's session facts (`sessions.agent`, `origin`,
`last_viewed_at`, `turn_outcome`; migrations 121, 124, 125, 129) and
account usage history (122), the Hosts page's probe facts (123), token use
(126), the cost of fleet's own `claude -p` runs (127), every PR a session's
branch has had (`pull_requests`, 128), cost per account (130), prompts
queued for a busy session (133), the agent CLIs a host has (134) and the
add-host wizard's state (135). The hub contract is revision 16
(`CONTRACT_REVISION`, `crates/fleet-core/src/wire_contract.rs`):
revisions 11 to 14 add tools a revision-10 hub does not serve, so the
desktop and its hub are upgraded together; 15 widens the form spec and
adds the attention model's two classes (a mission waiting on a person, and
Jev's "probably waiting" kept apart from Needs you) and no tool; 16 adds
named tokens (`api_tokens`) and + Add account (`add_account`), so a
desktop accepts only a revision-16 hub.

The canvas gap plan (M15, steps G0.1 to G6.2; transition plan section M15)
landed in #779, #812 and #814 and fleet-mobile #192 to #198: the form spec's
contract-15 fields and the form kit, settings with one Save bar, mission asks
and Jev proposals in the attention model, task due dates, timed Send later,
routine event triggers and guards, named Control API tokens, + Add account,
org switches and the project catalog, access requests for shared sessions,
and the phone's bottom sheets, chat forms and Inbox rows (migrations 153 to
160). The re-run audit (`docs/redesign/canvas-gaps-2026-10-10.md`) closes
233 of 396 rows and leaves 36 missing and 127 partial. Waiting on the owner:
tracker write-back from Fleet (D3, D29), start rules that name a host,
account, model or agent, and the six items the plan left out on purpose
(Wake host, liquid orbit for rebase, the database upgrade ring, waiting for
the other hub to sign, Bedrock and Vertex accounts, hub settings that follow
an org).

A session start reports its three real steps (worktree, tmux, agent) as
`start:progress` frames (redesign 5.13, `service/sessions/start_progress.rs`):
`new_session` takes an optional `start_token` the client mints, and the frames
carry that token and nothing else (event kind `start`, content-free on a
person's `/events` stream), so the New session dialog's Pulse sequence and its
Hex field move on the backend's events, and a desktop paired with a hub gets
them over the hub's stream. Shell terminals (5.3) can open in the home folder
as well as the worktree (`shell_terminals { at: "home" }`, the strip's "New
terminal opens on" picker), and a terminal's ⋯ menu has Kill terminal…, which
asks first. 13 of the 16 shortcut scopes match keys through the registry
(`shortcuts.ts` `MATCHED_SCOPES`, `viewKey`); the Quick switcher's two and
the New session dialog still read `e.key`, since they take some keys under any
mix of ⌘ and Ctrl.

Sessions can run OpenAI's Codex CLI as well as Claude Code (redesign 12.2,
`agent_adapter::CodexCli`, `sessions.agent` from migration 121):
`new_session { agent: "codex" }` refuses a login profile, restart,
recreate and repair resume the row as Codex, and its rollout feeds the
Conversation tab. The New session dialog offers Codex only on a host that
has it on its PATH (12.4b, migration 134). `move_session` refuses a
non-Claude row, because only Claude's state is carried. Agy (12.3) is not
startable: its adapter (`agent_adapter::agy`) is provisional, with no
captured pane fixtures and its SQLite transcripts unread, so the picker
shows it as coming and `new_session { agent: "agy" }`, restart, recreate and
repair refuse it with `E_UNSUPPORTED`
(`sessions::refuse_unvalidated_agent`); `move_session` refuses it too. The
adapter code stays, for when real captures validate it.

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
through the profile. The Accounts page (redesign 4.1/4.2) shows each
account's sessions, switched-on routines and spend today from the
`usage_daily_account` roll-up (`account_spend`, local only, so a paired
desktop shows no $), and an account at its limit says how many sessions it
paused, with Show and *Switch to <account>* (step 4.4's bulk move, one
click). Not built: a hub vault for setup-tokens and API keys,
and automatic switching when an account hits its limit.

The headless `fleet-hub` daemon, `fleet-agent` for hosts the hub cannot reach
over SSH, paired-client access for phones/browsers, and hub-client mode
(pairing the desktop itself to a hub) are landed; see `docs/hub.md`. Their
live acceptance (#156) is recorded in `docs/hub-acceptance.md`: the desktop
half is partly observed, the TLS, phone and agent-host steps wait on the owner.
The hub's fleet-agent install job (redesign 4.9, `service/agent_install.rs`)
starts from a paired desktop's Host detail (*Install <version>* on the
fleet-agent row, the job's steps beside the Hex field) and from the add-host
wizard's fleet-agent row where the checks say a hub could use one; a
standalone desktop reaches its hosts over SSH and offers none. Since contract revision 5 a hub client adds
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
M9.7 (the operator's starts and kills always confirmed; on a hub they wait,
`E_CONFIRM_REQUIRED`, for the owner's paired device to answer through
`mcp_confirms` / `answer_mcp_confirm`, redesign 9.2),
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
Redesign 6.3 in the Work UI: a task with an open dependency shows as *Needs
you* with its reason line ("Blocked on TASK-212", the plan's status-word
decision), and its spend (`cost_micros`, its sessions each once) shows on
the List rows, the Board's cards and Task details (which links what it
waits for); the grouped tree's org and group rows carry their tasks' spend.

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
The Work view's Sprint / Release axis (§6a) and bulk assignment (§6b) are
built: `filters.group_by: sprint | release` (`view::regroup`, the bucket of
each item read once per tree by `Store::bucket_membership`, fenced by
`sees_org`), a section header with the bucket's roll-up, a selection in the
Grouped tree that plans tasks into a sprint or release (moving one out of
the sprint that held it) or takes them out, and *Sprints & releases*
(`WorkBuckets.svelte`): create, start, release, close with the carry-over
confirmed (E9) and delete. Desktop commands `work_buckets` / `work_bucket`
(→ `work`) and `add_work_to_bucket` / `remove_work_from_bucket` (→
`work_link`) route; so does `work_bucket_admin`, as `work_link { action:
bucket_admin, bucket_op }` (owner decision 2026-10-10,
`buckets::person_admin` / `may_plan`): a person creates and changes
personal sprints and releases of their own (migration 162,
`work_buckets.owner_person_id`; one current sprint per owner, a personal
one wins a group by sprint for its person, never linked to a tracker), and
an org's team ones as its admin — or member, when the org turns the per-org
`work.members_plan_sprints` on (off by default; a viewer never). Standalone
it stays `work_admin`. The board is scoped to a sprint (§6c): a picker over All tasks, each
open sprint and *No sprint (backlog)* (`boardScope`, kept per machine; a
sprint is the section of a group by sprint, `boardFilters`, so no new read),
the sprint's roll-up, dates and goal above the columns with *Start sprint* /
*Close sprint…* (the close dialog of E9), and Done holding everything the
sprint delivered rather than the last week; a sprint that closes falls back
to All tasks. Epics for local items (phase 4, §3) are built: `work_link {
edit, epic }` marks a top-level local item an epic (`kind = 'epic'`,
`Store::set_local_epic`), `work_link { set_parent }` (`set_work_parent`,
routed) files a local item under an epic or a task or takes it out to the
top (`Store::set_local_parent`), `filters.group_by: epic` sections the Work
view by the epic a task is or is filed under, and every task carries its
children's roll-up (`children_total` / `children_done`, computed, never
closing the parent). The selection's *Under epic…* files tasks in bulk, the
Edit dialog's *Epic* box marks one, and the row and the Board card show
*Epic* and *n/m done*. Depth stays ONE, not the spec's three: a local item
with a parent is never a parent (`parent_for_new_child` now checks every
local item, not only a native one), because `item_org` — and the org fences
built on it — walks one level; a filed task cannot hold subtasks, and a task
with subtasks is not filed. Going deeper is the owner's decision (it moves
the org fence). Not yet: the phone. E9–E11 run on their defaults.

Task comments (migration 161, `work_item_comments`; the Comments tab G3.4
had cut is back on the owner's word, 2026-10-10): `work_link { comment,
item_id, notes }` (`comment_on_work`) and `{ comment_delete, comment_id }`
(`delete_work_comment`), both routed. A comment is about the ITEM — its
org fence, `edit`'s person gate for writing — stays in fleet (never a
tracker's), and is deleted by its author alone (the person when both sides
prove one, else the caller's label). `work { task }` serves them oldest
first with `mine`. Device names are their person's (owner decision
2026-10-10): a comment's author shows to its own person only, and
`Placement.updated_by` (a device label with no person) only to the hub
itself and the one person of a one-person hub. The
task page's Activity tab also lists each session that started, was
suggested, turned down or stopped, and each comment. Not yet: comments on
the phone.

Task attachments (migration 165, `work_item_attachments` and the
content-addressed `work_attachment_blobs`, deduplicated by SHA-256 and kept
in SQLite so a hub backup carries them; a blob goes with the last live
attachment naming it): `work_link { attach, item_id, name, mime,
data_base64, comment_id? }` (`attach_to_work`), `{ attachment_delete,
attachment_id }` (`delete_work_attachment`) and `work { attachment,
attachment_id }` (`work_attachment`, the bytes as base64), all routed. The
fences are a comment's: the item's org fence (outside it, unknown), `edit`'s
person gate for adding, its author alone for deleting; `work { task }`
serves `attachments` (metadata only, newest first, with `mine`), the author
withheld as a comment's. At most `work.attachment_max_mb` (default 10,
1–16): the ceiling is the transport — a routed attachment crosses as base64
in one `/mcp` request or answer, so the hub's request cap (`MCP_BODY_MAX`)
and the desktop's response cap (`http_client::MAX_RESPONSE`) went from 8 MiB
to ~23.4 MiB (`store::ATTACHMENT_WIRE_BYTES`). Names are a file name (no
path, no control characters, ≤ 200 chars); types come from a short
allowlist, an image's only when its bytes prove it, anything else stored as
`application/octet-stream`; SVG is refused. The task page's Overview has an
Attachments section (add, drop, or paste an image anywhere on the page;
images open in a lightbox, other files download); an image pasted into the
comment composer is attached there. Not yet: attachments on the phone, a
tracker's own attachments (`source`/`external_id` are reserved for them).

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
stdin). J1's live adapter is built, off (`decide.jev.work_link`,
`service/decide/work_link.rs`, redesign 6.8): in assist it pre-selects a
suggestion shown as "Proposed by Jev" (rule R12) on the session row, in
Review and at the head of Session Details' timeline (Link / Not this, and
who proposed a link after it is confirmed); shadow writes no link, so none
of them shows anything, and moving it past shadow waits on its acceptance
lines.
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
popover pre-selects Jev's repository in assist. K2 `control_route`, K3
`mission_triage`, K4 `duplicate` and K5 `work_placement` are built with the
redesign, each off, as are the redesign's other use cases (next paragraph).

The redesign (see the top of this file; New becomes the default layout at
step 7.6, parity audit `docs/redesign/parity-audit-7.5.md`) builds its AI
use cases OFF (`decide.jev.*`, each `off` by
default, guide `docs/decisions.md`): `sibling_repos`, `host_placement`,
`quick_answer`, `adopt_target` / `restore_target`, `turn_outcome`,
`duplicate`, `related_session`, `work_placement`, `routine_run_outcome`,
`control_route`, `mission_triage`, `summary_check`, the PR shepherd's
`pr_triage`, Review's J6 `main_ticket` and J7 `tracker_duplicate`
(6.8), and New session's N2 `resume_or_new` (gap plan G7.10, no
benchmark set yet). Each other closed-choice use case has a benchmark set, `fleet-hub decide
bench <use case>` (`bench/choice.rs`, docs/decisions.md *Benchmarking the
closed-choice use cases*), and J2's `turn-outcome` set holds 51 `asked`
tails; every built-in set is SYNTHETIC (no recorded data yet), so nothing
is judged and every use case stays `off` / shadow. J8 shows a warning in
the agent tab when the pane rules could not read a turn's end. J4's context
order for a drafted brief is a word-overlap rule, not a Jev use case. The ones that send
Claude's reply text (`turn_outcome`, `routine_run_outcome`) also need the
org's reply-text consent (D48), off by default.
**Waits on the owner (11.11):** the plan says Jev checks the "Since 13:20"
summary against the transcript (J9) *before* it shows. With
`decide.jev.summary_check` off — the default, like every Jev use case — the
summary still shows, unchecked, labelled "Drafted · Not checked" beside the
text (`WatchSummary.svelte`); a failed check (or one that could not run)
hides it. Whether an unchecked summary should show at all, or the use case
should default to `shadow`/`assist`, is the owner's decision: it trades a
summary every watcher can read for Jev budget and the org's consent. The notifications matrix and quiet hours (`notify.*`, 11.9) are
on: the phone and the desktop (while its window is in the background, and
only with OS notifications on for the desktop column) follow them.

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
Projects carry no org, so the repo allow-list is not org-checked yet. O2 is built: `work_item_deps` and
`work_items.held_at` (migration 116), drawn by `work_link { dep | hold }`
(cycles and cross-org edges refused in the store, `store/item_deps.rs`);
`service/work/graph.rs` derives each member's state (ready, waiting,
blocked, running, failed, held, …) and wave on every read and gives an
active mission its `running | blocked | waiting` phase. An agent proposes a
plan with `work_link { propose_tree }`, whose proposals join the parent's
mission; a person takes it with `accept_many` and can `undo_accept` within
10 minutes while nothing has touched it. The Missions tab lists tasks by
wave; in the New layout its Graph view draws them as lanes (repo, assignee or
none) × waves with the dependency arrows, the critical path and the progress
per lane and wave (`src/lib/mission_graph.ts`), read-only. A mission in
`plan` mode is tracked, never run: the loop takes no step, card, planner run
or grant on it, and it holds up to 200 tasks (`PLAN_MISSION_ITEM_CAP`) where
a finite or continuous one holds 30. Import plan (`work_link { mission_import
}`, `service/work/plan_import.rs`) reads a markdown plan's step tables
(`#`, `Step`, `Needs`, optional `Lane` / `Status`, plus a `Lane | Steps in
order` table) into the mission: each step a local task titled `<step>
<title>` with its lane as its assignee, its needs as edges; importing again
updates them in place. O3 is built: a run's prompt asks for a fenced JSON report after its
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
(`work_link { mission_grant }`: level, hours, budget, hosts, and the login
`profile` its runs bill, migration 137); brakes pause the mission on a spent budget or no
progress, a run on an account at or past `accounts.pause_at` waits (one
`account_limit` event says why) until the account is back under it, and
`missions_pause_all` pauses
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
In the New layout, Jev K3 (off) proposes a stuck mission's outcome and next
step, and Control shows a "Sent to a mission" receipt for what the operator
handed over (`control_handoffs`, migration 140, redesign 9.3).

Routines are landed (`service/routines/`, migrations 131 and 139, redesign
8.5 to 8.10): a saved prompt that starts a session on a cron schedule, on a
fleet event or when a person presses Run now. Since M15 G2.4 (migration
156) the events include a pull request's review, failing or passing checks
and merge, read from the `pull_requests` changes reconcile records, with a
repo filter, "me or anyone in its org" and a rate per PR. The scheduler stops on
`automation.paused`, a fire past the routine's day budget is skipped, a run
past its run budget fails and pauses the routine, and a run whose account
is at or past `accounts.pause_at` is skipped (8.7). M15 G3.8 (migration
159) adds the guards: a fleet daily budget for every routine's runs
(`automation.daily_budget`), a per-run time cap, a fallback host, retry
once, an autonomy line under the prompt, and a named fix on a failed run
from its error code. Each finished run
records what it came to (8.10; Jev N6 is off), and a failed run shows in
the Inbox. `runs` (8.3, migration 141) lists tasks, missions, Jev,
`claude -p` and routine runs in one list. Start rules (8.11,
`service/start_rules.rs`, migration 144) send a task key pattern to a
project, and optionally a host, before Jev K1 is asked; after five starts
of the same prefix in the same project, the start preview offers the rule.
The New layout's Automation screen holds Routines, Rules, Runs and Agents.

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

Chat forms, part 1 (spec 2026-10-07-chat-forms-design.md): the ask tool, fleet.form/1, the form card in the Conversation panel, secrets to host files (migration 119; guide `docs/forms.md`). Contract revision 9: the desktop and its hub ship together. The app's own wizards (New session, Add host, Pair a device, Add project, Link to a hub, Get started) are fleet.form/1 specs too (redesign 10.12, `src/lib/forms/wizards/`, `docs/forms.md` → *Wizards are forms too*). As dialogs: Link to a hub (Settings › Hub), Pair a device (Settings › Devices) and Get started's first session (`get_started`). In the chat (`ChatForm` at the end of the Conversation panel and of Control's chat): an agent's `wizard` block (`docs/chat-blocks.md`) opens Add host, Add project, Get started, New session or Pair a device, and Control's *Add project* chip opens Add project; nothing runs until the last button. An agent may stream a long form while it writes it, `ask { draft }` (migration 153, the row's `form_draft`), which the chat draws in as skeleton fields until `ask { form }` opens it. The phone's form sheet (14.7) is fleet-mobile's. Part 2 (forms in guide steps) is not started; the fleet-mobile card is its own plan.

PR shepherd, steps 1 to 4 of 5 (step 2 without its Inbox card), are built and do nothing until a person
grants a rule (spec `docs/superpowers/specs/2026-10-08-pr-shepherd-design.md`):
migrations 136 (`pr_shepherd_rules`, `pr_shepherd_episodes`) and 145
(`pr_shepherd_merges`),
`service/pr_shepherd/` on the reconcile tick (loop `pr_shepherd`, stopped by
`automation.paused`). For a project with a rule it
reads each session's `pr_evidence` and, once per conflict / behind / red CI
on a pushed commit, records a `pr_shepherd` timeline event (`watch`) or also
asks the session's own Claude to fix it (`nudge`). At `merge` it also merges
a PR GitHub confirms green right before the merge, one per project every
180 s, with `--match-head-commit`. Rules are granted with `fleet-hub
shepherd grant | revoke | pause-all | status` or the control API's
`pr_shepherd` tool, which is served only to the owner's own paired device.
Jev's `pr_triage` (`decide.jev.pr_triage`, off by default) records in
shadow what each new episode most likely needs. Not built: the Inbox
card, the UI.

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
in four phases: A, the org overview in Settings → Organisations (since
the OrgOverview / OrgSpend boards, the page's tabs Overview, Members,
Devices, Spend and Settings: budget tiles with Meters, "Needs an admin"
lines that open the tab to act on, and the 14-day spend chart with the daily
budget drawn across it); B, the hub
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
session. A grant's level is `watch`, `answer` (may also answer the dialog
on the session's pane; migration 142, redesign 11.7a) or `drive`. An org's
spend is also kept per person (`usage_daily_person`, migration 138,
redesign 11.8), shown all or nothing.

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
`docs/superpowers/specs/2026-09-24-hub-federation-design.md`. Since
contract revision 12 the desktop links and unlinks a peer through the
hub's `link_peer` / `unlink_peer` tools (redesign 11.5), and each link's
traffic is recorded for the Federation page (migration 132). The page
draws the links as a graph above the list (the spec's `graph`: solid line
up, dashed down), and Link a hub is the two-step
`link_peer` wizard (address, then the code) with a Counter-orbit while the
hubs trade keys (11.12). The link protocol has no step that waits on the
other side's approval — its operator mints the code first, and redeeming it
completes the link — so the wizard never says "waiting for … to sign".

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

**S4b is landed** (its rollouts landed with S9): `X-Fleet-Client` (`fleet_update::client_header`)
recorded into `update_observed` on `last_seen_at`'s once-a-minute beat in
`authorize`; the `update:changed` row event (kind `update`, ids only, in
`HOST_BOUND_HIDDEN_KINDS`); `fleet_health.updates` (`service::update::health`:
`update_required`, `update_failed`, `update_rolled_back`, `rollback_failed`,
`channel_stale`); and `update_status { target }`, the design's
`update_check_for`; `update:decision` on `/events`
(`service::update::push_decisions`, woken by a pin, a refreshed channel or an
`update.*` setting; the desktop checks again on it); and hub-e2e section U (an
`e2e` hub fetches a channel signed by a throwaway key from a fake GitHub
through `FLEET_E2E_UPDATE_PORT`, and a paired desktop is told
`update_available`, `update_required` and `client_too_new`).

**S6 is landed, opt-in:** `crates/fleet-updater` (image
`ghcr.io/martin-janci/fleet-updater`, built beside the hub's by
`hub-image.yml`) and the compose profile `auto-update` in both compose
files, plus `deploy/hub/fleet-updater.{service,timer}` for running one pass
from systemd instead of keeping the Docker socket in a long-running
container (owner's §13.4 answer: ship both). It asks `/update/check` as
`hub:self`, verifies the target, pulls by the signed digest, backs up
through `fleet-hub backup`, recreates the hub container on the new image,
gates it (running, healthy, `healthcheck --ready --json`, the manifest's
version / commit / build, the soak) and otherwise rolls back, restoring the
backup when the candidate may have migrated (§13.5: the validation window's
writes are lost and reported). Under `update.hub.mode=notify` (§13.6: the
default stays `notify`) it installs only a pin, a required update or a
rollback. It keeps the previous build's config and image id in its state
file instead of a renamed `-prev` container, so compose never sees two
containers for one service. `scripts/updater-e2e.sh` (CI hub-headless,
`ci-local.sh --updater-e2e`) runs it against a real Docker daemon. Not
built: its own self-update (its token does not reach `/events`, so it
keeps to its interval).

**S7 is landed:** the desktop updates itself (`src-tauri/src/self_update.rs`,
`update_check` / `update_install`, both `SameInBoth`; `src/lib/updates.ts`,
`UpdateBanner.svelte`). Paired it posts `/update/check` through
`HubBackend::post_update`, the one call outside the contract gate
(`update_routes_are_the_only_contract_exemption`); standalone it runs
`git_check` under its own settings. `tauri-plugin-updater` installs only the
target `verify_target` passed, fed from a one-shot loopback endpoint, and
checks the bundle's minisign signature again. Owner's answer to the Tauri
key: the release key itself — no `TAURI_SIGNING_PRIVATE_KEY`. Tauri's signer
cannot use the unencrypted minisign key, so `release-manifest.sh` signs the
`.app.tar.gz` / AppImage / NSIS bundles with `minisign` and the signatures
(`version:` in the trusted comment, `requireSignedVersion`) ride in the
manifest as `Artifact::Tauri`. Every desktop request carries
`X-Fleet-Client` (`fleet_core::http_client::set_client_header`). A `.deb` is
offered as a download. The footer's hover title already names both versions.

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
(read by `e2e` builds only) is what hub-e2e section U and
`scripts/updater-e2e.sh` sign with. **S2b is landed:** `nightly.yml` cuts a nightly of
a green `main` commit (`scripts/cut-nightly.sh`: scripts/release.sh's own
commit on top of it, never pushed to a branch, only its tag) and dispatches
the same `release.yml` / `hub-image.yml` at it — every green push, desktop
bundles included, as `X.Y.Z-dev.N.g<sha>`. Each one is listed on the `dev`
track at once and on `nightly` when that has not moved for two hours
(`NIGHTLY_EVERY_SECS`). `update.track` offers `nightly` and `dev`;
Git mode scans past releases without the caller's artifact, and the hub
keeps 20 manifests. Pruned to the newest 15 plus whatever `nightly.json` lists. §13 question 7 waits on
the owner (2 is answered: a signed amendment).

**S8 is landed:** fleet-mobile's release sends `repository_dispatch`
`android-release` (secret `FLEET_DISPATCH_TOKEN` there; without it the step
stands down with a notice) and `android-amendment.yml` signs the APK into the
release's manifest as an amendment, listed on every track that carries the
release. A phone asks its hub (`/update/check`, `X-Fleet-Client` on every hub
request), falls back to GitHub releases only against a hub with no
`/update/check`, and checks the APK's sha256 and its signer against both the
decision's `signer_sha256` and the installed app before Android's installer.

**S9 is landed.** Staged rollouts (migration 151 `update_rollouts`,
`service::update::rollout`; `update_admin` `rollout_start` / `rollout_pause`
/ `rollout_resume` / `rollout_abort`; waves advance on the decision pusher's
five-minute beat after `update.rollout_wave_secs` and pause themselves at
the halt ratio, `fleet_health` `rollout_paused`; `update_status.rollouts`);
the maintenance window (`update.window`, UTC, holds only `automatic`
components); per-org policy (migration 152 `update_org_policy`,
`update_admin set_policy` / `clear_policy`: an org's mode, floor, window and
pin for its clients and agent hosts; the master for any org, an org admin
for their own through `update_policy`); the artifact mirror (`update.mirror`,
OFF by default; `GET /update/artifact/<sha256>`, `target.mirror`, used by the
agent, the bare hub, the desktop and the Android app, each falling back to
GitHub); update now (`update_admin update_now`: pins the component, wakes
the hub's updater through `<data_dir>/update-now` and pokes agent hosts'
`fleet-agent-update.path`, decisions re-checked every two minutes while
pinned); and the binary target
(`fleet_updater::binary`, now a library too): `fleet-agent update` with
`install --auto-update`'s timer, and `fleet-hub update apply` / `pair` with
`deploy/hub/fleet-hub-update.{service,timer}` for a hub without Docker. The
binary loop is tested against a pretend machine (`binary::tests`); it has not
run against a real systemd yet.

Debug devices' first slice is landed (`docs/debug-devices.md`): per-host
inventory of Android phones, emulators and AVDs, iOS simulators and paired
iOS devices (migration 120), the `debug_devices` control-API tool (list,
scan, claim / release, run, install across hosts, logs, screenshot, boot,
shutdown; configure / forget for a person) and the desktop's Debug devices
page (the `debug_device` resource, routed to the hub, contract revision
10). Not built: an adb-server bridge so native tools on one host reach
another host's devices, live screen and input, physical-iOS logs, and a
device page in fleet-mobile. None of it has run against real hardware yet:
the scripts are tested against stub `adb` / `simctl` output.
