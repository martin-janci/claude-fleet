# Work view: design and contracts (work graph M14)

**Date:** 2026-09-27
**Roadmap:** `../2026-09-24-work-graph-roadmap.md` → *M14: the Work view*.
**Plan:** `../plans/2026-09-27-work-graph-m14-work-view.md` (binding for the order of work and the owner's answers).
**Builds on:** M0–M13 on `main` at `f10d0b92` (claude-fleet) and `8ca9afa` (fleet-mobile, with #50 and #51).
**User guide:** `docs/work-graph.md` → *The Work view*. **Acceptance:** `docs/work-graph-acceptance.md` → Part R.

## Goal

Two equal ways to navigate the same data:

1. **Sessions view** (exists): host / project → session → *all* of its tasks.
2. **Work view** (new): organisation → project or group → task → *all* of
   its sessions, including secondary, suggested and past links, and tasks
   that have no session at all.

The graph of identities and links is the source of truth; both trees are
projections of it. A session under several tasks is one identity (its
`session_id`, anchored on its participant), never a copy. Everything is
correctable by a person, every automatic value says where it came from, and
desktop and phone read one contract under one set of permissions.

## Stage 0: what exists, what is extended, what is new

Written against `main` `be0e2bc`; re-checked against `main` `f10d0b92` on 2026-09-27.
The migration is `0NN_work_view.sql`: its number is taken when it merges (plan M14,
design decision 3). On `f10d0b92` 061–064 are taken, so it is 065 or later.

| Capability | State before M14 | M14 |
|---|---|---|
| Work item identity (`work_items.id`; tracker `(tracker_id, external_id)`; local item; bare `ref_key`) | exists (046, 048) | **reused**; a task's wire id is `item:<id>` or `ref:<KEY>` |
| Session identity independent of tmux name / host (participant, 043/045) | exists | reused |
| Link session ↔ task, N:M, `state` confirmed / rejected / suggested, `source`, `strength`, `rule`, `evidence`, `created_at`, `decided_at`, `ended_at`, snapshot | exists (046, 049) | **extended**: `version` (`0NN`) |
| One primary link per session | exists, but every `link` / `confirm` *takes* the primary; no way to add a secondary link or to move the primary without rewriting the link's source | **extended**: `link` / `confirm` `{primary: false}`; new `set_primary` (atomic, compare-and-set) |
| Group-by-primary-work in the sidebar | exists (`buildSessionsByWork`) | kept as is |
| All links of a session (live + ended + suggested) with task details | partly: `work { links, session_id }` returns live links only, no titles | **new** `work { session_tasks }` |
| All sessions of a task (active, suggested, past) | partly: `live_session_ids` on a ticket, the resume plan's candidates | **new** `work { task }` |
| Tasks without a session | partly: ⌘K *My work* (tracker cache, 20 rows) | **new** `work { tree }` lists every cached / local task |
| Paginated, server-filtered read | none (sidebar builds groups client-side from `list_sessions`) | **new** keyset cursor on `tree` / `review` |
| Organisation as a boundary | exists for per-host tokens (`OrgScope::Host`); a paired client is always `All` | **extended**: a paired client can be **bound to an org** (`OrgScope::Org`, migration `0NN`) |
| Organisation of a task | tracker item → tracker's org; local item → none (its links take their session's) | **extended**: a local item can carry an explicit org (`work_items.org_id`), with an impact preview |
| Project / group of a task | none (the sidebar groups by work key only) | **new**: derived (tracker container, repository, key prefix) or placed by a person / a rule, with provenance |
| Local placement ("put this task under group X") | none | **new** `work_placements` + `work_link { place }` |
| Rules for similar items | trust-a-project for branch keys (M4); org rules (master) | **new** placement rules (`work_rules`) with a preview, edit, disable, delete |
| Review inbox | "N link suggestions · Review" sheet (suggestions only, one at a time) | **new** `work { review }`: suggestions + conflicts (cross-org, unavailable ticket, no primary), bulk decisions with per-item results, undo |
| Undo of a decision | "Not this" after an automatic link | **new** `work_link { reconsider }` (back to a suggestion) |
| Saved views | desktop remembers the last filters locally; phone remembers its filters | **new** server-side `work_views`, same filters on both |
| Optimistic concurrency | none (`sessions.row_version` is bumped by every UPDATE, so it cannot be a CAS token) | **new** per-link `version`, per-placement / rule / view `version`, `expected_*` parameters, `E_CONFLICT` |
| Duplicate start of one ticket from two devices | **bug**: `start` re-checks only after the SSH spawn; the loser's session stays up unlinked | **fixed**: `start` claims the key in the in-flight registry `resume` already used |
| Structure change events | links → `session:updated` (whole row); items → `work:item` | **new** `work:changed` (ids only) for placement / rule / view / org; lagged / not-resumed stream → full reload (existing `ready.resumed` / `lagged`) |
| Upgrade | 064 | **`0NN`** (065 or later), additive; older hubs and phones keep working (below) |

## Product model

- **Organisation** — the access boundary *and* the top navigation group. The
  Work view groups by a task's *effective* org, which is exactly the org the
  boundary uses; there is no separate "display org". Changing it is a
  security change (preview, confirmation, server re-evaluation).
- **Tracker** — a Jira / Asana / GitHub / Linear integration, in an org.
- **Project / group** — navigation only, never a boundary. Each task's group
  has an identity (`group.id`), a source and an editability:

  | `group.source` | From | Editable |
  |---|---|---|
  | `manual` | a person placed the task (`work_placements`) | yes (clear to fall back) |
  | `rule` | an enabled placement rule matched | yes (place it by hand, or edit / disable the rule) |
  | `tracker` | the tracker's container: Jira / Linear project or team key, GitHub `owner/repo`, Asana project gid | local placement only — fleet never edits the tracker |
  | `repo` | the repository (project `owner/repo`) of its most recent session | local placement |
  | `key` | a bare key's prefix (`ABC` of `ABC-12`) | local placement |
  | `none` | nothing known ("No group") | local placement |

  `group.id` is `label:<label>` for both `manual` and `rule` (a rule and a
  person placing into the same label land in the same group; `source` tells
  them apart), `tracker:<tracker_id>:<container>`,
  `repo:<owner/repo>`, `key:<PREFIX>`, `none`.
- **Task** — a work item (tracker or local) or a bare key: `task_id` is
  `item:<work_items.id>` or `ref:<KEY>`. Titles and positions are
  attributes, never the identity. When a sync binds a bare key to a tracker
  item, `work { task, task_id: "ref:KEY" }` answers the item and lists the
  old id in `aliases`, so a client's selection survives.
- **Session** — `session_id` (the live row) on a stable participant; past
  sessions are named by their link's snapshot.
- **Link** — the existing `work_links` row. On the wire its state is one of
  `active` (confirmed, session live), `ended` (the session or the link has
  ended — never shown as active), `suggested` (a guess, never groups, never
  changes access) and `rejected` (shown only in details). `primary` is true
  for at most one active link per session. Changing the primary never
  deletes or ends another link.
- **Suggestion** — a `suggested` link. It does not change the binding link or
  any permission.
- **A person's decision** — a one-off correction (link, place, confirm,
  reject, set primary) or, as a separate deliberate step, a rule.

