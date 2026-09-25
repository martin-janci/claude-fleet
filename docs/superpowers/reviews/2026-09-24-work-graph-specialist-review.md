# Work graph: specialist review, round 1

**Date:** 2026-09-24 · **Reviewed:** `specs/2026-09-24-work-graph-design.md`
(revision 1) · **Outcome:** revision 2 of the design, recorded in that spec's
section 0, and the roadmap `../2026-09-24-work-graph-roadmap.md`.

Six read-only reviewers each took one angle and checked their claims against
the code. What follows is the consolidated result: findings that change the
design, ideas that were adopted, and ideas that were deferred. File references
are relative to `crates/fleet-core/src/` unless another root is given.

| Reviewer | Angle |
|---|---|
| Backend / data | identity, reconcile, delete paths, schema, migrations, MCP plumbing |
| Integrations | Jira Cloud and Data Center, GitHub Issues, Linear, Asana, transport, cargo-deny |
| Detection | signals, resolution, work memory, handover |
| Product / UX | flows, zero-config path, how rigid the design feels |
| Hub / mobile / security | hub-client mode, guard, events, secrets, phone |
| Devil's advocate | what to cut, the thinnest end-to-end version |

## 1. Corrections to revision 1 (the spec was wrong)

### Identity and lifecycle

| # | Finding | Evidence |
|---|---|---|
| C1 | **Not every session has a participant.** Participants are minted lazily, only by `insert_message`. Migration 043 backfilled the existing rows once. A move with no participant skips the repoint. | `store/timeline.rs:310`, `service/move_session/finalise.rs:117-132` |
| C2 | **`rename_session` probably destroys identity.** The upsert is keyed `(host_alias, tmux_name)`, so the renamed session INSERTs a new row. The old row is ghosted and reaped, and its participant retired. This already affects messaging today. *Verify with a failing test first.* | `service/sessions/lifecycle.rs:1080-1118`, `store/reconcile.rs:376,621,655` |
| C3 | **Five raw-SQL sites retire participants,** so ending links "on retire" by hand would miss one. Use a trigger on `participants.retired_at`, following the precedent of migration 044's `trg_read_cursors_on_session_delete`. | `store/sessions.rs:920`, `hosts_accounts.rs:427`, `reconcile.rs:655`, `projects.rs:567`, `participants.rs:246` |
| C4 | **Ids are reused.** Neither `sessions.id` nor `participants.id` is AUTOINCREMENT. `work_links.participant_id` must be a real FK with `ON DELETE SET NULL`; nulling it by hand in the sweep risks re-attaching a link to a stranger. | `store/sessions.rs:899`, `store/schema.rs:415` |
| C5 | **The snapshot must be taken when the link ends, not when it starts.** A move changes host and tmux name, and `/clear` changes the conversation. The snapshot must keep **every** conversation id, because `conversations` cascade away on delete. | migration 037 |
| C6 | **"Transcripts are never deleted" is false:** `purge_project` runs `claude purge`. Purge must warn when links reference those conversations. | `service/bg_sessions.rs:326-340` |
| C7 | **Move with `keep_source`** does no repoint, so the fork starts unlinked. **Resume after a reap** creates a new participant. Both must carry links: copy them to a fork, and match a resumed `claude_session_id` against ended links. | `finalise.rs:82`, `reconcile.rs:520-560` |
| C8 | **The collision merge in `repoint_participant`** must also reassign `work_links`, or the target's links end by mistake. | `store/participants.rs:134-145` |

### Detection

