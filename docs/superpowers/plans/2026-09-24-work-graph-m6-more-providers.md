# Work graph M6: more providers (plan)

**Date:** 2026-09-24
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M6. Order per decision D1.
**Design:** `../specs/2026-09-24-work-graph-design.md` §0.4 (the provider trait and `HttpTransport`)
**Review:** `../reviews/2026-09-24-work-graph-specialist-review.md`, the integrations review (trait generalisation, GitHub, Linear, Asana, ViaHost, Jira DC), C26 (resolution)
**Depends on:** M3 (the trait, `trackers`, sync, secrets, admin surface, ⌘K tickets)
**Uses when present:** M4 (URL recognition), M5 (the tracker's org)

## Goal

> One work model across every tracker the user actually has: Jira for
> Company A, Asana for Company B, GitHub Issues for personal repos. Adding a
> provider is one adapter plus fixtures. Core, UI and store never learn
> provider concepts.

**Value line:** ⌘K, the chips, start work, resume and detection behave the same
whatever tracker a ticket lives in.

## The rule that keeps M6 cheap: a provider conformance suite (M6.0)

Before any new adapter, lift M3's Jira tests into a **provider-agnostic
conformance suite**: `service/trackers/conformance.rs`, a macro or generic
test fn run once per adapter over that adapter's fixtures. Every adapter
must prove the same contract:

| # | Scenario | Expected |
|---|---|---|
| 1 | probe | returns `instance_id`, me, key prefixes (or `human_keys = false`), tz or UTC |
| 2 | a view lists items | normalised snapshots, stable `external_id` |
| 3 | incremental | `since` mark plus overlap; dedupe on `(id, updated)` |
| 4 | by-id fetch of 3 ids, one missing | 2 `Found`, 1 `Unavailable{reason}` |
| 5 | status mapping | todo / in_progress / done, plus `resolution` (completed, not_planned, duplicate) |
| 6 | hierarchy | `parent` and `hierarchy_level` where `caps.hierarchy` |
| 7 | the item moved or was renamed (key or repo changed) | same `external_id`, old key in `aliases` |
| 8 | recognise | keys (if `human_keys`), URLs, repo-relative `#n` (if `caps.repo_relative`) |
| 9 | errors | 401, 403 on one view, 429 with Retry-After, offline, garbage JSON: each gives the right `TrackerState` |
| 10 | no secret in any `Debug` / serialised output or error | holds |

Also add a **snapshot golden** of each adapter's normalised output, so a
provider API change shows up as a diff in review, not as a production
surprise.

## Tasks

Each task is one PR in the **Worker** environment, with the usual checks plus
`cargo deny check` whenever a dependency changes.

### M6.0: conformance suite and trait refinements

- Extract the suite from M3's Jira tests. Jira must still pass it unchanged.
- Trait refinements that the later providers need. Each lands with a Jira
  no-op implementation:
  - `Caps { human_keys, repo_relative, hierarchy, iterations, multi_container, query_lang: Jql|Gql|Search|None, incremental: Watermark|SyncToken|None }`.
  - `ItemRef::RepoNumber { repo, n }` and `ItemRef::Url`. `recognize` gets
    the session's `owner/repo` in its context.
  - `SyncMark` becomes opaque per provider (Asana sync tokens, Linear
    `updatedAt` cursors).

### M6.1: GitHub Issues through `gh`, with no credentials

**Why first:** `gh` is already installed and authenticated on the hosts, and
the PR probe already uses it (`service/outcome.rs`). It exercises the
repo-relative `#n`, multiple assignees and `NOT_PLANNED`, and it needs no
token in fleet.

- **Transport:** `ViaHost` (a generalised M6.3 variant) that runs `gh` on a
  chosen host through `SshExec`. A tracker row is
  `provider='github', transport='via_host:<alias>'`, plus a repo list or
  owner scope in `config`.
- **Identity:**
  - `external_id` = the issue's **node id**, which survives a transfer.
  - `key` = `owner/repo#n`.
  - `aliases` gains the old `owner/repo#n` after a transfer.
- **List:**
  `gh issue list -R <repo> --json number,title,state,stateReason,assignees,labels,url,updatedAt,id --search "<query>" --limit N`.
  Views:
  - `mine` is `assignee:@me is:open`;
  - `recent` is `updated:>=<date>`;
  - with gh ≥ 2.94, add `parent,subIssues,issueType`, and use `caps.hierarchy`
    when they are present.
- **Status:**
  - `OPEN` maps to todo, or in_progress when it has a linked branch or PR.
    That is enriched from M4's PR probe, never guessed.
  - `CLOSED` with `COMPLETED` maps to done / completed.
  - `CLOSED` with `NOT_PLANNED` maps to done / not_planned.
- **Detection hooks:**
  - The M4 PR probe's `closingIssuesReferences` becomes a strong link.
  - `Fixes #n` in commit trailers is weak.
  - A bare `#n` in a prompt resolves only against the session's own repo.
- **Fixtures:** recorded `gh --json` outputs, including a transfer, a
  NOT_PLANNED close, and a missing-`gh` / not-logged-in host (the state
  becomes `unreachable` with an actionable message).

### M6.2: Asana (Company B's tracker)

**Why second:** it is a real tracker the user relies on. It is also the
hardest fit: there are no human keys, and a task can sit in several
projects. If it fits the model, anything will.

- **Transport:** `Direct` HTTPS. Auth is a Personal Access Token
  (`Authorization: Bearer`). The SSRF allowlist gains
  `https://app.asana.com/api/1.0` only.
- **Probe:**
  - `GET /users/me` gives the gid and workspaces.
  - `instance_id` = the workspace gid.
  - `caps.human_keys = false`, `multi_container = true`,
    `hierarchy = true` (subtasks, `parent`).
- **Identity:**
  - `external_id` = the task gid.
  - `key` = none. A display key is synthesised for the UI only
    (`B:1207…` shortened) and is never used for matching.
  - Detection is by **URL only**. Both URL forms are handled:
    `app.asana.com/0/<project>/<task>` and
    `/1/<ws>/project/<p>/task/<t>`.
- **Views:**
  - `mine`: `GET /tasks?assignee=me&workspace=<ws>&completed_since=now` for
    open tasks, with `opt_fields` for the fields fleet needs.
  - A project: `GET /projects/<gid>/tasks`.
  - Search: `GET /workspaces/<ws>/tasks/search` where the plan allows it.
    This is a Premium feature; without Premium the view is disabled via
    `caps`.
- **Incremental:** use the **events API** with sync tokens per project.
  - Tokens expire after about 24 hours. On a 412 / `sync` error, fall back to
    a full refetch of that project's open tasks.
  - The `SyncMark` is the token.
- **Status:**
  - `completed = true` maps to done / completed.
  - Otherwise the mapping comes from the task's **section** in its first
    container, through an optional per-tracker map in `config`, for example
    `{"In progress": "in_progress", "Done": "done"}`.
  - The map is **inferred on first sync**: sections named like
    progress/doing/review map to in_progress, and ones named like
    done/shipped map to done. It is shown in Settings for correction.
  - Everything else is todo. This is the one provider where fleet needs the
    user's help for status, and it asks inline, not as a setup step.
- **Containers:** memberships become `containers[]`. An org / project scope
  can match on any of them.
- **Rate limits:** per-minute plus a cost quota, handled through `429` and
  `Retry-After`. Use `opt_fields` everywhere and never `opt_expand=*`.
- **Fixtures:**
  - a multi-project task;
  - a subtask;
  - a completed task;
  - a section-based in_progress task;
  - an expired sync token;
  - a 429;
  - a non-Premium workspace (search disabled).

### M6.3: generalised `ViaHost` transport

- `HttpTransport::ViaHost { host }` runs `curl` on the host. The token is
  piped on stdin into a header file, never passed in argv (the same pattern
  as `service/account_usage.rs`). The response is capped, and there is a
  timeout.
- Use it when a tracker is reachable **only** from a machine on a VPN or
  internal network (Jira DC), or when the user prefers the token to stay on
  their own host.
- A **CLI variant** for providers with a trusted CLI: `gh` (M6.1) and Atlassian
  `acli` (`acli jira workitem search --jql … --json --paginate`). The token
  then lives in the CLI's own store, **never in fleet**.
  - A tracker row can say `transport = 'via_cli:<alias>'` with
    `cli = gh|acli`.
- **Tests:**
  - the curl script is built with `shell::quote` (grep test: no
    unquoted interpolation);
  - the token never appears in the script text or argv, which is asserted on
    the `FakeSsh` call log;
  - a host that cannot be reached leads to `unreachable`.

### M6.4: Linear

- **Transport:** `Direct` GraphQL at `https://api.linear.app/graphql`. Auth
  is a personal API key (`Authorization: <key>`). Add it to the allowlist.
- **Probe:** `viewer { id organization { id urlKey } teams { key } }`. Key
  prefixes are the team keys, which settles the Jira/Linear `ENG-123`
  collision through `recognize`, not by guessing.
- **Identity:** `external_id` = the issue id. Key = `identifier`; a team move
  changes it, and `aliases` keeps the old identifier.
- **Views:**
  - `mine`: `assignedIssues(filter: {state: {type: {nin: [completed, canceled]}}})`;
  - the current cycle: `cycle.isActive`, only when the team has cycles;
  - `recent`: `updatedAt` greater than or equal to the watermark.
  - Pagination uses GraphQL `after` cursors.
- **Status:** `state.type` maps as follows:

  | Linear `state.type` | fleet status |
  |---|---|
  | `triage`, `backlog`, `unstarted` | todo |
  | `started` | in_progress |
  | `completed` | done / completed |
  | `canceled` | done / not_planned |

- **Parent / sub-issues:** hierarchy.
- **Rate limits:** 2,500 req/h plus complexity points. Keep queries narrow and
  honour the `X-RateLimit-*` headers.
- **Fixtures:**
  - a team move (identifier alias);
  - canceled vs completed;
  - a team without cycles;
  - a complexity-limit error.

### M6.5: Jira Data Center

**Only if D6 says it is needed.**

- **Transport:** `Direct` when reachable. Otherwise `ViaHost` / `via_cli`.
  An `extra_ca` PEM can be set per tracker (the internal CA). The SSRF
  allowlist becomes "the configured site only", entered by Master.
- **Auth:** a PAT (`Authorization: Bearer`).
- **API:** `/rest/api/2/search` with `startAt` / `total` (not
  `search/jql`), and `/rest/api/2/issue/{id}`.
- **Epic:** the **Epic Link** custom field is discovered through `/field`;
  `parent` covers sub-tasks. `hierarchy_level` comes from the issue type
  where available.
- **Shared code:** everything else (status category, resolution, views,
  favourite filters) is shared with the Cloud adapter via a
  `jira_common.rs`.
- **Fixtures:** the DC search shape, Epic Link, a self-signed CA through
  `extra_ca`, and a CAPTCHA lockout.

### M6.6: UI and docs

- **Settings → Work → Connect:**
  - A provider picker. Paste any ticket or issue URL and fleet infers the
    provider and site: `atlassian.net` → Jira Cloud, `app.asana.com` →
    Asana, `linear.app` → Linear, `github.com/…/issues/…` → GitHub.
  - It asks only for what that provider needs: GitHub asks nothing (it
    offers a host with `gh`), Asana asks for a PAT, Linear for an API key,
    Jira Cloud for an email and API token, DC for a PAT and optional CA.
- **Provider badge** on chips and ⌘K rows (a small icon), with the provider
  name in the tooltip.
- **Asana section map** editor, shown inline the first time status is
  ambiguous: "Which Asana sections mean *in progress*?"
- **Docs:** `docs/hub.md` → *Trackers* per provider: auth, what fleet reads,
  rate limits, `ViaHost` / `via_cli`. Update the roadmap.

## Acceptance (manual, one per provider)

1. **GitHub:** connect with no token (pick a host with `gh`). ⌘K lists
   `assignee:@me` issues. A PR with `Fixes #42` links issue 42 (M4). A
   NOT_PLANNED close shows as done with a "not planned" badge.
2. **Asana:**
   - Connect with a PAT. My tasks appear, and a task in two projects shows
     under both scopes.
   - Pasting its URL into a prompt suggests the link (M4).
   - Confirming the inferred section map makes the chips show in_progress.
   - Letting the sync token expire (simulated) recovers with a full refetch.
3. **Linear:** `ENG-123` in a branch links to Linear, not Jira, when both
   trackers exist, because it matches Linear's team keys. A team move keeps
   the link.
4. **ViaHost:** a tracker only reachable from host X syncs through X. The
   token is not in any argv (`ps` on host X during the sync) and not in
   fleet's logs.
5. **Jira DC** (if built): syncs through a self-signed CA configured per
   tracker.
6. **Isolation (M5):** a Company A host sees none of Company B's Asana tasks.

## Risks

| Risk | Mitigation |
|---|---|
| Provider APIs drift | Conformance suite plus snapshot goldens; each adapter isolated behind the trait; `caps` let the UI degrade per feature |
| Asana status is ambiguous | Section map inferred, shown and correctable inline; `completed` stays authoritative |
| A CLI's JSON output changes (`gh`, `acli`) | Pinned minimum versions checked in probe; fixtures per version; unknown fields ignored |
| Tokens on hosts (ViaHost) | stdin-only header files, removed after use; `via_cli` keeps the token in the CLI's own store |
| Rate limits across many trackers | Per-tracker sequential sync, `Retry-After` honoured, by-id refresh only for linked items |
| UI overload | One provider badge; everything else is the same UI as Jira |

## Decisions (defaults if unanswered)

| # | Question | Default |
|---|---|---|
| D1 | Provider order after Jira | GitHub → Asana → ViaHost → Linear → Jira DC. Swap Asana first if Company B's work is more urgent |
| D6 | Jira DC needed? | No; M6.5 is skipped unless requested |
| new | Asana status source | `completed` plus the inferred section map, confirmed once by the user |
| new | GitHub scope | Repos of the user's live and recent sessions plus `assignee:@me` across them; no org-wide crawl |