## Security model

### Scopes (the one place: `Caller::org_scope`)

| Caller | Scope | Work data | Sessions |
|---|---|---|---|
| master, desktop, paired client (unbound) | `All` | everything; org is a view | everything |
| paired client **bound to org O** (new) | `Org { org: O }` | O's, and unassigned while O's `bound_sees_unassigned` is on (D31) | O's, and unassigned while that flag is on (strict: the client asked to be restricted) |
| per-host token of host H in org O | `Host { alias: H, org: O }` | O's and unassigned, plus M3's host fence | D7 `isolate_sessions` |

A client is bound with `fleet-hub pair --org <name>` (or `fleet-hub client
bind <name> --org <org>` / `--no-org`). The binding is stored in
`client_tokens.org_id` without a foreign key: deleting the org leaves the
client bound to an org that no longer exists, which sees unassigned data
only — **fail closed**, never widened to `All`.

Whether a bound client sees *unassigned* work and sessions is a per-org
setting (D31): `orgs.bound_sees_unassigned`, added by migration `0NN`,
**default on** (as a host sees today), edited through `work_admin`'s org
edit and Settings → Work → Organisations. Off: the org's bound clients see
only rows assigned to their org. The isolation matrix has a row for both
values.

Everything that filtered on `scope.host()` for the org half of the boundary
now filters on `!scope.is_all()` (`scope_links`, `tickets::allowed`,
`today`, `card`, `local_items`, `tidy`, `health`). The host-only fences
(ended links only of the own host, a host starts only on itself) stay
host-only. `GET /events` hides `work` frames from org-bound clients as it
does from hosts (they carry no session to fence by) and fences / redacts
`session` frames with the same `fence_frame`.

