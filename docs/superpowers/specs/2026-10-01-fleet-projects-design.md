# Fleet Projects (shared coordination units) — design

**Date:** 2026-10-01
**Status:** draft for review. Nothing is built. §3's rows marked *agreed* were
answered by the owner on 2026-10-01. The rows marked *recommended* carry a
reasoned recommendation and the cost of being wrong (§16); the rows marked
*open* are forced by §2's findings and need only a confirming "yes".
**Input:** the owner's conversation of 2026-10-01 (requirements, boundaries and
vocabulary in §1 and §4), and their three answers of the same day (§3).
**Builds on:** orgs (migration 050), the work graph
(`2026-09-24-work-graph-design.md`), the Work view
(`2026-09-27-work-view-design.md`), shared work context
(`2026-09-29-shared-work-context-design.md`), the asset catalog and its
scope boundary (`2026-09-29-assets-workspace-design.md`,
`2026-09-30-assets-s1b-s2-design.md`, plan `2026-09-30-assets-m2-sync.md`),
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
| R2 | Several people collaborate: shared briefs, context, decisions, results | §6 context, §9 access (deferred to the parallel permissions system) |
| R3 | Not bound to one Claude account or one provider | §5 connections, §7 capability matrix |
| R4 | Shared context: goals, instructions, references, memory | §6 |
| R5 | Coordination: tasks, dependencies, across repos, results collected | §8 |
| R6 | Use a provider's native capability where it is available **and controllable**; substitute the rest; judge capability by capability, not "supports Projects" | §7 |
| R7 | One way of working whatever executes it; a gap Fleet cannot fill must be visible | §7, §11 |
| R8 | Project settings ride Fleet's existing sync, not a second mechanism | §12 |

### Non-goals

- **No second task registry.** A Project never stores its own copy of a task.
  It groups existing `work_items` (§8).
- **No second permission system.** A permissions and synchronisation system is
  already being developed in parallel (owner, 2026-10-01). This spec defines a
  seam to it and introduces **no** subject, role or grant table of its own (§9).
