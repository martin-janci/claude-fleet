# Work graph: the decided-against list, revisited (M12.6)

**Date:** 2026-09-26
**Plan:** `../plans/2026-09-25-work-graph-m12-ship-and-operate.md` → M12.6, design decision 4
**Roadmap:** `../2026-09-24-work-graph-roadmap.md` → *Decisions still open*

> This note records evidence and recommendations. **The user decides.**
> Nothing here is built in M12. Any decision that turns to "yes" becomes its
> own milestone (M13+). The decisions table is the user's to update.

## The evidence available, stated plainly

M12.6 was meant to follow the user's M10.3 acceptance run and some real use.
At `main` a87b30d (2026-09-26), neither has been recorded:

- **No acceptance run.** M10.3's document (`docs/work-graph-acceptance.md`)
  has not been written, and the roadmap's *Revisions* record no run. Every
  milestone's status still ends with "the manual acceptance … is still to do".
- **No usage data.** Fleet collects no product telemetry, and nothing in this
  repository records how the work graph was used: which trackers were
  connected, how often suggestions were confirmed, how often resume or
  handover ran.
- **What does exist:**
  - automated tests, including the provider conformance suite and the
    isolation matrix;
  - the M12.2 scale numbers;
  - the replay-ring measurement (`2026-09-25-replay-ring-pressure.md`);
  - the M10.2 e2e leg against a fake Jira.

  These show that what was built works as designed and within budget. They
  cannot show that anyone *missed* the things decided against.

So each section below says what the built system now makes cheaper or
riskier, and names the evidence that would change the recommendation. **The
recommendation for every item is to keep the decision until M10.3 and a few
weeks of use are recorded.** Then re-read this note against that record.

## D3: write-back to trackers

**Decided (2026-09-25):** none. M9.5 is planned, not built.

- **What the build shows since.**
  - Every provider sits behind `TrackerProvider` and `HttpTransport`, so the
    planned `write(&WriteOp)` is one method per provider.
  - Credentials are read only by `Store::resolve_tracker_credential`.
  - Hosts are fenced per provider.
  - M12.4's health roll-up would now surface a failing write path: a
    tracker in `auth_failed` raises a *Reconnect* item.
  - The tokens users are told to create are still full-account API tokens.
    Jira Cloud API tokens and Asana/Linear personal tokens carry the user's
    whole write permission, and fleet cannot narrow them.
- **What use would show.** Whether people move tickets to *In progress* by
  hand right after *Start work*, and whether they paste PR links into tickets.
  Neither is measured today.
- **Smallest safe version.** Only *transition on start*:
  - per tracker, opt-in (`write_back.transition_on_start`), master-set through
    `work_admin` with a confirm;
  - only on a person's `work_link start`, only when the item is in `todo`;
  - the transition chosen by **target status category** `in_progress`, never
    by name, and skipped with a note when none or several match;
  - through an outbox, so a retry or a repeated trigger is a no-op;
  - never from detection, sync, a suggestion, the operator or a per-host
    token.

  The PR remote link comes next if wanted. It is idempotent by `globalId` on
  Jira, but has no clean equivalent on Asana or Linear. Worklog stays out.
- **Risk.**
  - Fleet becomes able to change a company's tracker. A wrong or duplicate
    transition is visible to that whole team.
  - Any bug in scoping becomes a write, not a leak.
  - Workflows with required fields on transition fail in ways fleet cannot
    fix, so the outbox then needs a user-visible failure state.
  - The read-only guarantee is also what makes sharing a token with fleet an
    easy decision.
- **Recommendation: keep "none".** Revisit only if the acceptance run or use
  shows the manual transition after *Start work* is a real, repeated chore.
  If so, build only transition-on-start, Jira first, as M13.

## D10: summaries of dead sessions

**Decided (2026-09-25):** off. M9.4 is planned, not built.

- **What the build shows since.**
  - The handover brief is deterministic and already carries what is known
    without a model: the last `turn_done` entries, Claude Code's own
    compaction summary, the outcome (branch, head, ahead, PR, diff stat).
  - M9.3 added an **agent-written handover on demand**, which covers the case
    where a person wants prose while the session is still alive.
  - M12.3 bounds the journal (365 days), so a summary row would be swept like
    any other.
  - M11.2 made resume probe whether the transcript still exists. A summary
    matters most exactly when it does not, and by then the
    `claude -p --resume --fork-session` source is gone too.
