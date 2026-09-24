# Work graph M5: organisations and isolation (plan)

**Date:** 2026-09-24
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M5
**Design:** `../specs/2026-09-24-work-graph-design.md` §0.1 (principle 2), §0.2 (`orgs`, `org_rules`, `hosts.org_id`)
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md` §2 (org isolation for per-host tokens), C22 (text-keyed rules), UX review (scope selector, needs-you never hidden)
**Depends on:** M1b, and M3's `trackers.org_id` column
**Replaces:** M3's interim "own-host linked items only" fence, with a real org boundary

## Goal

> Company A and Company B stay separate twice over:
> - in what the user sees at a glance, where scope is a view;
> - in what each company's hosts can read and do, where the org is a
>   boundary.
>
> A user with a single org configures nothing and sees no new chrome.

**Value line:** one selector switches Personal, Company A and Company B. A
Claude session running on a Company A host cannot list, read or link
Company B's tickets, links or work events.

## Facts this plan builds on (verified in code)

- **`Caller`** (`mcp/auth.rs:63`) has three shapes:
  - master: `host_alias = None`, `client = None`;
  - paired client (phone or desktop): `client = Some`;
  - per-host token (every in-session Claude): `host_alias = Some(alias)`.
- **`require_host`** (`mcp/tools/support.rs:171`) binds only *session-addressed*
  tools. List reads and `GET /events` (`mcp/events_route.rs`) are fleet-wide
  for every authenticated caller. That is today's gap.
- **Projects are unsuitable as the org anchor** (C22). `projects.owner` is a
  GitHub owner, or `local` for adopted folders. Project rows are deleted and
  re-created by `refresh_projects` and purge. `project_id` is re-derived on
  every pass. Mapping must therefore be text-keyed.

## Design decisions

1. **Scope and boundary are one concept at two strengths.**
   - Every item, link and session resolves to **at most one org**, or none,
     which means *unassigned*.
   - For people (master, paired clients) the org is a **view filter**, and
     they see all orgs.
   - For per-host tokens it is a **boundary**: a host with `hosts.org_id = A`
     sees only A plus unassigned.
2. **Zero-config default.**
   - With no named orgs, scope is derived from `projects.owner`, excluding
     `local`, and there is no boundary.
   - The selector appears only when two or more scopes exist.
   - Naming an org is optional. It is needed only to merge owners
     (`acme` + `acme-labs`), to split one owner, to attach a tracker, or to
     turn on the boundary.
3. **Resolution order** for an entity's org (first hit wins):

   | Entity | Order |
   |---|---|
   | Tracker item | its tracker's `org_id` |
   | Link | its item's org, else its session's org |
   | Session | `org_rules` match on `path_prefix`, then `owner/repo`, then `owner`, then the session's **host** org |

   Rules are text and survive project-row churn. Ties go to the most
   specific rule.
4. **The boundary applies to work data first; sessions are opt-in.**
   - Work data (items, links, journal, trackers, `work` and `work_link`
     results, `work:*` events) is always fenced for host-bound callers in an
     org.
   - Session-level isolation (`list_sessions`, `session_*` reads,
     `send_message` across orgs, `session:*` events) sits behind a per-org
     `isolate_sessions` flag, **default off**. Turning it on can break
     legitimate cross-host orchestration, such as a controller dispatching
     tasks to workers, so it is the user's call (decision D7 below).
5. **Mutating the boundary is Master-only.** Org and rule CRUD and
   `hosts.org_id` go through `work_admin` (Master). A paired desktop gets
   `LocalOnly` with the sentence "configure on the hub", plus the
   `fleet-hub org …` CLI. A host can never move itself into another org.
6. **Needs-you is never hidden by scope.** When the view scope hides a session
   that needs the user, Attention says "2 need you in Personal →". That
   applies to the view only; the boundary is never relaxed for this.

## Tasks

Each task is one reviewable PR in the **Worker** environment. The usual
checks apply, plus `REGEN_*` and a **security-focused review** of M5.3 and
M5.4 (`/security-review`) before they are pushed.

### M5.1: schema and resolution (backend)

- **Migration** (next free number, after M2–M4's):

  ```sql
  CREATE TABLE IF NOT EXISTS orgs(
    id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE, color TEXT,
    isolate_sessions INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
  CREATE TABLE IF NOT EXISTS org_rules(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    org_id INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    owner TEXT, repo TEXT, path_prefix TEXT, host_alias TEXT,
    CHECK (owner IS NOT NULL OR path_prefix IS NOT NULL OR host_alias IS NOT NULL),
    CHECK (owner IS NULL OR owner <> 'local'));
  -- guarded ALTERs:
  ALTER TABLE hosts ADD COLUMN org_id INTEGER REFERENCES orgs(id) ON DELETE SET NULL;
  -- trackers.org_id (M3) gains its FK meaning here; no column change needed.
  ```

- **`store/orgs.rs`** holds CRUD and the pure resolver
  `org_of_session(row, rules, host_org) -> Option<OrgId>`, table-tested.
  The tests cover:
  - specificity: path beats repo beats owner beats host;
  - `local` is never matched by owner;
  - a re-created project keeps its org, because rules are text;
  - a session whose cwd is outside any project resolves by path or host.
- **`SessionRow.org_id: Option<i64>`** is a computed column in
  `SESSION_COLUMNS`, the same pattern as `work`. Resolving it in SQL keeps
  list and emit paths consistent; if rule matching is too awkward in SQL,
  post-fill it at the few list and emit points instead and document why.
- **Derived scopes for zero-config:** `work { action: scopes }` returns named
  orgs, plus owner-derived pseudo-scopes when no org covers them. Each
  entry is `{ id | owner, label, session_count, needs_you }`.

### M5.2: admin surface (backend + CLI)

- **`work_admin` actions:** `add_org`, `update_org`, `remove_org` (refused
  while trackers reference it, naming them), `add_rule`, `remove_rule`,
  `assign_host`, `unassign_host`, `assign_tracker`.
  - Master-only, confirm-gated for removals.
  - Verdicts: `LocalOnly` with `REASONS` entries.
- **`fleet-hub org list|add|rule add|rule rm|assign-host|assign-tracker`**, with
  `--isolate-sessions on|off`.
- **Suggestions, never automatic:** `work { action: org_suggestions }`
  proposes rules from owners seen in live sessions and from tracker `site_url`
  domains. The UI offers them as one-click "Create org Company A from
  `acme/*`".
- **Tests:** CRUD, refusal messages, CLI parsing, and the suggestion
  heuristics.

### M5.3: the boundary (backend, security-critical)

- **`Caller::org_scope(store) -> Scope`** returns one of:
  - `Scope::All` for master and paired clients;
  - `Scope::Org(Some(id))` for a host-bound caller whose host has an org;
  - `Scope::Org(None)` for a host-bound caller whose host has none, which
    sees only **unassigned** data. This is deliberate: a host nobody placed
    in an org must not read an org's data.

  A single function with no call-site logic.
- **Enforcement points**, each covered by the isolation matrix test:
  - **`work` reads** (links, items, tickets, lookup, context, resume_plan,
    scopes) filter in SQL by org.
  - **`work_link` writes:**
    - A host-bound caller may link only its own-host sessions
      (`require_host`, unchanged), and only to targets in its org or
      unassigned.
    - Linking a session to an item of a different org is `E_FORBIDDEN` for
      every caller, including master, unless `force_cross_org: true` is
      passed. This is a data-integrity rule, not access control: it stops
      Company B's ticket from being attached to a Company A session by
      mistake.
  - **`/events`:**
    - `work:*` frames are dropped for host-bound streams outside their
      scope.
    - With `isolate_sessions` on, `session:*` frames are dropped too.
    - The filter runs per frame on the sending side, cheaply: the frame
      carries `org_id`, and the stream holds its caller's scope.
  - **When `isolate_sessions` is on:**
    - `list_sessions` and every session-addressed read are filtered for
      host-bound callers.
    - `send_message` and `dispatch_task` across orgs are refused with a clear
      sentence. Master and clients are unaffected.
- **The M3 interim fence is removed** in the same PR, because this rule
  supersedes it. A host whose host has no org still sees only unassigned
  data (above), so nothing widens silently.
- **Tests:** one **isolation matrix** that runs every work read and write and
  `/events` against these callers: master, client full, client readonly,
  host in A, host in B, host with no org. Check both `isolate_sessions`
  values, and include the cross-org link refusal and its `force` override.
  This matrix is the acceptance gate for the PR.

### M5.4: UI (frontend)

- **Scope selector:**
  - A compact `All ▾` left of the search in `SidebarFilters.svelte`, shown
    only when two or more scopes exist.
  - Persisted like `hostFilter`.
  - Chord ⌘⇧O / Ctrl+Shift+O in `app_views.ts` `appChord`.
  - It filters the sidebar (project and work modes), ⌘K and the Today view
    (M9).
- **Needs-you in hidden scopes:** `Attention.svelte` gets a line such as "2
  need you in Personal →" that switches scope.
- **Settings → Work gains Organisations:**
  - The list, colours, and rules shown as chips (`acme/*`,
    `path: ~/work/acme`, `host: hetzner-a`).
  - Assign hosts and trackers.
  - An `isolate sessions` toggle with a plain-language warning.
  - Suggestions from `org_suggestions`.
  - In hub-client mode it is read-only, with "configure on the hub" and the
    CLI line.
- **Visual cue:** a thin colour bar on work group headers and session rows by
  org colour, only when two or more orgs exist.
- **Tests:** the selector appearing (1 scope versus 2), filter composition
  with the host filter, needs-you across scopes, the settings section in both
  modes, and the colour bar.

### M5.5: filters compose

- One pure `rowMatches(filters)` in `sidebar_index.ts` over org, host,
  tracker, status category, assignee, has-session, archived and the
  needs-you predicate. It is used by both project and work modes and by ⌘K,
  so the filters never diverge.
- **Tests:** composition truth tables.

### M5.6: docs and roadmap

- `docs/hub.md` gains an *Organisations and isolation* section: what the
  boundary does and does not cover, `isolate_sessions`, and the CLI.
- `docs/concepts.md` gets a paragraph.
- Security notes: per-host tokens and org scope.
- Update the roadmap status and Revisions.

## Acceptance (manual, a hub with two orgs)

1. **Single org:** no selector and no colour bars appear, and nothing changes
   from M4.
2. **Creating an org from a suggestion:** "Company A from `acme/*`" regroups
   sessions under the selector without any other setup.
3. **Isolation of work data:**
   - An in-session Claude on a Company A host calls `work { tickets }` and
     gets only A's tickets.
   - `work { key: "B-12" }` for a Company B ticket answers `E_FORBIDDEN`
     with a sentence that says why.
   - `/events` from that host never carries B's `work:*` frames.
4. **Unplaced host:** a host with no org sees only unassigned work.
5. **Cross-org link:** linking a Company A session to a Company B ticket from
   the desktop is refused unless forced, and the UI explains why.
6. **`isolate_sessions` on for B:**
   - A's host can no longer list or message B's sessions.
   - The master desktop still sees everything.
   - A controller in A dispatching to a worker in A still works.
7. **Needs-you across scopes:** in scope A, a blocked session in Personal
   shows "1 needs you in Personal →".

## Risks

| Risk | Mitigation |
|---|---|
| Breaking cross-host orchestration | Session isolation is opt-in per org, default off; work-data isolation cannot affect orchestration |
| A leak through a path nobody listed | One `org_scope` function; enforcement in SQL where possible; the isolation matrix covers every `work` action and `/events`; a `/security-review` gate |
| Performance of per-row org resolution | Rules are few; resolve in SQL with an index on `org_rules(owner)`, or cache per reconcile pass |
| A host moving itself into another org | Assignment is Master-only; a per-host token has no admin path |
| Zero-config users see new chrome | The selector and colour bars appear only at two or more scopes |

## Decisions (defaults if unanswered)

| # | Question | Default |
|---|---|---|
| D4 | Is org isolation needed before the second company's tracker is connected? | Yes. M3's interim fence covers the gap; M5 replaces it |
| D7 (new) | Isolate sessions (list, message, dispatch) across orgs? | Off per org; the user turns it on per org |
| new | May a host with no org see org data? | No, only unassigned |
| new | Cross-org link from the desktop? | Refused unless `force_cross_org` (data integrity) |
