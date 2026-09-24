# Work graph M3: tracker foundation and Jira Cloud, read-only (plan)

- **Date:** 2026-09-24
- **Roadmap:** `../2026-09-24-work-graph-roadmap.md`, milestone M3
- **Design:** `../specs/2026-09-24-work-graph-design.md`, §0.2 (schema), §0.4 (provider), §0.5 (API)
- **Review:** `../reviews/2026-09-24-work-graph-specialist-review.md`, C17–C29 and §2 (security)
- **Depends on:** M1b (`work_items` / `work_links`, `SessionRow.work`, `work` / `work_link`)
- **Independent of:** M2, except that the migration numbers follow it

## Goal

> Start work from a Jira ticket and see its status next to the sessions, with
> zero setup beyond pasting one ticket URL and a token. No Jira clone, and no
> writes to Jira.

**Value line:**
- ⌘K lists *My work*; Enter starts a session on the ticket, or jumps to the
  one already running.
- Chips and group headers show the ticket's title and status.
- Keys typed before Jira was connected bind to their tickets on their own.

## Design decisions for M3

1. **Trackers enrich and never gate.** Everything in M1 and M2 keeps working
   with Jira down, the token expired or no tracker at all. A tracker adds
   title, status and URL to keys and items that already exist.
2. **Identity is the tracker's id, never its key (C24).**
   - `work_items` is unique on `(tracker_id, external_id)`.
   - The key and the old keys (`aliases`) are attributes.
   - A moved or renamed issue keeps its links.
3. **Missing is not gone (C25).**
   - A 404 or a vanished search result sets `unavailable_at` and
     `unavailable_reason` (`not_found_or_no_permission`).
   - Links and history stay.
   - Nothing is deleted because of a tracker answer.
4. **Poll, don't subscribe.** A `FleetTasks` sync tick (C20), with per-view
   time watermarks and overlap. Every item that has a link is refreshed by id
   on each tick. Webhooks come later, and even then only as a nudge.
5. **Credentials never ride a read path.**
   - They live in `tracker_secrets` with `env:` / `file:` refs.
   - Admin tools are Master-only, so on a paired desktop they are `LocalOnly`
     (C17), plus a hub CLI.
   - Redaction covers Basic auth and `ATATT` tokens, and there is an SSRF
     allowlist.