- **What use would show.** How often a resume or brief lands on work with
  no compaction summary and too few `turn_done` lines to be useful. The
  acceptance run's resume steps would show this.
- **Smallest safe version.**
  - `work.summaries` off by default, per org later.
  - Only for sessions with a **confirmed** link, run once at end, one per
    host at a time, time-capped and output-capped.
  - A small model on the session's own account, the fork with fleet's hooks
    disabled, the output stored as a `summary` journal row fenced like any
    agent text.
- **Risk.**
  - It spends the user's quota without a person asking.
  - It runs `claude` on a host after the session is gone, when nobody is
    watching it.
  - It feeds model-written text into later briefs, as memory that looks
    authoritative.
  - It adds a second path that must never touch the original transcript.
- **Recommendation: keep "off".** On-demand handover (M9.3) serves the same
  need with a person in the loop. Revisit if resumed briefs are found thin in
  practice *and* the transcript still exists at that point often enough to
  summarise.

## D13: webhook nudges

**Decided (2026-09-25):** no. M9.8 is planned, not built.

- **What the build shows since.**
  - Polling costs little and is bounded:
    - `work.sync_interval_secs` is 300 s by default;
    - views sync incrementally with a 2-minute overlap;
    - linked items refresh by id (≤ 500 per pass);
    - Asana uses sync tokens.
  - The replay-ring report measured the first-sync flood and recommended
    accepting it (D18). A steady poll emits frames only on a real change.
  - M12.4 shows sync health in `fleet_health`, so a stalled poll is visible.
  - Most hubs are reached through a tunnel or a private network. A public
    `hub.public_url` is the minority setup that webhooks would need.
- **What use would show.** Whether five minutes of staleness on a status
  change is ever noticed: a chip still *In progress* after the ticket moved,
  or a tidy suggestion arriving late.
- **Smallest safe version.** Tried first without any endpoint:
  - a lower `work.sync_interval_secs` for that hub;
  - a "sync now" button (`work_admin`, master) for the rare moment it
    matters.

  If a webhook is still wanted, `POST /hooks/tracker/<id>` only on a hub with
  a public URL:
  - HMAC per tracker, size-capped and rate-limited;
  - the payload used only to pick one item for a targeted `fetch_one`;
  - nothing stored from the payload itself.
- **Risk.**
  - The first internet-facing, unauthenticated-by-token endpoint on the hub.
  - Per-provider signature schemes to get right.
  - A secret to rotate per tracker.
  - A burst from a bulk edit to coalesce.
  - All this for a freshness gain the poll interval already controls.
- **Recommendation: keep "no".** Offer "sync now" and a shorter interval
  first if staleness is ever reported.

## D15 / D20: the phone

**Decided:** D15, the phone gets Today and the ticket card read-only (no
handover or multi-start actions). D20, no local work items on the phone
(name or rename).

