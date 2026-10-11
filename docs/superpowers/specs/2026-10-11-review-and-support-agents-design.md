# Reviewer and Support agents: design

Status: design only, nothing built (2026-10-11).
Asked for by the owner: bring the functionality of
[`claude-code-reviewer`](https://github.com/martin-janci/claude-code-reviewer)
and [`support-agent`](https://github.com/martin-janci/support-agent) into
fleet as agents, as a port and a merge rather than two services running
beside it. Polling only for now; inbound webhooks (D13) stay no.

## The problem

Two Node services do Claude work outside fleet:

- **claude-code-reviewer** watches the pull requests of a list of repos,
  runs `claude -p` with `Read,Grep,Glob` in a worktree of the PR head, and
  posts a GitHub review with inline Conventional Comments.
- **support-agent** (a fork of `papayapos/agent-seed`, which was itself cut
  out of the reviewer) takes Jira L2S tickets ("Tech help!", `/analyze`,
  assignment to the bot user), enriches them (Jira, tenant lookup, Graylog,
  Countly, a knowledge base), runs `claude` on a clone of the backend repo,
  and comments the investigation back on the ticket, creating PD bugs.

Each carries its own copy of the same skeleton: a poller and a webhook
server, a JSON or SQLite state store, retries and backoff, a rate limiter,
a token budget, a dashboard on its own port with its own bearer token,
Prometheus metrics, a `claude` wrapper. Fleet already has every one of
those, once, with a person-facing UI, a phone, an org model and a kill
switch. What fleet does not have is the domain logic: deciding when a PR
needs a review, turning a diff into anchored inline comments, turning a
ticket into an investigation.

Reading both repos also showed what their copies of the skeleton never
finished, and the port fixes these by construction instead of carrying
them over:

- support-agent's autonomy engine (`shouldAutoExecute`) is never called:
  PD bugs and comments are created without consulting it;
- its approval queue is always empty (`createPendingAction` has no
  caller), and approving an action executes nothing;
- its content guardrails (`allowedWriteProjects`, `maxTicketBodyLength`,
  the per-ticket and per-hour limits) are parsed and never enforced;
  `dryRun` only switches the store to `:memory:` and still writes to Jira;
- its loop guard (idempotency, cooldown) lives in memory and is lost on
  restart;
- `mode: polling` starts nothing: no poller subclass exists;
- the reviewer's `/review` and test-debate triggers work only through the
  webhook; a polling deployment never sees them.

## What it does

Three layers, built in that order.

| Layer | Where | What |
|---|---|---|
| **A. Agent runtime** | `crates/fleet-core/src/service/agents/` | Rules, pollers, jobs, the per-subject loop guard, a durable action queue a person answers, the run vehicle. Generic; knows nothing about GitHub reviews or Jira tickets. |
| **B. Reviewer** | `service/agents/review/` | Port of the reviewer's gating, diff handling, verdict and posting logic. Generic for any GitHub repo, so it lives in fleet-core. |
| **C. Support** | `service/agents/support/` (thin) + the `support-agent` repo as MCP servers and catalog assets | The Jira trigger, the investigation run and its write-backs in fleet; everything PapayaPOS-specific (Graylog, Countly, tenant resolver, knowledge base, Confluence) stays in the `support-agent` repo as MCP servers installed through the catalog. |

The rule that holds across all three: **the model proposes, fleet acts.**
A run reads (code, logs, tickets) and returns structured JSON; it never
holds a GitHub or Jira write credential. Posting a review, commenting on a
ticket or creating a bug is done by fleet's executor after the action
passes the rule's level, the guardrails and, where the level asks, a
person. This is what makes the autonomy and approval story real instead
of a config section nothing reads.

## A. Agent runtime

### Rules, not modes

As with the PR shepherd: nothing runs without a person's rule, and only a
person writes one (`fleet-hub agents grant | revoke | pause-all | status`,
plus an `agents` control-API tool served only to the owner's own paired
device). A master-token agent cannot grant itself a rule.

`agent_rules` (one migration, number taken from `origin/main` at the time,
see the `migration-conflicts` skill):

| column | meaning |
|---|---|
| `agent` | `review` \| `support` |
| `scope` | `gh:<owner>/<repo>` for review, `jira:<tracker_id>:<project_key>` for support |
| `level` | `watch` \| `propose` \| `act` (below) |
| `host_alias`, `project_id` | where runs happen (the project's checkout gives the clone and the worktree root) |
| `config_json` | the agent's per-scope options (below); validated on write |
| `expires_at`, `granted_by`, `created_at` | as `pr_shepherd_rules` |

| level | review | support |
|---|---|---|
| `watch` | runs the review, stores it, shows it in fleet; posts nothing | runs the investigation, stores it; writes nothing to Jira |
| `propose` | queues the review as an action a person approves before it is posted | queues each Jira write as an action |
| `act` | posts the review | posts the investigation comment; creating a bug or a follow-up ticket still goes through the queue unless the rule's `auto` list names it |

`watch` is the safe first grant and the equivalent of the old `dryRun`,
except that it really writes nothing.

### Pollers

One pausable loop, `agents_poll` (`acts(...)` in `service::loops::LOOPS`,
gated in `service/tick.rs` like `pr_shepherd`), single-flight, one poll
pass per rule per `agents.poll_interval_secs` (settings `SPECS`, default
300, `0` = off). A poll pass only discovers work and writes jobs; it never
runs Claude.

- **review**: on the rule's host, `gh pr list --repo <o/r> --state open
  --json number,title,headRefOid,isDraft,baseRefName,headRefName,author
  --limit 1000`, plus, for PRs the store holds as open and the list does
  not, `gh pr view N --json state,mergedAt` (at most 5 per pass, as the
  reviewer did). Comments since the last pass
  (`gh api repos/<o/r>/issues/comments?since=<cursor>`) bring `/review`
  to polling. Every value goes through `shell::quote`.
  This is repo-wide, unlike reconcile's PR probe, which sees only PRs on
  fleet sessions' branches (`service/outcome.rs::build_pr_probe_script`);
  a PR the probe already recorded in `pull_requests` is joined by URL so
  the session view shows its review.
- **support**: through the existing Jira tracker client
  (`service/trackers/jira.rs`, its credentials in `tracker_secrets`), JQL
  `project = <KEY> AND updated >= <cursor>` with `fields=comment,assignee,
  status,updated`. The tracker sync does not fetch comments today
  (`fields()` has no `comment`); the support poller asks for them itself
  and does not widen the general sync.

Each rule keeps a cursor (`agent_cursors`: rule id, last poll time, last
comment id seen) so a restart neither replays nor skips.

### Jobs

`agent_jobs`: `id, rule_id, agent, subject` (`gh:o/r#12`,
`jira:L2S-123`), `trigger` (`opened`, `pushed`, `comment`, `assigned`,
`manual`, `reverify`), `dedup_key` (UNIQUE with `agent`), `state`
(`pending | running | done | failed | skipped | needs_person`), `attempts`,
`next_attempt_at`, `error_kind`, `result_json`, `cost_micros`,
`created_at`, `updated_at`.

- **Idempotency** is the unique `dedup_key`: `<subject>@<head sha>` for a
  review, `<issue>-<comment id>` or `<issue>-assigned-<updated>` for
  support. A second poll writing the same key is a no-op, across restarts.
- **Retry** follows the reviewer's error classes (`transient`, `permanent`,
  `rate_limit`, `spending_limit`, `overloaded`, `max_turns`): backoff
  `60 s · 2^(n-1)`, at most `agents.max_retries` (3); `permanent`, or
  `max_turns` twice on the same subject state, stops at once. A rate or
  spending limit sets `next_attempt_at` from the account, not the job.
- A second pausable loop, `agents_run`, takes due jobs, at most
  `agents.max_concurrent` (3) at a time, at most one per subject (the
  reviewer's per-PR mutex, now a store lease like `take_routine_lease`).
- Every job is a row in `runs` (new kinds `review` and `support`, source
  `agent`) and its cost goes to `aux_usage`, so the fleet daily budget
  (`automation.daily_budget`), the account pause line (`accounts.pause_at`)
  and Pause all (`automation.paused`) stop it like any routine.

### Loop guard

Generic, in the runtime, durable (it reads `agent_jobs`, not memory):

1. never act on our own output: a comment from the identity the rule
   writes as, or one carrying the agent's marker
   (`<!-- fleet-agent:review -->`, and for Jira the visible heading the
   support agent writes, defined once as a constant shared by the writer
   and the guard, as support-agent's `BOT_OUTPUT_MARKER` was);
2. per-subject cooldown (`agents.subject_cooldown_secs`, 300);
3. per-subject daily cap (`maxJobsPerTicket` 3 for support; for review,
   one job per head sha plus forced re-reviews, capped at 5 a day);
4. per-rule hourly cap on writes of each kind (support-agent's
   `maxJiraTicketsCreatedPerHour` 10, enforced this time).

A job refused by the guard is recorded `skipped` with the reason; a poll
that finds nothing new writes nothing.

### Actions a person answers

The in-memory MCP confirm (`mcp/guard.rs::PendingConfirms`, 10-minute TTL)
is the wrong tool for an approval that may wait a day. A durable queue,
`agent_actions`:

| column | meaning |
|---|---|
| `job_id`, `kind` | `post_review`, `jira_comment`, `jira_create_bug`, `jira_link`, … |
| `payload_json` | exactly what will be sent, rendered, so a person approves the bytes, not a summary |
| `state` | `pending | approved | rejected | executed | failed | expired` |
| `decided_by`, `decided_at`, `executed_at`, `error` | |

- Approving **executes** the action through the same executor that `act`
  uses, then records `executed` or `failed`; an action whose subject moved
  on (a new head sha, a resolved ticket) expires instead.
- Answered with `agents { action: approve | reject, id }` from the owner's
  paired device, and shown in the Inbox as a new source (`src/lib/inbox.ts`
  derives the Inbox client-side; it gains a list read from the hub).
- Reminders and escalation (support-agent's 240 / 1440 min) are out of
  scope for v1; the Inbox item is the reminder.

### The run vehicle

Both agents run a locked-down `claude -p` on the rule's host, built from
`service/claude_print.rs`, not an interactive session: the reviewer and the
support agent were both one-shot print runs, and a print run has a cost,
a time cap and a parseable envelope.

`isolation_flags()` hard-codes `--tools ''` today. A new
`isolation_flags_with(tools: &[&str], mcp: Option<&McpConfigPath>)` keeps
`disableAllHooks`, `--strict-mcp-config` and `--no-session-persistence`
and adds:

- `--tools Read,Grep,Glob` for both agents (the reviewer's set);
- for support only, `--mcp-config <file>` listing the rule's MCP servers by
  their catalog install name, and `--allowedTools` limited to their
  read tools (`mcp__graylog__search`, …). The file is written by fleet
  from the catalog's resolved spec, with secrets substituted the way
  `catalog/sync/secrets.rs` does, into a `0600` temp file the run script
  removes.

The skill text goes in with `--append-system-prompt-file`, the job's input
on stdin, `--output-format json`, `--max-turns` and `--model` from the
rule's config. The existing test that asserts `--tools` appears once keeps
holding for each variant.

### Workspace for a run

Neither agent may run in a session's worktree. A run gets its own
detached worktree under the project's checkout on the rule's host:
`git fetch origin pull/<n>/head` (review) or `<branch>` (support, the
default branch) then `git worktree add --detach <root>/.fleet-agents/<job>
FETCH_HEAD`, removed when the job finishes, with a prune of anything older
than `agents.stale_worktree_mins` (60) on each `agents_run` pass. The
script is built like `service/repair.rs::BranchSource::Mirror` (fetch or
refuse); lifecycle's `worktree_add_script` never fetches and is not reused.

### UI

- `src/lib/automation.ts`: `BuiltInAgent.id` gains `'review' | 'support'`;
  each shows its loop state and today's jobs. `AutomationView.svelte`'s
  hard-coded `count: () => 3` becomes the array's length.
- An agent's page (the declarative pages catalog,
  `REGEN_PAGE_DOCS=1 … page_docs_are_current`): its rules, recent jobs with
  their result, pending actions. No separate dashboard, port or token.
- fleet-mobile: the pending actions in the Inbox (approve / reject); the
  rest later.
- Notifications to Slack or Teams: fleet has no outbound notifier
  (`src/lib/notifications.ts` names the hub-side matrix as step 11.9, not
  built). Out of scope here; the event bus and the phone carry it.

## B. Reviewer

A port of the reviewer's logic into `service/agents/review/`, with the TS
tests ported alongside (`decisions.test.ts`, `diff-parser.test.ts`,
`claude.test.ts`) as the first tests of each module.

### Subject state

`review_prs` (one row per `owner/repo#n` under a rule): `status`
(`pending_review | reviewing | reviewed | changes_pushed | error | skipped
| closed | merged`), `skip_reason` (`draft | wip_title | diff_too_large |
empty_diff`), `title, is_draft, head_sha, base_ref, head_ref,
last_reviewed_sha, last_push_at, consecutive_errors, last_error_json,
review_id, review_verified_at, skipped_at_sha, skip_diff_lines`; and
`review_records` (per posted review: sha, verdict, findings JSON, review
id), the last `agents.review.max_history` (20) kept.

### Gating (`plan.rs`, pure)

`evaluate_transitions` then `should_review`, in the reviewer's order:

1. stale `reviewing` → `pending_review`; `reviewed` with a new sha →
   `changes_pushed`; `skipped` cleared → `pending_review` when the draft or
   WIP condition is gone, or for `diff_too_large` / `empty_diff` when the
   head moved past `skipped_at_sha`;
2. `merged` / `closed` → never; `reviewing` → never;
3. draft (`skip_drafts`, true) or WIP title (`skip_wip`, true) → `skipped`.
   The port matches WIP as a word (`^\s*\[?wip\b`, case-insensitive), not
   the original's `startsWith("wip")`, which also skipped "wipe …";
4. `skipped` → only when forced; `reviewed` at the same sha → only when
   forced;
5. debounce: a push younger than `debounce_secs` (60) waits, unless forced;
6. error backoff as in *Jobs*;
7. otherwise `pending_review`, `changes_pushed`, `error` and `reviewed`
   with a new sha are due.

A `/review` comment (`comment_trigger`, default `^\s*/review\s*$`,
multi-line) forces a review, with the same overrides (`--max-turns=N`,
`--focus=a,b`); as before it does not override draft or WIP.

### Diff (`diff.rs`, pure)

- Fetched on the host with `gh pr diff`; an incremental re-review uses
  `gh api repos/o/r/compare/<last_reviewed>...<head>`. When GitHub refuses
  the diff, the local diff from the run's worktree, then a partial diff of
  the latest commits, as before.
- Exclusions: `.claudeignore` from the **base** branch, then the rule's
  `exclude_paths`, last match wins, `!` negations; the glob and
  `.claudeignore` semantics port 1:1 (`*`, `?`, `**/`, `**`, anchoring, the
  directory-only `/`, the `X` + `X/**` expansion).
- `filter_diff` before any size measurement; `count_diff_lines` on the
  filtered diff only; empty after filtering → `skipped:empty_diff`; over
  `max_diff_lines` (5000) → `skipped:diff_too_large`.
- `commentable_lines`: right-side lines of `+` and context lines per file;
  `snap_line` tries the exact line, then `±1..3` with `+d` first; praise
  and unsnappable findings go to the review body.

### Run and output

The skill (`code-review/skill.md`, moved into `catalog-seed/skills/` so
the same text is the catalog asset and the run's system prompt) and the
output schema are kept as they are: `verdict` (`APPROVE | REQUEST_CHANGES
| COMMENT`), `summary`, `findings[]` (`severity` issue / suggestion /
nitpick / question / praise, `path`, `line`, `body`, `blocking`,
`confidence`, `securityRelated`, `testRelated`, `isNew`), and the optional
`prSummary`, `testCoverage`, `resolutions`. Parsing keeps the three tiers
(whole text, a fenced block, the last balanced object). The freeform
legacy path (an issue comment with a regex verdict) is not ported: an
unparseable reply fails the job `permanent:unparseable` and shows the raw
text in fleet.

Run options from the rule: `max_turns` (15), `timeout_secs` (600),
`model` / `light_model` with `light_model_max_diff_lines`, and `max_turns`
escalating to the full model on a second `max_turns` failure on the same
sha, as before.

### Posting (`post.rs`)

- `gh api --method POST repos/o/r/pulls/<n>/reviews --input -` with
  `{body, event, commit_id: head_sha, comments: [{path, line, body}]}`.
- Event: `APPROVE` → APPROVE; `REQUEST_CHANGES` → REQUEST_CHANGES when
  `request_changes_event` (true), else COMMENT; anything else COMMENT. A
  422 "your own pull request" retries once as COMMENT.
- Verdict adjustments on a re-review (escalate when a previous blocking
  finding is unresolved, de-escalate COMMENT to APPROVE when every blocker
  is settled) and resolving the threads of resolved findings
  (`resolveReviewThread`) port as they are.
- The body and inline formats port as they are, with the marker renamed
  `<!-- fleet-agent:review -->`; the guard also recognises the old
  `<!-- claude-code-review -->` so the first fleet review of a PR the old
  service reviewed is an incremental one.
- The "review started" status comment is dropped: fleet shows the job.

### Verification

On the `agents_poll` pass, for `reviewed` rows not verified for
`verify_interval_mins` (60): `gh api repos/o/r/pulls/<n>/reviews/<id>`; a
404 or `DISMISSED` sends the row back to `pending_review`.

### Later (not v1)

Test-debate (the reply-thread judge), auto-label, auto-description,
autofix. Autofix is already fleet's: the PR shepherd's `nudge` and a
routine with `autonomy: push` cover it for PRs a fleet session owns.
The Jira link in the review body uses fleet's tracker link
(`work_items`) instead of the reviewer's own Jira client.

## C. Support

### What moves into fleet

- **Triggers**, by polling (A): a new comment containing one of the rule's
  `phrases` (default `Tech help!`, `/analyze`, `/investigate`,
  case-insensitive substring) in the rule's project; an issue newly
  assigned to the rule's `bot_account_id` (accountId, not display name,
  which the original compared). `jira-created` and `jira-resolved` are
  later; `jira-resolved` only fed the knowledge base.
- **Enrichment that is plain Jira**: the ticket, its last comments and its
  attachments (text types up to 1 MB inline, images up to 5 MB written into
  the run's workspace for `Read`), the incident window (last comment's time
  −60 / +5 min), read with the tracker's own credentials.
- **The investigation run**: the analyzer skill (`claude-analyzer/skill.md`
  with `log-bug-analyzer.md`) as a catalog skill, the input as one JSON
  document on stdin, the output schema unchanged (`rootCause`,
  `confidence`, `suggestedFix`, `diff`, `severity`, `affectedComponent`,
  `affectedVersion`, `stepsToReproduce`, `summary`, `needsMoreInfo`). The
  original ran `claude --print` with every default tool and 80 turns on a
  clone with a `GITHUB_TOKEN` in its remote URL; the port runs
  `Read,Grep,Glob` plus the rule's read-only MCP tools on a detached
  worktree of the project's checkout, with no token in the tree.
- **Fingerprint** (`sha256(exceptionClass | normalised message | first 5
  app frames)`, the app-frame prefix a rule option instead of
  `com.papayapos.`) and the per-fingerprint record, so a repeat is linked,
  not re-investigated, inside `incident_window_mins` (120).
- **Writes, as actions** (A):
  - `jira_comment`: the investigation comment (ADF; heading, root cause,
    details, suggested fix, diff, next steps linking to the fleet job), or
    the `needsMoreInfo` question;
  - `jira_create_bug` in the rule's `write_project` (PD), with a `Causes`
    link to the source ticket, when no open bug matches (the original's
    JQL + similarity score, ≥ 80 reuse, ≥ 60 follow-up);
  - `jira_link` to an existing bug.

  These are new `WriteOp` variants (`service/trackers/mod.rs`, which today
  has only `PrRemoteLink`) on the tracker outbox (`tracker_writes`), so a
  failed write is retried by the existing drain. A write outside the
  rule's `write_projects`, or a body over `max_body_chars` (5000), is
  refused by the executor, which is the guardrail the original never
  applied.
- **Autonomy** maps onto the rule level: at `act`, `jira_comment` and
  `jira_link` run; `jira_create_bug` runs only when listed in the rule's
  `auto` with its condition (`confidence: high` and a known fingerprint, as
  the original's defaults said), otherwise it is queued.

### What stays in the `support-agent` repo

Everything specific to PapayaPOS's systems, as MCP servers (TypeScript,
reusing the existing plugin code) and catalog assets:

| MCP server | from | read tools |
|---|---|---|
| `graylog` | `plugins/graylog`, `engines/scanner.ts` (Graylog part) | search a window, by client |
| `countly` | `plugins/countly` | recent crashes |
| `tenant-resolver` | `plugins/tenant-resolver` | resolve tenant from URL / IČO / name, architecture, version |
| `deploy-correlator` | `plugins/deploy-correlator` | deployments in a window |
| `support-kb` | `kb-sync`, `kb-git-sync`, `confluence-*`, `embedding-service`, `graph-store` | FTS over known errors, semantic doc search, related nodes |

Each is a `kind: mcp_server` asset with its secrets as `${NAME}`
(`GRAYLOG_API_TOKEN`, `COUNTLY_API_KEY`, `STORE_API_TOKEN`,
`VOYAGER_API_KEY`), installed to the rule's host by the catalog sync. The
tenant resolver's "ask in ticket when unknown" becomes the analyzer's
`needsMoreInfo` path, so it writes through fleet like every other write.
`support-kb` keeps its own SQLite and embedding model; fleet does not
learn about embeddings. Its write side (auto-learning a high-confidence
finding into the KB repo, resolution YAMLs) is a later action kind.

The Graylog/Countly **scanner** (a periodic sweep that opens jobs for new
fingerprints) becomes a later trigger: a poller that calls the `graylog`
MCP server's search on a schedule. It is not in v1; the original ships it
off.

### What is dropped

`agent-seed`'s skeleton (`src/agent`, `src/events`, `src/state`,
`src/dashboard`, `src/guardrails`, `src/telemetry`, `src/notifications`,
`config-manager`), the support API on port 3002, the in-memory incident
aggregator (the fingerprint record replaces it), and the Prometheus and
dashboard pieces of the reviewer.

## The merge

- **claude-code-reviewer**: imported with history as a reference that is
  not built, `git subtree add --prefix=tools/code-reviewer`, in a PR of its
  own. Each port step deletes the TS files it replaces in the same PR; the
  step that ships posting deletes the rest and the directory, and the
  repo is archived once one rule has run on `act` for a week.
- **support-agent**: stays a repo and becomes a package of MCP servers and
  catalog assets (`mcp/<server>/`, `catalog/`); its `agent-seed` skeleton
  is deleted there once step 6 runs. Nothing of it is subtree-merged into
  fleet; the parts that move into fleet are rewritten in Rust (A, C), and
  their tests are ported, not the code.
- Both services keep running unchanged until the fleet rule for the same
  scope is on `act`; then the service's config drops that scope. A scope
  is never served by both: the review marker check makes a double review
  visible, not silent.

## Steps

1. **Runtime core**: migration (`agent_rules`, `agent_cursors`,
   `agent_jobs`, `agent_actions`), `fleet-hub agents grant | revoke |
   pause-all | status`, the `agents_poll` / `agents_run` loops (empty
   pollers), settings rows, `runs` kinds, the `agents` tool (list rules,
   jobs, actions; approve / reject). `REGEN_SETTINGS_DOCS`, `REGEN_DOCS`.
2. **Run vehicle**: `claude_print::isolation_flags_with`, the detached
   review worktree script, the MCP-config temp file; tests on the rendered
   scripts.
3. **Reviewer, read side**: `review_prs`, the GitHub poller, `plan.rs`
   and `diff.rs` with the ported tests, the run and its parse; level
   `watch` works end to end (review stored and shown).
4. **Reviewer, write side**: `post.rs`, `propose` and `act`, verification,
   `/review` by polling; `tools/code-reviewer` deleted.
5. **UI**: Automation › Agents rows, the agent page, Inbox actions;
   fleet-mobile's Inbox approve / reject.
6. **Support, read side**: Jira comment and assignment poller, enrichment,
   the `support-agent` repo's `graylog`, `tenant-resolver` and `support-kb`
   MCP servers in the catalog, the analyzer run at `watch`.
7. **Support, write side**: the `WriteOp` variants, `jira_comment` /
   `jira_create_bug` / `jira_link` actions, fingerprint dedup,
   `propose` and `act`.
8. **Later**: test-debate, auto-label, auto-description, the Graylog
   scanner trigger, KB write-back, outbound Slack/Teams with step 11.9.

## Not in scope

- Inbound webhooks (D13 stays no). Polling at 300 s is the latency both
  agents get; `/review` and "Tech help!" answer within one interval.
- A second dashboard, port, bearer token or metrics endpoint.
- Running either agent inside a person's session or worktree.
- Giving the model write credentials for GitHub or Jira.
- Porting `agent-seed` as a framework.

## Open questions

1. **Which identity writes.** The reviewer posted as the `gh` login of its
   container; under fleet that is the `gh` login on the rule's host. A
   dedicated bot account per org is cleaner (and makes the loop guard's
   "own output" check exact) but means a second `gh` auth on that host.
   Proposal: the rule names the expected login and a run refuses to post
   if `gh api user` disagrees.
2. **Where support's Jira writes authenticate.** The tracker's credentials
   belong to the person who added the tracker; support-agent wrote as a
   bot (`bot@papayapos.com`). Proposal: a support rule may name a separate
   tracker secret for writes.
3. **Cost line.** Support runs were capped by turns only (80). Proposal:
   `run_budget_micros` on the rule, failing the job past it as routines
   do.