- **No dependency graph between tasks** (owner's answer 1 — §8).
- **No write-back to a provider's native Project.** Not possible today (§2 F1).
- **No cross-hub Project replication.** Never confirmed as a requirement, and
  federation carries only session-addressed messages (§2 F6). §16 keeps it open.
- **No renaming of the existing `projects` table.** The UI strings change
  (§4); the schema does not.

## 2. Verified findings

The conversation's six open items, checked against this repo and against vendor
documentation on 2026-10-01.

### F1 — Claude Projects has no supported interface Fleet can drive

`https://code.claude.com/docs/en/claude-projects`, *Limitations*, read 2026-10-01:

- Projects exist "at claude.ai/code, in the desktop app, and in the Claude
  mobile app, **not in the terminal CLI**, the VS Code extension, or the
  JetBrains plugin". No API, SDK or MCP surface.
- The CLI's `claude project` namespace is **unrelated**: its only subcommand is
  `claude project purge [path]`, which deletes *local* Claude Code state for a
  directory (`cli-reference`, read 2026-10-01).
- "A project belongs to one user. You can't share a project or its threads with
  another user… There are no organization-level controls for projects during the
  beta." Public beta, **Pro and Max only, not Team or Enterprise**.
- "You can't add a session you started yourself on your machine to a project."
- A thread belongs to the one project that started it; threads cannot move
  between projects, and two projects cannot merge.

**Consequence.** A native Claude Project cannot be the base of a Fleet Project,
and Fleet cannot control one. It contradicts R2 (one user, not shareable), R3
(single provider) and R7 (not drivable). The conversation's conclusion — *Fleet
owns the Project; a native project is at most one possible connection* — is
confirmed, and that connection is **reference only** (§7).

### F2 — What *is* natively controllable

From `cli-reference` and `claude-code-on-the-web`, read 2026-10-01:

| Native capability | Surface | Fleet's use |
|---|---|---|
| Several repositories in one session | `--add-dir`, `permissions.additionalDirectories` | the per-session half of R1 |
| Standing instructions | `CLAUDE.md` / `AGENTS.md` per repo; `--append-system-prompt-file`; `--settings` | §6 |
| Create a cloud session | `claude --cloud "task"` — **one repository at a time**, clones the cwd's GitHub remote at the current branch. **Needs a TTY** (run, 2026-10-01: refused with `--print` and with piped stdout); prints the session id and exits 0 (§16, last item) | FP6 (§15), run in a Fleet pane |
| Create a cloud session on your own infrastructure | `claude --cloud "task" --environment <ccpool_…>` — "a new cloud session that runs on the given self-hosted environment" (CLI help, 2.1.285; not run) | not used; noted in §16 Q2 |
| Steer a cloud session | `claude -p "msg" --cloud <session-id>`, with `--output-format json` → `{ok, session_id, url}` — a delivery ack, **not** the reply (run, 2026-10-01) | FP6 |
| Land a cloud session locally | `claude --teleport <session-id>` — needs a clean tree, the same repo, the branch pushed, the same account | FP6: how a cloud session becomes Fleet-owned |
| A provider thread in a Fleet folder | `claude remote-control` (server mode) | §16 Q2 |
| Worktree isolation | `--worktree`, `--tmux` | already used |

What has **no** surface, and is Fleet's to substitute: cross-session project
memory, project membership, cross-repo coordination, cross-account and
cross-harness continuity, and — for cloud sessions — **listing, status,
transcript, diff and PR creation** (claude.ai/code only; `--teleport` is the one
documented way back to a machine).

### F3 — The existing boundaries hold, and a Project is genuinely new

- **Org** is the boundary: `orgs`, `org_rules`, `hosts.org_id`,
  `client_tokens.org_id`, and `service::orgs::OrgScope`, built only by
  `Caller::org_scope`, which filters every work read.
- **Work** owns tasks: `work_items` (`parent_id`, `origin`, `notes`,
  `project_id`, migration 086), `work_links`, journal, handover, resume.
- **Today's `projects`** is `(owner, repo, base_path)` + `worktrees` — a
  registered repository or adopted folder, not a coordination unit.
- The Work view's **group is only a label**: `work_placements.group_label TEXT`,
  documented in migration 066 as "Navigation only: never a boundary". A Project
  is therefore not today's group; §8 wires the two instead of duplicating.

### F4 — Fleet has no person entity, and the answer is not this spec's to give

Access today is held by **devices and hosts**: `client_tokens` (`mode`,
`trusted_at`, `org_id`, `assets_admin_at`), `host_tokens`, and the master.
`accounts` is a *provider* account, not a Fleet user. So "project members"
cannot be expressed today.

The owner's answer (2026-10-01): **a permissions and synchronisation system is
already in development in parallel.** This spec therefore does not model
subjects, roles or grants. §9 is a seam: what Projects needs from that system,
and what it does in the meantime using only the access classes that exist.

### F5 — The sync engine exists, is host-scoped, and already has a scope boundary

The asset catalog is the one content engine, and it is the "synchronisation"
half of the parallel work:

- `catalogs` (migration 090) — one row per catalog, `org_id IS NULL` for the
  personal one; assets carry `scope: private | shared`.
- **Assets M2** (open PR
  [martin-janci/claude-fleet#416](https://github.com/martin-janci/claude-fleet/pull/416),
  plan `2026-09-30-assets-m2-sync.md`) takes **migration 091**: `catalog_id` on
  `host_layers` and `asset_inventory`, a per-catalog layer API
  (`set_host_layers_for(host, catalog_id, role, contexts)`), a pure
  `acceptance()` rule, and `effective_for_host()` composing one effective
  catalog per host. A scope-boundary refusal or a cross-catalog collision
  becomes a **per-asset `blocked` action with a reason**, and the rest of the
  host plans normally.
- **Assets M3** adds `host_catalogs` (admissions) and `client_catalog_grants`.

Consequences for a Project:

- **Migration 091 is taken.** This spec uses **092** (§5).
- The sync **unit is a host**, not a session or a worktree. A project layer
  reaches every session on the hosts it is attached to; per-session scoping is
  the hook path (§6).
- `Kind` is `Skill | Agent | Hook | McpServer | PluginRef`. There is **no
  `Instructions` kind** — multi-harness F3 item 1, a hard dependency for
  syncing a Project's instructions as an asset (§12).
- The per-asset `blocked` reason is already the right channel for R7's visible
  gaps; §7 reuses it rather than inventing a second one.

### F6 — Hub↔hub federation carries messages, not state

`2026-09-24-hub-federation-design.md` non-goals: only `session` addresses cross
a link, and a hub never re-forwards. No replication of orgs, work or catalogs.

## 3. Decisions

*agreed* = the owner decided it. *recommended* = this spec recommends it and §16
says why, what it costs if wrong, and what to verify. *open* = forced by §2, so
it needs a confirming "yes" rather than a debate.

| # | Question | Decision | State |
|---|---|---|---|
| P1 | Does Fleet own the Project, with a provider project only ever a connection? | Yes. F1 leaves no alternative. | open |
| P2 | Is a native Claude Project connection built in v1? | Yes, as `reference` only: a stored link plus an exportable instructions block a person pastes. No automation claimed. | open |
| P3 | Does a Project become a boundary? | No. Org stays the only boundary. | open |
| P4 | How are members modelled? | **Not here.** Deferred to the parallel permissions system; §9 is the seam. | **agreed** (owner, 2026-10-01) |
| P5 | Where do a Project's tasks live? | In `work_items`, unchanged: one column, one read. | open |
| P6 | Is a Project org-bound? | Optional: `org_id` nullable, like `catalogs`. Never crosses its org without `force_cross_org`. | open |
| P7 | Does a Project's context ride the catalog? | Both: assets as a context-axis layer in a catalog; the per-session header on the existing hook budget. | open |
| P8 | Does the UI rename today's `projects` to "Repository"? | Yes, strings only. No schema or API rename. | open |
| P9 | Dependencies between tasks in v1? | **No.** Status-and-repository grouping is enough to start. | **agreed** (owner, 2026-10-01) |
| P10 | Cloud sessions as an execution target? | **Yes**, as its own phase FP6 (§15), with the capability rows F2 verifies. | **agreed** (owner, 2026-10-01) |
| P11 | Which provider account runs a Project's cloud work? | **The host's.** No account selector; a Project may *require* an account and Fleet refuses a host that does not carry it (§16 Q1). | **agreed** (owner, 2026-10-01) |
| P12 | Remote Control as a bridge to native Claude Projects? | **Yes**, as phase FP7. It is the only documented way a native Project thread runs on infrastructure Fleet owns, and the one where Fleet's hooks and assets still apply (§16 Q2). | recommended |
| P13 | Cross-hub Projects? | **No.** Out of scope with a stated reason; reopened only when two hubs share people (§16 Q3). | **agreed** (owner, 2026-10-01) |
| P14 | Rename `projects` → `repositories` in the schema and API? | **Never.** ~3 500 code sites, 100 MCP references and a wire break against fleet-mobile, for no user-visible gain; the 141 UI strings are the whole benefit (§16 Q4). | recommended |
| P15 | Review the §9 seam before FP0? | **Yes, review; no, don't block.** FP0 lands the `Actor` word now and a subject id additively later (§16 Q5). | recommended |

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
execution. Only user-visible strings change (P8), so every id, tool name,
column and `project_id` keeps its name.

## 5. Model

```
FleetProject (fleet_projects)
├── org_id?       — NULL = personal, else the one org it lives in (P6)
├── goal, status  — one line; active | paused | archived
├── repos         — fleet_project_repos → projects(id), role primary|secondary
├── context       — fleet_project_context rows (goal | instructions | reference | decision | memory)
├── connections   — fleet_project_connections (provider, kind, external_ref, mode)
└── tasks         — work_items.fleet_project_id (grouping only; Work owns the task)
```

Membership is **absent on purpose** (P4, §9).

### Migration `092_fleet_projects.sql` (additive)

091 is taken by assets M2 (F5). This migration lands after it.

```sql
CREATE TABLE IF NOT EXISTS fleet_projects (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT    NOT NULL,
  org_id      INTEGER REFERENCES orgs(id) ON DELETE RESTRICT,
  goal        TEXT,
  status      TEXT    NOT NULL DEFAULT 'active',   -- active | paused | archived
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  updated_by  TEXT,                                -- service::settings::Actor's word
  cloud_account_uuid TEXT REFERENCES accounts(uuid) ON DELETE SET NULL,
                                                   -- P11: cloud work runs only on a host carrying this account
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

CREATE TABLE IF NOT EXISTS fleet_project_context (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  fleet_project_id INTEGER NOT NULL REFERENCES fleet_projects(id) ON DELETE CASCADE,
  kind             TEXT    NOT NULL,   -- goal | instructions | reference | decision | memory
  title            TEXT,
  body             TEXT    NOT NULL,
  pinned           INTEGER NOT NULL DEFAULT 0,  -- pinned rows feed the session header (§6)
  author           TEXT,                        -- Actor's word, as work_placements.updated_by
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fpc_project ON fleet_project_context(fleet_project_id, kind);

CREATE TABLE IF NOT EXISTS fleet_project_connections (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  fleet_project_id INTEGER NOT NULL REFERENCES fleet_projects(id) ON DELETE CASCADE,
  provider         TEXT    NOT NULL,   -- anthropic | openai | google | …
  kind             TEXT    NOT NULL,   -- native_project | account | cloud_session | catalog_layer
  external_ref     TEXT,               -- a URL or an opaque id; NEVER a credential
  mode             TEXT    NOT NULL,   -- reference | assisted | controlled
  checked_at       INTEGER,            -- when the capability behind it was last verified (§7)
  created_at       INTEGER NOT NULL
);

ALTER TABLE work_items ADD COLUMN fleet_project_id INTEGER
  REFERENCES fleet_projects(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_fleet_project
  ON work_items(fleet_project_id) WHERE fleet_project_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (92);
```

`ALTER TABLE` is not idempotent, so the migration's `MIGRATIONS` entry in
`store/schema.rs` carries an `already_applied` column guard for
`work_items.fleet_project_id`, as 086, 087 and 089 do. The new tables are
`CREATE TABLE IF NOT EXISTS` and need none.

**A credential is never stored here.** `external_ref` holds a reference only; a
provider credential keeps its existing home and its single reader, as
`Store::resolve_tracker_credential` and `Store::resolve_decision_credential` do.

**Attribution without a subject table.** `updated_by` / `author` hold the word
`service::settings::Actor` already produces (`Person`, `PersonVia(client)`,
`Agent(name)`, `System`) — the convention `work_placements.updated_by` and
`settings::set_by` use. When the parallel permissions system defines a real
subject, these columns are what it attaches to.

### Invariants

| # | Invariant | Enforced by |
|---|---|---|
| I1 | A Project is never a boundary | no `OrgScope` equivalent; every read still goes through `Caller::org_scope` first |
| I2 | A task in an org-bound Project is in that org | a check on `work_items.fleet_project_id`, `E_FORBIDDEN` without `force_cross_org` |
| I3 | A repo in an org-bound Project resolves to that org | the existing `org_rules` / `hosts.org_id` resolution; a mismatch is a warning on the Project page, never a silent re-home |
| I4 | A Project never widens a caller | §9; a per-host token still sees only its host's and org's rows |
| I5 | One archived name may repeat | the partial unique index above |

## 6. Shared context and how it reaches a session

Two delivery paths, because the existing mechanisms have two scopes (F5).
Neither is a new sync engine (R8).

**Path A — per session, through the hook `additionalContext`.** A new *project
header* slice, composed before the work brief in today's ladder:

- A hook's `additionalContext` carries **8000 chars / 200 lines**, shared by the
  mail, the brief and the nudge (a `Stop` block's `reason` has its own far
  smaller 2000 / 20 budget and is not a path for this).
- `SessionStart` is capped at `SESSION_START_CONTEXT_MAX = 4000`, and the M2
  handover brief at `BRIEF_MAX_CHARS = 4000`.
- The project header gets `PROJECT_HEADER_MAX = 800` inside that, and is
  **dropped whole** when it does not fit — it never truncates a brief, and
  nothing inside an `UNTRUSTED_END` fence is ever cut, as M4.5 requires.
- Content: the Project name and goal (Fleet's own lines, markers defused), its
  repository list, and its pinned `instructions` rows inside one
  `mark_untrusted` fence. A person's and an agent's text is third-party text.
- Gated by `projects.session_header` (default **off**), following D5's
  precedent: the owner measures the cost with
  `scripts/measure-session-start.sh` before it goes on.

**Path B — project assets, through the catalog.** §12.

**Honest limits, both stated in the UI (R7):** Path B is host-scoped, and Path
B's instructions need the catalog `Instructions` kind, which does not exist yet.
Until it lands, a Project's instructions are Path A only, and the Project page
says so.

## 7. Provider capabilities: native, substituted, or a visible gap

R6 asks for a judgement **per capability**, not per product. The registry
extends the `Capability` enum multi-harness F5 introduces for `SessionRuntime`,
rather than starting a second one. `project_capabilities` is **computed, never
stored**, from (harness, provider, connection mode), and served on the Project
page and by the MCP read.

| Capability | Claude Code (host session) | Claude cloud session | Claude via Remote Control on a Fleet host | Codex | Verdict |
|---|---|---|---|---|---|
| Several repositories in one session | native (`--add-dir`) | **no** — `--cloud` is one repository at a time | native (it is local Claude Code) | per-harness flag | native where present; else one session per repo |
| Standing instructions | native | native from the repo's `CLAUDE.md` | native, **plus Fleet's catalog assets and hooks** | `AGENTS.md`, 32 KiB cap | **native**, rendered by the catalog |
| Start work | Fleet owns the pane | `claude --cloud "task"` in a Fleet pane (it needs a TTY); the id is read from the pane | **the person starts it from claude.ai**; Fleet only provisions the server | Fleet owns the pane | mixed — see the modes below |
| Steer / follow up | Fleet owns the pane | `claude -p --cloud <id> --output-format json` — delivery only, the reply is not returned | from claude.ai or the Claude app, not from Fleet | Fleet owns the pane | **controlled** on a pane; `reference` on a Remote Control thread |
| Status, transcript, usage | native (hooks, pane-intel, `~/.claude/projects`) | **none from the CLI** — the steer ack carries no reply either | the transcript is on the host; status likely through `claude agents --json` (**to verify**, §16 Q2) | partial | cloud: **gap**; Remote Control: probably observable |
| Bring the session to a host | n/a | `claude --teleport <id>` | it already runs on the host | n/a | **assisted** (cloud); inherent (Remote Control) |
| Diff, PR creation | Fleet + the PR probe | claude.ai/code only | the branch is on the host, so Fleet's PR probe sees it | Fleet | cloud: **gap**; Remote Control: native |
| Cross-session project memory | none | none | Fleet's journal applies (its hooks run) | none | **substituted** (journal, handover, §6) |
| Project membership / sharing | none (one user, F1) | Pro/Max: Private or Public only; Team visibility needs Team/Enterprise | none — the native Project is still one user's | none | **substituted** (§9), and a real constraint on R2 |
| Parallel threads under one goal | native inside a provider Project Fleet cannot drive | n/a | **native, on Fleet's own machine** (`--spawn worktree`, `--capacity N`) | none | native at the infrastructure layer; **substituted** for coordination |
| Cross-repo task coordination | none | none | none | none | **substituted** (§8) |
| Native provider Project, driven by Fleet | **unavailable** (F1) | n/a | **unavailable** — Fleet hosts the execution, never the coordination | n/a | **gap** — shown, never faked |
| Rewind, fork, move | native | **no** | the transcript is local, so possibly yes (**to verify**) | no | capability-gated, as F5 defines |

Three connection modes, and `controlled` is claimed only where F2 names the
interface:

- `reference` — Fleet stores the link and can **export** an instructions block
  for a person to paste. The only mode a native Claude Project gets (P2).
- `assisted` — Fleet prepares an action a person confirms (`--teleport`).
- `controlled` — Fleet drives it through a supported interface (`--cloud`
  create in a pane and `-p --cloud` follow-up, both run 2026-10-01). It means
  Fleet can *act*; it does not mean Fleet can *see* — a cloud session is
  `controlled` and still has the status gap.

A gap is a first-class row in the UI with its reason and its `checked_at` date,
so "there is no interface, checked 2026-10-01" is visible rather than
remembered. Where the gap is a *sync* refusal, the reason is the per-asset
`blocked` string assets M2 already produces (F5) — not a second channel.

## 8. Coordination without a second registry

A Project **groups** work; it never stores a task.

- `work_items.fleet_project_id` places an existing task (tracker item or native
  item) in a Project. A tracker item stays read-only (shared-work-context C3).
- Subtasks, proposals, steps, jobs, journal, handover, summarise, tidy: all
  unchanged. A Project adds no level to the one-deep subtask rule.
- **Dependencies are out of v1** (P9). The Project page shows tasks by status
  and by repository. A `blocked_by` relation waits for evidence that the list is
  not enough — it is the likeliest source of process without value.
- The **Work view** gains a *Project* grouping next to today's org → group
  tree; `work_placements.group_label` stays a label (F3), and a task's Project
  is read from its column.
- **Multi-repo start** already takes `work_link start { project_ids }` (M9.6).
  A Project supplies the default `project_ids` — its `primary` repo first — so
  starting from a Project is the existing path with a filled-in argument.
- **Results** are collected where they already are: the journal, the PR probe,
  `summarize`, the handover brief. The Project page reads them per task.

## 9. Access: the seam to the parallel permissions system

A permissions and synchronisation system is already in development (owner,
2026-10-01). This spec adds **no** subject, role or grant table, and no
boundary.

**What v1 does, using only what exists:**

| Question | v1 |
|---|---|
| Is a Project an access boundary? | No. Org is (I1). |
| Who may create or edit a Project? | The master, or a trusted `full` device — the rule `settings_writer` already uses. |
| What may an org-bound client do? | Read and write Projects of **its** org, through the `OrgScope::Org` path M14.1 built. |
| What may a per-host token do? | Read the Projects its host's sessions work in; place its own tasks; never create, never connect. |
| What may a `readonly` client do? | Read only. |
| Cross-org placement? | `E_FORBIDDEN` unless `force_cross_org`, as a cross-org work link. |
| Who did what? | The `Actor` word in `updated_by` / `author` (§5). |

**What Projects will need from that system**, stated now so the seam is
designed rather than discovered:

1. A **stable subject id** for a person, resolvable from a caller, to replace
   the `Actor` word in `updated_by` / `author` without losing history.
2. A place to express **"this person is on this Project"** — whether as a role
   on the subject, a group, or a grant. Projects reads it; it must not become a
   second copy.
3. A rule for how a Project-level role **composes with the org boundary**. The
   invariant Projects needs is intersection: a Project never grants what the org
   denies (I4).
4. Whether the sync side's `client_catalog_grants` (assets M3) is the same
   mechanism or a different one, since a Project's assets ride a catalog (§12).

Until those exist, a Project's member list is simply **not modelled**, and the
UI shows participation as what it can prove: the accounts, hosts and sessions
that have worked the Project's tasks.

## 10. API surface

**MCP.** One tool, `project`, with actions:

| Action | Access | Notes |
|---|---|---|
| `list`, `get` | `Access::Client` | org-filtered; a per-host token sees its host's Projects |
| `create`, `update`, `archive` | master or trusted `full` device | compare-and-set on `version`, `E_CONFLICT` |
| `add_repo`, `remove_repo` | master or trusted device | validates the repo's org (I3) |
| `context_add`, `context_update`, `context_remove`, `context_list` | writes: trusted device; `context_add` with `kind: memory` also from a session's own token | an agent may add a memory note, never instructions |
| `connect`, `disconnect`, `capabilities` | writes: master or trusted device; `capabilities` is `Access::Client` | `capabilities` is §7's computed matrix |
| `place_task`, `unplace_task` | the task's existing write access | wraps `work_items.fleet_project_id` |

Every action needs a row in the isolation matrix
(`mcp/tools/tests_isolation.rs`), which fails without one.

**Desktop commands** (`src-tauri/src/commands/`), thin over
`service/projects/`. Each needs a verdict row in `backend/verdicts.rs` **by
command name**, then `REGEN_HUB_VERDICTS=1`, then — for a `LocalOnly` command
the UI can reach — a `REASONS` entry or an allowlist line in
`src/lib/hub_verdicts.test.ts`. Expected: all `Routed` (a Project is hub state,
like work), except anything that writes a local file (a connection export) and
FP6's cloud actions, which run a CLI on a host and are `Routed` through the
host's own path.

**Events.** One row-event kind, `project`, ids only, added to
`HOST_BOUND_HIDDEN_KINDS` (`mcp/events_route.rs`) beside `work`, `settings` and
`update`: a Project is not a host's, so it never rides a host- or org-bound
stream. `projects:changed` joins `work:changed` in the frontend's re-read tick.

**Wire.** Additive: new tool, new actions, new row fields. No
`CONTRACT_REVISION` bump, following `wire_contract.rs`; an older hub answers
`E_INVALID "unknown tool"`, which the desktop shows as a capability gap.

**Settings.** `projects.session_header` (bool, off), `projects.header_max_chars`
(800), and FP6's `projects.cloud_execution` (bool, off). Each needs a `SPECS`
row with full metadata, a home on a page (`every_setting_has_one_home`), and
`REGEN_SETTINGS_DOCS=1`.

## 11. UI

- **Projects** becomes a top-level screen beside Sessions and Work: name, goal,
  org, repository count, open-task count, live-session count, capability-gap
  badge.
- **Project page**: Goal · Context (pinned first) · Repositories · Tasks (by
  status, by repository) · Sessions · Connections & capabilities. No Members
  section in v1 (§9) — participation is shown as the accounts, hosts and
  sessions that worked its tasks.
- **Capabilities panel** is where R7 lives: one row per capability with
  *native* / *Fleet* / *not available*, the reason, and `checked_at`. A native
  Claude Project reads "reference only — no supported interface (checked
  2026-10-01)". A cloud session reads "started and steered by Fleet; status and
  diff only on claude.ai/code".
- A session's header gets a Project chip next to the work chip.
- Today's "Project" labels become "Repository" (P8), including the New-session
  dialog, Host detail and `docs/`.
- Every third-party string renders as **text, never markup**, as the Work view
  already does.

## 12. Sync (R8)

A Project adds **no** sync mechanism. It uses the engine assets S1b/M1/M2 build:

- Project **assets** → one `Axis::Context` layer in a catalog (the org's, or
  the personal one), attached per host with M2's
  `set_host_layers_for(host, catalog_id, role, contexts)`. `effective_for_host`
  composes it with every other accepted catalog; `acceptance()` decides what
  the host may take.
- The **scope boundary is already there**: an asset is `private` or `shared`
  (migration 090), a private asset is never planned onto an org-bound host, and
  a layer may not override `scope`. A Project asset meant for an org host must
  be `shared`.
- A **refusal or collision is per asset, with a reason** — M2's `blocked`
  action. That is §7's visible-gap channel; the Project page renders those
  reasons rather than inventing its own.
- Project **instructions as an asset** → the catalog `Instructions` kind
  (multi-harness F3 item 1). **Dependency, not yet built.**
- Project **rows** → the hub, read by desktops and phones over the existing
  tools and `/events`, like work.
- Project **context into a session** → the existing hook `additionalContext`
  (§6 Path A).
- **Admissions** (`host_catalogs`, `client_catalog_grants`, assets M3) stay the
  sync side's. A Project never admits a catalog to a host.

What a Project must never do: write its own files to a host outside the
catalog, or keep a second copy of a repository's `CLAUDE.md`.

## 13. Risks and counter-measures

| Risk | Counter-measure |
|---|---|
| Another system to maintain by hand | A Project owns only what nothing else owns: goal, context, repo set, connections. Tasks stay in Work, repos in `projects`, content in the catalog, boundaries in Org, permissions in the parallel system. |
| A Project becomes a second permission system | §9 adds no table and no boundary; I1 and I4; the isolation matrix fails a new action without a row. |
| Two designs for permissions diverge | §9's four seam requirements are written down before either side builds, and v1 ships with membership simply absent rather than guessed. |
| AI creates more process than value | No dependency graph (P9). An agent may add a `memory` note, never instructions. The session header is off by default and capped at 800 chars. |
| A convincing capability claim without a working interface | §7's three modes, `checked_at` on every connection, and `controlled` only where F2 names the interface. |
| Cloud execution silently loses work | FP6's constraints are stated in the UI: one repo per cloud session, no Fleet-side status, the VM reclaimed on inactivity, rate limits shared with the account. |
| Context bloats every session | One cap, dropped whole rather than truncated; measured before the setting goes on. |
| The vocabulary change confuses existing users | Strings only (P8); every id, tool name and column keeps its name. |

## 14. Testing

- **Store:** migration 092 on a v91 DB, its re-run, and the column guard; the
  archived-name index; cascade on archive-then-delete.
- **Boundary:** I1–I5 each as a test. A per-host token's `project list`; an
  org-bound client confined to its org; a cross-org `place_task` refused and
  allowed with `force_cross_org`; an org-mismatched repo surfacing as a warning
  and not re-homing.
- **Context:** the header at, just over and far over `PROJECT_HEADER_MAX`; the
  header dropped rather than a brief truncated; a fence never cut; a forged
  `UNTRUSTED_END` in a goal defused; the setting off meaning no header.
- **Capabilities:** the computed matrix per (harness, provider, mode); a native
  Claude Project reporting `reference` and never `controlled`; a cloud session
  reporting the status gap.
- **Sync:** a Project layer through `effective_for_host`; a `private` Project
  asset blocked for an org host with M2's reason, rendered on the Project page;
  a collision between a Project layer and another catalog blocked both ways.
- **Coordination:** a Project's `project_ids` default on start; the Work view
  grouped by Project; a tracker item placed in a Project staying read-only.
- **Cloud (FP6):** the argv for `--cloud` and `-p --cloud <id>` against a stub
  `claude`; the session id parsed from a captured pane (the three lines §16
  quotes, plus a pane that never prints them and one stuck on the trust
  prompt); the json parse of `{ok, session_id, url}`; a teleport refused on a
  dirty tree; no capability claimed that F2 does not name.
- **Isolation matrix:** every new action.
- **Frontend (vitest):** the Projects list and page; the capability panel's
  three states; text-not-markup for every third-party string; the "Repository"
  strings; `hub_verdicts.test.ts` per new command.
- **Docs generators:** `REGEN_HUB_VERDICTS=1`, `REGEN_SETTINGS_DOCS=1`,
  `REGEN_PAGE_DOCS=1`, `REGEN_DOCS=1`.
- **CI:** `scripts/ci-local.sh`, plus a `scripts/hub-e2e.sh` section that
  creates a Project, places a task, and starts it multi-repo on a real hub.

## 15. Roadmap

| Phase | Deliverable | Exit criterion |
|---|---|---|
| **FP0 Model** | migration 092, `service/projects/`, the `project` tool, desktop commands + verdicts, events, isolation rows | a Project with repos exists on a hub and reads back correctly under every access class |
| **FP1 Context** | context rows, Path A header behind `projects.session_header`, the measurement run | a session on a Project's repo starts with the header when on, byte-identically without it when off |
| **FP2 Capabilities** | the computed matrix, the `reference` connection, the UI panel, the "Repository" strings | the Project page states every capability as native / Fleet / not available, with its date |
| **FP3 Coordination** | `place_task`, Project grouping in the Work view, `project_ids` default on start | starting from a Project opens the right repos, and its tasks group under it |
| **FP4 Project assets** | the context-axis layer per Project through M2's per-catalog API; the catalog `Instructions` kind | one sync delivers a Project's skills and instructions to its hosts for Claude and Codex, and a scope refusal shows its reason |
| **FP5 Non-Claude harnesses** | per-harness capability rows as multi-harness F5/F6 land | a Codex session in a Project shows the same Project with its gaps named |
| **FP6 Cloud execution** | `projects.cloud_execution` (off); start a Project task as a cloud session on a Project host (`claude --cloud`), steer it (`-p --cloud <id>`), record it as a `cloud_session` connection, and land it with `claude --teleport` | a task starts in the cloud from a Project, is steered from Fleet, and teleports into a Fleet session on a host — with the status and diff gaps shown, not faked |
| **FP7 Remote Control host** | `projects.remote_control` (off); provision and supervise `claude remote-control --spawn worktree --capacity N` on a Project host, link the sessions it serves to the Project, and show the native Project it serves as a `reference` connection | a thread the owner starts in a native Claude Project runs in a Fleet worktree on a Fleet host, with Fleet's catalog assets, hooks and project header applied, and appears on the Project page |

FP0 → FP1 → FP2 is the critical path to R7. FP4 depends on assets M2 (PR #416)
and on multi-harness F3 item 1; FP5 on multi-harness F5. FP6 and FP7 are
independent of FP4/FP5 and can run after FP3, in either order — **FP7 delivers
more of R6 than FP6** (§16 Q2), so if only one is built, build FP7.

**FP6's creation is `controlled`, through a pane.** `claude --cloud` refuses
`--print` and a non-TTY stdout, but run in a tmux pane it prints the session id
and exits; Fleet owns the pane, so it starts the cloud session and reads the id
off the screen without a person in the loop (verified 2026-10-01, §16's last
item). What FP6 still cannot do is read the session's replies: that gap is
unchanged.

## 16. Open questions — findings and recommendations

Each item states what was checked, what the evidence says, the recommendation,
and what it costs if the recommendation is wrong. The owner closed P9, P4 and P10
on 2026-10-01, then Q1 (P11) and Q3 (P13) on the same day. Q2 and Q5 are out for
independent verification; Q4 is settled below.

### Q1 — Which provider account runs a Project's cloud work? → **the host's** (P11)

**What was checked.** How a Fleet session acquires an account, and what a cloud
session inherits from the account that starts it.

**Evidence.**

- Fleet **observes** an account, it never chooses one. `hosts.account_uuid` is
  captured by the reconcile probe from the host's own Claude login
  (`sessions/reconcile.rs`), and a session keeps the value it was created with.
  There is no account selector anywhere: the only write is
  `set_account_nickname`. One host carries one Claude login.
- A cloud session belongs to the account that started it, shares that account's
  rate limits, and on Pro/Max is *Private* or *Public* — **Team visibility needs
  Team or Enterprise** (`claude-code-on-the-web`, read 2026-10-01). It also
  needs claude.ai subscription auth and the org's `allow_remote_sessions`
  policy; a host authenticated with an API key cannot start one at all.

**Recommendation.** Do **not** add a per-session or per-Project account
selector. The host is already Fleet's account boundary; a selector would be a
second source of truth and would force Fleet to switch `~/.claude.json` logins —
credential juggling Fleet deliberately avoids (it never stores token values, and
`agent-creds` is the owner's separate tool).

Instead, a Project may state a **requirement**: `fleet_projects.cloud_account_uuid`
(§5). When cloud work starts, Fleet picks among the Project's hosts one whose
`hosts.account_uuid` matches, and otherwise refuses with the reason — "no host
in this Project carries account <nickname>". With the column NULL, any Project
host may run it.

The visibility limit is a **product fact to display, not a problem to solve**:
the capability panel says "a cloud session is visible only to the account that
started it (Pro/Max)". That is a real constraint on R2, and hiding it would be
exactly the kind of claim §7 exists to prevent.

**If this is wrong.** The column is nullable and additive; adding a selector
later costs one more field and the credential work, which this decision only
defers.

### Q2 — Remote Control as a bridge? → **yes, as phase FP7, and it beats FP6** (P12)

**What was checked.** The whole of `remote-control`, read 2026-10-01, against
Fleet's architecture.

**Evidence, and why it is stronger than expected.**

- `claude remote-control` is a **server-mode process**: no interactive session,
  multiple concurrent sessions from one process, `--capacity N` (default 32).
- **`--spawn worktree` gives each on-demand session its own git worktree.** That
  is Fleet's own model for parallel work.
- The docs say, in the limitations: "To keep a session running on a remote
  machine after you disconnect from SSH, start it inside `tmux` or `screen`."
  **Fleet's architecture is precisely tmux over SSH.**
- The flag table says a session the server starts "for one of your project
  threads" follows the server's Chrome setting — i.e. **server mode is how a
  native Claude Project thread executes on a machine you control.**
- Because such a thread *is* local Claude Code on that host, it gets Fleet's
  whole context layer: the catalog's assets, Fleet's hooks, the repository's
  `CLAUDE.md`, and the §6 project header. A **cloud** thread gets none of that.
- Fleet already has somewhere to put these sessions: `kind='external'` rows,
  parsed from `claude agents --json` (`claude_agents.rs`), for "an interactive
  Claude session running outside fleet".

**Recommendation.** Build it, as phase FP7, and treat it as the **more valuable**
of the two execution bridges. It converts F1's flat "gap" into a real partial
win: Fleet still cannot drive the coordination of a native Project, but it owns
the machine, the folder, the worktree, the assets and the hooks its threads run
in — which is R6's actual request ("use the native capability where it is
available and controllable; supply the rest") applied at the infrastructure
layer, rather than giving up because the coordinator is closed.

**Constraints that must be designed for, not discovered.**

| Constraint | What FP7 must do |
|---|---|
| A one-time interactive `Enable Remote Control? (y/n)` on first run | a provisioning step, not a session start; a host is "remote-control ready" or it is not |
| A global `claude` flag before `remote-control` is **refused** when dropping it would change what the sessions can do (e.g. `--settings`) | the `ag` launcher must not wrap this command the way it wraps a session; FP7 names the exact argv |
| Server mode exits after roughly 10 minutes of network outage | Fleet supervises and restarts it, the way it supervises a pane; the Project page shows the server as up or down |
| Sessions belong to the host's account | the same rule as Q1, same refusal |
| Fleet does not start or steer these sessions | they are `reference` in §7; the Project page never offers a Send button for one |
| Needs a claude.ai subscription on that host | an API-key host is ineligible, and the capability row says so |

**Out for independent verification** (owner, 2026-10-01): the review request is
`docs/superpowers/reviews/2026-10-01-fleet-projects-q2-remote-control-review-request.md`,
which asks a second agent to try to break each of the five claims above rather
than confirm them.

**The one thing to verify first**, and it decides FP7's size: whether
`claude agents --json` lists the sessions a `remote-control` server serves. If it
does, Fleet's existing discovery gives status and labels for free and FP7 is
small. If it does not, FP7 needs its own observation path (the transcript under
`~/.claude/projects` is still there), and is perhaps twice the work. Check it on
one host before planning.

What is known so far (run on the owner's Mac, CLI 2.1.285, 2026-10-01, with no
remote-control server running): `claude agents --json` is documented as "active
sessions (interactive and background)", and its rows carry exactly `id, kind,
name, pid, sessionId, cwd, startedAt, state, status`, with `kind` only ever
`interactive` or `background`. There is **no field that marks a Remote Control
session**, so even if the server's sessions are listed, Fleet could tell them
from an ordinary interactive session only by `pid` ancestry or `cwd` (the
server's worktree root). Whether they are listed at all still needs one
`claude remote-control` run.

**A third option §7 does not have a column for.** `--cloud --environment
<ccpool_…>` creates a cloud session that "runs on the given self-hosted
environment". If that environment can be a Fleet host, it is FP6's interface
(`controlled` create and steer) on FP7's machine (Fleet's assets and hooks).
Unread beyond the CLI help; worth one look before FP6 or FP7 is planned.

**If this is wrong.** The cost is bounded: a provisioning step and a supervised
process. Nothing in FP0–FP5 depends on it.

### Q3 — Cross-hub Projects? → **out of scope** (P13)

**What was checked.** What hub↔hub federation actually replicates.

**Evidence.** `2026-09-24-hub-federation-design.md`'s non-goals: only `session`
addresses cross a link, a hub never re-forwards (no A→B→C), there is no peer
discovery, and revocation is the only remedy for a misbehaving peer. No org,
work, catalog or settings state replicates. A shared Project across two hubs
would therefore need a replication layer with conflict resolution for every
table in §5 — and the permissions and synchronisation system now in development
would have to span hubs before a Project could.

**Recommendation.** Closed as out of scope, with the reason written down rather
than left as a silence: a Project coordinates work inside one fleet, and two
hubs are two fleets. Reopen it only when there is a concrete case — two hubs
that share the same people — and then as its own cycle, after the permissions
system has an answer for cross-hub identity.

**If this is wrong.** Nothing in §5 blocks it: every table is hub-local and
additive, so a later replication cycle starts from a clean model rather than
from a half-built one.

### Q4 — Rename `projects` → `repositories` in the schema? → **never** (P14)

The count of call sites was the weakest part of the argument, because a large
mechanical rename is a solved problem — a compiler finds every site. What
actually decides this is the three places where a rename is **not** mechanical.

#### 4a. The wire renames silently rather than failing

`crates/fleet-core/src/wire_contract.rs` states the hazard in its own words:

> A hub-client desktop deserialises hub tool results straight into the same
> `fleet-core` row structs the hub serialised them from … that symmetry is a
> hole: **a renamed optional field does not fail to parse, it silently
> defaults**.

`project_id` is exactly such a field. Rename it to `repository_id` and an older
desktop or phone does not error — it parses the row with `project_id: None` and
**shows every session as belonging to no repository**. A wrong-but-plausible
screen is the worst failure shape this codebase has, and it is the one a rename
produces by default.

The mechanism that exists to stop that is `CONTRACT_REVISION`, and its rule
names this case directly: bump for "a change that **removes or renames a row
field**, or changes what an existing field means", on any type in
`src-tauri/src/backend/hub_contract.golden.json`. That golden file pins
`project_id` in **six** places.

#### 4b. A bump is a forced, coordinated upgrade of three codebases

`src-tauri/src/backend/contract.rs`:

```rust
pub const MIN_HUB_CONTRACT: u32 = 6;
pub const MAX_HUB_CONTRACT: u32 = 6;
```

The desktop accepts **exactly one** revision. Outside that range it "does not
trust the hub's rows at all rather than risk showing a stuck or lost session as
healthy". So bumping to 7 means:

- every desktop still on 6 stops trusting the hub the moment the hub upgrades;
- `compat_json()` (`fleet-hub/src/main.rs`) feeds the release manifest's compat
  windows, so the bump propagates into the update machinery that S2–S5 just
  built — the manifest, the channel documents and `fleet-hub compat`;
- fleet-mobile has to ship in lockstep. It is not incidental there: it calls
  `list_projects` as a **literal string**, holds its own `ProjectRow`, consumes
  the `project:updated` event, and its own source comments that a session row
  "names its project by `project_id` and nothing else — there is no project name
  anywhere on it … so this list is the only thing that can turn `project_id: 3`
  into a heading". ~148 occurrences across its shared Kotlin.

That is a release event with a user-visible blackout window, spent entirely on
vocabulary.

#### 4c. SQL text is not refactored by the compiler

`projects` is named inside **SQL string literals**, which no rename tool
follows:

- `store/orgs.rs` builds `SESSION_ORG_SQL` with `LEFT JOIN projects op`, and
  CLAUDE.md records that a test holds that expression **byte-equal** to the
  trigger body shipped in migration 050. A rename means a new migration that
  drops and re-creates that trigger, kept in sync with the Rust string, on every
  existing database.
- Six migrations name `projects`, and the schema carries 22 triggers and views
  in total.
- The rename itself must then replay cleanly over 91 prior migrations in
  `store::testgen`'s upgrade test, and an older hub opening the renamed database
  is the downgrade case.

#### 4d. The cheap middle paths do not exist

| Idea | Why it fails |
|---|---|
| `ALTER TABLE projects RENAME TO repositories` + a compatibility view named `projects` | SQLite views are read-only; the code writes to `projects`, so it would need `INSTEAD OF` triggers for every write — more machinery than the rename saves |
| Rename the Rust types only, keep `#[serde(rename = "project_id")]` | Zero wire risk and zero user benefit: ~1 500 sites of churn so that the code says one word and the wire says another, which is *worse* for the next reader |
| Rename in new code, leave the old | Two vocabularies in one codebase — the outcome this decision exists to avoid |
| Ride along with a `CONTRACT_REVISION` bump happening anyway | A ~3 500-site diff would swamp the review of whatever real change it rode with |

#### The decision, and when it would change

**Never rename the schema, the serialised field names, or the MCP tool names.**
`projects`, `project_id`, `add_project`, `list_projects`, `refresh_projects` and
`forget_project` keep their names permanently. Only the ~141 user-visible strings
move to "Repository" (P8), which costs nothing and delivers the entire benefit.

Write the convention into `CLAUDE.md` when FP0 lands, so it is not re-proposed:

> In this codebase `projects` is a **repository** (owner/repo or an adopted
> folder) and `fleet_projects` is a **Project** (the coordination unit). The UI
> calls them "Repository" and "Project". The schema and the wire keep their
> names — see `2026-10-01-fleet-projects-design.md` §16 Q4.

The one condition that would reopen it: if `projects` had to **change meaning**
rather than merely be renamed — for instance if a repository stopped being
`(owner, repo, base_path)`. Then the wire breaks for a real reason, the bump is
justified on its own, and the rename is a side effect rather than the purpose.

### Q5 — Review the §9 seam before FP0? → **review, but do not block** (P15)

**What was checked.** What FP0 would have to commit to, and whether it can be
made additive.

**Evidence.** The attribution columns (`fleet_projects.updated_by`,
`fleet_project_context.author`) hold the word `service::settings::Actor` already
produces, which is the repo's existing convention in two places:
`settings::set_by`'s audit and `work_placements.updated_by`. So P0 invents
nothing; it reuses. A real subject id can then be added **beside** the word as a
nullable column, with the word kept for history — the same shape migration 086
used for `origin` (backfill a derived value, read NULL as a default).

**Out for independent verification** (owner, 2026-10-01): the review request is
`docs/superpowers/reviews/2026-10-01-fleet-projects-q5-permissions-seam-review-request.md`,
which asks a second agent to judge §9's four requirements against the permissions
and synchronisation system actually being built, and to say whether the
"subject id is additive" claim holds.

**Recommendation.** Two separate things, and only one of them is a gate:

1. **Send §9's four requirements to the permissions design now**, as a review
   item, before FP0 writes its first column. That is cheap, and it is the only
   way the two designs agree on the *shape* of a subject rather than discovering
   a mismatch later. The fourth requirement matters most: whether the sync
   side's `client_catalog_grants` (assets M3) is the same mechanism as a
   Project's membership, since a Project's assets ride a catalog (§12).
2. **Do not block FP0 on the answer.** FP0 lands the `Actor` word, and the
   subject id is additive.

**If this is wrong** — if the permissions system defines a subject that cannot
be reconciled with the `Actor` word — the cost is one backfill migration over
two columns in tables that are new and small. That is a far smaller risk than
stalling FP0 behind another design's schedule.

### Verified: Fleet can capture a new cloud session's id

**The question.** Can Fleet start a cloud session and learn its id without a
person pasting it? The earlier draft could not run `claude --cloud` (a real
transaction against the owner's account; that container was also API-key
authenticated) and proposed `claude --cloud "…" --output-format json`.

**What was run**, on the owner's Mac, signed in with the claude.ai account,
Claude Code 2.1.285, 2026-10-01, in an empty non-git scratch directory:

| Invocation | Result |
|---|---|
| `claude -p --cloud "…" --output-format json` | refused, exit 1: "--cloud cannot be combined with --print. Starting a new cloud session with --cloud is interactive only" |
| `claude --cloud "…"` with stdout redirected | refused, exit 1: "--cloud requires an interactive terminal. Non-interactive invocations (piped stdout, …) run locally and would silently ignore --cloud" |
| `claude --cloud "…"` in a detached tmux pane | the folder trust prompt first (new folder); after it, the pane shows the three lines below and the process **exits 0** on its own |
| `claude -p "…" --cloud <id> --output-format json` | exit 0, `{"ok":true,"session_id":"session_…","url":"https://claude.ai/code/session_…?from=cli&m=0"}` — no reply text |

The pane output of a successful create:

```text
Created cloud session: <title>
View: https://claude.ai/code/session_<id>?from=cli&m=0
Resume with: claude --teleport session_<id>
```

**So the proposed command was wrong** (`--output-format` needs `--print`, which
`--cloud` refuses), and **the answer is still yes**: creation needs a TTY, and a
Fleet pane is one. FP6 runs `claude --cloud "task"` in a pane it owns, waits
for the process to exit, and parses `session_[A-Za-z0-9]+` from the
`Resume with:` line. Creation is `controlled` (§7, §15).

**What this does not settle.**

- The trust prompt appears in a folder the host has not trusted. A Project
  repo on a Fleet host normally is trusted; where it is not, the pane shows
  `stuck_kind = trust_prompt` and FP6 refuses rather than answering it.
- The test ran outside a git repository and still created a session, though F2
  says creation clones the cwd's GitHub remote. Its title came out as a generic
  "Session description"; whether the task text reached the session as its
  first prompt was not checked. FP6's plan must check it in a real repo.
- Reading a cloud session's replies is still a gap: the steer ack is delivery
  only, and `--teleport` (not run) is the one way back.

The test sessions were left on the account (claude.ai/code), not deleted.
