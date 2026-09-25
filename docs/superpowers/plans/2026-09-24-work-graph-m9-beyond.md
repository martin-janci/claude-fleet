# Work graph M9: beyond (plan)

**Date:** 2026-09-24
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M9 and *Decisions still open*
**Design:** `../specs/2026-09-24-work-graph-design.md` §0 (0.1.5: *no Work tab — context in Details, the overview in a Today view*)
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md` §5 (the M9 rows) and §6 (summaries and webhooks deferred on purpose)
**Depends on (landed on `main`):** M1b (`SessionRow.work`, `work` / `work_link`), M2 (journal, handover, resume), M3 (tracker cache, `lookup`, `start`), M4 (`work_suggested`), M5 (`OrgScope`, the isolation matrix), M8.0 (the `action` schema enums)
**Uses when merged (not on `main` yet — do not touch their code here):**
- M6 (more providers, `claude/cloud-fleet-work-graph-m6`): write-back needs a per-provider adapter; M9.5 is designed against the `TrackerProvider` trait so GitHub / Asana / Linear write-back is one method each.
- M7 (self-cleaning lifecycle, `claude/cloud-fleet-work-graph-m7`): the Today view's **Stale** section should open M7's Tidy-up sheet once it exists; until then it jumps to the session.

## Goal

> The work graph pays off in the moments around the work: the morning
> overview, the standup, reading the ticket while prompting, and the
> hand-off when a session ends. M9 adds those moments without a Work tab and
> without writing to anybody's tracker unless the user turned it on.

**Value line:** open the app, press ⌘⇧T: *PAY-7 waits on you (permission
prompt), PAY-9 is in progress with CI green, PAY-3 shipped today, ENG-12 has
been idle for five days.* **Copy standup** puts that on the clipboard as
plain text. Select the PAY-9 session: Details shows its acceptance criteria,
and **Insert into composer** puts them — fenced as untrusted — into the
prompt box, unsent.

## Which items need the user first

| Item | New user decision? | External side effect? | This branch |
|---|---|---|---|
| M9.1 Today view + Copy standup | no | no (cache and rows only; the clipboard is the user's own action) | **implement** |
| M9.2 Ticket context card | no | no (cache only; inserts, never sends) | **implement** |
| M9.3 Agent-written handover | D9 decided: on demand | a turn of the session's model, only when a person asks | **implement** (on demand only) |
| M9.4 Dead-session summaries | D10 decided: off | runs `claude -p` on a host (tokens, a model call) | not built |
| M9.5 Tracker write-back | D3 decided: none | writes to Jira / other trackers | not built |
| M9.6 Multi-repo start | D11 decided: same branch name, prior projects offered | creates N sessions / worktrees, only when a person asks | **implement** |
| M9.7 Operator (AgentFab) work commands | D12 decided: always confirm | starts / kills sessions, each confirmed on the desktop | **implement** (the confirm gate; "tidy up" needs M7) |
| M9.8 Webhook nudges | D13 decided: no | an inbound, internet-facing endpoint | not built |

## Facts this plan builds on (verified 2026-09-24 at `main` 6fbd26f)

**Work reads**

- `work` is one grouped MCP tool (`mcp/tools/orchestration.rs`), `Access::Client`, readonly. Its actions come from ONE table, `service::work::WORK_ACTIONS` (`service/work/mod.rs`): the parser (`WorkArgs::parsed_action`), the schema `enum` (M8.0) and the isolation matrix's coverage check all read it. A new action is a table row + an enum variant + a dispatch arm; `every_action_has_a_matrix_row` fails until the matrix has a row.
- `ROUTED_WORK_COMMANDS` in the same file maps each desktop command to `(tool, action)`; `src-tauri`'s routing tests hold it to `backend/verdicts.rs` (163 `Verdict::` rows incl. the enum's own), and `every_routed_work_command_names_a_covered_action` to the matrix.
- The tool budget: `BUDGET_BYTES = 69_271` (`mcp/tools/tests.rs`), measured 69,171 after M8.0.
- Every work read takes an `OrgScope` from `Caller::org_scope` (`service/orgs.rs`): `All` for master / clients / the desktop, `Host { alias, org, isolated }` for a per-host token. `sees_row`, `sees_org`, `redact_row`, `scope_links` (a host sees past links only of its own host) and the tickets fence (`tickets::allowed`: items linked on the host's sessions, inside its org) are the building blocks.
- `call_tool` redacts session rows' work in anything a host receives (M5.3 backstop), but only for fields shaped like session rows; a new digest must scope itself.

**Data M9.1 / M9.2 derive from (no new column is needed)**

- `SessionRow`: `work` (primary confirmed link, with the item's status / url / org), `last_activity_at`, `claude_status`, `idle_since`, `pr_url`, `ci_status`, `friendly_name`, `org_id`, `lost_at`.
- `service::attention::needs_attention(&SessionRow)` is the hub's one "a person is needed" classifier (waiting / stuck / failed / lifecycle), the same order as the desktop's `TRIAGE_BUCKETS`.
- Ended links carry `ended_at`, `snap_name`, `snap_host`, `snap_pr_url` (`store/work.rs`); `recent_ended_work_links(since, max)` reads them.
- Tracker items carry `status_category`, `status_changed_at`, `url`, `assignees`; the sync writes `status_changed_at` on a real move (M3.3). `ItemMeta.assignee_id` + the tracker's `config.account_id` say "mine".
- The **description** is `ItemMeta.description`: ≤ `DESCRIPTION_MAX_CHARS` (2000) chars of plain text; ADF headings become their own line, list items `- …` (`jira::adf_walk`). Third-party text.
- There is **no merged-PR state** anywhere (only `pr_url` and the check roll-up `ci_status`), and the journal's `outcome` kind is declared but never written. "Shipped" therefore means: *a ticket that moved to done* or *a session that ended with a PR*.

**Frontend**

- `Details.svelte` is the centre pane: `SessionDetails` for a selected session, else a one-line empty state (`details-empty`).
- App chords live in `app_views.ts` `appChord` (⌘I hosts, ⌘J session view, ⌘E agent, ⌘, settings, ⌘⇧O scope). ⌘⇧T / Ctrl+Shift+T is free.
- The composer is `ConversationPanel.svelte`'s `draft`, persisted per session in `conversation.ts` `composerDrafts` (the panel is one instance for every session and restores the draft on a session switch).
- Scope: `orgs.ts` `effectiveScope` + `scopeOf(row)` decide which rows the sidebar shows (M5.4); `attention.ts` `classify` is the desktop's triage.
- Tracker text is already rendered as plain text everywhere (Svelte text interpolation, never `{@html}`).

**For the planned-only items**

- Safe kill (`service/safe_kill.rs`) types a prompt with a nonce and scans the pane on the next Stop hook for `SAFE_REMOVE_READY_<nonce>` / `SAFE_REMOVE_FAILED_<nonce>`.
- The operator (`service/operator.rs`) is an ordinary session the UX agent drives through MCP; `mcp::guard` has per-tool `confirm` (gated by `mcp.confirm_destructive`) with nonce-consumed desktop confirmations. `work_link` is `confirm: false`.
- `hub.public_url` exists (`service/hub.rs`); nothing inbound is unauthenticated today.
- The tracker layer is read-only: `TrackerProvider` (M3.2) has no write method, and the Jira token's scopes are whatever the user pasted.

## Design decisions

1. **Reads go on `work`, as actions.** M9.1 is `work { action: today, since? }`, M9.2 `work { action: card, key }`. No new tool (roadmap risk: the budget). One new parameter, `since`. Each gets a matrix row, a `ROUTED_WORK_COMMANDS` row and a verdict row.
2. **The hub builds the digest; the desktop builds the words.** `today` returns buckets of structured entries, scoped by the caller's `OrgScope`, so a phone can use it later. The **standup text** is built in the frontend from what the Today view shows *after* the desktop's own scope filter (⌘⇧O), so the copied text is exactly what is on screen. It is a pure function, deterministic, no network, no LLM.
3. **Buckets, one per live group, decided in this order:**
   - **waiting** — any of the group's sessions `needs_attention` (the hub's classifier, so the digest and the phone agree);
   - **stale** — every session is idle past `STALE_AFTER_SECS` (3 days, a constant — not a setting until someone asks), or the ticket is `done` while a session still runs;
   - **in progress** — the rest.
   Groups are the primary work key; sessions without work form one *No work* group per bucket (hybrid, no "Unclassified" rule of M1 — they are listed by name).
   **shipped** — since `since`: tracker items linked to any work of the caller's that moved to `done` (`status_changed_at ≥ since`), items assigned to the tracker account that moved to `done`, and links that ended with a `snap_pr_url`. De-duplicated by key.
4. **`since` is the caller's local midnight.** The hub does not know the user's timezone; the desktop passes it. Absent → `now − 24 h`.
5. **Scope.** `today` under a per-host scope: only that host's sessions (`sees_row` and `host_alias == alias`), their work redacted with `redact_row`; ended links through `scope_links`; done items only inside `tickets::allowed`. So a host reads its own day and nothing of another org (matrix rows prove it).
6. **The card reads the cache only.** No live fetch (that is `lookup`'s job; the card is drawn on every selection). Uncached → the card shows the session's own `work` summary (key, title, link) and says the description is not cached.
7. **Acceptance criteria** are parsed in Rust (`service/work/card.rs`, pure, table-tested): a line that names the section (*Acceptance criteria*, *AC*, *Definition of done*, optional `#`, `*`, `:`), then its list items (`-`, `*`, `•`, `1.`, `[ ]`, `[x]`) or Given/When/Then lines, up to the next heading-looking line; at most 20 items of 300 chars. No section → no criteria, and the card shows the description excerpt instead.
8. **Untrusted text.** The card returns `composer_text`: fleet's own line (`KEY — title`, the URL) followed by the criteria (or the excerpt) inside `mcp::guard::fence_untrusted`. The frontend inserts **that string verbatim** — the fence is applied once, in Rust, never re-implemented in TS. Rendering is plain text. For a per-host token the plain `acceptance` / `excerpt` fields are left empty: an agent gets the fenced `composer_text` only (the M3 rule for `lookup`).
9. **Insert, never send.** "Insert into composer" appends to the session's `composerDrafts` entry (and to the live `draft` when the panel shows that session), switches the Session tab to Conversation, and focuses the box. Nothing is typed into a pane.
10. **No contract bump; every new wire field `#[serde(default)]`.** New structs are new replies, not new fields on old ones.