### A session with tasks from several orgs (the explicit behaviour)

Session S is in org A. It has task T1 (org A) and task T2 (org B), linked
with `force_cross_org` (the only way such a link exists).

- **Unbound callers** see both. The T2 link carries `cross_org: true`, and it
  is an item of the review inbox (kind `cross_org`) until a person unlinks it
  or acknowledges it (`work_link { ack }`).
- **A caller scoped to A** (host of A, client bound to A) never receives T2:
  not as a task in `tree`, not in `task` (it answers as a task that does not
  exist), not in S's `session_tasks`, not as S's `work` if T2 is primary (the
  existing `redact_row`), not its evidence, title, key, conversation ids,
  journal or summary. `other_tasks` counts only links the caller sees.
- **A caller scoped to B** sees T2 but **not S**: a link's session is shown
  only when the caller may see that session (`sees_session`; strict for a
  bound client, D7 for a host). A link whose session the caller may not see
  is omitted, not counted, and not hinted at.
- Neither side ever sees the other's evidence: evidence rides on the link,
  and a link reaches a caller only when both its task and its session are
  visible.
- Past (ended) links follow the same rule with the snapshot's org
  (`snap_org_id`) and host.

The isolation matrix (`mcp/tools/tests_isolation.rs`) gains two callers
(client bound to A, client bound to B) and a cross-org session fixture;
every new action has a row, and the leak check runs over every answer.

### Mutations

- Readonly tokens: `work_link` is refused before dispatch (existing
  `enforce_mode`), whatever the action.
