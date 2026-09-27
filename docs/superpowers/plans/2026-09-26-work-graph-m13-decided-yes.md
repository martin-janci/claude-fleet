# Work graph M13.4: the decision-gated builds the user said yes to (build notes)

**Date:** 2026-09-26
**Roadmap:** `../2026-09-24-work-graph-roadmap.md` (the D3, D10, D13 and D20
rows, and D27–D30 for the defaults below)
**Plan:** `2026-09-26-work-graph-m13-live-use.md` §M13.4. That plan reached
`main` while this one was written on a branch, under the same milestone
number. This file is now its build notes: M13.1 here is **M13.4c**, M13.2
is **M13.4e**, M13.3 is **M13.4f** (new) and M13.4 is **M13.4a**. The
decision numbers D24–D27 below are **D27–D30** on `main` (D24–D26 are the
live-use plan's).
**Review:** `../reviews/2026-09-26-work-graph-decisions-revisited.md` (M12.6),
which gives each item's smallest safe version, modules and risks. This plan
builds exactly those versions and nothing larger.
**Depends on:** M0–M12 on `main` (`21ac1174`).

## Goal

> On 2026-09-26 the user said **yes** to four items M12.6 had left decided
> against: D10 (dead-session summaries), D3 (write-back), D13 (inbound
> webhooks) and D20 (naming work on the phone). M13 builds each in its
> smallest safe form: on demand, opt-in, off by default, fenced by org and
> by token, and never trusting third-party text.

**Value line:**
- A dead session's past work gets a Claude-written summary on request, and
  the next brief shows it.
- A PR opened in a session started on PAY-9 appears as a link on the Jira
  ticket, once, without anyone pasting it.
- A ticket moved in Jira reaches the sidebar in seconds on a public hub,
  while polling stays the source of truth.
- A phone with a full token can name the work of a session that has none.

## Facts this plan builds on (verified 2026-09-26 at `main` 21ac1174)

- **Migrations** run through **060** (`060_auth_epoch.sql`). M13's first new
  migration is **061**.
- **`TrackerProvider`** (`service/trackers/mod.rs`) has `caps`, `probe`,
  `views`, `list`, `fetch`, `recognize` and `changes`. It has no write method.
- **Targeted fetch.** `service/trackers/sync.rs::fetch_one` exists and is what
  a webhook nudge calls.
- **Agent handover (M9.3, D9)** works only on a live, idle REPL
  (`service/work/agent_handover.rs`). A dead session has only the built brief.
- **Redaction** is `logging::redact`; untrusted text is fenced with
  `fence_untrusted`.
- **Inbound routes outside `authorize`** are `/healthz` and `/pair` only.
  `hub.public_url` is `service::hub::SETTING_PUBLIC_URL`.
- **Tracker secrets** are read only by `Store::resolve_tracker_credential`.
- **The phone** (fleet-mobile) is a separate repository. The hub already
  serves `work_link { name }` to a full client token (M11.1, isolation row).
- **Guard tests that every slice must keep green:**
  - the isolation matrix (`mcp/tools/tests_isolation.rs`), which needs a row
    for each new action;
  - `BUDGET_BYTES` (`mcp/tools/tests.rs`), set to measured + 100;
  - `work_settings_are_in_the_user_guide`: every `work.*` setting is in
    `docs/work-graph.md`'s table;
  - the generated control-API reference and hub verdicts.

## Design decisions

1. **On demand or opt-in, never automatic by default.** Summaries run on a
   click. Write-back and webhooks are per tracker and off until an admin turns
   them on.
2. **Third-party text is never trusted.** A summary may quote tool output: it
   is redacted, stored as `agent` text and fenced wherever it is shown. A
   webhook payload is read only for an item id or key. Everything stored comes
   from the tracker API, exactly as on a poll.
3. **Writes are the narrowest possible.** Only a PR remote link, only to Jira
   (Cloud and Data Center), idempotent by `globalId`, and only from a link a
   person made (`manual` or `started`), never a guess.
4. **Org and token fences as everywhere else.** A per-host token cannot
   trigger a write or a webhook secret, and may summarise only its own host's
   past work in its own org. A write goes only to the tracker of the link's
   own org, never across orgs.
5. **Every new surface reports in `fleet_health`.** Write-back failures and
   rejected webhook deliveries are counted per tracker in the M12.4 roll-up.

## Milestones

### M13.4c (was M13.1): Summaries of dead sessions (D10)
- **Action.** `work_link { action: summarize, session_id | conversation_id }`,
  on demand only. There is one summary per conversation; asking again replaces
  it.
- **The run.** On the session's own host, under its own account:

  `claude -p --resume <id> --fork-session --no-session-persistence
  --model <m> --settings '{"hooks":{}}' --tools '' --strict-mcp-config
  "<fixed summary prompt>"`

  Checked against Claude Code 2.1.283's `--help`:
  - `--tools ""` disables every built-in tool;
  - `--strict-mcp-config` without `--mcp-config` loads no MCP server, so no
    MCP tool either;
  - `--no-session-persistence` means the fork leaves no transcript behind.

  A test pins these flags.

  - Every value is `shq`-quoted.
  - The fork never writes to the original transcript.
  - Fleet's hooks are disabled, so the run cannot journal or deliver into
    itself.
  - **No tools**: a test pins the flags.
  - It runs one per host at a time, with a timeout (120 s) and an output cap.
  - It reads `claude -p`'s stdout, never a pane (M10.2 showed pane reading is
    fragile).