## Tasks

Each task is one PR-sized commit. Checks per commit: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p fleet-core -p claude-fleet -p fleet-hub` (only the four root-chmod tests may fail); `pnpm install --frozen-lockfile && pnpm check && pnpm test`; `REGEN_DOCS` / `REGEN_HUB_VERDICTS` when tools or verdicts change.

### M9.0: this plan

Commit and push before code.

### M9.1: Today view and Copy standup — implement

**Hub**

- `service/work/today.rs`: `Today { since, now, groups: Vec<TodayGroup>, shipped: Vec<TodayShipped> }`; `TodayGroup { bucket, key?, title, item_id?, status_category?, status_name?, url?, org_id?, sessions: Vec<TodaySession> }`; `TodaySession { id, name, host_alias, org_id?, attention?, stale?, pr_url?, ci_status?, last_activity_at }`; `TodayShipped { key?, title, url?, pr_url?, at, how: done|pr, org_id? }`. A pure `digest(rows, ended, done_items, now, since)` plus the store-reading wrapper (one lock, no `.await`).
- `WorkAction::Today` in `WORK_ACTIONS`; `WorkArgs.since` (`#[serde(default)]`); dispatch arm; tool description gains `today {since}`. Measure and record `BUDGET_BYTES`; `REGEN_DOCS`.
- Isolation matrix row: master / clients see A's and B's day; host A sees only h-a's sessions and A / unassigned items; host none sees none of either; no marker leaks.
- Tauri `work_today` (Routed → `work today`), a `ROUTED_WORK_COMMANDS` row, a verdict row, `REGEN_HUB_VERDICTS`.

