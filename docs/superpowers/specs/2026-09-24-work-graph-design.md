# Work graph: work items, trackers, work-aware sessions — discovery & design

Status: **analysis / proposal, nothing implemented — revision 2 (see §0).** Answers the "Cloud Fleet
takeover prompt" (Work Graph, Work Items, External Trackers & Intelligent
Session Lifecycle), sections A–M of its §40, against the code at `57447e8`.

"Cloud Fleet" in the prompt is this repository, `claude-fleet`.

---

## 0. Revision 2 (after the specialist review) — supersedes D–M where they differ

Round-1 review: `../reviews/2026-09-24-work-graph-specialist-review.md`
(C1–C29 are its corrections). Roadmap: `../2026-09-24-work-graph-roadmap.md`.
Sections A–C below still stand; D–M are kept as the revision-1 record and are
overridden by this section wherever they disagree.

### 0.1 Principles that changed

1. **Work exists before any tracker.** A branch key, a local item or a pasted
   URL is enough to group, start and resume work. Trackers *enrich*
   (title, status, url); they never gate.
2. **Organisations are optional.** Default scope = GitHub owner (derived, no
   setup). A named org exists to merge/split owners and — once used — is the
   security boundary for per-host tokens.
3. **State signals are current, not accumulated.** Branch and PR head are
   re-derived; when they change, links they created end. Only event signals
   (prompt, URL, agent declaration) produce suggestions with evidence.
4. **The link, not the session, carries lifecycle.** Unchanged from rev 1,
   now enforced by a trigger instead of call sites.
5. **Nothing is a separate app.** No Work tab: tickets live in ⌘K, context in
   Details, the overview in a Today view, grouping in the sidebar.

### 0.2 Schema (migration `046_work_graph.sql`, re-runnable; 045 is M0.3's participant trigger)

```sql
CREATE TABLE IF NOT EXISTS orgs(id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE, color TEXT, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS org_rules(id INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
  owner TEXT, repo TEXT, path_prefix TEXT, host_alias TEXT);   -- text-keyed (C22); 'local' owner never matches
CREATE TABLE IF NOT EXISTS trackers(id INTEGER PRIMARY KEY AUTOINCREMENT,
  org_id INTEGER REFERENCES orgs(id) ON DELETE SET NULL,
  provider TEXT NOT NULL,                  -- jira | github | asana | linear
  instance_id TEXT, site_url TEXT, api_base TEXT,   -- cloudId vs browse url (C23+)
  transport TEXT NOT NULL DEFAULT 'direct',          -- direct | via_host:<alias>
  config TEXT, state TEXT NOT NULL DEFAULT 'unconfigured',
  last_sync_at INTEGER, last_error TEXT, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS tracker_secrets(tracker_id INTEGER PRIMARY KEY
  REFERENCES trackers(id) ON DELETE CASCADE, value TEXT, credential_ref TEXT);  -- never on a read path
CREATE TABLE IF NOT EXISTS work_items(id INTEGER PRIMARY KEY AUTOINCREMENT,
  source TEXT NOT NULL,                    -- local | jira | github | asana | linear
  tracker_id INTEGER REFERENCES trackers(id) ON DELETE SET NULL,
  external_id TEXT, key TEXT, aliases TEXT, title TEXT NOT NULL, url TEXT,
  kind TEXT, hierarchy_level INTEGER,
  status_name TEXT, status_category TEXT NOT NULL DEFAULT 'todo',  -- todo|in_progress|done|unknown
  resolution TEXT,                         -- completed|not_planned|duplicate|NULL (C26)
  parent_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL,
  containers TEXT, assignees TEXT, iteration TEXT, meta TEXT,
  updated_ext INTEGER, status_changed_at INTEGER, fetched_at INTEGER,
  unavailable_at INTEGER, unavailable_reason TEXT,           -- not "gone" (C25)
  created_at INTEGER NOT NULL, UNIQUE(tracker_id, external_id));
CREATE TABLE IF NOT EXISTS work_links(id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id INTEGER REFERENCES work_items(id) ON DELETE CASCADE,
  ref_kind TEXT, ref_key TEXT,             -- unresolved key / url / PR, bound later
  participant_id INTEGER REFERENCES participants(id) ON DELETE SET NULL,  -- C4
  claude_session_id TEXT,                  -- conversation window start (NULL = whole session)
  role TEXT NOT NULL DEFAULT 'work',       -- work | review | worker
  state TEXT NOT NULL,                     -- suggested | confirmed | rejected
  source TEXT NOT NULL,                    -- manual | started | agent | branch | pr | url | prompt | resumed | forked | inherited
  strength TEXT NOT NULL,                  -- explicit | strong | weak
  is_primary INTEGER NOT NULL DEFAULT 0, evidence TEXT,   -- denormalised (C13)
  created_at INTEGER NOT NULL, decided_at INTEGER, decided_by TEXT, ended_at INTEGER,
  snap_host TEXT, snap_tmux TEXT, snap_name TEXT, snap_repo TEXT, snap_worktree TEXT,
  snap_branch TEXT, snap_claude_ids TEXT, snap_pr_url TEXT,
  CHECK (item_id IS NOT NULL OR ref_key IS NOT NULL));
CREATE TABLE IF NOT EXISTS work_journal(id INTEGER PRIMARY KEY AUTOINCREMENT,
  participant_id INTEGER REFERENCES participants(id) ON DELETE SET NULL,
  item_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL,
  claude_session_id TEXT, at INTEGER NOT NULL,
  kind TEXT NOT NULL,       -- conversation|progress|compact_summary|outcome|handover|note|status_change|decision
  source TEXT NOT NULL, body TEXT, meta TEXT);
-- Snapshot at END (C3, C5): every retire site UPDATEs participants before DELETE FROM sessions.
CREATE TRIGGER IF NOT EXISTS trg_work_links_end AFTER UPDATE OF retired_at ON participants
WHEN OLD.retired_at IS NULL AND NEW.retired_at IS NOT NULL BEGIN
  UPDATE work_links SET ended_at = NEW.retired_at,
    snap_host = (SELECT host_alias FROM sessions WHERE id = OLD.session_id),
    snap_tmux = (SELECT tmux_name  FROM sessions WHERE id = OLD.session_id),
    snap_claude_ids = (SELECT json_group_array(claude_session_id) FROM conversations
                        WHERE session_id = OLD.session_id)
    /* … name, repo, worktree, branch, pr_url likewise */
  WHERE participant_id = OLD.id AND ended_at IS NULL;
END;
```