- **Before the run**, the M11.2 transcript probe runs: a missing transcript
  gives `E_NOTFOUND`, not a failed model call.
- **Storage.** A work journal `summary` row from `source = agent`. It goes
  through `logging::redact`, is capped at 4,000 characters and fenced in
  every brief. It needs no migration, because 047's `kind` has no CHECK
  constraint. Retention (M12.3) sweeps it like any journal row.
- **Brief.** `build_context` shows the newest summary after the agent
  handover, fenced.
- **Setting.** `work.summary_model`, default `haiku` (decision D24). It is
  added to the guide's settings table.
- **Fences.**
  - A per-host token may summarise only its own host's past work, in its org.
  - The operator's request is confirm-gated like a kill (M9.7) and refused on
    a hub.
  - The isolation matrix gets a row.
- **UI.** A *Summarise* button on a past work entry, with a spinner and the
  error in words.
- **Tests.**
  - the command string (quoting, the no-tools flags, the fork);
  - the queue and timeout through `FakeSsh`;
  - a missing transcript;
  - redaction and the cap;
  - the brief's fence;
  - isolation;
  - Vitest for the button.

### M13.4e (was M13.2): Write-back, PR remote link only (D3)
- **Trait.** `TrackerProvider::write(&WriteOp) -> Result<WriteOutcome,
  TrackerError>`, with a default of `Unsupported`. `WriteOp::PrRemoteLink {
  item, url, title }` is its only variant.
- **Jira.** Cloud and Data Center implement it in `jira_common.rs`:
  - `POST /rest/api/{2,3}/issue/{key}/remotelink` with `globalId =
    "fleet:pr:<url>"`, which Jira upserts, so it is idempotent by
    construction;
  - the title is fleet's own: `PR: <repo>#<n>`;
  - the conformance suite gets a write row, including 429 with
    `Retry-After`.
- **Trigger.** A live link in `manual` or `started` state whose session gains
  a `pr_url` (the PR probe). Never `suggested` or `agent_inferred`.
- **Outbox.** Migration **061**, `tracker_writes`:
  - columns: `(id, tracker_id, item_id, op, payload, state, attempts,
    last_error, next_at, created_at, done_at)`, unique on `(tracker_id,
    op, payload)`, so a repeated trigger is a no-op;
  - the sync tick drains it with backoff; after 5 failed attempts the row is
    marked `failed`;
  - retention (M12.3) sweeps `done` rows after the journal window.
- **Journal.** One `write_back` row per write that lands.
- **Opt-in.** Per tracker, `trackers.settings.write_back.pr_remote_link`
  (false). It is set through `work_admin { action: set_write_back }`, which is
  master-only and confirm-gated for the operator. Settings → Work shows it
  with the warning that the token needs write scope.
- **Fences.**
  - Every write is refused for per-host tokens.
  - A write goes only to the tracker of the link's `snap_org_id`, even with
    `force_cross_org`.
  - The isolation matrix gets a row per caller.