- **What the build shows since. The wording "read-only phone" no longer
  matches what is planned and landed.**
  - M8 (fleet-mobile #32) already gives a **full** token Confirm / *Not
    this* on suggestions, and *Start here* / *Resume* from the Tickets sheet.
  - M8.6.3 adds *Ask for a handover* (`work_link handover`) for a full token.
  - A readonly token sees none of it: the hub hides `work_link` from its
    `tools/list`, and the phone gates on that.

  In practice the line D15 draws is therefore **"no flows that need a
  desktop-sized form"**: multi-repo start, the editable brief, org and
  tracker administration, local-item naming (D20). It is not "no writes".
  - Where M8.6 stands on fleet-mobile's `main` is not visible from this
    repository. The roadmap records it on
    `claude/cloud-fleet-work-graph-m8-6`.
- **What use would show.**
  - Whether people try to name or rename work from the phone (D20).
  - Whether they start work on several repos away from a desk (D15).
  - The acceptance run's phone step (M10.3 item 5) is where this would first
    show.
- **Smallest safe version.**
  - **D20**, *name this work* on the phone: one text field, calling the
    existing `work_link { action: "name" }` from M11.1 (the desktop's
    `name_session_work` and `rename_work_item` both route to it), for a
    full token only. No new tool, no action, no contract bump.
  - **D15**, multi-start: a checkbox list of the projects the key ran in
    before (M9.6's own list) under *Start here*. The editable brief stays on
    the desktop.
- **Risk.**
  - Low for D20: the action exists, is tested, and is gated by token mode.
    Typos in names on a phone keyboard are the main cost.
  - Moderate for multi-start: several sessions on several hosts from one tap,
    with less context on screen to check the host and projects.
- **Recommendation.**
  - **D15: keep, and reword it** in the decisions table to "no desktop-sized
    flows on the phone (multi-start, the editable brief, administration)",
    so it matches what M8 and M8.6 ship.
  - **D20: the cheapest candidate on this list to turn to yes**, if the
    phone step of the acceptance run shows people want it.

## D17: `acli` as a Jira transport

**Decided (M11):** against. REST covers it.

- **What the build shows since.**
  - Every case `acli` was meant for is covered without it:
    - Jira Cloud and Data Center over REST (M3, M6.5);
    - a Jira reachable only from one machine through `via_host` (M6.3):
      `curl` on that host, the token on stdin, never in argv;
    - GitHub, including Enterprise Server (M11.4), through `gh`.
  - `acli`'s JSON output would add a second Jira parser to keep in step, and
    a per-version fixture set (the M6 plan's risk table).
- **What use would show.** A Jira that neither REST nor `via_host` curl can
  reach, for example one behind SSO that blocks API tokens while `acli`'s
  own OAuth login works. No such site is known.
- **Smallest safe version.** `transport = via_cli:<host>` with
  `cli = acli`:
  - `acli jira workitem search --jql … --json --paginate` on the host, under
    `acli`'s own login;
  - no token in fleet, like `gh`;
  - read-only;
  - a minimum version checked in the probe.
- **Risk.** A third Jira path, including its own normalisation, fixtures and
  failure modes, for no known user.
- **Recommendation: keep "against"**, unless a real site turns up that only
  `acli` can read.

## D16: correction (not a revisit)

The roadmap listed D16, *run `hub-e2e` in GitHub CI*, with the default
"local opt-in, as today". That was already out of date when M12 was
planned:

- `scripts/hub-e2e.sh` runs in CI in the `hub-headless` job
  (`.github/workflows/ci.yml`) on every pull request and every push to
  `main`, with its logs uploaded on failure.
- Only its work-graph leg, *hub W* (M10.2), still runs locally only. That leg
  needs a `fleet-hub` built with the test-only `e2e` feature (`WBIN`), and
  CI's plain build skips it by design. CI still runs its first check: the
  plain `fleet-hub` refuses the fake-tracker override.
- `scripts/ci-local.sh --hub-e2e` builds the `e2e` hub and runs the whole
  script, hub W included.

The roadmap's D16 row now says this. Whether the CI job should also build
the `e2e` hub and run hub W is a new, smaller question. It costs one more
`fleet-hub` build per run, and the fake tracker must never reach a release
build (already pinned by that first check). It is left to the user.

## Summary for the decisions table

| # | Now | Recommendation | Evidence that would change it |
|---|---|---|---|
| D3 | none | keep | the acceptance run or use shows the manual *In progress* move after *Start work* is a repeated chore → M13: transition-on-start only, Jira first |
| D10 | off | keep | resumed briefs found thin while the transcript still exists |
| D13 | no | keep; offer "sync now" / a shorter interval first | reported staleness a 60–120 s poll does not fix |
| D15 | read-only M9 on the phone | keep, **reword** to "no desktop-sized flows on the phone" | people start multi-repo work away from a desk |
| D20 | no | the cheapest to turn to yes (existing action, full token only) | the acceptance run's phone step shows people want to name work there |
| D17 | against | keep | a Jira only `acli` can read |
| D16 | (stale) "local opt-in" | corrected: in CI since before M12; only hub W is local | — (new question: build the `e2e` hub in CI?) |
