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

## Revisions

- **2026-09-24, M6 landed** on `claude/cloud-fleet-work-graph-m6` (from M4,
  with M5 merged in): M6.0 (008de56), M6.1 + M6.2 (2694653), M6.3
  (456f517), M6.4 (5017bab), M6.5 (33ff03e), the M5 merge (a478a51), M6.6
  (ea01b01). Verified per task with `cargo fmt`, `clippy -D warnings`
  (workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub; only the
  four chmod tests that fail as root fail), `cargo deny check`,
  `scripts/hub-e2e.sh` (102/102) for every transport change, and `pnpm check`
  / `pnpm test`. No test reaches a real tracker. Deviations from the tasks
  above, and why:
  1. **Order and scope.** D1's order was followed. **M6.5 was built**: the
     M6 brief asked for it, which overrides D6's "skip unless requested".
     `acli` (the Jira `via_cli` variant) was not built: `gh` is the one
     trusted CLI; a Jira only a VPN host reaches uses `via_host` (curl).
  2. **One seam for every provider.** GitHub, Asana, Linear and Data Center
     are all HTTP providers behind `HttpTransport`; `gh` and host-side `curl`
     are *transports* (`net/via_host.rs`: `GhCliTransport` for
     `via_cli:<alias>`, `CurlTransport` for `via_host:<alias>`) over a new
     `SshExec::run_with_stdin` (the request body — and for curl the header
     file with the credential — is piped on stdin, never in argv). So the
     conformance suite drives every provider over `FakeTransport`, and
     `TrackerNet` (fake / direct / via a host) replaces the bare transport
     argument; the hub and a standalone desktop install theirs once.
     A fleet-agent host cannot pipe stdin (`E_UNSUPPORTED`): `via_host` /
     `via_cli` need an SSH-reachable host.
  3. **GitHub reads GraphQL through `gh api`**, not `gh issue list --json`:
     one shape for listings and by-id / by-number reads, values as GraphQL
     variables (never in the query text or a flag), `stateReason`, `parent`
     and node ids from the server whatever the `gh` version (so no version
     probe). `in_progress` comes from GitHub's own `linkedBranches` /
     `closedByPullRequestsReferences` (a fact, not a guess) rather than from
     M4's PR probe, so an issue no fleet session works on still shows it.
     GitHub is `via_cli` only: `work_admin` refuses `direct` / `via_host` for
     it and `set_credential` refuses a `via_cli` tracker (setting that
     transport drops any stored secret). The site is `https://github.com` or
     `https://github.com/<owner>`, `settings.repos` narrows it. M4's closing
     refs, trailers and bare `#n` needed nothing new: a GitHub tracker lifts
     R3u, and `tracker_claims` binds `owner/repo#n` refs to the tracker whose
     scope covers the repository.
  4. **Conformance suite** (`service/trackers/conformance.rs`): a `Harness`
     trait plus `conformance_suite!`, ten tests per adapter, and a golden of
     each adapter's normalised listing (`REGEN_TRACKER_GOLDENS=1`). Two
     relaxations, stated in the suite: the *moved* scenario may be `None`
     only for a provider without keys or repos (Asana's `asana:<gid>` never
     changes), and status coverage requires todo / in_progress / done-
     completed, with `not_planned` wherever the provider has it (Asana has
     only `completed`).
  5. **Trait refinements** as planned, plus: `Caps.incremental` is an enum
     (`watermark` | `sync_token` | `none`); `recognize(text, RefCtx { repo })`;
     `TrackerProvider::changes` (default: no token); the opaque mark is
     `tracker_views.sync_mark`. Migration **051** (written as 050, renumbered
     when M5, which took 050, was merged) adds it and `trackers.settings` —
     what the ADMIN sets (GitHub repos, Asana's confirmed section map, Data
     Center's CA and private-network opt-in), kept apart from `config`,
     which every probe replaces.
  6. **Asana's key is `asana:<gid>`** (the recogniser's canonical form of an
     Asana URL, M4) instead of none: every key-centric path — grouping,
     lookup, start, "already running", resume — works unchanged, and nothing
     types it or matches it in prose. The UI shows `Asana …123456`. Views:
     `mine`, one per project the user's open tasks sit in (at most 10, found
     by the probe, not configured), and `recent` only where search works
     (the probe tries it; 402 → Premium missing). Project views read the
     events API by sync token; `mine` is listed whole each pass (it has no
     event stream); more than 20 changed tasks, or `has_more`, lists the
     project whole. By-id reads use `/batch` (10 per call). The inferred
     section map lives in `config.section_map`, the confirmed one in
     `settings.section_map` (which wins, and stops inference); a "Done"
     section is done without a resolution — only `completed` says completed.
  7. **Linear** lists root `issues` filtered to `isMe` (plan:
     `viewer.assignedIssues`), all filters as GraphQL variables; the cycle
     view's id is `sprint` (label *Current cycle*) so the local view
     evaluation serves it; aliases come from `previousIdentifiers`; the API
     key goes in `Authorization` bare, as Linear expects. Complexity and
     request limits (`RATELIMITED`, often on a 400) back off until
     `X-RateLimit-Requests-Reset`.
  8. **Jira Data Center.** No port in the site (a port could aim the hub at
     another service on the host; use 443 or `via_host`). RFC 1918 private
     addresses are allowed — a corporate server lives there — while
     loopback, link-local (incl. 169.254.169.254), unspecified, broadcast and
     multicast are refused after resolution unless `allow_private_network`;
     the connect goes to the checked address (no DNS rebinding). DC has no
     bulk fetch: by-reference reads are one `/search` with
     `validateQuery: warn`. The Epic Link becomes `parent_key`, and the
     parent id only when the epic is in the same page; `hierarchy_level`
     only where the site says (no type-name guess, C28); the legacy
     `Sprint@…[state=…,name=…]` strings are parsed; `extra_ca` applies to
     `direct` only (`via_host` uses the host's trust store). What Cloud and
     DC share moved to `jira_common.rs`; Jira Cloud is unchanged and still
     passes.
  9. **SSRF, per provider.** Each tracker's transport allows exactly its API
     host (`*.atlassian.net`, `api.github.com` through `gh`, `app.asana.com`,
     `api.linear.app`, the one DC host) — `host_policy(row)` — for `direct`
     and `via_host` alike; https only, no redirects, a body cap and a
     timeout everywhere.
  10. **Secrets.** Redaction gained GitHub (`ghp_`, `gho_`, `ghu_`, `ghs_`,
      `ghr_`, `github_pat_`), Linear (`lin_api_`, `lin_oauth_`) and Asana
      PAT shapes. Test tokens are assembled at run time: literal token
      shapes in the source tripped GitHub push protection although every one
      is invented.
  11. **API.** No new tool, no contract bump: `work_admin` gained
      `transport` and `settings` (+168 B on the M4 base; measured 69,288 over
      M5, `BUDGET_BYTES` 69,388) and its description lost the provider list
      (M5 then made it "Trackers and orgs; see action"). The Tauri
      `add_tracker` / `update_tracker` / `set_tracker_credential` commands
      take the new fields (`#[serde(default)]`), no new command, verdicts
      unchanged. `fleet-hub tracker add` gained `--provider`, `--via-cli`,
      `--via-host`, `--repo`; `set-credential --email` is optional (bearer
      without it).
  12. **M5 merged** (a478a51): `TrackerRow` carries `org_id` and `settings`;
      the bare-reference binding combines `tracker_claims` with M5's org
      rule; tickets / lookup / start take `OrgScope` and `TrackerNet`.
      Acceptance 6 is proved by `tests_isolation_providers.rs`: M5's rules
      run once per new provider (a Company A host sees none of Company B's
      GitHub, Asana, Linear or DC items or trackers by key, URL or tracker
      id, never makes B's tracker fetch, and B's tracker never binds a
      reference an A session made), rather than by parameterising M5's MCP
      matrix, which stays Jira-shaped.
  13. **UI.** Provider badges show only once trackers of two or more
      providers exist (a Jira-only fleet looks as before). The GitHub host
      picker lists every host; whether its `gh` is logged in is what the
      Connect test reports. The Asana section map is asked inline under the
      tracker until confirmed.
  14. **Not done:** `acli`; GitHub Enterprise Server; a per-provider
      `/metrics` series; the phone (M8); the manual acceptance on real
      GitHub, Asana, Linear and Data Center accounts.