6. **Host-bound callers are fenced in from day one.** Until M5's orgs exist, a
   per-host token (every in-session Claude) sees only the tracker items linked
   to sessions **on its own host**, never the whole tracker. Master and paired
   clients (the user's own desktop and phone) see everything. This
   interim rule keeps Company A's hosts out of Company B's tickets before orgs
   ship, and it is the safe answer to decision D4.
7. **One transport seam.** Provider logic sits behind an `HttpTransport` trait:
   - `Direct` in M3.
   - `ViaHost` (curl / `acli` / `gh` on a host) in M6.
   - A `FakeTransport` for tests.

## Tasks

Each task is one reviewable PR in the Worker environment. For every task run:
- fmt, clippy `-D warnings`, and the touched crates' tests;
- `pnpm check` and `pnpm test`;
- `cargo deny check`;
- `REGEN_DOCS` / `REGEN_HUB_VERDICTS` whenever tools or verdicts change;
- `scripts/ci-local.sh --hub-e2e` for anything that touches the wire.

### M3.0: transport spike, decided by `cargo deny` (≤1 day)

Two options. Pick the one that passes `cargo deny check` with the fewest new
crates.

- **A (preferred): lift the TLS client that already exists.**
  - Take `src-tauri/src/backend/{http1.rs, remote.rs::connect}` and
    `tokio-rustls` (ring) with `rustls-native-certs`, all already accepted by
    `deny.toml` and the reason `webpki-roots` was avoided.
  - Build on them `fleet-core/src/net/https.rs`: HTTP/1.1 GET/POST with JSON,
    chunked decoding, a timeout, **no redirect following**, and a response
    size cap.
  - src-tauri then imports it instead of owning it.
- **B: `reqwest`** with `default-features = false` and
  `features = ["rustls-no-provider", "json"]` plus the ring provider (C29).

Deliverables:
- the `HttpTransport` trait (`async fn send(Request) -> Result<Response>`);
- `DirectTransport` and `FakeTransport` (scripted responses, recorded
  requests);
- a note in this plan recording which option won and why.

Also confirm the hub Docker image has CA certificates for `rustls-native-certs`
(`deploy/`).

### M3.1: schema, secrets and the admin surface (backend)

**Migration 048**, re-runnable, with a guard function for the ALTERs:

```sql
CREATE TABLE IF NOT EXISTS trackers(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL,                     -- jira (github/asana/linear later)
  name TEXT NOT NULL,                         -- "Acme Jira"
  instance_id TEXT, site_url TEXT NOT NULL, api_base TEXT,
  org_id INTEGER,                             -- FK arrives with M5's orgs
  transport TEXT NOT NULL DEFAULT 'direct',
  config TEXT,                                -- JSON: account_id, tz, key_prefixes, views, has_sprints per scope
  state TEXT NOT NULL DEFAULT 'unconfigured', -- ok|auth_failed|rate_limited|unreachable|captcha|unconfigured
  last_sync_at INTEGER, last_error TEXT, created_at INTEGER NOT NULL,
  UNIQUE(provider, site_url));
CREATE TABLE IF NOT EXISTS tracker_secrets(
  tracker_id INTEGER PRIMARY KEY REFERENCES trackers(id) ON DELETE CASCADE,
  auth_kind TEXT NOT NULL,                    -- basic (email+API token) | bearer (PAT, DC later)
  username TEXT, value TEXT, credential_ref TEXT);   -- ref: env:NAME | file:/run/secrets/x
CREATE TABLE IF NOT EXISTS tracker_views(
  tracker_id INTEGER NOT NULL REFERENCES trackers(id) ON DELETE CASCADE,
  view_id TEXT NOT NULL, label TEXT NOT NULL, query TEXT NOT NULL,
  watermark INTEGER, PRIMARY KEY(tracker_id, view_id));
-- work_items gains (guarded ALTERs): aliases, kind, hierarchy_level, status_name,
-- resolution, parent_id, containers, assignees, iteration, meta, updated_ext,
-- status_changed_at, fetched_at, unavailable_at, unavailable_reason.
```

**`store/trackers.rs`:**
- CRUD for trackers and views.
- `TrackerRow` exposes `has_credential: bool` and `credential_hint: "…abcd"`,
  **never** the value.
- `resolve_credential()` reads the ref first (env or file), then the stored
  value. It is the one place a secret is read, and it is used only by
  transport code.

**Admin surface:**
- MCP tool `work_admin`: `Access::Master`, confirm-gated for deletes, with the
  actions `add_tracker | update_tracker | set_credential | test | remove_tracker | list_trackers`.
- `fleet-hub tracker add|set-credential|test|list|remove` in
  `crates/fleet-hub/src/`. The credential comes from stdin or `--from-env`,
  never from argv.
- Tauri commands with **LocalOnly** verdicts ("configure on the hub"), and
  `REASONS` entries in `hub.ts`.

**Hardening:**
- `logging::redact` learns Basic-auth headers, `ATATT[A-Za-z0-9_\-=]{20,}` and
  `Authorization: Basic …`.
- `diagnostics.rs` adds the resolved tracker secrets to its literal-mask list.
- `last_error` is passed through `redact` before it is stored.
- Metrics carry labels `tracker_id` and `result` only.

**SSRF:**
- `site_url` must be `https://<name>.atlassian.net`: no userinfo, no port, no
  path beyond `/`.
- Other hosts are refused with `E_INVALID` until DC support (D6).

**Tests:**
- no read path serialises a secret (serde test on every row type);
- ref precedence;
- the allowlist;
- redaction patterns;
- guard rows;
- the hub CLI parses and never echoes the secret.

### M3.2: provider trait and the Jira Cloud adapter (backend, pure logic plus transport)

**`service/trackers/mod.rs`:** the trait from design §0.4:
- `caps`, `probe`, `views`, `list(view, since, cursor)`, `fetch(refs)`,
  `recognize(text, ctx)`.
- The normalised `WorkItemSnapshot` from the review, including `aliases`,
  `status { name, category, resolution }`, `hierarchy_level`, `containers`
  and `assignees`.

**`service/trackers/jira.rs`**, per review C23–C29:
- **`probe`:**
  - `GET /rest/api/3/myself` gives the accountId and timezone.
  - `/_edge/tenant_info` gives the cloudId, stored as `instance_id`.
  - `/rest/api/3/project/search` gives the key prefixes.
  - `/rest/api/3/field` finds the sprint field, the one whose
    `schema.custom` is `com.pyxis.greenhopper.jira:gh-sprint`. Whether
    sprints exist is recorded **per project**.
- **Search:** `POST /rest/api/3/search/jql`:
  - always with an explicit `fields` list, paginated by `nextPageToken`;
  - guard against a page token that repeats, and never persist tokens;
  - use `reconcileIssues` for the ids fleet just wrote (none in M3).
- **Fetch by id:** `POST /rest/api/3/issue/bulkfetch`, at most 100 per call.
- **Built-in views:**
  - `mine`: `assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC`.
  - `sprint`: `sprint in openSprints() AND assignee = currentUser()`, only
    where sprints exist.
  - `recent`: `assignee = currentUser() AND updated >= -14d`.
  - The user's favourite filters, from `GET /rest/api/3/filter/favourite`,
    wrapped as `filter = <id> AND …`.
- **Normalisation:**
  - `statusCategory.key` maps as new → todo, indeterminate → in_progress,
    done → done, undefined → todo.
  - `resolution` becomes completed, not_planned or duplicate. Won't Do and
    Duplicate are told apart by the resolution name, conservatively, and
    anything else falls back to `completed`.
  - Parent comes from the unified `parent` field; the level from
    `issuetype.hierarchyLevel`.
  - Descriptions are ADF. Only a short text extraction goes into `meta` (the
    title and the first 2k chars); the full description is not stored.
- **Incremental sync:** use `updated >= "<watermark in /myself tz, minute precision>"`
  with a 2-minute overlap. Dedupe on `(id, updated)`.
- **Errors:**
  - 401, or a 403 on `/myself`, sets `auth_failed`. Polling stops and the
    user is told API tokens expire yearly.
  - A 403 on one view disables that view only.
  - `X-Seraph-LoginReason: AUTHENTICATION_DENIED` sets `captcha` (log in via
    the browser).
  - 429 honours `Retry-After` and backs off with jitter.
  - Network errors set `unreachable`.
- **Tests:** everything runs over `FakeTransport` with recorded JSON fixtures
  (sanitised, committed under `service/trackers/testdata/jira/`):
  - a company-managed project with sprints and epics;
  - a team-managed project with no sprints and project-scoped statuses;
  - a moved issue (key alias);
  - an `undefined` status category;
  - Won't Do versus Done;
  - an ADF description;
  - a pagination loop;
  - 401, a 403 on a view, CAPTCHA, 429, and offline.

### M3.3: sync, binding and events (backend)

- **Sync tick:** `service/trackers/sync.rs`, started as
  `FleetTasks::start_tracker_sync` (desktop Local mode only; `tests_startup.rs`
  proves a paired desktop does not run it) and in `fleet-hub/src/serve.rs`.
  - Its interval is the setting `work.sync_interval_secs`: default 300, `0`
    turns it off.
  - It is single-flight (`AtomicBool`) and per-tracker sequential.
- **Each pass, per tracker whose state is `ok`:**
  1. Run every enabled view from its watermark.
  2. `fetch` every linked item by id.
  3. Upsert `work_items` by `(tracker_id, external_id)`.
  4. Mark items that come back missing on a by-id fetch as `unavailable`.
  5. Record `status_changed_at` transitions into the journal (M2's
     `status_change` kind, if M2 has landed).
- **Binding:** every live or ended `work_links.ref_key` whose prefix belongs to
  the tracker's `key_prefixes` and that now matches an item (by key or alias)
  gets `item_id` set. `ref_key` is kept for history.
  - When the key belongs to two trackers, it is not bound; it waits for a
    manual pick (C28 / §0.3).
  - This is the "keys typed before Jira was connected" retro-bind.
- **Events:** `work:item` and `work:tracker` names under the `work` kind,
  emitted **only on a real change** by comparing the normalised row before
  writing (replay-ring pressure).
- **Tests:**
  - sync over `FakeTransport` with `Store::open_in_memory`: views, watermarks,
    overlap dedupe, linked refresh, unavailable, bind and ambiguous-bind;
  - no event on an unchanged pass;
  - a paired desktop never starts the sync.

### M3.4: reading tickets, start work, lookup (backend API)

- **`work` read actions** (grouped tool, budget C21):
  - `tickets { tracker_id?, view?, query?, limit }` serves the **cache**.
  - `lookup { key | url }` answers from the cache, or does a live
    fetch-on-demand for one item and caches it. A URL is parsed by
    `recognize`, which settles which tracker it belongs to.
  - `trackers` lists trackers (no secrets).
- **`work_link { action: start }`:** a compound on the hub, so desktop and phone
  make one call.
  1. Resolve the item.
  2. Pick the project: the `project_hint` argument, else the last project
     where that key prefix was seen (from links), else `E_AMBIGUOUS` with
     candidates.
  3. Pick the host.
  4. Name the new worktree branch `slug(key + " " + title)`.
  5. Set the friendly name `KEY title`.
  6. Call `new_session`, then link with `source='started'`.
  7. With `brief: true`, enqueue M2's handover (if M2 landed) or the ticket
     context: title, status, URL and the short description inside
     `mark_untrusted`.

  When the key already has a live session, return `E_EXISTS` with that
  session, so the UI jumps to it instead.
- **Scoping (decision 6):** host-bound callers may `lookup` or `tickets` only
  items linked to sessions on their host. Otherwise the answer is
  `E_FORBIDDEN`, with a sentence that says why.
- **Verdicts:** `work` and `work_link` stay Routed. Update the docs and
  `skills/claude-fleet-control/SKILL.md`: an agent can `lookup` its own
  ticket.
- **Tests:** the scoping matrix (master, client full, client readonly, host A
  with a link, host A without), `start` against FakeSsh (project resolution,
  duplicate, brief), and the budget test.

### M3.5: UI (frontend)

- **Connect by paste.** Settings → Work is a small section:
  - Tracker list with state badges (ok, token expired, rate-limited, …) and
    "Test".
  - "Connect Jira": paste any ticket URL, and fleet infers the site and key,
    then asks only for the email and API token.
  - In hub-client mode it shows "configure on the hub" with the CLI line.
  - An unbound key chip (`ABC-123` whose prefix matches no tracker) offers
    "Connect Jira to see ABC-123" inline.
- **⌘K ticket kind** in `quick_switcher.ts`:
  - `kind: 'ticket'`, matching key, title, status and assignee.
  - Tickets rank below sessions, except on an exact key match, which ranks
    first.
  - Sections: *My work*, then *Current sprint* (only when present), then
    recent.
  - Pasting a URL or typing an unknown exact key runs a `lookup` row.
  - **Enter** jumps to the live session, or opens NewSessionDialog prefilled
    (project, branch, name, and a "Brief Claude with the ticket" checkbox
    with an editable preview).
  - **⌘↵** runs `start` with the defaults and skips the dialog.
- **Status everywhere:**
  - The work chip and the group header show the title and a status-category
    dot.
  - The tooltip gives the status name, assignee and "synced 4 min ago".
  - An item with `unavailable_at` shows as struck-through grey with the
    reason.
  - Stale data (last sync older than twice the interval) shows a small
    clock.
- **Retro-link reveal:** after the first sync, a toast "14 sessions mention
  ABC-* · Review" opens the sidebar in Group-by-Work.
- **Tests:**
  - quick_switcher ranking and the paste path;
  - the NewSessionDialog prefill and preview;
  - chip status rendering, unavailable and stale;
  - the Settings Work section in both modes (standalone and hub client);
  - `hub_verdicts.test.ts` entries.

### M3.6: docs and roadmap

- `docs/hub.md` gains a *Trackers* section: the CLI, docker secrets
  (`file:/run/secrets/jira`), token expiry, and the migrate-from-desktop
  note: re-enter or rotate the token.
- `docs/concepts.md` gets a short *Work* paragraph.
- Update the roadmap status and Revisions.

## Acceptance (manual, real Jira Cloud site)

1. **Connect with only a URL, email and token.** The tracker shows `ok`. ⌘K
   lists My work within one sync interval. No project, org or board had to be
   configured.
2. **Retro-bind:** a session whose branch was `abc-123-fix` before connecting
   shows the ticket's title and status after the first sync.
3. **Start and duplicate:**
   - Enter on a ticket with no session opens the prefilled dialog; creating it
     links with `started`.
   - Enter on a ticket that has a live session jumps to that session.
4. **Status and unavailability:** moving the ticket to Done in Jira updates the
   chip within one interval. Removing your access marks the ticket
   unavailable, and its links stay.
5. **Failure modes:**
   - A revoked token gives `auth_failed`, a banner, and polling stops.
   - With the network down everything still works from the cache, and the
     stale clock shows.
6. **Isolation:**
   - An in-session Claude on host A asking `work { tickets }` gets only its own
     linked items.
   - `work_admin` from a paired desktop is refused with the "configure on the
     hub" sentence.
7. **Secrets:** grep the logs, diagnostics, error reports and metrics for the
   token after a failing sync. It is never there.

## Risks

| Risk | Mitigation |
|---|---|
| First outbound HTTP client in fleet-core | Reuse the audited TLS stack (A); no redirects; size cap; timeout; allowlist |
| A token in plaintext SQLite | `tracker_secrets` never on a read path; env / file refs for the hub; documented; keychain later for standalone |
| Jira API drift (the search API already moved once) | The adapter is isolated behind the trait; fixtures document the contract; `caps` let the UI degrade |
| Sync noise flooding the replay ring | Events only on a real change; linked items refreshed by id, not whole projects |
| A key collision between Jira and Linear, or two Jira sites | `recognize` only against probed prefixes; an ambiguous key is never auto-bound |
| An older hub or phone | UI gated on the hub's tool list; new wire enums have `Unknown` |

## Decisions needed (from the roadmap, restated for M3)

| # | Question | Default if unanswered |
|---|---|---|
| D6 | Jira Data Center needed? | No: Cloud only in M3 |
| D4 | Host-token isolation before orgs? | **Yes**, via decision 6 (own-host linked items only) |
| D3 | Any write-back in M3? | No: read-only |
| new | Default views? | `mine` + `sprint` (where present) + favourites |
| new | Can an in-session agent `lookup` tickets it is not linked to? | No in M3; revisit with M5 orgs |

## Revisions

- **2026-09-24, M3.0 — transport: option A (lift the existing client).**
  `fleet-core/src/net/` now holds the hand-rolled HTTP/1.1 client the desktop
  used to reach a hub: `http1` (head parsing, chunked decoding, moved from
  `src-tauri/src/backend/http1.rs` with its tests), `tls` (the cached
  `tokio-rustls` + ring + `rustls-native-certs` connector, moved from
  `backend/remote.rs` with its two source-guard tests), `conn` (connect and
  one `Connection: close` exchange, which `remote.rs` now delegates to) and
  `https` (the `HttpTransport` trait, `DirectTransport`, `FakeTransport`).
  **Why A:** it adds **zero** packages to `Cargo.lock` — both crates were
  already in the tree through `src-tauri`, `fleet-hub` and `fleet-agent`, so
  `Cargo.lock` gains two dependency edges and nothing else, and `cargo deny
  check` stays `advisories ok, bans ok, licenses ok, sources ok`. B was not
  trialled: `reqwest` is in the lock only behind a Tauri target that is not
  compiled here, so B would add its client half (`hyper-util` client,
  `hyper-rustls`, `tower`/`tower-http` bits, `ipnet`, …) for no capability A
  lacks. `DirectTransport` refuses plaintext and any host its policy does not
  allow before connecting, never follows a redirect (a 3xx is returned),
  caps the body (4 MiB) and bounds the whole exchange (20 s; 5 s connect and
  handshake). The hub image already installs `ca-certificates`
  (`crates/fleet-hub/Dockerfile`), which `rustls-native-certs` reads.
- **2026-09-24, M3 landed** (commits 7260779 … d147523 on
  `claude/cloud-fleet-work-graph-m3`). Deviations from the tasks above, and
  why:
  1. **Order.** M3.1's store, secrets and hardening landed first (b44a5e4);
     its admin surface (`work_admin`, the CLI, the desktop commands) landed
     after M3.2 (98debf2), because `test` is a probe and needs the adapter.
  2. **Incremental windows are relative** (`updated >= -Nm`, rounded up,
     from the watermark minus the 2-minute overlap) instead of a watermark
     formatted in `/myself`'s timezone: Jira evaluates a relative window in
     its own clock and the API user's zone, so no timezone database enters
     the tree (C27 allows either). `tz` is still recorded. The residual risk
     is a local clock running more than the overlap behind Jira's; the
     hourly whole listing of every view bounds it.
  3. **Views are evaluated locally for reads.** An incremental listing never
     says an item *left* a view, so `tickets { view }` evaluates `mine`
     (assignee is the API account, not done), `sprint` (in an active sprint)
     and `recent` (14 days) from the cached attributes; a favourite filter
     uses membership recorded in `meta.views`, exact after each hourly
     whole listing. `tracker_views` gained an `enabled` column so a 403
     disables one view persistently.
  4. **Sprints per project** are found with one bounded probe search
     (`sprint in openSprints()`, the projects of its results) rather than a
     board lookup per project. The sprint field is found by
     `schema.custom`, never by name.
  5. **Credential precedence:** the reference wins while it reads; the
     stored value is the fallback when it cannot be read (the plan's "ref
     first, then the stored value").
  6. **`work_admin` actions** are `list | add | update | set_credential |
     test | remove`; the plan's `*_tracker` spellings are accepted as
     aliases. `remove` keeps the items (and their `tracker_id`, which
     AUTOINCREMENT never reuses) marked unavailable (`tracker_removed`).
  7. **A newly set credential is `unconfigured`** until a `test`: the sync
     does not poll it (nor `auth_failed` / `captcha`). The Settings Connect
     flow tests right away; the CLI prints the `test` line.
  8. **Events.** `work:item`, `work:tracker` and `work:tracker_removed`,
     only on a real change; a tracker's first sync is announced once (the
     retro-link reveal). `last_sync_at` otherwise moves without a frame, so
     the desktop refreshes trackers every two minutes. Status reaches the
     sidebar through `SessionRow.work` (`status_category`, `status_name`,
     `url`, `unavailable`; all skipped when absent) and `session:updated`
     for the sessions whose primary work changed — no separate item store.
  9. **`start`** queues the ticket's context (title, status, URL, branch,
     and the description inside `mark_untrusted` / `UNTRUSTED_END`) as the
     `handover` row; M2's journal-built handover stays with `resume`. It
     also takes the dialog's edited `name` and `worktree`. A key no tracker
     knows still starts (trackers never gate).
  10. **Linking a key resolves to a tracker item** (the one tracker owning
      it, by key or alias) before a local item, keeping `ref_key` on the
      link for history; an existing link to the same item is reused rather
      than duplicated.
  11. **Tool budget:** +891 B (`work_admin`), +754 B (tickets / lookup /
      start parameters), +147 B (`name` / `worktree`), each measured and
      recorded at `BUDGET_BYTES`.
  12. **Not done in M3:** `/metrics` has no tracker series yet (logs carry
      only `tracker_id` and the result state); the chip's stale clock uses
      the tracker that owns the key's prefix; an unbound key chip explains
      where to connect Jira in its tooltip rather than with an inline
      button; the manual acceptance on a real Jira Cloud site.

