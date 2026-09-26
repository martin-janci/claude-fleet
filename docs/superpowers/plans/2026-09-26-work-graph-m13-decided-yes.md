# Work graph M13: the decided-against list, built (plan)

**Date:** 2026-09-26
**Roadmap:** `../2026-09-24-work-graph-roadmap.md` (M13 is new; the D3, D10, D13
and D20 rows point here)
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

### M13.1: Summaries of dead sessions (D10)
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

### M13.2: Write-back, PR remote link only (D3)
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

### M13.3: Webhook nudges (D13)
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

### M13.4: Naming work on the phone (D20), in fleet-mobile
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
| D24 | Summary model and quota | a small model · the session's configured model | `haiku` (`work.summary_model`), on the session's own account |
| D25 | Webhook providers in M13 | GitHub, Jira Cloud, Linear · also Asana, Jira DC | GitHub, Jira Cloud, Linear |
| D26 | Write-back ops in M13 | PR remote link · also transition on start · also worklog | PR remote link only |
| D27 | Automatic summaries at session end | off · on | off: on demand only |

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