Plus `hosts.org_id` (guarded `ALTER`, `already_applied` fn, `schema.rs`
043 pattern) when orgs ship. **Eager participant minting** (C1) landed
separately as migration 045's `AFTER INSERT` trigger (roadmap M0.3). No `projects.org_id`, no `work_observations`.

### 0.3 Resolution (replaces §G rules)

| Signal | Kind | Tier | Source of data |
|---|---|---|---|
| user action / Start from item | event | explicit | command |
| agent declares (`work_link` action) | event | explicit | MCP |
| **current** branch key | state | strong | transcript `gitBranch` (tail already read by `context::refresh`), fallback `worktrees.branch` |
| PR `headRefName` / `closingIssuesReferences` | state | strong | extended `gh pr view --json` |
| ticket **URL** in a prompt | event | strong (host names the tracker) | UserPromptSubmit |
| key in the first prompt of a conversation, sole candidate | event | strong→suggested, pre-selected | UserPromptSubmit |
| key elsewhere in prompts; commit trailers | event | weak | hook / probe |
| repo/owner → org | config | scope only | `org_rules` |

Rules: manual decisions final (reject sticky per participant+item);
explicit → confirmed; one strong → confirmed (`auto` marker + Undo toast);
several strong → suggested; weak → suggested, decays at the next conversation
boundary; state-signal change ends the auto link it made; fleet-injected text
is excluded (loop guard); >3 keys in one prompt = reference list (dump guard);
same key in two trackers → never auto. Keys are recognised only against known
prefixes (probe) **or**, with no tracker, as unresolved refs from branch
names only. Reviews and task workers inherit the parent's primary link with
`role`.

#### 0.3.1 As landed in M4 (2026-09-24) — the final rules and the chip

The resolver is `service::work::resolve::resolve`, a pure function; the
store applies its changes (`store::work_detect`). Tiers, not scores; inside
a tier the most recent decision. Nothing is learned.

