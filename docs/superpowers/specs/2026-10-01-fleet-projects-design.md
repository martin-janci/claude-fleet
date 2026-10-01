# Fleet Projects (shared coordination units) — design

**Date:** 2026-10-01
**Status:** draft for review. Nothing is built. Every decision table below is a
*proposal with a recommendation*; the owner's "yes" per row is what starts work
(the roadmap's rule for a decision-gated feature).
**Input:** the owner's conversation of 2026-10-01 (requirements, boundaries and
vocabulary are reproduced in §1 and §4).
**Builds on:** orgs (migration 050), the work graph (`2026-09-24-work-graph-design.md`),
the Work view (`2026-09-27-work-view-design.md`), shared work context
(`2026-09-29-shared-work-context-design.md`), the asset catalog
(`2026-09-29-assets-workspace-design.md`, `2026-09-30-assets-s1b-s2-design.md`),
multi-harness (`2026-09-29-multi-harness-agents-design.md`).

## 1. Goal

A **Project** is a shared coordination unit: one goal, one context, one set of
repositories, several people, several provider accounts and several coding
tools. Work on it continues across sessions, hosts and harnesses, and a person
sees the same Project whichever tool executes it.

From the owner's requirements:

| # | Requirement | Where it is answered |
|---|---|---|
| R1 | Several repositories in one project, worked as a whole | §5 `fleet_project_repos`, §8 multi-repo start |
| R2 | Several people collaborate: shared briefs, context, decisions, results | §5 `people`, §6 context, §9 access |
| R3 | Not bound to one Claude account or one provider | §5 connections, §7 capability matrix |
| R4 | Shared context: goals, instructions, references, memory | §6 |
| R5 | Coordination: tasks, dependencies, across repos, results collected | §8 |
| R6 | Use a provider's native capability where it is available **and controllable**; substitute the rest; judge capability by capability, not "supports Projects" | §7 |
| R7 | One way of working whatever executes it; a gap Fleet cannot fill must be visible | §7 `project_capabilities`, §11 UI |
| R8 | Project settings ride Fleet's existing sync, not a second mechanism | §12 |

### Non-goals

- **No second task registry.** A Project never stores its own copy of a task.
  It groups existing `work_items` (§8).
- **No second permission system.** Org stays the access boundary. Project
  membership never widens what the org boundary allows (§9).
- **No write-back to a provider's native Project.** Not possible today (§2).
- **No cross-hub Project replication.** Open question 6 of the conversation was
  never confirmed as a requirement, and federation carries only
  session-addressed messages today (§2 F6). Out of scope; §16 keeps it open.
- **No renaming of the existing `projects` table.** The UI strings change
  (§4); the schema does not.

## 2. Verified findings

These are the conversation's six open items, checked against the code in this
repo and against vendor documentation on 2026-10-01.

### F1 — Claude Projects has no supported interface Fleet can drive (blocking for R6)

`https://code.claude.com/docs/en/claude-projects`, *Limitations*, read 2026-10-01:

- Projects exist "at claude.ai/code, in the desktop app, and in the Claude
  mobile app, **not in the terminal CLI**, the VS Code extension, or the
  JetBrains plugin". There is no API, SDK or MCP surface for them.
- The CLI's `claude project` namespace is **unrelated**: its only subcommand is
  `claude project purge [path]`, which deletes *local* Claude Code state for a
  directory (`cli-reference`, read 2026-10-01).
- "A project belongs to one user. You can't share a project or its threads with
  another user… There are no organization-level controls for projects during the
  beta." Public beta, **Pro and Max only, not Team or Enterprise**.
- "You can't add a session you started yourself on your machine to a project."
  A thread is an Anthropic cloud session, or a session on your machine through
  Remote Control. Anthropic is the model provider in both cases.
- A thread belongs to the one project that started it; threads cannot move
  between projects, and two projects cannot merge.

**Consequence.** A native Claude Project cannot be the base of a Fleet Project,
and Fleet cannot control one. It structurally contradicts R2 (one user, not
shareable), R3 (single provider) and R7 (Fleet cannot drive it). The conclusion
the conversation reached — *Fleet owns the Project; a native project is at most
one possible connection* — is confirmed, and the connection is **reference
only** (§7).

### F2 — What *is* natively controllable (the usable half of R6)

From `cli-reference`, read 2026-10-01:

