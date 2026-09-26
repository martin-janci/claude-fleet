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
| new | A `move_session` whose target host puts the session in another org than its live links? | Refused like a link (`E_FORBIDDEN`, details `cross_org: true`, before anything is copied) unless `force_cross_org: true`, which carries every link as it is and names each crossing in the report's `warnings`; the Transfer sheet offers "Move anyway". (Revised 2026-09-25 on review: it was warned-only, the one accepted cross-org path, while the move had no force flag.) |

## Revisions

- **2026-09-24, M5 landed** on `claude/cloud-fleet-work-graph-m5` (stacked on
  M4): M5.1 + M5.2 (bb9e44a), M5.3 (5515199), M5.4 + M5.5 (74a29c3), M5.6
  (docs). Verified with `cargo fmt`, `clippy -D warnings` (workspace), `cargo
  test` (fleet-core, claude-fleet, fleet-hub; only the four chmod tests that
  fail as root on `main` fail), `cargo deny check`, `pnpm check` / `pnpm
  test`, and `scripts/hub-e2e.sh` (102/102). Deviations from the tasks
  above, and why:
  1. **M3's interim fence is kept, not removed** (M5.3 said "removed in the
     same PR"). The org scope alone is WIDER than the per-host fence: every
     host of org A would read every ticket any A host works on. The fence
     is therefore composed with the org one — a per-host token reads an
     item only when it is linked on its own host AND inside its org or
     unassigned — and a test (`a_per_host_token_still_cannot_read_another_
     hosts_tickets_in_its_org`) pins it. The same holds for past links
     (only its own host's) and for context / resume plans (some of the work
     ran on its host). Relaxing tickets to org-wide reads is an open
     decision below.
  2. **The scope type.** `OrgScope::{All, Host { alias, org, isolated }}`
     rather than `Scope::{All, Org(Option<id>)}`: the host fence and D7 need
     the alias and the isolating orgs. It lives in `service::orgs` (the
     service layer filters with it; Tauri commands pass `All`), and
     `Caller::org_scope` is the one place a caller becomes one.
  3. **Two enforcement layers, one decision.** The service layer filters
     what it reads (links, items, trackers, journal) with the scope's
     predicates; `call_tool` then redacts every result AND error a per-host
     token receives (`redact_work_for`: each session row's work fields,
     the row's org looked up by id, failing closed). The backstop exists
     because session rows reach a host through ~20 tools
     (`new_session`, `whoami`, `work_link`'s own answer …); `list_sessions`
     also redacts typed rows before `fresh_for` hashes the page.
  4. **`work_rejected` never reaches a per-host token.** It is a list of
     bare keys with no org of their own, read only by the sidebar's fallback
     recognition; the matrix found an A session carrying B's rejected key.
  5. **No existence oracle, two shapes.** An item / link / tracker id
     outside the scope answers exactly as an unknown id (`E_NOTFOUND`, same
     sentence). A key or URL outside it: `lookup`, `context`, `resume_plan`
     and `resume` answer one `E_FORBIDDEN` sentence whether or not the key
     exists (the plan's acceptance 3); `work_link { link | reject, key }`
     links it as the BARE key the host typed (`WorkTarget::Ref`) — exactly
     what an unknown key does — instead of refusing, because refusing only
     keys that exist was an oracle (security review, finding M3).
  6. **Cross-org integrity reaches further than `link`.** `confirm` (a
     suggestion becoming a link), `start` and `resume` apply the same rule
     with the same `force_cross_org`; detection never creates or promotes a
     cross-org link; the sync never binds a bare key to another org's
     tracker item, nor fetches one on behalf of another org's session
     (review finding M4). The desktop offers "Link anyway" for link and
     confirm; start and resume accept the flag but have no UI for it yet.
  7. **Briefs are written for their reader.** `work { context }` under the
     caller's scope; a resume brief under the landing host's (a per-host
     token is always its own reader, so it cannot borrow another host's
     scope — review finding H1); a start's ticket brief is dropped when the
     ticket is not visible to the landing host (a forced cross-org start);
     the SessionStart context (M4.5) under the row's host.
  8. **D7 covers more than list / message / dispatch**: `whoami`,
     `peer_status`, `related_sessions`, `session_history`, the eight repo
     reads, `send_message` (both addressing forms, before the "retired"
     check), `broadcast_prompt`, `plan_start`'s `E_EXISTS` details, a
     ticket's `live_session_ids`, and `session:*` frames including
     `session:event` / `session:conversations` (review finding H2).
     `dispatch_task` needed nothing: a per-host token already reaches only
     its own host's workers. D7 is symmetric: an isolating org's sessions
     are hidden from other orgs' hosts, and its hosts see only its own and
     unassigned sessions; a host always sees its own host's sessions.
  9. **`/events`.** `work:*` frames stay off host-bound streams entirely
     (M3's rule, stricter than an org filter). Session frames are fenced
     per frame from the row's own `org_id`; a stream re-reads its scope when
     an org change bumps a process-wide generation (checked per frame), and
     on the keep-alive beat, and ENDS when the scope moved — the client
     reconnects under the new one.
  10. **Resolution details.** Path rules match the worktree's path, else the
      project's `base_path`, on a directory boundary; fleet stores no cwd, so
      a session with no project resolves by host rules and the host's org
      only. A host-only rule ranks below owner rules and above
      `hosts.org_id`. Owner / repo match ASCII case-insensitively (SQL
      `lower()`); ties go to the lower rule id. `repo` requires `owner`
      (a CHECK the plan did not have), and `path_prefix = "/"` is refused.
  11. **Past links keep their org.** `work_links.snap_org_id` is written by a
      second retirement trigger (the 046 snapshot's companion, independent of
      trigger order); links that ended before 050 fall back to the rules over
      their snapshot (host, project).
  12. **Existing data lands in no org** ("the default org" is *unassigned*):
      a fleet with no named org has no boundary and no new chrome, exactly as
      before; a test migrates an M4 database and reads it back unchanged.
  13. **Admin surface.** `work_admin` actions `list_orgs`, `add_org`,
      `update_org`, `remove_org` (`E_INVALID_STATE` naming the trackers),
      `add_rule`, `remove_rule`, `assign_host`, `unassign_host`,
      `assign_tracker` (no `org_id` = unassign). Desktop: seven LocalOnly
      commands with REASONS (`add_org`, `update_org`, `remove_org`,
      `add_org_rule`, `remove_org_rule`, `assign_host_org`,
      `assign_tracker_org`) and three Routed reads (`work_scopes`,
      `list_orgs`, `org_suggestions`). CLI: `fleet-hub org list | add | set |
      rm | rule add | rule rm | assign-host | assign-tracker` (`set` and
      `rm` in place of the plan's unnamed update / remove). An org change
      that moves sessions bumps their `row_version` and emits
      `session:updated` for each — no new event kind.
  14. **Suggestions** pair a tracker site with the GitHub owner of the same
      name (`acme.atlassian.net` + `acme/*`), and are empty for a single-owner
      fleet with no tracker (nothing to separate).
  15. **UI.** The needs-you line lives in a new `ScopeAttention.svelte` next
      to `Attention.svelte` (which is a screen-reader announcer with no
      visible entries). "Unassigned" is offered by the selector but never
      counts towards the two scopes that show it. ⌘K scopes sessions by
      their scope and tickets by their tracker's org; an unassigned
      tracker's tickets show in every scope. The Today view does not exist
      yet (M9).
  16. **M5.5's `rowMatches`** takes a normalised `FilterRow` (session,
      ticket or past link) so one function serves all three; tracker,
      status category, assignee, has-session and archived compose and are
      truth-table tested, but only scope, host, bg and needs-you have
      controls today.
  17. **Tool budget:** +627 B (`work_admin`'s eight org parameters; its
      description cut to "see action") and +87 B (`force_cross_org`),
      measured and recorded at `BUDGET_BYTES` (69,199). No new tool, no
      contract revision bump; the contract golden gained `org_id` on
      `SessionRow`, `HostRow` and `WorkLinkRow`.
  18. **Accepted trade-off (review finding M5):** with `isolate_sessions`
      off, a session's own fields — friendly name, branch, worktree, last
      prompt, its timeline — stay readable by other orgs' hosts, and a start
      names the worktree after the ticket. These are session data, which the
      plan fences only under D7; the docs say so.
- **Open decisions (the user's):**
  - ~~Should a per-host token read its whole org's tickets (not only those
    linked on its own host)?~~ **Decided 2026-09-24 (the user): keep own
    host AND own org** — the composed fence stays.
  - Should `isolate_sessions` also redact session names / branches of
    other orgs' sessions when off, rather than being all-or-nothing?