**Desktop**

- `today.ts`: `loadToday(since)`, `localMidnight(now)`, `scopeToday(today, scope, scopeOf)` (rows through the same `scopeOf` the sidebar uses; shipped entries by `org_id`), `standupText(view)` — plain text, stable order, no markdown beyond `-`.
- `TodayView.svelte`: four sections (Waiting on you — grouped by work; In progress; Shipped today; Stale), each row jumps to the session; **Copy standup**; **Refresh**. Refreshes on open and on row events (debounced).
- `Details.svelte`: Today is the empty state, and ⌘⇧T (`appChord` → `today`) toggles it over a selected session.
- Tests: the digest table (buckets, precedence, done-while-running, no-work groups, shipped de-dup, `since`), the matrix row, the routed call; `standupText` goldens; the view's sections, jump and copy; the chord.

### M9.2: Ticket context card — implement

**Hub**

- `service/work/card.rs`: `acceptance_criteria(text) -> Vec<String>` (pure, table-tested: Jira ADF text, Markdown, `AC:` inline, Given/When/Then, none, a section that runs into *Notes*, CRLF, the caps); `TicketCard { key, title, url?, status_name?, status_category?, org_id?, cached, acceptance, excerpt?, composer_text }`; `card(store, key, scope)` from the cache. A per-host scope: `orgs::require_key` + `tickets::allowed` (the lookup fence), plain fields empty (decision 8).
- `WorkAction::Card`; description `card {key}`; budget; `REGEN_DOCS`; matrix row (host A reading BB-1's card answers exactly as an unknown key; no B text in any host A answer).
- Tauri `work_ticket_card` (Routed → `work card`), rows, `REGEN_HUB_VERDICTS`.

**Desktop**

- `TicketCard.svelte` in `SessionDetails` under the work chip: title, status, **Open ticket**, the criteria as a list (plain text), the excerpt when there are none, and **Insert into composer**.
- `conversation.ts` `insertIntoComposer(sessionId, text)`: appends with a blank line to the draft; the panel listens (a store) and updates its live `draft`; App switches to Conversation. Never sends.
- Tests: the parser table; the card's scope; the routed call; the component (plain-text rendering of a `<script>` title, insert calls the helper with the fenced text, never `send_prompt`); the draft helper.

### M9.3: Agent-written handover — plan only (needs D9)

- **What:** at the end of a session (safe kill, or an explicit *Write handover* on the work group), ask the session's Claude to write the hand-off the next session needs, emitted between `WORK_HANDOVER_BEGIN_<nonce>` / `WORK_HANDOVER_END_<nonce>` markers — the safe-kill marker pattern.
- **How:** reuse `safe_kill`'s nonce + Stop-hook scan (a second marker family in the same scan); store the text as a `handover` journal row with `source = agent`, capped at `BRIEF_MAX_CHARS`; `build_context` puts it ahead of the deterministic template, fenced (the agent's text is untrusted too: it may quote tool output).
- **Guards:** only when the REPL is idle and not `stuck_kind=trust_prompt`; one request per conversation; a timeout falls back to the deterministic brief; never on a session the operator guard protects.
- **Tests:** marker scan with prompt echo; timeout; the brief order and fence; the idle guard.

