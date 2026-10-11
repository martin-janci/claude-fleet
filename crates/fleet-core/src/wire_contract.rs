//! The wire-contract revision: one additive integer the hub's `/events`
//! hello frame carries (`contract`) so a client can tell whether the row
//! shapes and tool results it depends on are the ones actually on the wire.
//!
//! # Why this exists
//!
//! A hub-client desktop deserialises hub tool results straight into the same
//! `fleet-core` row structs the hub serialised them from
//! (`src-tauri/src/backend/contract.rs` explains why that symmetry is a
//! hole: a renamed optional field does not fail to parse, it silently
//! defaults). Before this field existed the hub's version was logged and
//! nothing more — there was no way for a client to tell "an older hub, whose
//! rows I still understand" from "a hub whose rows changed shape under me".
//!
//! `CONTRACT_REVISION` is that signal. It is not the crate version and not
//! the app version (both change on every release, including ones that touch
//! nothing a client reads); it moves only when a client's assumptions about
//! the wire would actually break.
//!
//! # When to bump this
//!
//! Bump [`CONTRACT_REVISION`] for a change that **removes or renames a row
//! field, or changes what an existing field means**, on any type a client
//! deserialises off `/events` or a tool result — the set
//! `src-tauri/src/backend/hub_contract.golden.json` pins.
//!
//! Do **not** bump it for an additive change **an older client is built to
//! absorb**: a new field (row structs carry `#[serde(default)]` precisely so
//! an older client tolerates one), a new row type, or a new event kind. An
//! unknown field is ignored and an unknown event name is dropped by
//! `known_event_name`, so bumping for those would make a perfectly
//! compatible client refuse a hub for no reason.
//!
//! An addition an older client **cannot** absorb is not one of those. Two
//! shapes of it have bitten this build, and both belong in the "bump" list:
//!
//! * a new variant of an enum a client deserialises — serde fails the whole
//!   payload on a tag it does not know, so one new `ConvItem` kind costs an
//!   older desktop the entire `Conversation` rather than one line;
//! * a command the desktop now routes to a hub **tool that did not exist
//!   before** (an older hub's router answers "unknown tool", which no
//!   `#[serde(default)]` can soften).
//!
//! A client compares this against the range of revisions it understands
//! (`MIN_HUB_CONTRACT`..=`MAX_HUB_CONTRACT` in the desktop's
//! `src-tauri/src/backend/contract.rs`) and, outside that range, does not
//! trust the hub's rows at all rather than risk showing a stuck or lost
//! session as healthy.
//!
//! # Revision history
//!
//! - **1** — the mechanism's own introduction (#148): no prior revision to
//!   compare against, so this is the bootstrap value every hub and this
//!   build started at together.
//! - **2** — `move_session` answers a tagged `MoveOutcome`
//!   (`{"kind": "moved" | "preview", ...}`) instead of a bare `MoveReport`,
//!   and honours a `dry_run` argument. An older hub's answer has no `kind`,
//!   which this build can no longer parse for a real move (`E_PARSE` after
//!   the move already happened); and an older hub's `MoveSessionParams`
//!   silently ignores an unknown `dry_run` field and runs a real move where
//!   this build asked for a read-only preview. Both are exactly what this
//!   mechanism exists to refuse instead of risking.
//! - **3** — `move_session` takes a `when` argument (`now` | `idle` |
//!   `cancel`) and can answer `MoveOutcome::Waiting` or `WaitCancelled` in
//!   addition to `Moved`/`Preview`. An older hub ignores `when` entirely: for
//!   `idle` that is harmless (it sees no such field and refuses a busy
//!   source exactly as it always has), but for `cancel` it is not — the old
//!   hub sees an ordinary move request and MOVES the session, so cancelling
//!   a wait would perform the very move it was meant to stop. That is the
//!   one case this bump exists to refuse instead of risking.
//! - **4** — *additive enum variants, and a brand-new tool*. Two changes in
//!   one release that an older client cannot absorb, which is why the rule
//!   above grew the paragraph it did.
//!
//!   `ConvItem` gained `bash` and `harness` kinds. It is internally tagged,
//!   so a client that has never heard of `bash` does not skip that item — it
//!   fails to deserialise the **whole** `Conversation`, and the Conversation
//!   tab shows a parse error where a session's history used to be. (From
//!   this revision on, `ConvTurn::items` degrades an unreadable item to one
//!   placeholder line instead; that is what keeps revision 5 from costing a
//!   revision-4 client anything. Revision 3 and earlier have no such
//!   tolerance, so this bump is what tells them to stand off.)
//!
//!   `session_activity` is also new here — a tool the desktop now routes to
//!   the hub, and an older hub's router does not serve it. The caller
//!   swallows that failure, so the live-activity indicator the tool exists
//!   to drive silently never appears while the app retries every two
//!   seconds. A version skew this build cannot see is exactly what this
//!   number is for.
//! - **5** — *a brand-new tool the desktop routes to.* `add_project` and
//!   `list_github_repos` become hub tools, and the desktop routes both to
//!   them instead of refusing them as local-only. A revision-4 hub serves
//!   neither: the sidebar's "Add project" would be enabled and every attempt
//!   would fail with an unknown-tool error. `GithubRepo` also joins the
//!   report types the desktop deserialises.
//!
//!   `rewind_conversation` (reply actions: Rewind here, Fork here, Retry)
//!   belongs here too. v0.3.3 shipped it as a routed hub tool while still
//!   on revision 4, so a revision-4 hub may or may not serve it — the same
//!   skew revision 5 exists to refuse; from 5 on every hub serves it.
//!
//!   Its `new_worktree` parameter later became functional (a fork into a
//!   new worktree) with no bump: a hub before that refuses it with
//!   `E_UNSUPPORTED` — a clear refusal, never a silent different action —
//!   and the Fork sheet reads that code as "update the hub".
//! - **6** — *a brand-new tool the desktop routes to.* `catalog_admin`: the
//!   desktop's asset-catalog commands (config, authoring, commit / push,
//!   lint, Sync, secrets, layers) route to it instead of refusing as
//!   local-only, for the master or a paired client granted the catalog. A
//!   revision-5 hub does not serve it, so even a granted desktop would open
//!   the full Assets panel and fail every action with an unknown tool.
//! - **7** — *brand-new tools the desktop routes to.* File downloads:
//!   `list_downloads`, `send_file` and `remove_download` (and the
//!   `GET /downloads/<id>` route `save_download` streams from). A
//!   revision-6 hub serves none of them, so the Downloads sheet and the file
//!   viewer's "Send to downloads" would fail every action with an unknown
//!   tool.
//! - **8** — *five brand-new tools the desktop routes to.* Multi-user M1's
//!   sharing surface: `session_share`, `session_unshare`, `session_narrow`,
//!   `session_access` and `my_grants` become hub tools, and the desktop
//!   routes a command to each of them instead of having no command at all.
//!   A revision-7 hub serves none of the five, so a desktop paired with one
//!   would draw the Share sheet and fail every button with an unknown tool —
//!   and, worse than a failed button, `my_grants` is the ONE place a client
//!   learns its own person id and its own grant set, so a hub that cannot
//!   answer it leaves every watcher's reach unreadable (`src/lib/access.ts`
//!   holds the previous answer rather than widening, which means a shared
//!   session simply never becomes reachable). Refusing such a hub with the
//!   skew banner says what to do; leaving it `InRange` would ship a Share
//!   sheet that silently does nothing.
//!
//!   `capture_session` becomes a routed desktop command in the same release
//!   and needs no bump of its own — the tool has existed on every hub since
//!   long before this mechanism — but it is the reason the bump matters to a
//!   WATCHER: sharing never confers a terminal, so the read-only pane
//!   snapshot is the whole of what a `watch` grant gives back.
//!
//!   M1's new `SessionRow` fields (`owner_person_id`, `visibility`) are
//!   deliberately NOT a reason for this bump: they are additive fields, which
//!   the rule above says explicitly not to bump for, and an older client
//!   absorbs them through `#[serde(default)]`. One bump covers the milestone.
//!   There is deliberately no mixed window: hub and desktop upgrade together.
//! - **9** — *a brand-new tool the desktop routes to.* Chat forms: the
//!   desktop routes `list_forms` / `get_form` / `answer_form` /
//!   `decline_form` to the hub's new `ask` tool, and session rows carry
//!   `pending_form`. A revision-8 hub serves no `ask`, so a desktop paired
//!   with one would draw the forms card and fail every answer with an
//!   unknown tool.
//! - **10** — *a brand-new tool the desktop routes to.* Debug devices: the
//!   desktop routes the Debug devices page's seven commands
//!   (`list_debug_devices`, `scan_debug_devices`, `update_debug_device`,
//!   `release_debug_device`, `forget_debug_device`, `boot_debug_device`,
//!   `shutdown_debug_device`) to the hub's new `debug_devices` tool. A
//!   revision-9 hub serves none of it, so the page would fail every read
//!   with an unknown tool.
//! - **11** — *new enum variants and new tools*: the Orbit Fleet redesign's
//!   M2 (steps 2.1–2.8). `needs_attention.reason` gains `host_down`,
//!   `account_limit` and `no_credentials` (step 2.4, decided from the facts
//!   the hub's bus follows), and `needs_attention` carries the attention
//!   `state` beside the reason; a phone that decodes the reason as a closed
//!   enum fails the row on the first new one. The desktop routes
//!   `touch_session_viewed` (step 2.3) and `repo_blame` to tools a
//!   revision-10 hub does not serve, `list_account_usage` to the new
//!   `account_usage` tool, and `update_device` to an `org_admin` action it
//!   does not know. Session rows also carry `agent`,
//!   `origin`, `last_viewed_at`, `turn_outcome` and `proposals` (additive).
//! - **12** — *new tools*: the redesign's tools after M2. The desktop routes
//!   `queue_prompt`, `queued_prompts` and `cancel_queued_prompt` to the new
//!   `queue_prompt` / `queued_prompts` tools, `repo_branch_diff` and
//!   `repo_range_diff` to tools of the same names, `session_presence`
//!   (11.7b) to its tool, `link_peer_hub` / `unlink_peer_hub` to
//!   `link_peer` / `unlink_peer` (11.5), and `mcp_pending_confirms` /
//!   `mcp_confirm` to `mcp_confirms` / `answer_mcp_confirm` (9.2; those two
//!   fall back on an older hub), `shell_terminals` to its tool,
//!   `list_runs` to `runs` (8.3) and `control_handoffs` to its tool (9.3). A revision-11 hub serves none of them. The
//!   hub also serves `routines` (8.5), `install_agent` and `agent_installs`
//!   to the phone. The golden file now pins `PullRequestRow`, `PrList`,
//!   `PresenceView`, `ConfirmRequest`, `PeerLinkSummary`,
//!   `QueuePromptResult`, `DeferredPromptRow`, `BranchDiff`,
//!   `ShellTerminalsResult`, `RunsPage`, `RunRow`, `ControlHandoffRow`,
//!   `HandoffItem` and `AccountUsageSnapshot`.
//! - **13** — *new tools, a new grant level and wider access*. The desktop
//!   routes `control_route_propose` / `control_route_follow` to the new
//!   `control_route` tool (9.9), `list_library`, `add_library_items` and
//!   `remove_library_item` to `library` (9.7), and `lost_target`,
//!   `place_transcript` and `start_rules` (8.11) to tools of those names;
//!   a revision-12 hub serves none of them. A grant's level gains `answer`
//!   (11.7a) in `my_grants`, `session_access`, `session_share` and
//!   `grant:changed`, which a client reading the level as a closed enum
//!   fails on. And the hub owner's trusted `full` phone may now call
//!   `add_host`, `install_agent` and `work_admin`'s tracker actions
//!   (Martin's "Owner's phone" and trackers "Allow on phone"); a phone
//!   offers them from this revision on. The desktop also routes
//!   `session_summary_since` (11.11) to its tool. The golden file pins
//!   `ControlRoute`, `LostTarget`, `PlacedTranscript` and `WatchSummary`.
//! - **14** — *new tools and new arguments*. The desktop routes
//!   `check_account_headroom` (8.7) to its new tool and `mission_triage`
//!   (9.10) to `work_link`; a revision-13 hub serves neither. The hub
//!   serves `pr_shepherd` (status, grant, revoke, pause_all) to a person's
//!   device, and `routines` gains the `failing` action (8.6).
//!   `new_bg_session` takes `project_id`, `agent`, `read_only`,
//!   `stop_after_secs` and `stop_after_usd` (the phone's background agent
//!   sheet), which a revision-13 hub would silently drop: the agent would
//!   start in `$HOME`, writable and without its stop. Additive: `HostRow`
//!   carries `agents_on_path` (12.4), `PendingInput` its `detail` (5.9), a
//!   listed mission its `cost_micros` / `budget_micros`, and a mission's
//!   plan a `run_estimate`. The golden file pins `HostLogin`, `Headroom`,
//!   `ShepherdRuleView`, `MissionRow` and `RunEstimate`.
//! - **15** — *a wider form spec* (gap plan G1.1). `fleet.form/1` takes
//!   optional keys an older reader refuses: an option may be an object
//!   (`{value, label, detail?, proposed?}`) beside the `[value, label]`
//!   pair, a step a `name` and `kind: "review"` (with no fields), a field
//!   `other`, `disabled_reason`, `drafted` and `secret_note`, the form
//!   `save_later`. A `FormView`'s `spec` rides as JSON, but a
//!   revision-14 client reads each option as a `[value, label]` pair (the
//!   desktop destructures it, the phone decodes a typed spec) and knows
//!   no review step, so a form using them breaks its card there. Specs in
//!   the older shape are unchanged. The desktop routes no new
//!   tool, so it still accepts a revision-14 hub (`MIN_HUB_CONTRACT`
//!   stays 14).
//!   The attention model gains two classes (G1.6). `needs_attention.reason`
//!   gains `probably_waiting` and its `state` gains `proposed` (Jev read a
//!   silent turn's end as a question: kept apart from Needs you, never
//!   counted by the badge; before, such a row read `waiting` /
//!   `action_required`), new enum values a client reading either as a
//!   closed enum fails on. Additive: `MissionRow` carries `waiting_on`
//!   (`{reason: question | sign_grant | confirm, since, open_cards}`) where
//!   it carries `cost_micros`, and `work { action: today }` answers
//!   `missions` (those waiting on a person) and a session's `proposed`.
//!   The golden file pins `MissionRow.waiting_on`.
//! - **16** — *new tools and new actions* (gap plan M15, batch 2). The
//!   desktop routes `api_tokens` (named Control API tokens with a scope,
//!   expiry and host limit, G2.8) and `add_account` (a subscription login
//!   pane or an API key with a daily limit, G2.9) to tools of those names;
//!   a revision-15 hub serves neither. A shared session's watcher asks for
//!   more access with `session_ask_access`, and its owner answers on
//!   `access_requests`. `routines` takes trigger guards (fleet budget, time
//!   cap, host fallback, retry once, autonomy), and `org_admin` gains the
//!   org's project catalog, a rule's live impact and the Sharing tab's
//!   revoke and narrow actions, which a revision-15 hub refuses as unknown.
//!   Every row change is additive.
//! - **17** — *a new tool*. The composer's context help asks
//!   `session_context_help`, so a person a session is shared with at
//!   `answer` or `drive` gets it too; a revision-16 hub has no such tool.
pub const CONTRACT_REVISION: u32 = 17;