- **Health.** `fleet_health.trackers[].write_failures`, additive.
- **Tests.**
  - FakeTransport for the write;
  - `globalId` idempotency;
  - outbox retry and give-up;
  - the setting gate;
  - no write from a guessed link;
  - the cross-org refusal;
  - host tokens refused.

### M13.4f (was M13.3): Webhook nudges (D13)
- **Route.** `POST /hooks/tracker/<tracker_id>`:
  - It is served only when `hub.public_url` is set and the tracker has a
    webhook secret. Otherwise the answer is 404, so the route is invisible.
  - It sits outside `authorize` like `/pair`, has a 64 KiB body cap, and is
    rate-limited per tracker and per address.
- **Verifiers**, one per provider, each separately tested (decision D25):
  - GitHub: `X-Hub-Signature-256`, HMAC-SHA256;
  - Jira Cloud: `X-Hub-Signature`, HMAC-SHA256;
  - Linear: `Linear-Signature`, HMAC-SHA256 hex, plus its timestamp window.

  Asana (handshake) and Jira Data Center are not in M13. Comparison is
  constant-time. A bad or missing signature gives 401 and fetches nothing.
- **Payload.** Read only for an item id or key, which must belong to that
  tracker; anything else is ignored. The item goes to `fetch_one`. A burst
  within 5 s coalesces into one fetch per item. Polling stays unchanged.
- **Secret.** Stored like tracker secrets and read only by a sibling of
  `resolve_tracker_credential` (`resolve_tracker_webhook_secret`). Minted by
  `fleet-hub tracker webhook <id> [--rotate]`, which prints the URL and the
  secret once. There is no MCP tool, so `BUDGET_BYTES` is unchanged.
- **Health.** `fleet_health.trackers[].webhook` reports `{ enabled,
  last_delivery_at, rejected }`.