### M9.4: Opt-in summaries of dead sessions — plan only (needs D10)

- **What:** after a session ends with confirmed work, summarise its last conversation into a `summary` journal row for the next brief.
- **How:** on the session's host, `claude -p --resume <id> --fork-session --model <small> --settings '{"hooks":{}}' "<fixed summary prompt>"`, over `ssh.rs` with every value `shq`-quoted; a `FleetTasks` queue, one per host at a time, a timeout, output capped; the fork never touches the original transcript; fleet's hooks are disabled so the run cannot journal or deliver into itself.
- **Setting:** `work.summaries` (off), `work.summary_model` (default a small model); per org later.
- **Tests:** the command string (quoting, flags), queue and timeout via `FakeSsh`, the setting gate, no run for unlinked sessions.

### M9.5: Tracker write-back — plan only (needs D3)

- **Default stays NONE.** Nothing is written without the setting; no write is ever triggered by detection, sync or a guess.
- **Per tracker, opt-in:** `trackers.config.write_back = { transition_on_start, pr_remote_link, worklog }`, all false; set through `work_admin` (Master, confirm-gated), shown in Settings → Work with the account the token acts as.
- **Transition on start:** only on `work_link start` by a person; only when the item's category is `todo`; pick the transition whose **target status category** is `in_progress` (C24), never by name; none or several → skip and say so in the start reply.
- **PR remote link:** when a session linked `started` / `manual` gets a `pr_url`, add a Jira remote link with `globalId = "fleet:pr:<url>"` — idempotent by construction (Jira upserts by `globalId`).
- **Worklog:** from session active time, rounded, only on end, only with the setting; the least certain of the three.
- **Mechanics:** a `TrackerProvider::write(&WriteOp)` method (M6 providers add theirs); an outbox table so a failed write retries and a repeated trigger is a no-op; a `write_back` journal row per write; `E_TRACKER` surfaces in the chip; the per-host fence forbids every write for host tokens.
- **Tests:** FakeTransport transitions (category pick, none, several), the `globalId` idempotency, the outbox retry, the setting gate, host tokens refused.