| Native capability | Surface | Fleet's use |
|---|---|---|
| Several repositories in one session | `--add-dir`, `permissions.additionalDirectories` | the per-session half of R1 |
| Standing instructions | `CLAUDE.md` per repo; `--append-system-prompt-file`; `--settings` | §6 context delivery |
| Cloud session create / follow-up | `claude --cloud "task"`, `claude -p --cloud <session-id>` | a later execution target (§16 Q3) |
| A provider thread in a Fleet folder | `claude remote-control` (server mode) on a Fleet host | an option, not v1 (§16 Q4) |
| Worktree isolation | `--worktree`, `--tmux` | already used by Fleet |
| Config import from another agent | `claude import codex` | catalog import, already planned |

What has **no** provider surface, and is therefore Fleet's to substitute:
cross-session project memory, project membership, cross-repo task coordination,
and cross-account/cross-harness continuity.

### F3 — The existing boundaries hold, and a Project is genuinely new

- **Org** is the boundary: `orgs`, `org_rules`, `hosts.org_id`,
  `client_tokens.org_id`, and `service::orgs::OrgScope` built only by
  `Caller::org_scope`, which filters every work read. Nothing else may become a
  boundary.
- **Work** owns tasks: `work_items` (with `parent_id`, `origin`, `notes`,
  `project_id`, migration 086), `work_links`, the journal, handover, resume.
- **Today's `projects`** is `(owner, repo, base_path)` + `worktrees` — a
  registered repository or adopted folder, *not* a coordination unit.
- The Work view's **group is only a label**: `work_placements.group_label TEXT`,
  documented in migration 066 as "Navigation only: never a boundary". So a
  Project is not today's group, and §8 wires the two rather than duplicating.

### F4 — Fleet has no person entity (answers conversation item 4)

Access today is held by **devices and hosts**, never people:
`client_tokens` (`mode` full/readonly, `trusted_at`, `org_id`,
`assets_admin_at`), `host_tokens`, and the master. `accounts` is a *provider*
account (`uuid`, `email`, `organization_uuid`, `seat_tier`), not a Fleet user.

So "project members" cannot be expressed today. §9 proposes the minimum that
does not become a second permission system: a `people` row for **attribution
and filtering only**, with authorization left exactly as it is.

### F5 — The sync engine exists and is host-scoped (constrains R8)

The asset catalog is the one content engine: `catalogs` (migration 090, per-org
rows), layers on two axes (`Axis::Role`, `Axis::Context`) selected per host in
`host_layers`, `plan_sync` / `apply_sync` with CAS, per-harness render through
the `Harness` impls, and the `harness_gate` per host (migration 089).

Two consequences for project context:

- The sync **unit is a host**, not a session or a worktree. A project-scoped
  catalog layer reaches every session on the hosts it is attached to. Per-session
  scoping needs the hook path instead (§6).
- `Kind` is `Skill | Agent | Hook | McpServer | PluginRef`. There is **no
  `Instructions` kind** yet — it is item 1 of multi-harness phase F3 and is a
  hard dependency for syncing a project's instructions as an asset (§12).

### F6 — Hub↔hub federation carries messages, not state

`2026-09-24-hub-federation-design.md` non-goals: only `session` addresses cross
a link, and a hub never re-forwards. There is no replication of orgs, work or
catalogs. A Project spanning two hubs is new work; §16 Q5 keeps it open.

## 3. Decisions to confirm

| # | Question | Recommendation |
|---|---|---|
| P1 | Does Fleet own the Project, with a provider project only ever a connection? | **Yes.** F1 leaves no alternative. |
| P2 | Is a native Claude Project connection built at all in v1? | **Yes, as `reference` only**: a stored link plus an *exportable* instructions block a person pastes. No automation claimed. |
| P3 | Does a Project become a boundary? | **No.** Org stays the only boundary; a Project is a scope *inside* an org (§9). |
| P4 | How are members modelled? | A `people` row for attribution and filtering; authorization unchanged (F4, §9). |
| P5 | Where do a Project's tasks live? | In `work_items`, unchanged. A Project adds one column and one read (§8). |
| P6 | Is a Project org-bound? | **Optional**: `org_id` nullable (a personal Project), exactly like `catalogs`. A Project with an org never crosses it without `force_cross_org`. |
| P7 | Does a Project's context ride the catalog? | **Both**: assets ride the catalog as a context-axis layer; the per-session header rides the existing hook `additionalContext` budget (§6). |
| P8 | Does the UI rename today's `projects` to "Repository"? | **Yes, strings only.** The vocabulary needs the word "Project" free. No schema or API rename. |
| P9 | Scope of v1 | P0–P2 of §15 (model + context + visible capabilities). Coordination beyond grouping, and non-Claude harnesses, come after. |

