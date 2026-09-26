# Work graph: the decided-against list, revisited (M12.6)

**Date:** 2026-09-26
**Plan:** `../plans/2026-09-25-work-graph-m12-ship-and-operate.md` §M12.6 and
design decision 4 ("Revisiting a decision needs evidence")
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`, *Decisions still open*
**Verified at:** `main` f9401e6 (after #303, #304, #305)

This note does not change any decision. It collects, per decided-against
item, what the repository can show today, and recommends. **The user
decides.** Each "yes" becomes its own milestone (M13+), not part of M12.

## The caveat that governs every row

**The M10.3 acceptance run has not happened.** `docs/work-graph-acceptance.md`
does not exist, and neither the roadmap's nor the M10 plan's *Revisions*
record a user run. Design decision 4 asks for "what usage showed". Nothing in
this repository records real usage of the work graph. This note therefore
cites only what the repository holds:

- the M10.2 end-to-end scenarios (`scripts/hub-e2e.sh`, hub W);
- the M10.6 report (`2026-09-25-replay-ring-pressure.md`);
- the code on `main`;
- decisions and changes made after the original decision (GHES, retention,
  `fleet_health` trackers, local work items, the phone's M8.6 / M10.5).

It invents no usage figure. Every section is marked **waits on M10.3**. The
last section says what that run should measure so the next revisit can
cite it.

## Summary

| Decision | Current state | Recommendation | What the user needs to do |
|---|---|---|---|
| **D3** write-back to trackers | Decided none (2026-09-25). `TrackerProvider` has no write method; M9.5 is planned, not built. | **Stays decided against.** Revisit after M10.3 only for the PR remote link. | Nothing now. After M10.3, say whether keeping the ticket in step by hand was friction. |
| **D10** dead-session summaries | Decided off (2026-09-25). M9.4 planned, not built. The deterministic brief and on-demand agent handover (D9) are landed. | **Decide after M10.3.** | In M10.3, judge whether resume briefs of dead sessions were enough. Name a model and whose quota. |
| **D13** inbound webhooks | Decided no (2026-09-25). M9.8 planned, not built. Polling every 300 s; the M10.6 ring numbers are fine. | **Stays decided against.** | Nothing, unless the hub gets a public URL and 5-minute staleness hurts in M10.3. |
| **D15** handover and multi-start on the phone | Default "read-only (Today + card)". But M8.6.3 built *Ask for a handover* in fleet-mobile, and the hub serves `handover` and multi-start to any full token. | **Decide after M10.3.** First, correct the D15 row: handover is on the phone already. Multi-start stays desktop-only. | Confirm which fleet-mobile release carries M8.6.3, then restate D15 as "multi-start on the phone?". |
| **D20** naming local work on the phone | Default no. The hub already serves `work_link { name }` to full client tokens (M11.1, isolation row). Only the app lacks it. | **Decide after M10.3.** It is cheap if wanted: phone-only, no hub change. | In M10.3, say whether you wanted to name work away from the desk. |
| **D17** `acli` as a Jira transport | Default decided against. M6 did not build it; `via_host` (curl) and `gh` cover the cases. | **Stays decided against.** | Nothing, unless an org forbids API tokens and mandates `acli`. |
| **D16** hub-e2e in CI | Default "local opt-in" was stale. | **Done** (corrected in the roadmap). | Nothing. |

---

## D3: write-back to trackers

### 1. The original decision and why

- **Question** (roadmap *Decisions still open*): write-back to trackers —
  none · transition on start · plus a PR remote link · plus worklog.
- **Decided 2026-09-25: none.** M9.5 stays planned, not built (roadmap D3
  row; M9 plan *Revisions*, "2026-09-25, decisions": "the user … kept D3 at
  none").
- **Why:** the M9 plan's table rates M9.5 as the only item that "writes to
  Jira / other trackers". Its facts: "The tracker layer is read-only:
  `TrackerProvider` (M3.2) has no write method, and the Jira token's scopes
  are whatever the user pasted." Its goal line: "without writing to
  anybody's tracker unless the user turned it on". The user did not turn it
  on.

### 2. What use has shown (**waits on M10.3**)

No usage evidence exists. What the repository shows:

- **Code.** `TrackerProvider` (`crates/fleet-core/src/service/trackers/mod.rs`)
  still has only `caps`, `probe`, `views`, `list`, `fetch`, `recognize` and
  `changes`. The guide says plainly: "Fleet writes nothing back to a tracker
  (decision D3)" (`docs/work-graph.md` → *Trackers*).
- **M10.2.** The fake Jira (`scripts/e2e-fake-jira.py`) serves only read
  endpoints. It moves a ticket's status by a test control call. The e2e
  proves the read path (sync, tickets, start, tidy on done) end to end. It
  says nothing about the need for writes.
- **Changes since the decision that bear on it:**
  - **M6 / M11.4.** GitHub and GHES go through `gh` on a host with the
    host's own login (`net/via_host.rs`). A write through `gh` would act as
    that login, with that login's full scopes. Fleet cannot narrow it.
  - **M12.4.** `fleet_health.trackers` now reports `auth_failed` and
    repeated failures, and a *Reconnect* Attention item. A write-back
    failure would have somewhere to surface. Until now only reads failed.
  - **M12.3.** Retention protects rows by reference. A write outbox would be
    a new table that needs its own protection rule and window.

### 3. The smallest safe version, if built

- **Scope.** Only the **PR remote link** (the second option), and only for
  Jira Cloud / DC. It is the most mechanical write:
  - idempotent by `globalId = "fleet:pr:<url>"` (Jira upserts by it; the
    review's §5 row and the M9.5 plan);
  - it changes no workflow state and records no time.

  Transition on start and worklog stay out: a transition depends on
  workflow design, and a worklog on how time is counted.
- **Modules.**
  - `TrackerProvider::write(&WriteOp)`, default `Unsupported`;
  - `jira_common.rs` for the one call;
  - an outbox table (a new migration) so a failed write retries and a
    repeated trigger is a no-op;
  - a `write_back` journal row per write;
  - `work_admin` for the per-tracker opt-in.
- **Isolation and org boundary.**
  - Every write is refused for per-host tokens (the M9.5 plan says so).
  - A write only goes to the tracker of the link's own org
    (`work_links.snap_org_id`), never across orgs, even with
    `force_cross_org`.
  - The isolation matrix needs a row per caller.
- **Operator confirmations (M9.7).** Any operator-initiated write is gated
  like a kill. It is refused on a hub, where there is no approver.
- **`BUDGET_BYTES`.** One `work_admin` setting field, estimated well under
  100 B. Measure and record it.
- **Contract.** No `CONTRACT_REVISION` bump. The write is triggered by the
  sync / PR probe, not by a new tool.
- **Phone.** Nothing. The opt-in is administration and stays off the phone.

### 4. Risks

- **Security.**
  - A write needs a token with write scope. Today a read-only token is
    enough, and the guide says to paste one.
  - For GitHub through `gh`, the scope cannot be narrowed at all.
  - Third-party text is not written. Only fleet's own URL and title are,
    but the title still goes to someone else's system.
- **Isolation.** A write to the wrong org's tracker is a cross-company
  leak of a PR URL. It needs the org check above.
- **Tracker quotas.** Low: one call per PR, idempotent. Retries must honour
  `Retry-After` (the conformance suite's row 9 already covers 429 on reads).
- **Spam.** Remote links are visible to the whole team on the ticket. A
  mis-linked session (a guess) must never write. Only `manual` / `started`
  links, never `suggested` or `agent_inferred`.

### 5. Recommendation: **stays decided against**

Nothing in the repository shows a need. The cost is a new class of side
effect: writes to third-party systems, write-scoped tokens, and an outbox
with its own retention. After M10.3, revisit **only** the PR remote link,
and only if keeping tickets in step by hand was real friction.

---

## D10: summaries of dead sessions

### 1. The original decision and why

- **Question** (roadmap): summarise dead sessions with
  `claude -p --fork-session` (M9.4)? Which model?
- **Decided 2026-09-25: off.** M9.4 stays planned, not built (roadmap D10
  row; M9 plan *Revisions*, "2026-09-25, decisions").
- **Why:**
  - The specialist review §6 lists it under "Deferred on purpose": "LLM
    summarisation of dead sessions … Opt-in, later."
  - The M9 plan's table names the side effect: "runs `claude -p` on a host
    (tokens, a model call)".
  - The M9 plan's D10 row asks "Which model, whose quota?".

### 2. What use has shown (**waits on M10.3**)

No usage evidence exists. What the repository shows:

- **What covers the need today.**
  - The deterministic built brief (≤ 4,000 characters) from the journal,
    snapshots and a git probe.
  - Claude Code's own `compact_summary` journal rows (047), harvested
    before the cascade delete (M2).
  - The on-demand agent-written handover (M9.3, D9), which only works on a
    **live** session. A dead session has only the built brief. That is
    exactly the gap D10 was about.
- **M10.2.** Scenario 5 proves the handover path with a scripted fake
  Claude. It also found and fixed a bug in the sibling marker scan: the
  safe-kill prompt's own echo was read as the reply. #302 then fixed a
  wrapped `FAILED` echo. Reading agent output is fragile. M9.4 as planned
  reads `claude -p`'s stdout, not a pane, so it avoids that class of bug.
- **M11.2.** Resume now probes whether the transcript still exists. A
  summary would need the same probe (`--resume <id>` on a missing file
  fails).
- **M12.3.** Retention sweeps the journal after `work.retention.journal_days`
  (365). A summary row would be swept under the same rules.

### 3. The smallest safe version, if built

- **Scope.**
  - On demand only, the same shape as D9: a *Summarise* button on a past
    work entry. No automatic run at session end.
  - One summary per conversation.
  - Stored as a journal `note` from `source = agent`, or as a new kind.
    `kind` has no CHECK constraint in migration 047 (the values are
    enforced in Rust), so no migration is needed.
- **Modules.**
  - `service/work/` gets a new `summary.rs`;
  - `ssh.rs` for `claude -p --resume <id> --fork-session --model <m>
    --settings '{"hooks":{}}'`, every value `shq`-quoted;
  - a per-host `FleetTasks` queue with a timeout and an output cap;
  - the transcript probe from M11.2;
  - `build_context` puts the summary behind the agent handover, fenced
    (it may quote tool output).
- **Isolation and org boundary.**
  - A per-host token may ask only for its own host's past work, inside its
    org (like `handover`).
  - The summary runs on the session's own host, under its own account.
  - The isolation matrix needs a row.
- **Operator confirmations (M9.7).** It spends a model call, so it counts
  as a side effect: the operator's request is confirm-gated and refused on
  a hub.
- **`BUDGET_BYTES`.** One `work_link` action (≈ 20–40 B, measured then).
- **Contract.** No bump; an action is served from the parser table.
- **Phone.** A full token could call it like `handover`. Whether the app
  shows it is D15's question.
- **Settings.** `work.summary_model` (default a small model). It goes in the
  guide's settings table (`work_settings_are_in_the_user_guide`).

### 4. Risks

- **Security.**
  - The forked run must not execute tools. The M9.4 plan disables fleet's
    hooks but says nothing of tool permissions. A minimal version must run
    with no tools allowed, and a test must prove it.
  - The transcript may hold secrets. The summary is stored in fleet's
    journal and reaches later briefs. Run it through `logging::redact` and
    fence it.
- **Isolation.** A summary of org A's transcript must never reach org B's
  brief. The journal is keyed by work, and the per-host fence already
  scopes briefs.
- **Tracker quotas.** None. **Model quota:** a model call per click, on the
  session's own account.
- **Spam.** Low while it runs on demand only. An automatic mode would spend
  tokens on every dead session.

### 5. Recommendation: **decide after M10.3**

The gap is real on paper: a dead session gets no agent-written hand-off.
Whether the built brief is enough is a usage question. M10.3 should record
whether resuming a dead session's work lacked context. If it did, build the
on-demand version above as M13.x. The automatic version stays off.

---

## D13: inbound webhooks

### 1. The original decision and why

- **Question** (roadmap): expose an inbound webhook endpoint on a public hub
  (M9.8)? Options: no (poll) · yes (HMAC, targeted fetch only).
- **Decided 2026-09-25: no.** M9.8 stays planned, not built.
- **Why:**
  - The specialist review §6: "Webhooks: poll in v1. Later, an optional
    webhook only *nudges* a targeted fetch, and its payload is never
    trusted."
  - The M9 plan's table: "an inbound, internet-facing endpoint".
  - Its facts: "nothing inbound is unauthenticated today".

### 2. What use has shown (**waits on M10.3**)

No usage evidence exists. What the repository shows:

- **M10.6.** At the default 300 s interval with two 200-item boards, the
  replay ring reaches back 10–11.7 minutes after a sync burst, with no
  no-op item frames. Polling costs the ring little. A webhook would not
  reduce frames; it would only move them earlier.
- **The sync.** `work.sync_interval_secs` (300 s, `0` = off). Each pass
  reads from a watermark with a 2-minute overlap. The worst staleness is
  therefore about one interval. It is a setting, and a user who wants
  fresher data can lower it without an internet-facing endpoint.
- **Inbound routes (`mcp/mod.rs`).** Only `/healthz` and `/pair` are outside
  `authorize` today. `/pair` is guarded by a single-use code, constant-time
  comparison and a per-address rate limit. A webhook would be the third
  such route, and the first that a third party calls.
- **M12.4.** `fleet_health.trackers` reports sync failures. A webhook
  failure (a bad signature, a burst) would need its own row there.

### 3. The smallest safe version, if built

- **Scope.**
  - `POST /hooks/tracker/<tracker_id>`, only when `hub.public_url` is set
    and the tracker has a webhook secret.
  - HMAC-verified, size-capped, rate-limited, outside `authorize` like
    `/pair`.
  - The payload is read only for an item id/key, which triggers
    `fetch_one` (the targeted fetch).
  - Polling stays: the webhook only nudges.
- **Modules.**
  - `mcp/mod.rs` (the route);
  - a new `mcp/tracker_hook.rs`;
  - a secret stored like tracker secrets (read only by a sibling of
    `Store::resolve_tracker_credential`);
  - `service/trackers/sync.rs` for a coalesced targeted fetch;
  - `fleet-hub tracker webhook` to mint the secret.
- **Isolation and org boundary.** The route is per tracker. The fetched item
  is stored under that tracker's org exactly as a poll would store it. No
  caller identity is involved.
- **Operator confirmations (M9.7).** Not applicable: no session is started
  or killed.
- **`BUDGET_BYTES`.** None if it is administered only by `fleet-hub`. One
  `work_admin` field if the desktop shows it.
- **Contract.** No bump.
- **Phone.** Nothing.

### 4. Risks

- **Security.**
  - An internet-facing, unauthenticated-by-token endpoint on a daemon that
    can start sessions on the user's machines.
  - The HMAC secret is one more credential to store and rotate.
  - Jira Cloud's admin-registered webhooks need site-admin rights, and
    per-provider signature schemes differ (Jira, GitHub, Linear, Asana), so
    each is a separate, tested verifier.
- **Isolation.** Low if the payload is never trusted. A key outside the
  tracker must be ignored (the M9.8 plan's test).
- **Tracker quotas.** A burst of events must coalesce into one fetch.
  Otherwise a webhook storm turns into an API storm.
- **Spam.** A forged or replayed request can at most trigger fetches. The
  rate limit bounds that.

### 5. Recommendation: **stays decided against**

The M10.6 numbers show that polling is cheap, and the interval is already a
setting. The cost is a new internet-facing surface. Revisit only if M10.3
shows that ~5-minute staleness hurt a real flow, **and** the hub has a
public URL.

---

## D15 / D20: the phone stays read-only / no naming on the phone

### 1. The original decision and why

- **D15** (M10 plan *Decisions*; roadmap D15 row): "Handover and
  multi-start on the phone (M10.5)?" Default: read-only only (Today +
  card). Design decision 3 of M10: "The phone reads; it does not become a
  second desktop … Handover and multi-start stay desktop-only unless D15
  says otherwise."
- **D20** (M11 plan *Decisions*): "Local work items on the phone (name /
  rename)?" Default: "no, consistent with D15". M11.1: "Phone: out of scope,
  consistent with D15 (the phone stays read-only)."
- Neither was put to the user as a decision in its own right. Both are
  defaults.

### 2. What use has shown (**waits on M10.3**)

No usage evidence exists. What the repository shows, and it contradicts
the D15 row:

- **The hub does not enforce "phone read-only".** It gates by token mode,
  not by device. A full client token is served `work_link`. The isolation
  matrix (`mcp/tools/tests_isolation.rs`) asserts that a full client
  reaches:
  - `handover` (a send attempt, the same as master);
  - multi-start (`project_ids`);
  - `name` (M11.1: "everyone but the readonly client").

  The guide says the same: "the hub gates each action by the token, not by
  the app".
- **M8.6.3 built *Ask for a handover* on the phone.** The M8 plan's
  *Revisions* ("2026-09-25, M8.6 built" on fleet-mobile's
  `claude/cloud-fleet-work-graph-m8-6`) list the button, its gates (full
  token, the hub lists the action, a primary key, the session running) and
  the timeline states. The guide lists as desktop-only: "multi-start (D15),
  naming or renaming local work (D20), tracker and org administration, and
  retention". Handover is not on that list.

  So the D15 row ("read-only only") already understates the phone. Whether
  the M8.6 branch is on fleet-mobile's `main` cannot be checked from this
  repository.
- **M10.5** (fleet-mobile #36, per the M11 plan's *Depends on*) gave the
  phone Today and the ticket card read-only, with *Copy* instead of
  *Insert*.
- The phone already starts and resumes work (*Start here*, M8) with a full
  token. Those are session-creating actions. Multi-start is the same action
  with `project_ids`.

### 3. The smallest safe version, if built

- **Scope.**
  - **D20:** "Name this work…" on the phone's work sheet for a session with
    no work, plus rename in the Tickets sheet. Full token only.
  - **D15:** multi-start on the phone as a multi-select of the projects the
    hub offers (the desktop's *Also start in*).
  - Both are fleet-mobile only.
- **Modules.** fleet-mobile's `SessionWorkViewModel` / `TicketsViewModel`,
  gated on `HubCapabilities` listing `name` / `project_ids`.
  `ToolsTheAppMayCallTest` stays `work` and `work_link`. No hub change.
- **Isolation and org boundary.** Unchanged. Client tokens are
  `OrgScope::All` (M8 plan survey item 1), and the per-project cross-org
  refusal and `force_cross_org` apply as on the desktop. The phone must show
  the cross-org refusal in words and never retry with `force_cross_org`
  silently.
- **Operator confirmations (M9.7).** Not applicable: the phone is a person,
  not the operator. `mcp.confirm_destructive` does not gate starts.
- **`BUDGET_BYTES`.** No change.
- **Contract.** No `MAX_HUB_CONTRACT` / `CONTRACT_REVISION` change. The
  phone gates on the action enum.
- **Phone.** This *is* the phone work, in fleet-mobile.

### 4. Risks

- **Security.** A lost phone with a full token can already start sessions.
  Multi-start multiplies that by the number of repos (the Lifecycle cap
  bounds it). The phone should confirm with the project list shown.
- **Isolation.** None new at the hub. On the phone, the org label must be
  visible when picking projects across orgs.
- **Tracker quotas.** None (a start reads the cache).
- **Spam.** Accidental multi-starts from a small screen. A confirm sheet
  with the count mitigates it.

### 5. Recommendation: **decide after M10.3**

- **First, a correction for the user to make:** D15 as written ("handover
  and multi-start: read-only only") no longer matches what was built.
  Handover is on the phone (M8.6.3) for full tokens. D15 should be restated
  as "multi-start on the phone?", with handover recorded as built.
- **D20** is cheap (phone only, no hub change). Build it as M13.x if M10.3's
  phone step shows the need.
- **Multi-start on the phone** stays desktop-only unless M10.3 shows a real
  case. Picking projects is awkward on a small screen, and a single start
  already exists there.

---

## D17: `acli` as a Jira transport

### 1. The original decision and why

- **Question** (M11 plan *Decisions*): "Support `acli` (Atlassian CLI) as a
  Jira transport?" Default: "decided against: REST covers it".
- **Origin.** The specialist review §5 adopted "`ViaHost` transport
  (curl/acli/gh on a host)". The M6 plan §M6.3 planned an `acli` CLI variant
  ("the token then lives in the CLI's own store, **never in fleet**").
- **M6 *Revisions* item 1:** "`acli` … was not built: `gh` is the one
  trusted CLI; a Jira only a VPN host reaches uses `via_host` (curl)."
- **M11.4:** "decided against by default (D17): `gh` and the REST providers
  cover the use."

### 2. What use has shown (**waits on M10.3**)

No usage evidence exists. What the repository shows:

- **Code.** `net/via_host.rs` has `GhCliTransport` and `CurlTransport` only.
  `net/https.rs`'s module comment still says "A `ViaHost` transport (curl /
  `gh` / `acli` on a host) arrives with M6". That is stale: `acli` never
  arrived. It is left as is, since this milestone is docs only; fix it with
  the next code change there.
- **The case `acli` was for** is covered two ways:
  - a Jira reachable only from a VPN host → `via_host` (curl, token on stdin
    into a `umask 077` temp dir, never in argv);
  - Jira Data Center → M6.5 with its admin fence and `extra_ca`.

  The only uncovered case is an organisation that forbids API tokens outside
  `acli`'s own store.
- **M11.4 (GHES)** shows the cost of a CLI transport: a hostname fence, a
  URL allowlist, and version-dependent JSON. The M6 plan's risk table
  already names "A CLI's JSON output changes (`gh`, `acli`)".

### 3. The smallest safe version, if built

- **Scope.** `transport = via_cli:<alias>` with `cli = acli`, **reads
  only**: `acli jira workitem search --jql … --json --paginate`, plus a
  fetch by key. Jira Cloud only.
- **Modules.**
  - `net/via_host.rs` (an `AcliTransport`). It cannot be an
    `HttpTransport`, because `acli` is not HTTP, so the provider needs a
    non-HTTP seam. That is a larger change than `gh api`, which *is* HTTP.
  - `jira.rs` mapping `acli`'s JSON to the same `Fetched` / `Page`.
  - The conformance suite (`conformance_suite!`) and
    `tests_isolation_providers.rs`.
- **Isolation and org boundary.** Unchanged: the tracker row carries the
  org; the host alias is admin-set.
- **Operator confirmations (M9.7).** Not applicable.
- **`BUDGET_BYTES`.** One more `transport` value in `work_admin`'s
  description (a few bytes).
- **Contract.** No bump.
- **Phone.** Nothing.

### 4. Risks

- **Security.** `acli` runs with whatever the host user logged in as. Every
  argument must be `shq`-quoted, and JQL must come only from fleet's own
  view definitions.
- **Isolation.** None new.
- **Tracker quotas.** Same as REST, but `acli`'s own paging and retry are
  opaque, so a 429 cannot be read as `rate_limited` with `Retry-After`.
- **Spam.** None.
- **Maintenance.** A pinned CLI version and per-version fixtures, for no
  reach the REST provider and `via_host` lack.

### 5. Recommendation: **stays decided against**

REST plus `via_host` covers every case the repository knows of. `acli`
would need a non-HTTP seam in the provider layer. Revisit only if an
organisation forbids API tokens outside `acli`.

---

## D16: hub-e2e in CI (correction, not a revisit)

The roadmap's D16 row read "Local opt-in, as today". That was stale when
M10 was planned:

- `.github/workflows/ci.yml` runs on every pull request and every push to
  `main`.
- Its `hub-headless` job (line 94) builds `fleet-hub` and `fleet-agent`.
- It then runs `bash scripts/hub-e2e.sh` (the step *Run hub end-to-end
  script*, lines 113–118, 5-minute timeout), and uploads the logs on
  failure.

The roadmap row now reads **done**. One precision: the M10.2 work-graph leg
(hub W) needs a `fleet-hub` built with the test-only `e2e` feature (`WBIN`).
CI builds none, so that leg prints `SKIP` there. It runs with
`scripts/ci-local.sh --hub-e2e`. Adding an `e2e`-feature build to the job
would be a separate change, not a decision.

---

## What M10.3 should capture

To make the next revisit evidence-based, the acceptance run (M10.3) should
record, besides pass / fail:

- **D3.** How often, during the run, a ticket's status or PR had to be
  updated by hand in the tracker after a fleet start or PR, and whether
  that felt like friction.
- **D10.** For each resume of a **dead** session: was the built brief
  enough to continue, or was the transcript opened by hand? Note one
  example of what was missing.
- **D13.** The longest wait between a status change in the tracker and fleet
  showing it (expected: at most one `work.sync_interval_secs` plus the pass
  time). Did any step have to wait on it? Is the hub reachable at a public
  URL at all?
- **D15 / D20.**
  - Which fleet-mobile release the phone runs, and whether *Ask for a
    handover* is there (M8.6.3).
  - Whether any step made you want to start in several repos, or name a
    piece of work, from the phone.
- **D17.** Whether any organisation in use forbids API tokens (which would
  bring `acli` back).
- **Tracker health.** `fleet_health.trackers` and `work_admin { status }`
  (`fleet-hub tracker status`) after the run: failures, `rate_limited`
  passes, and sync duration. These are the quota baseline any write-back
  or webhook would add to.

## Revisions

- 2026-09-26: first version (M12.6). No decision changed. D16 corrected in
  the roadmap.