### M9.6: Multi-repo start — plan only (needs D11)

- `work_link start { key, project_ids: [...] }`: one sibling session per project, each linked `started`, one branch name `{key}-{slug}` in each repo, the brief to each with its repo named; an `E_EXISTS` per repo that already runs the key; the dialog picks projects from the tracker's history of that key (past links' `snap_project_id`).
- The group header shows *N repos*; resume offers "all siblings".
- **Tests:** partial failure (one host unreachable) leaves the created ones linked and reports the rest.

### M9.7: Operator (AgentFab) work commands — plan only (needs D12)

- "start ABC-123 on hetzner" → the operator calls `work_link start`; "tidy up done tickets" → M7's tidy candidates → `kill_session` (safe path).
- **Through the confirm dialog:** a call whose caller is the operator session (`operator::is_operator`) is confirm-gated for `work_link start|resume` and every kill, **regardless of** `mcp.confirm_destructive`; the dialog shows the ticket, host and project.
- **Tests:** the guard requires a nonce for the operator and not for the person; a denied confirm does nothing.

### M9.8: Webhook nudges — plan only (needs D13)

- `POST /hooks/tracker/<tracker_id>` on a hub with `hub.public_url`: HMAC-verified with a per-tracker secret (stored like tracker secrets, read only by `resolve_tracker_credential`'s sibling), size-capped, rate-limited.
- **The payload is never trusted:** only the issue id/key is read, and it triggers `fetch_one` for that item (the targeted fetch); everything stored comes from the tracker API, as on a poll.
- **Tests:** a bad signature is 401 and fetches nothing; a key outside the tracker is ignored; a burst coalesces into one fetch.

## Acceptance (manual)

1. With three sessions (one waiting on a permission prompt, one working, one idle for four days) and a ticket moved to Done today: ⌘⇧T shows them in Waiting / In progress / Stale / Shipped. **Copy standup** puts the same four lines on the clipboard.
2. The scope selector set to Company A: Today and the standup hold only A's work.
3. A session on PAY-9 whose description has *Acceptance criteria*: Details lists them; a title containing `<b>x</b>` shows the tags literally; **Insert into composer** puts the fenced block into the prompt box and sends nothing.
4. A per-host token asking `work { action: card, key: <another org's key> }` gets the same refusal as for an unknown key.

## Risks

| Risk | Mitigation |
|---|---|
| The digest grows with the fleet | One store pass, groups capped (200), no journal bodies in the reply |
| A standup leaks another company's ticket | The text is built from the scoped view (decision 2); matrix rows for host tokens |
| Tracker text reaching an agent unfenced | `composer_text` is fenced in Rust; the UI never builds agent text itself |
| The parser missing a team's AC style | No criteria → the excerpt is shown; the table is where a new style is added |
| The budget | One parameter and two short action names; measured and recorded |

## Decisions (defaults if unanswered)

| # | Question | Default |
|---|---|---|
| D3 | Write-back to trackers (none · transition on start · plus PR remote link · plus worklog) | **None.** When chosen: per tracker, opt-in, each part separately |
| D9 | May fleet spend a turn of the session's model to write a handover? When? | On demand only (a button); never automatically at safe kill |
| D10 | Summarise dead sessions with `claude -p --fork-session`? Which model, whose quota? | Off; when on, a small model on the session's own account |
| D11 | Multi-repo start: one branch name across repos, and which projects are offered? | Same `{key}-{slug}` in each; offer projects the key ran in before |
| D12 | Must operator-initiated starts / kills always confirm, even with `mcp.confirm_destructive` off? | Yes, always |
| D13 | Expose an inbound webhook endpoint on a public hub? | No; poll (as v1). When yes: HMAC, targeted fetch only |
| new | Stale threshold | 3 days, a constant (a setting when someone asks) |
| new | "Shipped" without a merged-PR signal | Done tickets + sessions that ended with a PR |

## Revisions

- 2026-09-24: first version.
- **2026-09-25, M9.1 and M9.2 landed** on `claude/cloud-fleet-work-graph-m9`
  (from `main` 6fbd26f). Verified with `cargo fmt`, `clippy -D warnings`
  (workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub; only the
  four chmod tests that fail as root on `main` fail) and `pnpm check` /
  `pnpm test`. Deviations and choices:
  1. **Budget.** `today` + `since` measured 69,265 (+94); `BUDGET_BYTES`
     raised to 69,365. `card` measured 69,284 (+19), inside the headroom, so
     the constant was not raised again (the measurement is recorded beside
     it).
  2. **Who counts for Today.** Shell and external sessions and the
     operator session are left out (no Claude of the fleet's to wait on or
     ship). A done ticket's time is `status_changed_at`, else its
     `updated_at`; a `since` in the future is clamped to now.
  3. **Scope on the desktop.** Sessions are cut by the sidebar's own
     `scopeOf(row)`; a shipped entry has no project owner, so an owner scope
     or *unassigned* keeps only shipped entries without an org, and an org
     scope those of that org. Groups left empty are dropped and re-bucketed
     with the hub's rule (`bucketOf`).
  4. **Refresh.** The view reloads on open and 2 s after the last row event
     (the sessions store), besides the Refresh button.
  5. **Ctrl+Shift+T** is taken by the app's capture-phase chord handler on
     Linux/Windows, like Ctrl+Shift+H/J/E; the terminal no longer receives
     it.
  6. **A key with no cached item** (a bare branch key, a local item) gets a
     card with `cached: false` and `composer_text` `Ticket KEY` — not an
     error — so Details still shows the key and the link from the row's
     `work`. For a per-host token the tickets fence is applied to tracker
     items; a local item is covered by `orgs::require_key`.
  7. **Insert into composer** is disabled on a session with no
     conversation composer (no pane, or no Claude conversation yet). The
     panel adopts an insert only while the stored draft still equals it, so
     a later remount never replays an old insert over what was typed since.
     App switches to the Conversation view and closes Today.
  8. **Helpers.** `Store::linked_work_item_ids` (new, one query);
     `tickets::allowed` is now `pub(crate)` so Today and the card share the
     M3/M5 fence rather than re-deriving it.
  9. No `CONTRACT_REVISION` bump, no new tool, no migration; every new wire
     struct's optional fields are `#[serde(default)]`.
- **2026-09-25, decisions.** The user took the defaults for D9–D13 and kept
  D3 at none: M9.3 (on demand only), M9.6 and M9.7 are now to be built;
  M9.4 (summaries off), M9.5 (no write-back) and M9.8 (no webhooks) stay
  planned and are not built. `main` (M6) was merged into the branch first.