## 4. Vocabulary

| UI (EN) | UI (SK) | Code |
|---|---|---|
| Project | Projekt | `fleet_projects`, `FleetProject`, `fleet_project_*` |
| Repository | Repozitár / lokálny priečinok | today's `projects`, `worktrees` (**unchanged**) |
| Task | Úloha | `work_items` |
| Session | Relácia | `sessions` |
| Organisation | Organizácia | `orgs` |
| Provider account | Účet poskytovateľa | `accounts` |
| Provider project | Natívny projekt poskytovateľa | `fleet_project_connections` |
| Hub | Fleet hub | `fleet-hub` |

"Cloud project" is not used: it conflates the provider with the place of
execution. The code keeps `projects` for repositories; only user-visible strings
change (P8), so `docs/` and the frontend labels move, and
`src/lib/hub_verdicts.generated.json`, the MCP tool names and every `project_id`
stay as they are.

## 5. Model

```
FleetProject (fleet_projects)
├── org_id?            — NULL = personal, else the one org it lives in (P6)
├── goal, status       — one line; active | paused | archived
├── repos              — fleet_project_repos → projects(id), role primary|secondary
├── members            — fleet_project_members → people(id), role owner|member|viewer
├── context            — fleet_project_context rows (goal | instructions | reference | decision | memory)
├── connections        — fleet_project_connections (provider, kind, external_ref, mode)
└── tasks              — work_items.fleet_project_id (grouping only; Work still owns the task)
```

### Migration `091_fleet_projects.sql` (additive)

```sql
CREATE TABLE IF NOT EXISTS fleet_projects (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT    NOT NULL,
  org_id      INTEGER REFERENCES orgs(id) ON DELETE RESTRICT,
  goal        TEXT,
  status      TEXT    NOT NULL DEFAULT 'active',   -- active | paused | archived
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  version     INTEGER NOT NULL DEFAULT 1           -- compare-and-set, as work_links
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_fleet_projects_name
  ON fleet_projects(name) WHERE status <> 'archived';

CREATE TABLE IF NOT EXISTS fleet_project_repos (
  fleet_project_id INTEGER NOT NULL REFERENCES fleet_projects(id) ON DELETE CASCADE,
  project_id       INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  role             TEXT    NOT NULL DEFAULT 'secondary',  -- primary | secondary
  PRIMARY KEY (fleet_project_id, project_id)
);

-- Attribution and filtering only. NEVER an authorization subject (§9).
CREATE TABLE IF NOT EXISTS people (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  display_name TEXT    NOT NULL,
  email        TEXT,
  created_at   INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_people_email ON people(email) WHERE email IS NOT NULL;

-- One person, many identities. Every column is optional; a row sets exactly one.
CREATE TABLE IF NOT EXISTS person_identities (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  person_id     INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  client_name   TEXT,            -- client_tokens.name of a live row; no FK (a revoked row keeps its name)
  host_alias    TEXT,
  account_uuid  TEXT REFERENCES accounts(uuid) ON DELETE SET NULL,
  CHECK ((client_name IS NOT NULL) + (host_alias IS NOT NULL) + (account_uuid IS NOT NULL) = 1)
);

CREATE TABLE IF NOT EXISTS fleet_project_members (
  fleet_project_id INTEGER NOT NULL REFERENCES fleet_projects(id) ON DELETE CASCADE,
  person_id        INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  role             TEXT    NOT NULL DEFAULT 'member',   -- owner | member | viewer
  added_at         INTEGER NOT NULL,
  PRIMARY KEY (fleet_project_id, person_id)
);

CREATE TABLE IF NOT EXISTS fleet_project_context (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  fleet_project_id INTEGER NOT NULL REFERENCES fleet_projects(id) ON DELETE CASCADE,
  kind             TEXT    NOT NULL,   -- goal | instructions | reference | decision | memory
  title            TEXT,
  body             TEXT    NOT NULL,
  pinned           INTEGER NOT NULL DEFAULT 0,  -- pinned rows feed the session header (§6)
  author_person_id INTEGER REFERENCES people(id) ON DELETE SET NULL,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fpc_project ON fleet_project_context(fleet_project_id, kind);

CREATE TABLE IF NOT EXISTS fleet_project_connections (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  fleet_project_id INTEGER NOT NULL REFERENCES fleet_projects(id) ON DELETE CASCADE,
  provider         TEXT    NOT NULL,   -- anthropic | openai | google | …
  kind             TEXT    NOT NULL,   -- native_project | account | catalog_layer
  external_ref     TEXT,               -- a URL or an opaque id; never a credential
  mode             TEXT    NOT NULL,   -- reference | assisted | controlled
  created_at       INTEGER NOT NULL
);

ALTER TABLE work_items ADD COLUMN fleet_project_id INTEGER
  REFERENCES fleet_projects(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_fleet_project
  ON work_items(fleet_project_id) WHERE fleet_project_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (91);
```