- Per-host tokens: `set_primary`, `reconsider`, `decide_batch`, `ack` only on
  their own host's sessions (existing `resolve_target_row` fence); `place`,
  `assign_org`, `rule_save`, `rule_delete`, `view_save`, `view_delete`
  refused (`E_FORBIDDEN`: a session's agent does not reorganise the fleet).
- Org-bound clients: link decisions and `place` on what they see; views they
  save are theirs (`work_views.owner_org`); `assign_org`, `rule_save`,
  `rule_delete` refused (a rule or an org move reaches beyond their org).
- `assign_org` (local items only) requires the `impact_token` of a fresh
  `work { org_impact }`: the server recomputes the impact and refuses with
  `E_CONFLICT` when it changed. A tracker item's org is its tracker's; the
  answer says so (`allowed: false`, `reason: tracker_controlled`) and names
  the admin path (`work_admin assign_tracker`, master).
- Every action re-checks visibility of every id it touches; an id out of
  scope answers exactly as an unknown id.

### Concurrency

- `work_links.version` (`0NN`) is bumped by a trigger on every change of
  `state`, `source`, `is_primary`, `ended_at`, `item_id`, `ref_key`,
  `archived_at` or `review_ack_at`.
- Link mutations accept `expected_version`; on a mismatch they answer
  `E_CONFLICT` with `details: { link_id, version, state, primary }` and
  change nothing. Without it they behave as before (older clients).
- `set_primary { session_id, link_id, expected_primary }` is a
  compare-and-set on the session's current primary link (`0` = none):
  two devices moving the primary concurrently cannot silently overwrite
  each other; the second gets `E_CONFLICT` naming the current primary.
  Setting the link that is already primary is a no-op success (idempotent).
- Placements, rules and views carry `version`; `expected_version` `0` means
  "I expect none", so a create never overwrites a concurrent create.
- Start and resume of the same key share the in-flight registry, so two
  devices starting / resuming one ticket at once produce one session and one
  `E_EXISTS` (with the session id when visible).

## Contracts

All reads are `work { action, … }` (readonly-eligible), all writes
`work_link { action, … }`. New parameters are optional; nothing existing
changes shape. `CONTRACT_REVISION` stays 4.

### Shared types

```jsonc
// WorkTreeFilters — the same object for tree pages, saved views, desktop and phone.
{
  "org": 3,            // org id, or "none" (unassigned); absent = all visible
  "tracker": 1,        // tracker id, or "local" (local items), or "ref" (bare keys)
  "status": "open",    // any (default) | open (todo + in_progress) | todo | in_progress | done
  "mine": true,        // assigned to me in its tracker (the tracker's `mine` view)
  "has": "active",     // any (default) | active | past_only | none (no session at all) | suggested
  "review": true,      // only tasks with something to review
  "query": "login",    // case-insensitive substring of key or title
  "group": "tracker:1:ABC",  // one group only (a section being expanded)
  "archived": false    // false hides archived tasks into `archived_hidden`; absent or true shows them (a client from before the archive never sends it); `status: done` and `has: past_only` show them anyway
}

// GroupRef
{ "id": "tracker:1:ABC", "label": "ABC", "source": "tracker",
  "rule_id": null, "tracker_value": "ABC", "editable": true }

// WorkTaskLink — one session under a task
{
  "link_id": 42, "link_version": 3,
  "state": "active",               // active | ended | suggested | rejected
  "primary": true,
  "session_id": 7,                 // live session (absent when ended)
  "name": "ABC-12 login", "host": "mefistos",
  "source": "manual", "strength": "explicit", "rule": null,
  "why": "branch abc-12-login · R3",     // one line, from the evidence
  "evidence": [ … ],              // task / session_tasks only, never in tree
  "created_at": 1790000000, "decided_at": 1790000100, "ended_at": null, "end_reason": null,
  "claude_status": "idle", "needs_you": false, "archived": false,
  "resumable": true, "branch": "abc-12-login", "pr_url": null,
  "cross_org": false,
  "other_tasks": 1                // other active tasks of this session the caller sees
}

// WorkTask
{
  "task_id": "item:12", "item_id": 12, "key": "ABC-12", "title": "Login fails",
  "url": "https://acme.atlassian.net/browse/ABC-12",
  "kind": "tracker",               // tracker | local | ref
  "tracker_id": 1, "tracker_name": "Jira (acme)", "provider": "jira",
  "tracker_state": "ok",           // the tracker's sync state: an outage is not "no sessions"
  "status_category": "in_progress", "status_name": "In Review", "resolution": null,
  "unavailable": false, "unavailable_reason": null,
  "assignees": ["Ana"], "mine": true,
  "org_id": 1, "org_source": "tracker",   // tracker | item | sessions | none
  "org_fenced": true,                     // the org is a boundary (tracker / item), not inferred
  "org_mixed": false,                     // unfenced task whose sessions span orgs
  "group": { GroupRef },
  "counts": { "active": 1, "ended": 2, "suggested": 1 },
  "needs_you": false, "review": false,
  "last_activity_at": 1790000200,
  "repos": ["acme/api"],
  "placement_version": 0,                 // 0 = no placement
  "sessions": [ WorkTaskLink … ],         // active (primary first), suggested, ended newest first
  "sessions_more": 0,
  "archived": false                       // no active session, and done or every link (one past) archived
}
```

### Reads

| Action | Parameters | Answer |
|---|---|---|
| `tree` | `filters?`, `cursor?`, `limit?` (1–200, default 50), `per_task?` (0–50, default 8) | `{ tasks: [WorkTask], groups: [{org_id, org_name, group: GroupRef, count}], orgs: [{id, name, color}], trackers: [{id, name, provider, state, org_id}], total, archived_hidden, next_cursor, generated_at }` (`archived_hidden`: tasks every other filter passed but hidden as archived, over the whole result) |
| `task` | `task_id` | `{ task: WorkTask (all sessions, with evidence), aliases: [task_id], description: string? (tracker text, fenced for an agent, ≤ 600 chars), last_outcome: {at, name, host, branch, pr_url, summary?}?, placement: {group, note, version, updated_at, updated_by}?, rules: [rule ids that match] }` |
| `session_tasks` | `session_id` | `{ session_id, org_id, primary_link_id, links: [WorkTaskLink + task: {task_id, key, title, kind, status_category, status_name, url, unavailable, org_id, tracker_name}] }` — live, suggested, rejected and ended links of the session's participant |
| `review` | `cursor?`, `limit?` | `{ items: [ReviewItem], total, next_cursor }` |
| `rules` | — | `[WorkRule]` |
| `rule_preview` | `rule` | `{ affected: [{task_id, key, title, from: GroupRef, to: GroupRef}], total, kept_manual }` |
| `views` | — | `[WorkView]` |
| `org_impact` | `task_id`, `org_id` (`0` = no org) | `OrgImpact` |

`groups` counts cover the whole filtered result, so a client draws every
section header (with its count) while loading tasks page by page — per
section with `filters.group`. Order: named orgs by name, unassigned last;
groups by label, `none` last; tasks needing you first, then latest
activity, then `task_id`. The cursor is opaque (keyset over that order plus
a hash of the filters): a cursor used with other filters is refused
(`E_INVALID`), and a page is stable under concurrent changes (no row is
repeated; a task that moved may be skipped and appears on the next full
read).

```jsonc
// ReviewItem
{
  "review_id": "link:42", "kind": "suggestion",   // suggestion | cross_org | unavailable | no_primary
  "session_id": 7, "session_name": "api", "host": "mefistos",
  "link_id": 42, "link_version": 2,
  "task": { "task_id": "item:12", "key": "ABC-12", "title": "Login fails", "org_id": 1 },
  "why": ["branch abc-12-login since 09:05 · R3"],
  "strength": "strong", "rule": "R3", "preselected": false,
  "alternatives": [ { "link_id": 43, "task_id": "ref:ABC-13", "key": "ABC-13", "title": "" } ],
  "created_at": 1790000000
}
// WorkRule
{ "id": 3, "name": "Payments", "enabled": true, "version": 2,
  "conditions": { "tracker_id": 1, "container": "PAY", "key_prefix": null,
                  "title_contains": null, "repo": null },
  "group": "Payments", "created_at": 1790000000, "updated_at": 1790000000 }
// WorkView
{ "id": 1, "name": "My open work", "filters": { WorkTreeFilters }, "version": 1, "updated_at": 1790000000 }
// OrgImpact
{ "task_id": "item:77", "from_org": null, "to_org": 2, "allowed": true, "reason": null,
  "links": [ { "link_id": 5, "session_id": 9, "name": "api", "host": "h-a", "state": "active",
               "session_org": 1, "becomes_cross_org": true } ],
  "hosts_losing": ["h-b"], "hosts_gaining": ["h-c"],
  "bound_clients_losing": 1, "bound_clients_gaining": 0,
  "journal_entries": 4, "summaries": 1,
  "impact_token": "…" }
```

### Writes

| Action | Parameters | Answer |
|---|---|---|
| `link` (extended) | + `primary?` (default true: today's behaviour), `expected_version?` | `SessionRow` |
| `confirm` (extended) | + `primary?`, `expected_version?` | `SessionRow` |
| `reject`, `unlink` (extended) | + `expected_version?` | `SessionRow` |
| `set_primary` | `session_id`, `link_id`, `expected_primary?` (link id, `0` none) | `SessionRow` |
| `reconsider` | `session_id`, `link_id`, `expected_version?` | `SessionRow` — a person's confirm / reject goes back to a suggestion (undo) |
| `ack` | `session_id`, `link_id`, `expected_version?` | `SessionRow` — a conflict (cross-org, unavailable) is kept on purpose |
| `decide_batch` | `decisions: [{session_id, link_id, decision: confirm\|reject\|reconsider\|ack, expected_version?, primary?}]` (≤ 100) | `{ results: [{link_id, ok, code?, message?, version?}] }` — each checked on its own against scope and version |
| `place` | `task_id`, `group` (empty = clear), `note?`, `expected_version` | `WorkTask` |
| `assign_org` | `task_id`, `org_id` (`0` = none), `impact_token` | `WorkTask` |
| `rule_save` | `rule` (`{id?, name, enabled, conditions, group, expected_version?}`) | `WorkRule` |
| `rule_delete` | `rule_id`, `expected_version?` | `{ deleted: true }` |
| `view_save` | `view` (`{id?, name, filters, expected_version?}`) | `WorkView` |
| `view_delete` | `view_id` | `{ deleted: true }` |

Errors: `E_CONFLICT` (version / impact / primary changed; `details` carry the
current value), `E_NOTFOUND` (unknown or not visible), `E_FORBIDDEN`
(readonly, the caller may not change this, cross-org without force,
tracker-controlled org), `E_INVALID` (malformed, a cursor from other
filters).

### Events

- Link changes keep emitting `session:updated` (the whole row). The row
  carries `work_rev` (omitted when 0): a digest of the versions and ids of
  the session's live links, so a client sees a *secondary* link change
  (added, ended, made primary) that `work` (the primary only) does not show.
  It is cleared in everything a host-bound or org-bound caller receives,
  like the rest of the row's work.
- New `work:changed` `{ what: placement | rule | view | org, task_id?, rule_id?, view_id? }`
  (ids only; kind `work`, so hidden from host-bound and org-bound streams).
- A client re-reads the visible tree page (debounced) on `session:*` or
  `work:*`; on `ready { resumed: false }` or `lagged` it reloads the whole
  view (existing rules).

### Desktop commands (Routed to the hub on a paired desktop)

Reads: `work_tree`, `work_task`, `work_session_tasks`, `work_review`,
`work_rules`, `work_rule_preview`, `work_views`, `work_org_impact` → `work`.
Writes: `set_primary_work`, `reconsider_work_link`, `ack_work_link`,
`decide_work_batch`, `place_work`, `assign_work_org`, `save_work_rule`,
`delete_work_rule`, `save_work_view`, `delete_work_view` → `work_link`; and
the existing `link_session_work` / `confirm_session_work` /
`reject_session_work` / `unlink_session_work` take `primary` and
`expected_version`. Every command's argument object is `{ args: { … } }`
with the fields of the action above.

### Compatibility

- Older phone / desktop on a new hub: nothing they call changed shape;
  `link` / `confirm` without `primary` still take the primary.
- New phone on an older hub: the Work tab is shown only when `tools/list`
  lists `tree` in `work`'s action enum; each write only when its action is
  listed (the phone's existing capability gate). The desktop on an older hub
  gets `E_INVALID unknown work action` and shows "Needs a newer hub".
- Migration `0NN` is additive (new columns with defaults, new tables); the
  upgrade test and the downgrade guard (`store::testgen`) cover it.

## Desktop UX

- **Sidebar switch** `Sessions | Work` (⌘⇧W / Ctrl+Shift+W). Sessions is
  today's tree; its group-by-primary-work mode stays.
- **Work tree**: org sections → group sections → tasks → session
  occurrences. Each task: key, title, tracker badge, status, `active / past`
  counts, `needs you`, a `?` for pending review, `unavailable` struck
  through, `tracker down` when its tracker is failing. Each occurrence:
  primary ★, secondary, suggested (dashed), past (dimmed, never "active").
  Opening any occurrence selects the same `session_id`; every occurrence of
  the selected session is highlighted.
- **Scale**: section headers come from `groups`; a section loads its tasks
  page by page (`Load more`), expansion state and the last selection are
  kept per view (pref), loading / empty / error states are explicit.
- **Filters**: org, tracker, status, mine, has (active / past only / none /
  suggested), review, search. **Saved views**: a menu of `work_views`, Save
  as…, Update, Delete.
- **Task detail** (center pane): tracker data (title, status, link,
  assignees, description excerpt), where the org and group come from, the
  repositories, every session with its state and "why", last outcome,
  actions *Open*, *Continue* (resume `last`), *Start new* (start), and edits
  (*Place in group…*, *Assign org…* with the impact dialog, *Make a rule…*
  with preview).
- **Session detail**: a *Tasks* section with every link (primary /
  secondary / suggested / past), *Make primary*, *Remove*, *Add task…*, and
  *Show in Work view*.
- **Review** tab in the Work view: suggestions and conflicts with Confirm /
  Change… / Reject / Keep, multi-select with a count, per-item results, and
  Undo for the last decision.

## Phone UX (fleet-mobile)

- **My work** tab: saved views as chips, a filter sheet (same filters),
  collapsible org → group sections, compact task cards, `Load more` per
  section, stale banner ("Offline · as of 10:42") when not connected.
- **Task screen**: every session with its state and why; *Open*,
  *Continue*, *Start here*; *Place in group…* (a bottom sheet with a list).
- **Session screen**: *Tasks* section with every link; *Make primary*,
  *Remove*, *Add task…* (a searchable list, never drag and drop).
- **Review** sheet: one card per item with its reason; Confirm / Reject /
  Change…; Undo on the snackbar.
- Writes are sent only while connected and shown as saved only after the
  hub's answer; `E_CONFLICT` shows the current value with *Reload*; a
  refused write shows the hub's sentence. No offline queue.

## Stages (this milestone)

0. This document (matrix and contracts).
1. Read contract: migration `0NN`, `tree` / `task` / `session_tasks` /
   `review` / `rules` / `rule_preview` / `views` / `org_impact`, the org-bound
   client scope, isolation rows, scale test.
2. Desktop Work view (read, filters, detail, navigation both ways).
3. Mutations (`set_primary`, versions, `reconsider`, `ack`, `decide_batch`,
   `place`, `assign_org`, rules, views), the start race fix, review inbox.
4. Phone.
5. Acceptance (Part R of `docs/work-graph-acceptance.md`), user guide.

## Decisions (the owner's, answered 2026-09-27)

| # | Question | Answer |
|---|---|---|
| D31 | May an org-bound client see *unassigned* work and sessions? | **By setting**: a per-org flag `orgs.bound_sees_unassigned`, default on (as a host sees today); off, the org's bound clients see only rows assigned to their org. Built in M14.1b's migration, with an isolation row for both values |
| D32 | Should a cross-org link (forced) raise a review item until acknowledged? | **The default**: yes (`cross_org` review kind, cleared by `ack`) |
| D33 | May a full, unbound phone change a local task's org? | **The default**: yes, with the impact preview; bound clients and hosts may not |
| D34 | Placement rules only (navigation), or also link rules ("sessions in repo X are task Y")? | **The default**: placement only; link rules would bypass detection's evidence and R9 |
| D35 | Saved views: shared on the hub, or per device? | **The default**: shared on the hub (a bound client's views are its org's) |

D36 (M14 as a milestone despite D26, and who drives it) is the plan's, not
this design's.

## Revisions

- 2026-09-27 (M14.1d, desktop commands and events): the eighteen commands
  above, each `Routed` to its `work` / `work_link` action (no `LocalOnly`
  row), and `link_session_work` / `confirm_session_work` take `primary`
  and `expected_version`, `reject_session_work` / `unlink_session_work`
  `expected_version`. `delete_work_view` takes the optional
  `expected_version` M14.1c gave `view_delete`. `work:changed` is emitted
  by the store after the write commits: `placement` (`task_id`), `rule`
  (`rule_id`, on save and delete), `view` (`view_id`, on save and delete)
  and `org` (`task_id`, a local task's org; its sessions' rows also go out
  as `session:updated`). The desktop's hub bridge never resumes a stream
  (every connection, the one after `lagged` included, re-lists), so its
  resync ends with a desktop-only `work:changed { what: "resync" }` —
  emitted only when the hub answered the re-list — which the Work view
  reads as "reload whole"; a hub never sends it. `SessionRow.work_rev`
  (*Events* above) was lost in the #343 merge and restored (cc07d36):
  computed in SQL with the row (`SESSION_COLUMNS` in `store/rows.rs`) as
  the sum of `version * 1000003 + id` over the session's live links (`0`,
  omitted on the wire, when there is none), so a
  link added, ended, removed, confirmed, rejected, made primary or archived
  moves it (066's trigger bumps `version` on each); every link write
  already re-reads and emits the row. `OrgScope::redact_row` /
  `redact_json` clear it for a scoped caller. Additive and optional: no
  contract bump.
- 2026-09-27 (M14.1c, the writes): no migration (066's columns carry every
  version). `E_CONFLICT` is a new `IpcError` code. A link's version is
  compared only after the link is known to be the caller's to name (this
  session's live link, in scope), so neither a version nor a state is an
  oracle for a hidden link. `link { expected_version }` compares against
  the session's live link to that work (`0`: none). `set_primary`'s
  compare-and-set is on the primary the caller sees, so a scoped caller
  whose session's primary is another org's (a forced link) expects `0`
  and is never told that link's id. `view_delete` also takes an optional
  `expected_version`; the master reaches every view (a replaced view keeps
  its owner); a bound client's view may name only an org or tracker it
  sees (`E_NOTFOUND` otherwise). A batch does not carry `force_cross_org`.
  The service refuses a bound client another org's session itself, not
  only at the transport's gate. `work:changed` and the Tauri commands are
  M14.1d's: the writes here emit only the existing `session:updated`.
- 2026-09-27 (M14.1b, the reads): migration `0NN` is two, **066**
  (`work_view`) and **067** (`orgs.bound_sees_unassigned`, its own
  migration because an `orgs` column is re-added when that table is rebuilt,
  as 053's `auto_tidy` is); 065 went to lifecycle F2's `stale_working` on
  `main` (#343) first. 066 carries `work_links.version` and its trigger
  although only M14.1c writes against it: the reads answer `link_version`.
  Security changes against the backend branch: `org_impact` is refused
  (`E_FORBIDDEN`) to every scoped caller — hosts and bound clients may not
  move an org (D33), and the impact named other orgs' hosts, bound-client
  counts and journal counts; a bound client's `rules` are only the rules
  that place a task it sees and name no tracker outside its org; a task with
  no work at all is unassigned data, so it follows D31; a bound client whose
  org was deleted sees no unassigned data either (stricter than *Scopes*
  above, which said it would). With D31 off, a bound client also may not
  start or resume a session in an unassigned project or host. `pair --org`
  takes the org's id.
  The inference the PR's threat note listed — a bound client reading
  fleet-wide daily spend in `fleet_health` and so another org's activity —
  is closed: for a bound client every roll-up there that sums across hosts
  (spend by day and by host, host / session / status counts, tunnels, the
  detection backlog) is taken over the hosts it sees only, its org's and,
  under D31, unassigned ones (`health::scope_to_org`). The master, unbound
  clients and host tokens read as before.
- 2026-09-27 (M14.0, brought to `main`): based on `main` `f10d0b92` instead
  of `be0e2bc`; the acceptance section is Part R (Part P is GHES on `main`);
  the migration is `0NN_work_view.sql`, numbered at merge time, not 063
  (063 is `row_version_on_visible_change` on `main`); D31 is the per-org
  setting `orgs.bound_sees_unassigned`, default on, instead of a fixed
  "yes"; D32–D35 are answered with their defaults.