| Rule | Condition | Outcome |
|---|---|---|
| R1 | a person's `confirmed` / `rejected` decision | final: the resolver never changes it |
| R2 | `explicit` (`started`, `agent`, and fleet's carries `resumed` / `forked` / `inherited`) | confirmed (written by those paths, not the resolver) |
| R3 | exactly one strong STATE candidate (branch key, PR head key, PR closing ref) in a trusted project | confirmed, auto (source `branch` / `pr`), Undo toast |
| R3b | the same in an untrusted project | a pre-selected suggestion |
| R3u | the same, but no tracker can resolve it (a GitHub `owner/repo#n` closing ref before a GitHub tracker exists, M6) | a pre-selected suggestion, even in a trusted project |
| R4 | several strong state candidates | all suggested, none pre-selected |
| R5 | a ticket URL in a prompt | confirmed when it is the sole reference of a conversation's first prompt, else suggested; a sole KEY there is a pre-selected suggestion |
| R6 | a weak candidate (a prompt key elsewhere, `#n`, keys in the PR title / body, commit trailers) | suggested |
| R7 | a state signal's value changes | the confirmed auto link it made ENDS (`end_reason` `branch_changed` / `pr_changed`, snapshotted as past work); a suggestion it made is withdrawn; manual / started / agent links stay |
| R8 | a key whose prefix two trackers claim (a URL's host settles it) | never automatic: a suggestion |
| R9 | a rejected (participant, target) pair | never proposed again, from any signal |
| R11 | the agent's answer to the opt-in classification nudge (M4.6, `work_link { source: agent_inferred }`) | a pre-selected suggestion, tier `inferred` (between weak and strong); never confirmed, never primary, never across orgs |
| decay | an EVENT suggestion (prompt, URL, trailer) or an `agent_inferred` guess from an earlier conversation, not seen in this one | removed at the boundary |
| primary | live confirmed links ranked by (decided in the current conversation, explicit > strong > weak, most recent decision) | it moves only when a run loses its primary or confirms a link — never re-ranks decisions |

Guards: a prompt carrying a `[claude-fleet` marker, equal (first 200 chars)
to the prompt fleet itself sent, or contained in a handover brief is not
evidence (loop guard); a prompt naming more than three distinct references
makes them all weak, none pre-selected, noted `reference` (dump guard).
With any tracker, keys must carry a tracker's prefix; with none, a prompt
key counts only when fleet already knows it (a local item or a link) —
unknown keys come from branch names only.

Chip vocabulary (sidebar row): **solid** = a confirmed link; a small **ring**
beside the key = linked automatically (R3 / R5; the tooltip names the source
and rule); **dashed with `?`** = a suggestion (`SessionRow.work_suggested`,
never a work group). The row's popover lists the evidence ("branch
`abc-123-login` since 09:05 · R3", "mentioned ABC-99 in a prompt at 10:12
(reference) · R6") with Confirm / Not this / Pick another… and "Trust branch
keys in this repo"; `y` / `n` decide the top suggestion on a focused row and
the "N link suggestions · Review" sheet decides them in bulk (j/k, y/n).

### 0.4 Provider (replaces §F trait)

```rust
trait TrackerProvider: Send + Sync {
    fn caps(&self) -> Caps;                               // query_lang, hierarchy, iterations, human_keys, incremental, write_*
    async fn probe(&self) -> Result<TrackerInfo>;         // instance_id, me(accountId), key_prefixes, tz
    async fn views(&self) -> Result<Vec<TrackerView>>;
    async fn list(&self, view: &ViewRef, since: Option<&SyncMark>, cur: Option<Cursor>) -> Result<Page<WorkItemSnapshot>>;
    async fn fetch(&self, refs: &[ItemRef]) -> Result<Vec<Fetched>>;   // Found | Unavailable{reason}
    fn recognize(&self, text: &str, ctx: &RecognizeCtx) -> Vec<ItemRef>; // key | url | #n (repo-relative)
}
```

Over an `HttpTransport` trait: `Direct` (reqwest minimal features + ring,
or the lifted `remote.rs` client — whichever passes `cargo deny`) and
`ViaHost` (curl/`gh`/`acli` on a host; token never enters fleet). Jira
specifics: C23–C29. Sync is a `FleetTasks` method (C20); per-view time
watermarks with overlap; every linked item refreshed by id each tick; events
only on real change.

### 0.5 API surface (replaces §E tool list)

MCP (budget C21): `work` (read: items, links, context, journal),
`work_link` (link / unlink / reject / primary / declare / start / resume),
`work_admin` (orgs, rules, trackers, credentials — **Master**). Tauri commands
map many-to-one onto them. Verdicts: `work`, `work_link` **Routed**;
`work_admin` **LocalOnly** ("configure on the hub"), with a
`fleet-hub tracker add|set-credential|test` CLI. UI gated on the hub's tool
list, no contract bump. Events: kind `work`. `SessionRow.work` (read-only
primary link summary, `#[serde(default)]`) + `PHONE_SESSION_FIELDS`.

### 0.6 Lifecycle (refines §H)

Archive = link `ended_at` (trigger). Archive action on a live session defaults
to **UI-only** (collapses into Done); killing is an explicit Tidy-up choice via
safe kill. Resume: continue last conversation / fresh with brief / fresh;
a resumed `claude_session_id` matching an ended link re-attaches it
automatically (C7). Brief = `hub`-participant message via `additionalContext`
+ short start prompt, never typed into a trust dialog (C16). Purge warns on
linked conversations (C6).

*As built (M2, 2026-09-24).* Work memory is `work_journal` (migration 047),
keyed by `claude_session_id` so a relink reassigns history without rewriting
it; triggers write each conversation's row on close and before every session
delete, hooks add progress lines and Claude's compaction summaries, and
confirmed work is never swept. The brief is a `handover` journal row (not a
`session_messages` row: `from_session_id` is NOT NULL), packed ahead of inbox
mail into the first hook's `additionalContext`; only a short start prompt is
typed, and only into a ready REPL. Resume is `work_link { action: resume }`
with `work { action: resume_plan | context }` as its reads; the store rebind
re-attaches ended work to whatever resumes its conversation. Details and
deviations: `../plans/2026-09-24-work-graph-m2-resume-and-memory.md`.

### 0.7 Decisions — resolved by default vs still the user's

Defaulted: Jira Cloud first (DC later with PAT + v2 + Epic Link); credentials
in `tracker_secrets` + `env:`/`file:` refs; transport per 0.4; auto-link on a
single strong signal; branch from key+title, edited in dialog; sync scope =
assigned-to-me + favourites + linked; poll, no webhooks.

Still the user's (see roadmap "Decisions"): provider order after Jira
(Asana is the user's Company-B tracker), whether "done" may ever kill a live
session automatically, write-back scope, whether org isolation for host
tokens is needed from day one.

---

## A. Current-state architecture (what the feature has to fit into)

| Area | What exists | Where |
|---|---|---|
| Stack | Tauri 2 desktop (`src-tauri`) + headless hub (`crates/fleet-hub`) over one Tauri-free core (`crates/fleet-core`); Svelte 5 frontend; KMP phone client (`fleet-mobile`) | `CLAUDE.md` |
| Persistence | One SQLite DB behind `Store` (std `Mutex`), 44 migrations registered in `store/schema.rs` `MIGRATIONS` | `crates/fleet-core/migrations/*.sql` |
| Session row | `sessions` (≈70 columns). **A mirror of tmux**, not an owned entity: rows are upserted by reconcile from `tmux list-sessions` (`store/reconcile.rs:376-430`, `ON CONFLICT(host_alias,tmux_name)`), which re-derives `project_id` from the pane cwd every pass | `store/rows.rs:123` `SessionRow`, `src/lib/sessions.ts:25` |
| Session identity | `sessions.id` is **not durable**: kill → `ghost` → hard-deleted one reconcile later (`store/reconcile.rs:525-558`); move creates a **new** row on the target (`move_session/mod.rs:3040`). The durable identity is `participants` (migration 043), re-pointed by a move (`store/participants.rs:106`), tombstoned on delete (`:166`), and **swept after a retention window** (`:228`) | |
| Lifecycle | `status` ∈ `running`/`ghost`; `lost_reason` ∈ `host_reboot`/`tmux_server_gone`/`missing`/`killed`; resumable ghosts kept `sessions.lost_ttl_secs` (14 d); `claude_status` & `stuck_kind` from `service/pane_intel.rs`. Kill keeps worktree **and** transcript; `safe_kill` asks Claude to commit+push first. **No archive concept.** Opt-in `service/gc.rs` *kills* idle sessions by TTL | `service/sessions/lifecycle.rs`, `restore.rs`, `discover.rs` |
| Claude conversation | `conversations` table (037), one row per Claude conversation of a session, **cascades on session delete**; transcripts on hosts are never deleted; `new_session{resume_claude_session_id}` and `discover_lost_sessions` recover them | `store/conversations.rs`, `service/transcript.rs` |
| Creation | `new_session` (`lifecycle.rs:309`): host, project, worktree / `new_worktree`+`base_branch` (`git worktree add -b`), kind, `friendly_name`, resume id. **No initial prompt**; seeding is spawn → `wait_for_repl_ready` → `send_prompt` (`review.rs:64-120`, `catalog/author_session.rs:247`) | |
| Projects | `projects` = a GitHub `owner/repo` (unique), host-independent; `worktrees` per host with `branch`. **No branch on sessions**, no live `git branch` observation | `store/projects.rs`, `service/worktrees.rs` |
| Hooks | 9 Claude Code hooks POST to `/hook` (`service/hooks_install.rs:117`): Stop, UserPromptSubmit, SessionStart/End, Pre/PostCompact, StopFailure, Notification, PostToolUse(Enter/ExitWorktree). Stored: `conversations.first_prompt` (200 chars, first only), `turn_done` detail (200 chars of last assistant msg), `transcript_path`, worktree branch from EnterWorktree. Hook responses can inject `additionalContext` (`service/delivery.rs`) | `mcp/hooks.rs`, `service/hooks.rs` |
| External data | **No HTTP client in fleet-core.** GitHub only through `gh pr view` on the host (`service/outcome.rs`) → `sessions.pr_url`, `ci_status` (300 s TTL). Anthropic usage via `curl` on host | `service/sessions/reconcile.rs:1160` |
| Timeline | `session_events` (013): free-text `kind`, capped 500/session, **deleted with the session** | `store/timeline.rs` |
| Labels | `sessions.tags` (JSON, ≤16 × ≤32 chars, MCP-only, not rendered, not carried by move); `friendly_name`; `notes` (dead column) | `mcp/tools/orchestration.rs:522` |
| Events | `RowChange` → `EVENT_NAMES` (`events.rs:30,469`) → desktop Tauri events / hub SSE `/events` → `subscribeToRowEvents` (`src/lib/events.ts`) → `createRowStore` (`src/lib/row_store.ts`) | |
| Background | `service/tick.rs` reconcile tick (20 s; playbooks, GC, usage, task sweep, repair, prune) + account-usage tick; started by the hub and by the **Local** desktop only | `bootstrap/tasks.rs`, `fleet-hub/src/serve.rs:810` |
| Hub-client mode | Desktop paired to a hub is a window onto it; every command is `Routed` / `LocalOnly` / `SameInBoth` in `src-tauri/src/backend/verdicts.rs` (*parity or refusal*) | `docs/hub.md` |
| Config / secrets | `settings` key→string with a typed registry (`service/settings.rs`); secrets (MCP token, host tokens, catalog secrets) are **plaintext SQLite**; only the desktop's hub token uses the keychain | |
| UI | 5-column grid (`App.svelte`): Sidebar (grouped by **project** only, `sidebar_index.ts` builders, `SidebarFilters.svelte` host/recency/triage/bg) · Details · right tabs Session/Files/Assets/Hosts (booleans, no router). Settings = one scrolling dialog of sections | `src/lib/Sidebar.svelte`, `SettingsDialog.svelte` |
| Mobile | `HubClient.kt` calls MCP tools over `/mcp/json`; SSE kinds `session/host/project`; groups host → project; phone sees only `PHONE_SESSION_FIELDS` (`mcp/tools/views.rs:71`) | `fleet-mobile/shared/...` |

Name collisions that constrain vocabulary:
**project** = a repository · **client** = a paired phone/browser token ·
**task** = a session-to-session dispatched job (`tasks`, 020) · **account** = a
Claude login (`accounts.organization_*` is display-only).

## B. Reusable components

- **Durable session identity:** `participants` — link work to *this*, and a
  move carries the link for free (`repoint_participant`).
- **Resume machinery:** `new_session{resume_claude_session_id}`,
  `recreate_session`, `restore_host_sessions`, `discover_lost_sessions`.
- **Work-preserving kill:** `safe_kill` (commit+push, keeps worktree),
  plain kill keeps worktree and transcript.
- **Prompt seeding:** `wait_for_repl_ready` + `send_prompt_inner` (review,
  catalog author, `dispatch_task`).
- **Signals:** hook bodies (prompt, cwd, transcript_path, worktree branch),
  `worktrees.branch`, the per-session `gh pr view` probe, `friendly_name`
  set by the agent over MCP, transcript parser (`parse_conversation`).
- **Precedence precedent:** hook-stamped state beats a pane guess
  (`last_hook_at` vs reconcile start) — the same "stronger source wins"
  rule the resolver needs.
- **Periodic work:** `service/tick.rs` pattern with settings-driven cadence,
  `AtomicBool` single-flight, backoff/Retry-After (`account_usage.rs`).
- **Plumbing:** `RowChange` + `createRowStore`; typed settings registry;
  verdict table; `ToolPolicy` (`mcp/guard.rs`); `FakeSsh`,
  `Store::open_in_memory`, `RecordingEventBus`.
- **UI:** `sidebar_index.ts` pure builders, `HostsList` group headers
  (`groupHostsByAccount`), `NewSessionDialog` (`initialName`, per-project
  prefs), `SecretsPanel`, right-column tab pattern, `McpSettings`-style
  settings sub-component.

## C. Gap analysis

| Target concept | State | Action |
|---|---|---|
| Organization | Missing (only display-only `accounts.organization_*`) | **New** small entity + `projects.org_id` |
| Tracker connection / provider | Missing; no HTTP client | **New** `trackers` table + `TrackerProvider` trait + first HTTP dependency |
| WorkItem | Missing (`tasks` is a different thing) | **New** cache table of normalised items |
| Session ↔ WorkItem link | Missing; `tags` is too weak (no provenance, not carried, 32-char cap) | **New** link table keyed on participant, with snapshot |
| Observations | Signals exist but are not collected as such | **New** small observations table + key extractor at existing hook/reconcile points |
| Live branch per session | Missing | **Extend** the existing PR probe script (same per-session cwd, same TTL) |
| Work timeline | `session_events` dies with the session | **New** `work_events`, domain-significant only |
| Archive | Missing | Model on the **link** (work ended), not on `sessions` (see H) |
| Initial prompt / handover | Missing on `new_session` | **Extend** `NewSessionArgs` with `initial_prompt` using the review.rs pattern |
| Work-aware list & filters | Sidebar groups by project only | **Extend** `sidebar_index.ts` + `SidebarFilters` |
| Project/Area hierarchy | `project` = repo | **Don't duplicate**: tracker hierarchy lives on work items (`parent`, `scope`); no Area entity yet |

Technical debt that bites this feature: plaintext secrets; `conversations` and
`session_events` cascade away with the row; retired participants are swept;
`move_session` copies only a few columns; `PHONE_SESSION_FIELDS` omits `tags`
while `SessionScreen.kt` pre-fills a tags editor from it (likely always empty
on the phone); `sessions.notes` is dead.

## D. Proposed domain model (smallest coherent)

```
Organization 1─* Tracker 1─* WorkItem ─parent→ WorkItem (epic etc.)
Organization 1─* Project (projects.org_id, nullable)
WorkItem *─* Participant(session)   via WorkLink  (+ snapshot, provenance, state)
Participant 1─* Observation  → resolver → WorkLink(suggested|confirmed|rejected)
WorkItem 1─* WorkEvent
```

| Entity | Purpose | Owner / source of truth | Lifecycle | Persistence |
|---|---|---|---|---|
| `orgs` | Isolation + grouping boundary ("Company A", "Personal") | User config | Created/renamed/deleted by user; delete refuses while trackers exist | `id, name, color, github_owners JSON` (auto-suggest rule) |
| `projects.org_id` | Repo → org | Explicit, else suggested from `github_owners` | Nullable = "unassigned" | column on existing table (**not** in reconcile's ON CONFLICT list) |
| `trackers` | One connection to one tracker instance | User config | `ok / auth_failed / unreachable`, `last_sync_at`, `last_error` | `id, org_id, provider, base_url, auth (secret), config JSON (project keys, views, branch template)` |
| `work_items` | Normalised cache of external items | **Tracker** (fleet never edits in v1) | Upserted by sync; `gone_at` when the tracker says 404; never hard-deleted while linked | `id, tracker_id, external_id, key, title, type, status_name, status_category (todo/in_progress/done), parent_id, scope (tracker project), assignee, url, updated_ext, fetched_at, gone_at, meta JSON`; **unique (tracker_id, external_id)** |
| `work_links` | Session ↔ item, N:M | **Fleet**: manual decisions + resolver | `suggested → confirmed` / `rejected` (sticky); `ended_at` when the participant retires (= archived work) | `id, work_item_id, participant_id NULL, state, source, strength, is_primary, evidence JSON, snapshot {host, tmux_name, friendly_name, project_id, worktree_key, branch, last_claude_session_id}, created_at, decided_by, ended_at` |
| `work_observations` | Raw signals, append/upsert, capped | Fleet | Pruned with a cap/age; never authoritative | `id, participant_id, signal, key, tracker_id NULL, detail, first_at, last_at, count` |
| `work_events` | Work timeline that survives sessions | Fleet | Capped per item | `id, work_item_id, at, kind, detail, participant_id NULL` |

Deliberately **not** entities: Area/Subproject (use `work_items.scope` +
`parent`; add a configured hierarchy only when a real org needs it),
Repository (= `projects`), Branch/Commit/PR (observations + snapshot fields;
PR already on the session row), LifecycleState (derived, see H), Agent/Instance
(= host/participant).

Every relationship answers "why": `source` ∈ `manual | started_from_item |
agent_tool | branch | pr | prompt | resumed`, `strength` ∈ `explicit | strong |
weak` (tiers, not floats — deterministic and explainable), `evidence` lists
observation ids.

## E. Work graph architecture

Relational, in the same SQLite, same `Store` mutex rules. Flow:

```
tracker sync ─┐                       ┌─> work_links ─┐
hooks ────────┼─> work_observations ──┤ resolver      ├─> RowChange work:* ─> stores ─> UI projections
reconcile ────┘   (signals)           └─> work_events ┘
manual action ───────────────────────────^ (highest precedence)
```

- New module `crates/fleet-core/src/work/` (or `service/work/`):
  `model.rs`, `keys.rs` (extractor), `resolve.rs` (pure), `sync.rs`,
  `providers/{mod.rs, jira.rs}`; store in `store/work.rs`.
- Queries are views over these tables (`list_work_items {org, tracker, status_category, scope, assignee, has_session, gone}`, `list_work_links {participant|item}`), so "sessions without work", "reopened items", "work with open PRs" are filters, not new storage.
- Events: `work_item:updated`, `work_link:updated`, `tracker:updated`, `org:updated` added to `EVENT_NAMES`; hub SSE and phone get them for free.
- All new commands are **`Routed`** in `verdicts.rs` (the hub owns data and credentials); tracker/org mutations are `Access::Master` in `mcp/guard.rs` (a paired client never reaches credentials); list/read tools are `Client` + `readonly`.

## F. Jira provider architecture

```rust
#[async_trait]
trait TrackerProvider {
    fn kind(&self) -> &'static str;                         // "jira"
    async fn probe(&self) -> Result<TrackerInfo>;           // whoami, fields, project keys, has_sprints
    async fn views(&self) -> Result<Vec<TrackerView>>;      // built-ins + user's favourite filters
    async fn query(&self, q: &TrackerQuery, page) -> Result<Page<WorkItemSnapshot>>;
    async fn get(&self, keys: &[String]) -> Result<Vec<Fetched>>;  // Fetched::Found | Gone
    fn key_pattern(&self) -> KeyPattern;                    // for the extractor
}
```

- **Normalisation, not assumptions:** status → `status_category` from Jira's
  own `statusCategory` (new / indeterminate / done), so custom workflows and
  names ("Reopened", "QA") need no mapping; keep `status_name` for display.
  Epic/parent from the unified `parent` field. Sprint field discovered via
  `/field` (schema `gh-sprint`) — "Current sprint" view offered **only if**
  present. Everything else → `meta` JSON, read only by the Jira adapter and
  its UI badge.
- **Views:** built-ins as JQL (`assignee = currentUser() AND statusCategory != Done`,
  `sprint in openSprints()` when sprints exist, recently updated), plus the
  user's favourite filters; free-form JQL as an advanced filter. Local filters
  run on the cache.
- **Sync:** `spawn_work_sync_tick` in `service/tick.rs`, started where the
  reconcile tick is (hub, Local desktop). Incremental: `updated >= last_sync -
  skew` for the configured views + refresh of every **linked** item (so a
  Done/Reopened transition is seen even when the item left "My work").
  Default 5 min, setting `work.sync_interval_secs`.
- **Failure:** 401/403 → `trackers.state = auth_failed`, stop polling that
  tracker, banner; 429 → Retry-After/backoff (copy `account_usage.rs`);
  network → keep cache, `fetched_at` shows staleness; 404 on a known item →
  `gone_at`, links kept. Fleet works fully offline on cached data.
- **Transport:** fleet-core has no HTTP client. Recommended: `reqwest` with
  `rustls` (cargo-deny check). Alternative: `curl` on the controller host
  over `SshExec`, like `account_usage` (no new dep, uglier).
- **Auth v1:** Jira Cloud, email + API token (Basic). DC/Server PAT later.

## G. Observation / detection architecture

Signals **available today** and where to tap them:

| Signal | Tap point | Strength |
|---|---|---|
| Started from item / manual link | new command | explicit |
| In-session agent declares it (`set_session_work_item` MCP tool, mirrors `set_friendly_name`) | `mcp/tools/orchestration.rs` | explicit |
| Branch name contains key (`ABC-123`) | `worktrees.branch` (scan + EnterWorktree hook) and a new `git rev-parse --abbrev-ref HEAD` appended to the existing PR probe script | strong |
| PR head branch / title (extend `gh pr view --json` with `headRefName,title`) | `service/outcome.rs` | strong |
| Key in a user prompt | `UserPromptSubmit` handler — extract keys from the **full** prompt, store only the matches | weak |
| Key in `friendly_name` / `last_assistant_message` | hook `Stop` / MCP | weak |
| Repo mapped to org / tracker project | config | scope only — **never** picks an item |

Key extractor: `[A-Z][A-Z0-9]{1,9}-[0-9]+`, accepted **only** if the prefix is
a project key of a tracker in the session's org (unassigned repo → all orgs,
capped to weak). Kills `UTF-8`, `SHA-256`, etc.

Resolver (pure fn, table-tested), per participant:
1. Manual `confirmed`/`rejected` decisions are final; a rejected
   (participant, item) pair is never re-suggested.
2. `explicit` → confirmed.
3. Single `strong` candidate → confirmed with `source = branch|pr` (shown as
   "auto"). Several strong candidates → all suggested, none primary.
4. `weak` → suggested only; two independent weak signals on the same key do
   **not** auto-confirm in v1 (keep it boring).
5. Primary = explicit > strong > most recent; user can change it.
6. Same key in two trackers (two Jira instances) → suggestion per tracker,
   never auto.

Re-runs on: hook arrival, probe result, tracker sync (a newly known key
retro-matches old observations — "ticket assigned after session exists"),
manual action. LLM inference is a later resolver input, not a v1 feature.

## H. Session lifecycle model

Six things the prompt's "session" conflates, and who owns each:

| Layer | Exists today | Owner |
|---|---|---|
| tmux process | `sessions.status = running` | reconcile (mirror of reality) |
| fleet session row | `running`/`ghost`, deleted after kill/TTL | reconcile |
| Claude conversation | `conversations` + transcript file on host (never deleted) | hooks |
| workspace | worktree + branch (removed only by safe-remove) | user / safe_kill |
| durable identity | `participants` (moved, retired, swept) | store |
| **work link** | new | work graph |

**Conflict with the prompt (existing convention should win):** the prompt puts
`active → idle → archived → resumed` on the *session*. Here a session row is a
projection of tmux; an `archived` flag on it would fight reconcile (which
resurrects ghosts it sees again) and would still vanish when the row is
deleted. So:

- **Active work** = link whose participant is live.
- **Idle** = derived (`idle_since`, `claude_status`), no new state.
- **Archived work** = link with `ended_at` (participant retired). The snapshot
  keeps host / project / worktree / branch / last Claude conversation, so the
  item still shows its history after the session row, `conversations` rows and
  even the participant are gone. `sweep_retired_participants` must null
  `work_links.participant_id` instead of orphaning it.
- **Archive action** on a live session = **existing** kill paths (safe kill if
  dirty). Never deletes worktree, branch, transcript or link.
- **Resume** from an item: (a) resume last conversation =
  `new_session{resume_claude_session_id, worktree}`; (b) fresh with handover
  = `new_session{initial_prompt}` built from item + link snapshots +
  `work_events`; (c) fresh. New link `source = resumed`.
- **Self-cleaning:** extend `gc.rs`'s pure planner with work state — "item
  `done` for ≥ N days and session idle ≥ M" becomes a **suggestion** by
  default; automatic archive only behind an opt-in setting, and only via the
  safe path. Conversely, never GC a session linked to an in-progress item.

## I. UX proposal (on the existing layout)

- **Org switcher / filter:** pills in `SidebarFilters` like host pills
  (persisted store), "All" default. Settings → new `WorkSettings.svelte`
  section: orgs, repo→org mapping (with owner-based suggestions), tracker
  connections (credentials via a `SecretsPanel`-like input, test button,
  state), lifecycle policy.
- **Sidebar "Group by: Project | Work"** toggle (pref). Work mode:
  `buildSessionsByWorkItem` in `sidebar_index.ts`; headers `KEY — title ·
  status` grouped under status category; **"Unclassified"** group (like
  "Other sessions"); collapsed "Done" group.
- **Row chip:** key on line 1. Confirmed = solid; suggested = dashed with
  `?` → popover *Confirm / Not this / Pick another…*; tooltip shows the
  evidence ("branch `feature/ABC-123-…`, prompt at 09:05").
- **Work tab** (right column, next to Files/Assets/Hosts): views list
  (My work, Current sprint when present, favourites), item list with status
  chip and linked-session count, item detail with linked sessions (live +
  archived), timeline, and **Start work** → `NewSessionDialog` prefilled:
  project from org mapping (picker if ambiguous), new worktree branch from the
  tracker's template (`{key}-{slug}`), friendly name `KEY title`, optional
  "brief Claude with the ticket" (initial prompt).
- **Reopened work:** an item whose `status_category` goes back from `done`
  shows a "has previous work" badge; opening it offers the three resume
  options.
- Not a Jira clone: no editing, commenting or transitions in v1.

## J. First vertical slice

Close to the prompt's, with two changes: **detection in slice 1 = explicit +
branch only** (the highest-precision signals), and **lifecycle in slice 1 =
archived-work view + resume, no automatic archive**.

Configure org → connect Jira Cloud → sync my items → Work tab browse/filter →
Start work (worktree + branch + link + optional brief) → sidebar grouped by
work item with Unclassified → branch-key auto-link with explanation →
manual confirm/reject/relink → killed sessions appear as archived work with
"resume".

## K. Incremental plan

Each phase is one PR, green on `scripts/ci-local.sh`.

1. **Domain storage.** Migration `045_work_graph.sql` (`orgs`, `projects.org_id`,
   `trackers`, `work_items`, `work_links`, `work_observations`, `work_events`),
   `store/work.rs`, row types (`#[serde(default)]` on every new wire field),
   `RowChange` variants, `sweep_retired_participants` nulls link participants.
   *Tests:* store unit tests, org isolation queries, link survives
   delete/move (participant repoint), sweep keeps links. *Accept:* no
   behaviour change; existing sessions all "Unclassified".
2. **Provider + sync.** HTTP dep, `TrackerProvider`, `JiraProvider`, sync tick,
   settings keys, MCP tools `list_trackers/add_tracker/sync_tracker/
   list_work_items/get_work_item` (+ guard rows, `REGEN_DOCS`), Tauri commands +
   verdict rows (`REGEN_HUB_VERDICTS`). *Tests:* normalisation on recorded Jira
   JSON fixtures (custom statuses, no sprints, no epics, next-gen parent),
   401/429/404/offline via a fake transport. *Accept:* items visible via MCP,
   stale data flagged.
3. **Links + Start work.** `link_work_item/unlink/reject/set_primary`,
   `new_session` gains `work_item_id` + `initial_prompt`, MCP
   `set_session_work_item` for agents. Frontend `work.ts` store, Work tab,
   Settings section, NewSessionDialog prefill. *Accept:* start from ticket →
   linked session, survives move and restart.
4. **Work-aware sidebar.** Group-by toggle, org pills, chips, Unclassified,
   filters. *Tests:* `sidebar_index` builder tests, Sidebar component tests.
5. **Detection.** Observations, key extractor, branch in probe, PR
   `headRefName/title`, prompt extraction, resolver + explanations UI.
   *Tests:* table-driven resolver (conflicts, reject stickiness, same key in two
   trackers, late-known key), hook/probe integration with `FakeSsh`.
6. **Lifecycle.** Archived-work view, resume options, deterministic handover
   template (no LLM), work-aware GC suggestions, opt-in auto-archive.
   *Tests:* planner table tests, resume builds the right `NewSessionArgs`.
7. **Reach + second provider.** Phone (`PHONE_SESSION_FIELDS`, event kinds,
   `ToolsTheAppMayCallTest`), and **GitHub Issues** via `gh` as the second
   provider to prove the abstraction cheaply.

## L. Risks and edge cases

| Case | Rule that handles it |
|---|---|
| One item, many sessions / one session, many items | `work_links` is N:M, one primary per participant |
| Ticket renamed / status moved | Sync upserts by `(tracker_id, external_id)`; key/title are attributes |
| Reopened | `status_category` leaves `done` → badge + resume offer |
| Deleted externally | `gone_at` tombstone; links and history kept |
| Jira down / offline / token expired | Cache serves; `fetched_at` staleness; `auth_failed` stops polling + banner |
| Repo in several contexts | `projects.org_id` explicit; ambiguous → suggestion only |
| Wrong key in branch | Manual reject is sticky; strong ≠ explicit |
| Ticket mentioned only as reference | Prompt mentions are weak → never auto-confirmed |
| Monorepo, several projects | Items carry `scope`; link is per item, not per repo |
| Session starts without a ticket | Unclassified; late sync retro-resolves observations |
| Ticket changes project/org | Item follows its tracker; links unchanged |
| Two Jira instances, same key | Identity is `(tracker_id, external_id)`; ambiguity never auto-links |
| Session moved / recreated / restored | Link on participant (repointed); restore keeps the row |
| Session row deleted | Link `ended_at` + snapshot; participant sweep nulls the pointer |
| Conflicting automatic signals | Several strong → all suggested, none auto |

Architectural risks: first outbound HTTP client in fleet-core (dependency and
TLS surface); tracker credentials in plaintext SQLite; reconcile's
`ON CONFLICT` list must never touch new columns; another tick adds load on the
hub; tool-description budget (`BUDGET_BYTES`) with ~10 new MCP tools — group
them.

## M. Open decisions requiring user input

1. **Jira flavour.** Cloud only (email + API token) vs also Server/DC (PAT).
   *Default:* Cloud only in v1.
2. **Credential storage.** (a) plaintext `settings`/new column, master-only,
   redacted everywhere — consistent with catalog secrets and host tokens;
   (b) OS keychain — desktop-only, breaks the hub; (c) env var on the hub.
   *Default:* (a), documented, plus (c) as an override.
3. **HTTP transport.** `reqwest`+`rustls` in fleet-core vs `curl` over
   `SshExec` on the controller. *Default:* reqwest.
4. **What "archive" does to a live session.** (a) UI-only: collapse into
   "Done", tmux keeps running; (b) kill via safe path (worktree, branch,
   transcript kept; resumable). *Default:* (a) automatically, (b) manual or
   opt-in policy.
5. **Auto-confirm on branch key.** Auto-link with an "auto" marker vs always
   ask. *Default:* auto for a single strong candidate in an org-mapped repo.
6. **Branch template for Start work.** `{key}-{slug}` vs `feature/{key}-{slug}`.
   *Default:* per-tracker setting, `{key}-{slug}`.
7. **Sync scope.** Assigned-to-me + favourites + linked items vs whole
   projects. *Default:* the former.
8. **Write-back to Jira** (transition to In Progress on Start, comment with PR)
   — confirm it stays out of v1. *Default:* out.