`ALTER TABLE` is not idempotent, so the migration's `MIGRATIONS` entry in
`store/schema.rs` carries an `already_applied` column guard for
`work_items.fleet_project_id`, as 086, 087 and 089 do for theirs. The new
tables are `CREATE TABLE IF NOT EXISTS`, so they need none.

**A credential is never stored here.** `external_ref` holds a reference only.
A provider credential keeps its existing home and its single reader, the way
`Store::resolve_tracker_credential` and `Store::resolve_decision_credential` do.

### Invariants

| # | Invariant | Enforced by |
|---|---|---|
| I1 | A Project is never a boundary | no `OrgScope` equivalent; every read still goes through `Caller::org_scope` first |
| I2 | A task in an org-bound Project is in that org | a check on `work_items.fleet_project_id`, refused `E_FORBIDDEN` without `force_cross_org` |
| I3 | A repo in an org-bound Project resolves to that org | the `org_rules` / `hosts.org_id` resolution already used for sessions; a mismatch is a warning on the Project page, never a silent re-home |
| I4 | A Project never widens a caller | §9's table; a per-host token still sees only its host's and org's rows |
| I5 | One archived name may repeat | the partial unique index above |

## 6. Shared context and how it reaches a session

Two delivery paths, because the existing mechanisms have two different scopes
(F5). Neither is a new sync engine (R8).

**Path A — per session, through the hook `additionalContext`.** A new *project
header* slice, composed before the work brief in today's ladder:

- A hook's `additionalContext` carries **8000 chars / 200 lines**, shared by
  the mail, the brief and the nudge (a `Stop` block's `reason` has its own far
  smaller 2000 / 20 budget and is not a path for this).
- `SessionStart` is capped at `SESSION_START_CONTEXT_MAX = 4000`, and the M2
  handover brief is `BRIEF_MAX_CHARS = 4000`.
- The project header gets its own `PROJECT_HEADER_MAX = 800` inside that, and is
  **dropped whole** when it does not fit — it never truncates a brief, and
  nothing inside an `UNTRUSTED_END` fence is ever cut, exactly as M4.5 requires.