- **Tests.**
  - each verifier (good, bad, missing, replayed outside Linear's window);
  - a key outside the tracker is ignored;
  - a burst makes one fetch;
  - 404 without a public URL or secret;
  - the body cap;
  - the rate limit;
  - an e2e scenario in `scripts/hub-e2e.sh` with the fake Jira.

### M13.4a (was M13.4): Naming work on the phone (D20), in fleet-mobile
- **App.** "Name this work…" on the phone's work sheet for a session with no
  work, and rename in the Tickets sheet.
  - Full token only.
  - Gated on the hub listing `name` in `work_link`'s action enum
    (`HubCapabilities`).
- **Hub.** No change: the action and its isolation row exist (M11.1).
- **Errors.** The cross-org refusal is shown in words and never retried with
  `force_cross_org`.
- **Scope.** D15 (multi-start on the phone) is **not** in M13. It stays
  desktop-only unless decided separately.
- **Tests.** The fleet-mobile ViewModel tests; `ToolsTheAppMayCallTest` is
  unchanged (`work`, `work_link`).

## Order and parallelism
```
M13.1 ── independent (hub + desktop)
M13.2 ── independent; adds migration 061
M13.3 ── after M13.2 if both touch fleet_health.trackers (merge order only)
M13.4 ── independent (fleet-mobile repository)
```

## Acceptance (manual)
- Summarise a session killed yesterday. The next resume's brief shows the
  summary, fenced. The original transcript is unchanged.
- Turn on write-back for a test Jira, start PAY-9, open a PR. One remote
  link appears on the ticket; re-probing adds no second link.
- On a public test hub, register the webhook in Jira and move a ticket. The
  sidebar changes within seconds. A forged request with a wrong signature is
  refused and shows in `fleet_health`.
- Name a session's work from the phone.

## Risks
- **A forked run executing tools.** Mitigation: `--tools ''` and
  `--strict-mcp-config`, a test that pins them, and no hooks.
- **Write scope.** Write-back needs a token with write scope where a
  read-only one sufficed. It is opt-in per tracker, and Settings says so.
- **An internet-facing route.** Mitigations: 404 unless enabled per tracker,
  HMAC, a body cap, a rate limit, an untrusted payload and a coalesced fetch.
  At worst a forged request causes fetches.
- **Wrong-org writes.** Mitigation: the `snap_org_id` check, with no
  `force_cross_org` escape.

## Decisions (defaults if unanswered)

| # | Question | Options | Default |
|---|---|---|---|
| D27 (was D24) | Summary model and quota | a small model · the session's configured model | `haiku` (`work.summary_model`), on the session's own account |
| D28 (was D25) | Webhook providers in M13 | GitHub, Jira Cloud, Linear · also Asana, Jira DC | GitHub, Jira Cloud, Linear |
| D29 (was D26) | Write-back ops in M13 | PR remote link · also transition on start · also worklog | PR remote link only |
| D30 (was D27) | Automatic summaries at session end | off · on | off: on demand only |

## Revisions
- 2026-09-26: first version, after the user said yes to D3, D10, D13 and D20.
- 2026-09-26: **M13.1 built** on `claude/cloud-fleet-work-graph-m13`.
  - `work_link { action: summarize, key, link_id }` addresses the ended link,
    like `resume`: a dead session has no row left.
  - `--resume` finds a transcript by the directory it ran in, so the host
    script finds the transcript (M11.2's `transcript_candidates`), reads its
    recorded `cwd`, and runs there. The M11.2 probe is folded into that one
    command: an absent transcript is `E_NO_TRANSCRIPT`, a missing directory
    `E_NOTFOUND`, and nothing is recreated.
  - One per host at a time is an in-process slot (`E_EXISTS`), not a
    `FleetTasks` queue: there is no such per-host queue in the tree, and a
    click that waits minutes behind another is worse than a clear refusal.
  - `work.summary_model` is a `Choice` of `haiku` / `sonnet` / `opus`, never
    free text: it ends up in a command.
  - The desktop's *Summarise* sits beside *Resume* on every past-work row
    (`SummarizeButton.svelte`), shows the answer as text below the row, and is
    Routed (`summarize_past_work`).
  - The tool surface is +125 B (56,854); `BUDGET_BYTES` is 56,954.
- 2026-09-27: renumbered to fit `main`'s M13 (live use): these are its
  M13.4c, e, f and a, and the defaults are D27–D30. M13.4f is new in that
  plan.
- 2026-09-27: **M13.4e built** on `claude/cloud-fleet-work-graph-m13`.
  - `TrackerProvider::write(&WriteOp)`, default `Refused`; Jira Cloud (v3)
    and Data Center (v2) send `POST …/issue/{key}/remotelink` with
    `globalId = "fleet:pr:<url>"` and `caps.write = true`. A key that is not
    a Jira key never reaches the path.
  - Opt-in is a field of the tracker's existing `settings`
    (`write_back.pr_remote_link`), validated as Jira-only, not a new
    `work_admin` action: the same master-only `update` sets it, and the tool
    surface does not grow.
  - The trigger is the PR probe's `set_pr_signals` branch in reconcile
    (`write_back::on_pr`), after the links resolved. An older `gh` that
    reports no signals triggers nothing.
  - The outbox is `tracker_writes` (migration 061): unique on `(tracker_id,
    op, item_key, url)`, drained by the sync pass after a read pass that
    worked, 20 per pass. A rate limit waits and spends no attempt; a 403,
    404 or invalid answer gives up at once; anything else backs off (1 min
    doubling to 6 h) for five attempts. The setting and the org are checked
    again before sending.
  - Retention: settled rows go after `work.retention.journal_days`, in the
    drain, not as a fourth retention table (no new setting, no UI change).
  - `fleet_health.trackers[].write_failures` (additive; never part of
    `health`) and the footer's "N writes not sent".
  - Settings → Work: a checkbox per Jira tracker.
- 2026-09-27: **M13.4f built** on `claude/cloud-fleet-work-graph-m13`.
  - Verifiers checked against the providers' docs: GitHub
    `X-Hub-Signature-256: sha256=<hex>`, Jira Cloud `X-Hub-Signature:
    sha256=<hex>` (admin and REST webhooks with a secret, since 2024),
    Linear `Linear-Signature: <hex>` plus `webhookTimestamp` within 60 s. HMAC
    from the `hmac` 0.12 crate (the only new dependency), checked against
    RFC 4231; compared with the existing `constant_time_eq`.
  - Narrower than planned: a delivery refreshes only an item fleet already
    has for that tracker (`tracker_item_for_key_in`), so a forged payload
    can at most cause refetches of known items; new items still come by
    polling.
  - The route is mounted only with `McpGuards::with_tracker_hooks` (set by
    fleet-hub), not inferred from a public URL: the desktop runs the same
    server and even a tracker sync. It still answers 404 until the hub has
    a public URL and the tracker a secret.
  - The secret is its own table, `tracker_webhooks` (migration **062**; 061
    is the M13.4e outbox), with one reader and a grep guard like
    `tracker_secrets`, masked in diagnostics. `fleet-hub tracker webhook`
    writes the store directly, like `peer add`, so no tool reply ever
    carries it and the tool surface does not grow.
  - Coalescing is per server (the route's `Coalescer`), not process-global.
    Health fields are flat (`webhook_enabled`, `webhook_last_delivery_at`,
    `webhook_rejected`) rather than a nested object.
  - hub-e2e: a signed delivery refreshes E2E-3 from the fake Jira without a
    sync pass; a forged one is 401, an unknown tracker 404, and
    `fleet_health` reports it.
- 2026-09-27: **M13.4a built** in fleet-mobile, branch
  `claude/work-graph-m13-4a-name-work` (61b09a7). No hub change.
  - *Name this work…* is in the session menu when the session has no
    confirmed work; *Rename* is in the work sheet when the work is local
    (an item with no tracker status and no URL, the desktop's test).
  - Both are gated on a write token and the hub listing `work_link` `name`.
  - The title is checked on the phone in the hub's terms. `E_EXISTS` with
    `tracker: true` says to use *Set work…*.
  - The phone's list of local items (`work { local_items }`) is not shown:
    rename is offered where the work already is, the session's sheet.
  - `:shared:jvmTest` passes 1,006 / 1,006. The Android emulator and iOS
    jobs run only in fleet-mobile's CI.
- 2026-09-27: **M13.4c review fixes.**
  - The fork ran with `--settings '{"hooks":{}}'`, which does **not** turn
    fleet's hooks off: Claude Code keeps the user-level hooks
    (`~/.claude/settings.json`) under an empty `hooks` object. Checked with
    Claude Code 2.1.283 in an isolated HOME: SessionStart and
    UserPromptSubmit hooks fired with `{}` and with `{"hooks":{}}`, and did
    not fire with `{"disableAllHooks":true}`. So every summary reported its
    fork to fleet as a new conversation and prompt. It now passes
    `{"disableAllHooks":true}`, and the flag test refuses the empty object.
  - A test pins the operator's `summarize`: it gets a confirm nonce, runs once
    approved, is `E_FORBIDDEN` once denied, and is refused outright on a hub
    with no approver. A phone's own request is not gated. Removing the gate
    fails it.
  - Recorded deviation: the run's timeout is 170 s on the host (`timeout`)
    and 180 s for the SSH call, not the plan's 120 s. A cold `--resume` of a
    long transcript needs the margin; the cap on the output is unchanged.
- 2026-09-27: **M13.4e review fixes.**
  - Conformance row 11 (`c11_write`, every provider): only a `caps.write`
    provider writes. For Jira Cloud and DC, that is one POST to
    `/issue/{key}/remotelink` with the same `globalId` on a repeat and no
    secret in the body; 429 maps to `RateLimited` with its `Retry-After`,
    and 403 to `Forbidden`. A bad key sends nothing. Every other provider
    refuses without a request.
  - An isolation row: turning `write_back.pr_remote_link` on through
    `work_admin update` is the master's alone; every other caller gets
    `E_FORBIDDEN`.
  - The outbox's retention moved from the Jira drain to the M12.3 GC
    sweep. The drain only ran after a successful pass, so a failing
    tracker's outbox never shrank. It is capped per tick, one batch per
    lock, by the journal's window, and counted as `tracker_writes` in the
    sweep record (`fleet-hub tracker status`, Settings → Retention).
    **Deviation kept**: failed rows go by the same window as done ones. A
    year-old failure is no longer a live signal for `write_failures`, and
    keeping it forever would be growth without bound. Pending rows never
    go.
  - Not changed, and left for the owner: `pr_title` accepts a PR URL on
    any `https` host, because the URL comes from `gh` on the session's
    host. Limiting it to github.com plus the configured GHES hostnames
    would need a small new rule.