| # | Finding | Evidence |
|---|---|---|
| C9 | **The PR probe is the wrong place for the live branch.** It exits when `gh` is missing, covers only github-layout worktrees, is capped at 12 sessions with a 300 s TTL, and bg/external rows have no pane. | `service/outcome.rs:48`, `service/sessions/reconcile.rs:175,1180` |
| C10 | **The branch is a state, not a history.** Counting observations turns a session that changed branches into a "conflict" and demotes the correct link. For state signals (branch, PR head) only the *current* value is a candidate; when it changes, an auto link made by that signal ends. | — |
| C11 | **The key regex has no anchors and is case-sensitive.** Branches are usually lowercase (`abc-123-fix`, Linear's `user/eng-123-…`). Build the pattern from the known key prefixes, case-insensitive, with `(?<![A-Za-z0-9])` / `(?![A-Za-z0-9])` boundaries, and normalise to upper case. | `src/lib/branch-slug.ts:14` lowercases |
| C12 | **The friendly-name signal is dead by design.** The skill forbids ticket ids in names. `last_assistant_message` is only 200 chars and repeats whatever keys tools printed. Drop both signals. | `skills/fleet-friendly-name/SKILL.md:56`, `service/hooks.rs:741` |
| C13 | **Evidence must not point at prunable observation ids;** store denormalised evidence on the link instead. | — |
| C14 | **Loop guard.** Prompts that fleet injects itself (brief, handover) come back through UserPromptSubmit and must not count as evidence. **Dump guard:** a prompt with more than 3 distinct keys is a reference list, so its keys are weak. | — |

### Session start and the brief

| # | Finding | Evidence |
|---|---|---|
| C15 | **SessionStart cannot inject anything today.** The server returns 204 for it, and the hook is installed `async: true` with `curl -o /dev/null`, which throws the response away. Making it synchronous costs up to about 2 s at start-up when the hub is down; measure that first. | `mcp/hooks.rs:183`, `service/hooks_install.rs:143,159` |
| C16 | **Typing the brief with `wait_for_repl_ready` + `send_prompt` is risky.** It gives up after 20 s and sends anyway. A fresh worktree usually shows the "trust this folder" dialog, and the Enter lands in it. It also blocks `new_session`. Prefer a message from the `hub` participant, delivered through `additionalContext`, followed by a short start prompt. Never send while `stuck_kind=trust_prompt`, and confirm delivery via `prompt_submit_seq`. | `service/tasks.rs:648,687`, `pane_intel.rs:389`, `service/delivery.rs` |

### Hub, events and the phone

| # | Finding | Evidence |
|---|---|---|
| C17 | **"All commands Routed" and "tracker mutations Master" cannot both be true.** A paired desktop is a client and never holds the master token. Admin commands (trackers, credentials, org rules) are `LocalOnly`, configured on the hub, as with the `catalog_set_secret` precedent. Reads, link decisions and start/resume are `Routed`. | `src-tauri/src/backend/pairing.rs:33`, `mcp/guard.rs:41-48`, `verdicts.rs:838` |
| C18 | **"The phone gets events for free" is false.** The phone subscribes only to `session`, `host` and `project` and drops everything else. The desktop bridge re-emits only the names in the static `EVENT_NAMES`. | `fleet-mobile/.../FleetSnapshot.kt:32,47`, `events.rs:451-469` |
| C19 | **New fields on `new_session` are silently dropped by an older hub** (no `deny_unknown_fields`). Use dedicated `start_work` / `resume_work` tools and gate the UI on the hub's tool list instead of bumping the contract, because a contract bump locks out every phone at `MAX_HUB_CONTRACT = 4`. | `mcp/tools/params.rs:76`, `wire_contract.rs:40-70`, `fleet-mobile/.../HubContract.kt:29` |
| C20 | **The sync tick must be a `FleetTasks` method** started through `start_background_tasks`. Otherwise a paired desktop also syncs and becomes a second brain. | `src-tauri/src/backend/startup.rs:47,78`, `tests_startup.rs:109` |
| C21 | **The MCP tool-description budget has about 100 bytes of headroom.** Plan at most three to four tools, grouped by action, and raise `BUDGET_BYTES` deliberately in the same PR. | `mcp/tools/tests.rs:2770` |
| C22 | **Project-level `org_id` is fragile.** Project rows are deleted and re-created by `refresh_projects` and purge. Adopted folders without a GitHub origin get `owner="local"`. `project_id` is re-derived on every pass. Use text-keyed `org_rules(owner, repo?, path_prefix?)` instead. | `store/projects.rs:623`, `service/add_project.rs:1376` |

### Jira API facts (verified against current Atlassian documentation)

| # | Finding |
|---|---|
| C23 | `/rest/api/3/search` has been removed. Use `/rest/api/3/search/jql`: it pages by `nextPageToken`, returns no `total`, returns only ids unless you pass `fields`, and is eventually consistent (`reconcileIssues`). Guard against a page token that repeats, and never persist tokens. |
| C24 | **Keys are not stable.** A project move or key rename changes them. Identity is `external_id` (numeric id), plus stored `aliases`. Fetch by id with `POST /issue/bulkfetch`. |
| C25 | **404 means "deleted or no permission"**, so record `unavailable_at` with a reason, not `gone_at`. A search silently drops issues you lost access to, so "missing from a view" never means gone. |
| C26 | **`statusCategory` has a fourth key, `undefined`** (map it to todo). Add `resolution` (done vs won't-do); GitHub has `NOT_PLANNED` and Linear has `canceled`. |
| C27 | **JQL dates use the API user's timezone at minute precision.** Use relative windows or the timezone from `/myself`, overlap the windows, and dedupe on `(id, updated)`. |
| C28 | **Sprints exist per project, not per site.** Wrap a favourite filter as `filter = <id> AND …`, never by string-concatenating its JQL (it may end in `ORDER BY`). Take the hierarchy from `issuetype.hierarchyLevel`, not from type names. Descriptions are ADF, so convert them or use `renderedFields`. Classic API tokens now expire (max one year). |
| C29 | **`reqwest 0.13` defaults to aws-lc-rs,** whose licence likely fails `deny.toml`. Use `default-features=false` + `rustls-no-provider` + ring, or lift the existing `src-tauri/src/backend/remote.rs` TLS client into fleet-core. |

## 2. Security findings (adopted)

- **Org isolation must be enforced for per-host tokens.**
  - Every host's Claude holds a Client token.
  - List reads and `/events` are fleet-wide today, so a Company-A host could read Company-B tickets.
  - Fix: add `hosts.org_id`. A host-bound caller sees only its org's (or unassigned) items and links, `work_*` frames are filtered for host-bound streams, and org mapping is Master-only.
- **SSRF:** `trackers.base_url` lets the hub fetch from its own network position. Allowlist `https://*.atlassian.net` in v1, reject userinfo, and don't follow redirects to other hosts.
- **Prompt injection:** ticket text is third-party. The brief, the handover and `additionalContext` go through `mark_untrusted` / `UNTRUSTED_END` (`mcp/guard.rs:1213`), capped and previewed.
- **Credentials:**
  - Stored in a separate `tracker_secrets` table and never exposed by a read tool. Rows expose only `has_credential` and a `…abcd` hint.
  - `credential_ref` can be `env:NAME` or `file:/run/secrets/…` for Docker hubs.
  - Add Basic-auth and `ATATT…` patterns to `logging::redact`, include the secrets in the diagnostics literal list, redact `trackers.last_error`, add `initial_prompt` to `REDACT_KEYS`, and use `without_url()` on reqwest errors.
- **Metrics** are labelled by tracker id and result only, never by key or URL.

## 3. Pre-existing bugs found on the way (fix independently)

- **Phone deletes tags.**
  - `PHONE_SESSION_FIELDS` omits `tags` (`mcp/tools/views.rs:71-88`), so a phone re-list sees `[]`.
  - `TagsDialog` then pre-fills from that empty list, and `set_session_tags` replaces the whole list. **Adding one tag on the phone wipes the rest.**
  - Fix: add `tags` to the projection and update the pinned test.
- **Rename loses identity** (C2): messages, timeline and conversations are orphaned.
- **Move drops tags** (only `friendly_name`, `started_at`, the claude id and usage are copied).
- **Hook-spec drift:** the 2026-09-15 hook spec says SessionStart and PreCompact are not installed; the code installs them.
- **Skill cost:** `fleet-friendly-name` calls `list_sessions` (about 2.6k tokens) where `whoami` (under 50) would do.

## 4. Simplifications adopted

- **Work exists without a tracker.** `work_items.source = 'local'`, and a link may point at an **unresolved ref** (`ref_key = 'ABC-123'`, `item_id NULL`) that a later sync binds. Keys from branches group sessions on day 1, with no configuration.
- **No `work_observations` table in v1.** State signals are re-derived on every pass; event signals (prompt, URL) write `suggested` links with evidence directly. A signal log comes back only with weak or LLM signals.
- **`work_events` + journal merged** into one durable `work_journal`, keyed on `(participant, claude_session_id)` and joined to items through link windows. A relink then reassigns history without rewriting it.
- **Orgs are optional.**
  - The default scope is derived from the GitHub owner, and the scope selector is shown only when there are two or more scopes.
  - Named orgs exist only to merge or split owners, and to act as the security boundary for per-host tokens.
- **No Work tab.** Tickets live in ⌘K, context lives in Details, and the overview is a Today view. This avoids building a Jira clone.
- **No "Unclassified" bucket.**
  - Group-by-Work is hybrid: work groups come first, and sessions without work fall back under their project header.
- **Branch template dropped as a setting.** The dialog prefills the branch from the key and title, and the user edits it.
- **A read-only `work` field on `SessionRow`**, from a subselect, plus `session_updated` on link change. The sidebar and phone get it without a new store; the full link list is a separate read.
- **One event kind, `work`**, with names `work:item`, `work:link`, `work:link_removed`, `work:tracker`. Frames are emitted only on an actual change, to protect the 512-entry replay ring.
- **Agent declaration:** one action on the grouped `work_link` tool, not a separate tool.

## 5. Ideas adopted into the roadmap

| Idea | Value | Effort | Milestone |
|---|---|---|---|
| Live branch from transcript `gitBranch` in the tail `context::refresh` already reads | high | trivial | M1 |
| PR/CI roll-up on work group headers | high | S | M1 |
| Friendly name from the key and title (fixes UX-05's "yes"/"clear" names) | high | S | M1 |
| Duplicate guard: "ABC-123 already running on mefistos · Jump / Start another" | high | S | M1 |
| Resume split button (continue last conversation / fresh with brief / fresh) | high | M | M2 |
| Harvest compaction summaries, `turn_done` and first prompts into the journal before the cascade delete | high | S | M2 |
| Handover template (deterministic) + `get_work_context` pull | high | M | M2 |
| ⌘K ticket kind, paste a URL to resolve, ⌘↵ start with defaults | high | M | M3 |
| Connect Jira by pasting a ticket URL (infer site and key) | high | S | M3 |
| Retro-link reveal: "14 sessions mention ABC-* · Review" | med | S | M3 |
| URL extraction as a strong tier (settles the multi-instance case; the only Asana signal) | high | S | M4 |
| PR probe: `headRefName,title,body,closingIssuesReferences` + commit trailers | high | S | M4 |
| Conversation link windows (`/clear` = likely new task) | high | M | M4 |
| Batch review of suggestions (j/k, y/n) + "trust branch keys in this repo" | med | S | M4 |
| SessionStart context injection on startup/resume/compact (after the latency check) | high | S | M4 |
| Opt-in classification nudge through `additionalContext` (the session's own model, no API key) | med | M | M4+ |
| GitHub Issues through `gh`, with no credentials | high | M | M6 |
| Tidy-up sheet (never auto-kill); work-aware GC suggestions | high | M | M7 |
| Reopened → Attention bucket + "has previous work" badge | med | S | M7 |
| Today view + deterministic standup copy | med | M | M9 |
| Ticket context card in Details, with "insert into composer" | high | M | M9 |
| Agent-written handover through the safe-kill marker pattern | med | S | M9 |
| Write-back: a transition whose *target category* is in-progress; a remote link for the PR (idempotent `globalId`) | med | M | M9 |
| Multi-repo start (frontend + backend for one ticket) | med | M | M9 |
| `ViaHost` transport (curl/acli/gh on a host), for trackers only reachable from a laptop VPN and with no token in fleet | med | M | M6 |

## 6. Deferred on purpose

- LLM summarisation of dead sessions (`claude -p --resume --fork-session`, hooks disabled). Opt-in, later.
- Webhooks: poll in v1. Later, an optional webhook only *nudges* a targeted fetch, and its payload is never trusted.
- A tracker provider over MCP (the Atlassian, Linear and Asana remote MCPs). It needs rmcp client support and OAuth.
- N:M beyond one primary link per conversation window: the schema allows it, the UI stays simple.
- OAuth 3LO: an open-source self-hosted app cannot ship a client secret.