- Content: the Project name and goal (Fleet's own lines, markers defused), its
  repository list, and its pinned `instructions` rows inside one
  `mark_untrusted` fence. A person's and an agent's text is third-party text.
- It is gated by its own setting, `projects.session_header` (default **off**),
  following D5's precedent: the owner measures the cost with
  `scripts/measure-session-start.sh` before it goes on.

**Path B — project assets, through the catalog.** A Project may own one
`Axis::Context` layer in its org's catalog (or the personal one). The layer is
attached to the hosts the Project uses, and `plan_sync` / `apply_sync` deliver
it unchanged. This is where a Project's skills, agents, MCP servers and
`Instructions` asset belong.

**Honest limits, both stated in the UI (R7):**

- Path B is **host-scoped**: a project layer reaches every session on that host,
  not only the Project's. Per-session scoping is Path A's.
- Path B's instructions need the catalog `Instructions` kind, which does not
  exist yet (F5). Until multi-harness F3 item 1 lands, a Project's instructions
  are Path A only, and the Project page says so.

## 7. Provider capabilities: native, substituted, or a visible gap

R6 asks for a judgement **per capability**, not per product. The registry
extends the `Capability` enum that multi-harness F5 introduces for
`SessionRuntime`, rather than starting a second one.

`project_capabilities` is **computed, never stored**, from (harness, provider,
connection mode) and served on the Project page and by the MCP read.

| Capability | Claude Code | Codex | Verdict |
|---|---|---|---|
| Several repositories in one session | native (`--add-dir`) | per-harness flag | **native** where present, else Fleet starts one session per repo |
| Standing instructions | native (`CLAUDE.md`, `--append-system-prompt-file`) | `AGENTS.md`, 32 KiB cap | **native**, rendered by the catalog |
| Cross-session project memory | none | none | **substituted** (work journal, handover, §6) |
| Project membership / sharing | none (one user, F1) | none | **substituted** (§9) |
| Parallel work threads under one goal | native *inside* a provider Project Fleet cannot drive | none | **substituted** (Fleet sessions, `dispatch_task`) |
| Cross-repo task coordination | none | none | **substituted** (§8) |
| Native provider Project, driven by Fleet | **unavailable** (F1) | n/a | **gap** — shown, never faked |
| Transcript / usage / rewind | native | partial | capability-gated, as F5 already defines |

Three modes, and only the first is claimed today:

- `reference` — Fleet stores the link and can **export** an instructions block
  for a person to paste. No read, no write, no status. This is the only mode a
  native Claude Project gets (P2).
- `assisted` — Fleet prepares an action a person confirms and performs.
- `controlled` — Fleet drives it through a supported interface. Nothing is in
  this mode at the time of writing.

A gap is a first-class row in the UI with its reason and its date of checking,
so "we checked on 2026-10-01 and there is no interface" is visible rather than
remembered.

## 8. Coordination without a second registry

A Project **groups** work; it never stores a task.

- `work_items.fleet_project_id` places an existing task (tracker item or native
  item) in a Project. A tracker item stays read-only (shared-work-context C3).
- Subtasks, proposals, steps, jobs, journal, handover, summarise, tidy: all
  unchanged. A Project adds no level to the one-deep subtask rule.
- **Dependencies** are the one genuinely new relation R5 asks for and the one
  most likely to become process for its own sake. Recommendation: **not in v1**.
  The Project page shows tasks by status and by repository; a `blocked_by`
  relation waits for evidence that the list is not enough (§16 Q1).
- The **Work view** gains a *Project* grouping next to today's org → group
  tree; `work_placements.group_label` stays what it is (a label, F3), and a
  task's Project is read from its column instead.
- **Multi-repo start** already takes `work_link start { project_ids }`
  (M9.6). A Project supplies the default `project_ids` — its `primary` repo
  first — so starting from a Project is the existing path with a filled-in
  argument, not a new one.
- **Results** are collected where they already are: the journal, the PR probe,
  `summarize`, and the handover brief. The Project page reads them per task.

## 9. Access: Project roles against Org

The honest answer to the conversation's item 4, given F4.

| Question | Answer in v1 |
|---|---|
| Is a Project an access boundary? | No. Org is (I1). |
| What does `fleet_project_members` do? | Attribution ("who decided this", "mine"), and filtering. |
| Can a member role grant access? | **No.** A role is recorded intent. |
| Who may create or edit a Project? | The master, or a trusted `full` device — the rule `settings_writer` already uses for settings writes. |
| What may an org-bound client do? | Read and write Projects of **its** org only, by the `OrgScope::Org` path M14.1 already built. |
| What may a per-host token do? | Read the Projects its host's sessions work in; place its own tasks; never create, never add a member. |
| What may a `readonly` client do? | Read only. |
| What happens on a cross-org placement? | Refused, `E_FORBIDDEN`, unless `force_cross_org`, exactly as a cross-org work link. |

When Fleet later needs real per-person authorization, `people` is the subject it
will attach to — but that is a separate design with its own threat model, not a
by-product of this one.

## 10. API surface

**MCP (`mcp/tools/`).** One tool, `project`, with actions:

| Action | Access | Notes |
|---|---|---|
| `list`, `get` | `Access::Client` | org-filtered; a per-host token sees its host's Projects |
| `create`, `update`, `archive` | master or trusted `full` device | compare-and-set on `version`, `E_CONFLICT` |
| `add_repo`, `remove_repo` | master or trusted device | validates the org of the repo (I3) |
| `add_member`, `remove_member` | master or trusted device | attribution only (§9) |
| `context_add`, `context_update`, `context_remove`, `context_list` | writes: trusted device; `context_add` with `kind: memory` also from a session's own token | an agent may add a memory note under its Project, never instructions |
| `connect`, `disconnect`, `capabilities` | master or trusted device; `capabilities` is `Access::Client` | `capabilities` is the computed matrix (§7) |
| `place_task`, `unplace_task` | the task's existing write access | wraps `work_items.fleet_project_id` |

Every action needs a row in the isolation matrix
(`mcp/tools/tests_isolation.rs`), which fails without one.

**Desktop commands** (`src-tauri/src/commands/`), one per action, thin over
`service/projects/`. Every one needs a verdict row in `backend/verdicts.rs`
**by command name**, then `REGEN_HUB_VERDICTS=1`, then — for anything
`LocalOnly` the UI can reach — a `REASONS` entry or an allowlist line in
`src/lib/hub_verdicts.test.ts`. Expected verdicts: all `Routed` (a Project is
hub state, like work), except a provider-connection export, which is
`LocalOnly` if it ever touches a local file.

**Events.** One row-event kind, `project`, ids only, added to
`HOST_BOUND_HIDDEN_KINDS` (`mcp/events_route.rs`) beside `work`, `settings` and
`update`: a Project is not a host's, so it never rides a host- or org-bound
stream. `projects:changed` joins `work:changed` in the frontend's re-read tick.

**Wire.** Additive: new tool, new actions, new row fields. No
`CONTRACT_REVISION` bump, following `wire_contract.rs`; an older hub answers
`E_INVALID "unknown tool"`, which the desktop shows as a capability gap.

**Settings.** `projects.session_header` (bool, off), `projects.header_max_chars`
(default 800). Both need a `SPECS` row with full metadata, a home on a page
(`every_setting_has_one_home`), and `REGEN_SETTINGS_DOCS=1`.

## 11. UI

- **Projects** becomes a top-level screen beside Sessions and Work. The list
  shows name, goal, org, repository count, open-task count, live-session count,
  and a capability-gap badge.
- **Project page**: Goal · Context (pinned first) · Repositories · Tasks (by
  status, by repository) · Sessions · Members · Connections & capabilities.
- **Capabilities panel** is where R7 lives: one row per capability with
  *native* / *Fleet* / *not available*, the reason, and the date checked. A
  native Claude Project connection reads "reference only — no supported
  interface (checked 2026-10-01)".
- A session's header gets a Project chip next to the work chip.
- Today's "Project" labels become "Repository" (P8), including the New-session
  dialog, Host detail and `docs/`.
- Every third-party string — a goal, a context body, a member name, a provider
  reference — renders as **text, never markup**, as the Work view already does.
- A declarative page is the right home for the Project *settings* rows (P7 and
  the pages framework); the Project page itself is a normal screen, since
  `master_detail` resources bind existing commands only.

## 12. Sync (R8)

A Project adds **no** sync mechanism:

- Project **assets** → one `Axis::Context` catalog layer, delivered by
  `plan_sync` / `apply_sync` to the hosts the Project uses, per-harness through
  the existing `Harness` impls, gated by `harness_gate`.
- Project **instructions as an asset** → the catalog `Instructions` kind
  (multi-harness F3 item 1). **Dependency, not yet built.**
- Project **rows** → the hub, read by desktops and phones over the existing
  tools and `/events`, like work.
- Project **context into a session** → the existing hook `additionalContext`
  (§6 Path A).

What a Project must never do: write its own files to a host outside the catalog,
or keep a second copy of a repository's `CLAUDE.md`.

## 13. Risks and counter-measures

Following the brainstorming's three product risks.

| Risk | Counter-measure |
|---|---|
| Another system to maintain by hand | A Project owns only what nothing else owns: goal, context, membership, repo set, connections. Tasks stay in Work, repos in `projects`, content in the catalog, boundaries in Org. |
| A Project becomes a second permission system | I1 and §9: never a boundary, never a grant. The isolation matrix fails a new action without a row. |
| AI creates more process than value | No dependency graph in v1 (§8). An agent may add a `memory` note, never instructions and never a member. The session header is off by default and capped at 800 chars. |
| A convincing capability claim without a working interface | §7's three modes, and a gap shown with its checking date. Nothing enters `controlled` without a named interface in the spec. |
| Context bloats every session | One cap, dropped whole rather than truncated; measured with `scripts/measure-session-start.sh` before the setting goes on. |
| The vocabulary change confuses existing users | Strings only (P8); every id, tool name and column keeps its name, so nothing breaks and a rename migration stays a separate proposal. |

## 14. Testing

- **Store:** migration 091 on a v90 DB, its re-run, and the column guard; the
  archived-name index; `person_identities`' one-of-three CHECK; cascade on
  archive-then-delete.
- **Boundary:** I1–I5 each as a test. A per-host token's `project list`; an
  org-bound client confined to its org; a cross-org `place_task` refused and
  allowed with `force_cross_org`; an org-mismatched repo surfacing as a warning
  and not re-homing.
- **Context:** the header at, just over and far over `PROJECT_HEADER_MAX`; the
  header dropped rather than a brief truncated; a fence never cut; a forged
  `UNTRUSTED_END` in a goal defused; the setting off meaning no header.
- **Capabilities:** the computed matrix per (harness, provider, mode); a native
  Claude Project reporting `reference` and never `controlled`.
- **Coordination:** a Project's `project_ids` default on start; the Work view
  grouped by Project; a tracker item placed in a Project staying read-only.
- **Isolation matrix:** every new action.
- **Frontend (vitest):** the Projects list and page; the capability panel's
  three states; text-not-markup for every third-party string; the
  "Repository" strings; `hub_verdicts.test.ts` for each new command.
- **Docs generators:** `REGEN_HUB_VERDICTS=1`, `REGEN_SETTINGS_DOCS=1`,
  `REGEN_PAGE_DOCS=1`, `REGEN_DOCS=1` for the control-API reference.
- **CI:** `scripts/ci-local.sh`, plus a `scripts/hub-e2e.sh` section that
  creates a Project, places a task, and starts it multi-repo on a real hub.

## 15. Roadmap

| Phase | Deliverable | Exit criterion |
|---|---|---|
| **P0 Model** | migration 091, `service/projects/`, the `project` tool, desktop commands + verdicts, events, isolation rows | a Project with repos and members exists on a hub and reads back correctly under every access class |
| **P1 Context** | context rows, Path A header behind `projects.session_header`, the measurement run | a session on a Project's repo starts with the header when it is on, and byte-identically without it when off |
| **P2 Capabilities** | the computed matrix, the `reference` connection, the UI panel, the "Repository" strings | the Project page states every capability as native / Fleet / not available, with its date |
| **P3 Coordination** | `place_task`, Project grouping in the Work view, `project_ids` default on start | starting from a Project opens the right repos, and its tasks group under it |
| **P4 Project assets** | the context-axis layer per Project; the catalog `Instructions` kind (needs multi-harness F3 item 1) | one sync delivers a Project's skills and instructions to its hosts for Claude and Codex |
| **P5 Non-Claude harnesses** | per-harness capability rows as F5/F6 land | a Codex session in a Project shows the same Project with its gaps named |

P0 → P1 → P2 is the critical path to R7 (nothing is claimed that is not true).
P4 depends on multi-harness F3; P5 on F5.

## 16. Open questions for the owner

1. **Dependencies between tasks.** Recommended out of v1 (§8). Is the Project
   page's status-and-repo grouping enough to start?
2. **Members without authorization.** §9 makes a role attribution only. Is that
   acceptable for the first release, or must per-person access come first — in
   which case it is its own design and this one waits?
3. **Cloud sessions as an execution target.** `claude --cloud` is a real
   interface (F2) and would let a Fleet Project start work with no host. Worth a
   phase of its own, or not wanted?
4. **Remote Control as a bridge.** Fleet could run `claude remote-control` on a
   host so a native Claude Project thread lands in a Fleet-managed folder. It
   gives a native project a Fleet workspace, but Fleet would not own the
   session. Explore, or leave?
5. **Cross-hub Projects.** Never confirmed as a requirement, and federation
   replicates no state (F6). Confirm out of scope, or open it as its own cycle?
6. **Where the vocabulary lands.** This spec renames UI strings only (P8). Is a
   later schema rename (`projects` → `repositories`) wanted at all, or does the
   code keep its names permanently?
