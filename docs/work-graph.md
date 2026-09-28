# Work: the user guide

Fleet can know **what work** each session is doing, group sessions by it,
start and resume work from a ticket, and tidy finished work away. Without
anything set up it already groups sessions by the ticket keys in their
branch names. A tracker (Jira, GitHub, Asana, Linear) adds titles, statuses
and a ticket list. Nothing about work is ever required, and nothing waits
on a tracker.

This page explains the feature. The exact tool parameters are in
[control-api.md](control-api.md) (`work`, `work_link`, `work_admin`) and the
generated [control-api-reference.md](control-api-reference.md). The hub's
side (commands, providers, orgs, retention) is in [hub.md](hub.md). To
check the whole feature on your own installation, walk through the manual
acceptance run, [work-graph-acceptance.md](work-graph-acceptance.md).

- [What "work" is](#what-work-is)
- [The Work view (reads)](#the-work-view-reads)
- [Work view: edits](#work-view-edits)
- [Linking and detection](#linking-and-detection)
- [Trackers](#trackers)
- [Starting work](#starting-work)
- [Resuming work](#resuming-work)
- [Handover](#handover)
- [Today and standup](#today-and-standup)
- [Tidy-up and auto-tidy](#tidy-up-and-auto-tidy)
- [Retention](#retention)
- [Organisations and isolation](#organisations-and-isolation)
- [Trackers in fleet health](#trackers-in-fleet-health)
- [Usage summary](#usage-summary)
- [The phone](#the-phone)
- [The operator and confirmations](#the-operator-and-confirmations)
- [Settings](#settings)

## What "work" is

A piece of **work** is one of three things:

- **a ticket** from a connected tracker: `ABC-123` (Jira, Linear),
  `owner/repo#42` (GitHub; `host/owner/repo#42` on an Enterprise instance)
  or an Asana task (`asana:<gid>`, shown as `Asana …123456`);
- **a bare key** that looks like a ticket but no tracker knows, for example
  `ABC-123` in a branch name with no Jira connected;
- **local work**: a title with no ticket, made with **Name this work…**.

A session is tied to work by a **link**. The link hangs on the session's
identity (its participant), not its tmux name, so it survives a rename, a
move to another host, a restart and a recreate. A session can have several
links; one of them is its **primary** work, the one it is grouped by.

When a session ends, its links end with a **snapshot** (host, branch,
worktree, PR, the Claude conversation), and the **work journal** keeps
what its conversations did: the first prompt, the last turns, Claude
Code's own compaction summary and the outcome (branch, head, PR, diff
stat). That is what makes old work resumable weeks later.

In the sidebar, **View ▾ → Work** (⧉ by work) groups sessions by their
primary work. Work groups come first; sessions with no work stay under
their project. Each group's header rolls up the PR / CI state of its
sessions, and ended sessions show in a collapsed **Done · n** section of
the group. The **⚑ work** pill in the triage row opens filter chips:
tracker (with two or more trackers), status, *mine*, *hide archived*, and
in work mode *any / with session / past only*.

> **[Screenshot placeholder]** The sidebar grouped by work, with a Done
> section open and the ⚑ work filter chips showing.

## The Work view (reads)

The hub answers the other way into the work graph — organisation → group →
task → *every* session of the task (primary, secondary, suggested and past),
including tasks with no session at all — as reads of the `work` tool
(work graph M14.1b; the desktop and phone views and the edits come later):

- `work { action: tree, filters?, cursor?, limit?, per_task? }`: a page of
  tasks with their sessions, the section headers (`groups`, each with its
  count under the filters), and the orgs and trackers the caller sees.
  Filters: `org` (an id or `"none"`), `tracker` (an id, `"local"` or
  `"ref"`), `status`, `mine`, `has` (`active` / `past_only` / `none` /
  `suggested`), `review`, `query`, `group`. Pages are a keyset: pass
  `next_cursor` back with the same filters (other filters refuse it). No
  task is repeated across pages while the fleet changes; a task that moved
  meanwhile may be skipped until the next full read.
- `work { action: task, task_id }` (`item:<id>` or `ref:<KEY>`): one task
  with every session and why it is linked, its tracker description, the
  last known outcome, its placement and the rules that match it.
- `work { action: session_tasks, session_id }`: every link of one session
  (active, suggested, rejected, ended), each with its task.
- `work { action: review, cursor?, limit? }`: suggestions and conflicts
  (a cross-org link, an unavailable ticket, a session with no primary).
- `work { action: rules }`, `work { action: rule_preview, rule }`: the
  placement rules, and what a drafted rule would move before it is saved.
- `work { action: views }`: saved filters (a device bound to an org lists
  its org's only).
- `work { action: org_impact, task_id, org_id }`: what moving a local task
  to another org would change (the master and unbound devices only).

Every read is fenced by the caller's organisation boundary, like every
other work read: a per-host token and a device bound to an org see only
what their org may, and a task outside it answers exactly as one that does
not exist. Each task says where its org and group come from (`org_source`:
tracker, item, sessions, none; `group.source`: manual, rule, tracker,
repo, key, none).

## Work view: edits

The Work view's changes are `work_link` actions (work graph M14.1c; the
desktop and phone screens come later). None of them is a new tool, and an
older device that sends none of the new parameters behaves as before.

- **A second task on a session.** `link { primary: false }` and
  `confirm { link_id, primary: false }` add a *secondary* link: the
  session's primary work stays where it is (a session with no primary
  still gets one). Without `primary`, a link or confirm takes the primary,
  as it always did.
- **Make primary.** `set_primary { session_id, link_id, expected_primary }`
  moves the primary to another confirmed link of the session, and changes
  nothing else: no link is removed or ended. `expected_primary` is the
  primary link you saw (`0`: none).
- **Undo.** `reconsider { session_id, link_id }` turns a confirm or a
  rejection of a *suggestion* back into a suggestion, with its evidence. A
  link you made by hand has nothing to go back to: remove it (`unlink`).
- **Keep a conflict.** `ack { session_id, link_id }` takes a conflict you
  mean to keep — a forced cross-org link, a link to an unavailable ticket —
  out of the review inbox (D32: a forced cross-org link is a review item
  until someone acks or removes it).
- **Decide many.** `decide_batch { decisions: [{session_id, link_id,
  decision: confirm | reject | reconsider | ack, expected_version?,
  primary?}] }` (at most 100) decides each item on its own, with the same
  checks as the single action, and answers per item (`ok`, `code`,
  `message`, the link's new `version`). One refused item never stops or
  undoes the others. A cross-org confirm needs `force_cross_org`, which a
  batch does not carry: decide it alone.
- **Place a task.** `place { task_id, group, note?, expected_version }`
  puts a task in a group of your choosing (an empty `group` and no `note`
  puts it back where it would sit by itself). Navigation only: it never
  changes who sees the task, and never writes to the tracker.
- **Rules.** `rule_save { rule }` creates or edits a placement rule
  (`{id?, name, enabled, conditions, group, expected_version?}`),
  `rule_delete { rule_id, expected_version? }` removes one. Rules only
  place tasks in groups (D34): they never link a session. Preview a rule
  with `work { rule_preview }` first; saving it moves exactly those tasks.
- **Saved views.** `view_save { view }` (`{id?, name, filters,
  expected_version?}`) and `view_delete { view_id, expected_version? }`.
  Views are shared on the hub (D35).
- **A local task's org.** `assign_org { task_id, org_id, impact_token }`
  (`org_id` `0`: none) moves a *local* task to another org, with the
  `impact_token` of a fresh `work { org_impact }`: if what the move changes
  is no longer what you previewed, it is refused with the new impact. A
  tracker ticket's org is its tracker's (`work_admin assign_tracker`).

**Two devices at once.** Each change names the version it saw
(`expected_version` on a link, a placement, a rule or a view;
`expected_primary` for the primary). If someone else changed it
meanwhile, the answer is `E_CONFLICT` with the current value in `details`,
and nothing is written: reload and decide again. Without `expected_*` a
change applies as before (older devices).

**Who may change what.** A read-only token changes nothing. A per-host
token decides only its own host's sessions' links (`set_primary`,
`reconsider`, `ack`, `decide_batch`) and does not place, write rules or
views, or move orgs. A device bound to an org decides links and places
tasks it sees, and keeps its org's saved views; it writes no rules and
moves no org (D33, D34). The master and an unbound full device may do all
of it. A cross-org link still needs `force_cross_org` from everyone, and
anything outside what you may see answers exactly as if it did not exist.

## Linking and detection

### Linking by hand

Every row has a **#** work menu: set a key or paste a ticket URL,
**Not KEY** (reject a recognised key for this session), **Clear**, and
**Name this work…** (new local work for this session). In work mode a
project group's header also has **Name this work…**, for its sessions that
have no work. An explicit link always wins over anything fleet recognised,
and a rejection is sticky: fleet never suggests that pair again.

Claude in the session can link its own work too (`work_link`, source
`agent`), which is how the friendly-name skill records "I'm working on
ABC-123".

### Detection

Fleet watches for work in:

- the session's current **branch** (from the transcript's `gitBranch`,
  else the worktree's branch);
- **ticket URLs** and **keys** in prompts (only the match is kept, never the
  prompt);
- the session's **pull request**: its head branch, closing references,
  title and body, and **commit trailers**;
- the tracker **sync**, which binds keys typed before the tracker was
  connected.

One recogniser (`service/work/recognize.rs`, mirrored in
`src/lib/work_keys.ts` over a shared fixture) finds keys; with a tracker
connected, a key must carry one of its prefixes, so `GPT-4` or `COVID-19`
are never keys. One resolver (`service/work/resolve.rs`, rules R1–R11)
decides what each sighting becomes:

- a **confirmed automatic link**, only for strong, unambiguous signals, for
  example a ticket URL that is the only candidate, or a sole branch key in
  a **trusted** project. A toast says "Linked NAME → KEY (branch) · Undo";
  Undo is *Not this*.
- otherwise a **suggestion**. A suggestion never regroups a session. It is
  shown as a dashed chip with `?` and waits for a person.

A project becomes trusted when you tick **Trust branch keys in this repo**
in the popover, or automatically after three branch suggestions in it were
confirmed. Settings → Limits → Lifecycle shows how many projects are trusted and has a
**Trust none** button.

### The chip and its popover

The work chip on a row says how the link came about:

| Chip | Meaning |
|---|---|
| solid | linked by a person, by Claude declaring it, or started for it |
| ring (with a dot) | linked automatically by detection |
| dashed with `?` | a suggestion, not a link |
| struck through | the ticket is unavailable (deleted or no longer visible) |
| ◷ | the tracker has not synced for two intervals, so the status may be old |

Clicking the chip opens the **popover**. It lists the session's links and
suggestions, each with its **evidence**: one line per signal, for example

- branch `abc-123-login` since 09:05 · R3
- mentioned ABC-99 in a prompt at 10:12 (reference) · R6
- pull request closes ABC-123 · R4
- Claude named ABC-7 when asked at 11:40 · R11

The rule tag (R1…R11) says which resolver rule decided it. With
`work.evidence_snippets` on (the default), hovering an evidence line shows
a redacted ±40-character snippet of the prompt around the match; off, only
the matched text is kept. The popover's buttons are **Confirm** (↵ / `y`),
**Not this** (⌫ / `n`, never suggested again) and **Pick another…** (type
or paste another key or URL). On a focused row, `y`, `n` and `l` do the
same.

> **[Screenshot placeholder]** A row's work popover with a suggestion, its
> evidence lines and the Trust branch keys checkbox.

### Batch review

When sessions have suggestions, the attention strip shows **N link
suggestions · Review**. The sheet lists each session's top suggestion with
its reason. `j` / `k` move, `y` (or ↵) confirms, `n` (or ⌫) rejects, `esc`
closes. Deciding one brings up that session's next suggestion, if it has
one. Clicking a row narrows the sidebar to that session so you can look at
it first.

> **[Screenshot placeholder]** The Link suggestions sheet.

### The classification nudge (off by default)

With `work.classify_nudge` on, a conversation that has gone three turns
with no link, while its host's scope has one to five candidate tickets,
gets one short note (at most 400 characters) asking Claude to name its
work. Claude's answer (`source: agent_inferred`) is only ever a
pre-selected suggestion (rule R11) that you confirm or reject. It spends
context on a guess, which is why it is off.

### SessionStart context (off by default)

With `work.session_start_context` on, the SessionStart hook hands Claude the
linked ticket's context when a conversation starts. It makes that hook
synchronous, which can add up to about 2 s to a start when the hub is down,
so it stays off until measured on your hosts
(`scripts/measure-session-start.sh`, decision D5). It takes effect when the
hooks are next installed (re-provision the host).

## Trackers

A tracker is **read-only** and polled. It never gates anything: with a
tracker down or its token expired, everything answers from the cache.
Trackers give you:

- titles and statuses on the chips and group headers;
- **⌘K → My work** (and *Current sprint* / *Current cycle* where there is
  one, *Recent*, and favourite filters), which searches the cache;
- starting work from a ticket in one step;
- ticket cards with acceptance criteria;
- the "done" signal that tidy-up uses.

Fleet writes one thing back, and only where you turn it on (decision D3):
a session's pull request as a link on its Jira ticket (see *Write-back*
below). It has no inbound webhook (D13).

### Connecting one

Trackers are fleet administration (`work_admin`, master token only), so
they are configured where the fleet lives:

- **Standalone desktop:** Settings → **Work** → **Connect a tracker**. Paste
  any ticket or issue URL (or the site); the provider is inferred from it.
  Then give the credential it asks for and press **Test**.
- **Hub:** on the hub machine, with `fleet-hub tracker`. A desktop paired
  with a hub shows the trackers read-only and says to configure them on the
  hub.

```sh
fleet-hub tracker add https://acme.atlassian.net/browse/ABC-123
fleet-hub tracker set-credential 1 --email you@acme.com < jira-token.txt
fleet-hub tracker test 1      # account, key prefixes, sprints, views
fleet-hub tracker status      # last sync pass per tracker, and retention
fleet-hub work usage          # how the work graph is used, as counts
```

A token is read from stdin, from `--from-env NAME`, or stored as a
reference (`--ref env:NAME` / `--ref file:/run/secrets/jira`) resolved at
each sync. It is never a command-line argument, never in an answer, event,
log line or error report; a tracker row shows only a `…abcd` hint.

> **[Screenshot placeholder]** Settings → Work with one connected tracker
> and the Connect a tracker form open.

### Per provider

| Provider | Paste | Credential | Notes |
|---|---|---|---|
| **Jira Cloud** | `https://<name>.atlassian.net` or any ticket URL | Atlassian email + API token (id.atlassian.com → Security → API tokens; they expire within a year) | *My work*, *Current sprint*, *Recent*, favourite filters; epics by hierarchy level |
| **Jira Data Center** | the site, with provider `jira_dc` (`--provider jira_dc`) | personal access token | one exact host, https only; the name is resolved and a loopback / link-local address is refused unless `allow_private_network`; an internal CA goes in `extra_ca` (PEM) |
| **GitHub** | `https://github.com/<owner>` or any issue URL, plus a host where `gh` is logged in (`--via-cli <host>`) | **none in fleet**: `gh` on that host uses its own `gh auth login` | `assignee:@me` issues in the owner's repos (`--repo owner/repo` narrows); fleet refuses to store a GitHub token |
| **GitHub Enterprise Server** | an issue URL, provider GitHub, plus the **hostname** (`--hostname ghe.corp.example[:port]`) and `--via-cli <host>` | none: `gh auth login --hostname …` on that host | keys are `host/owner/repo#n`, so the same repo name on github.com is different work |
| **Asana** | `https://app.asana.com[/<workspace>]` or any task URL | personal access token | tasks have no human keys: detection is by URL. Which sections mean *in progress* is guessed on the first test and shown with a **Confirm** button; your confirmed map wins |
| **Linear** | `https://linear.app/<workspace>` or any issue URL | personal API key | team keys are the prefixes; *My issues*, *Current cycle*, *Recent* |

A tracker only one machine can reach (a VPN) is read with `curl` on that
host: `--via-host <host>`, the token piped on stdin. A key prefix that two
trackers both claim (`ENG` in Jira and Linear) is never bound automatically.
The full details are in [hub.md → Trackers](hub.md#trackers).

### Write-back: the PR link (Jira, off by default)

For a Jira tracker (Cloud or Data Center), Settings → Work has **Add a
session's pull request to its ticket as a link**. With it on, when the PR
probe sees a pull request on a session, fleet adds that PR to the linked
ticket once, as a Jira remote link titled `PR: owner/repo#n`. Nothing else
is ever written: no transition, no worklog, no comment (D29), and nothing a
transcript or a tracker wrote.

- **Only work a person linked.** The link must be confirmed and made by
  hand or by *Start* (`manual` / `started`). A detection guess or an agent's
  suggestion never writes.
- **Only your own org's tracker.** A session in one org never writes to
  another org's tracker, even a link made with `force_cross_org`; the org is
  checked again just before sending.
- **Once per PR.** The link's global id is the PR's URL, so Jira updates
  the same link rather than adding a second one, and fleet queues each PR
  once.
- **Through the sync.** Writes wait in an outbox and go out with the next
  sync pass, on the tracker's own credential. A rate limit waits (without
  counting as a failure); other failures retry with backoff, and after five
  tries, or at once for a refusal (403, 404), fleet gives up. Given-up
  writes show in `fleet_health` as `write_failures` and in the footer as
  "N writes not sent"; they never make the tracker `degraded`.
- **The token needs write permission** (to edit issues). A read-only token
  is enough for everything else; with one, the writes are refused and given
  up.

Turning it off stops sending at once; writes already queued wait, and go
out if you turn it on again. Settled writes are dropped after the journal's
retention window (`work.retention.journal_days`).

### Sync

The sync runs every `work.sync_interval_secs` (300 s by default; `0` turns
it off; read when the app or hub starts). Each pass reads every view from
its watermark with a two-minute overlap, refreshes every linked ticket by
id, and binds keys typed before the tracker was connected. A ticket that
disappears is marked **unavailable** (struck-through chip), never deleted;
its links stay. A tracker's state is one of `ok`, `auth_failed`
(polling stops until a new credential is set and tested), `captcha` (log in
once in a browser), `rate_limited` and `unreachable` (both retry on their
own). See [troubleshooting.md](troubleshooting.md#work-and-trackers) when a
sync fails.

## Starting work

- **From ⌘K:** type a key or paste a ticket URL, or pick a ticket from
  *My work*, then ↵. Fleet starts it with a brief at once and selects the
  new session; when the project is ambiguous it opens the New session
  dialog on that ticket instead, and when a session is already on it, it
  jumps there.
- **From the New session dialog:** the optional *Work* field takes a key, a
  local item or a URL; **Start work** starts it.

A ticket start picks the project where that key's prefix last ran (asks
when it is ambiguous), creates the branch and worktree `slug(key + title)`,
names the session `KEY title` and links it with source `started`. With
**Brief Claude** on (the default), the ticket's context, its description
fenced as untrusted, rides the first hook's context, and a short start
prompt is typed only once Claude's REPL is ready, never into a trust
dialog. The brief is editable before you start.

If a live session is already on that key, the dialog says so ("ABC-123
already running on X") and offers **Jump** instead of starting a second
one. While another device is still starting or resuming the same key (a
multi-repo start holds it until its last repository), a second start or
resume is refused: "ABC-123 is being started or resumed already; wait for
that session, then jump to it".

**Multi-start.** For work that spans repositories, the dialog's **Also
start in** list (the projects the key ran in before) starts one sibling
session per project, up to 8, all on the same branch name, each linked
`started` and each brief naming its siblings (`work_link start {
project_ids }`). A repository where the key already runs is skipped, not
refused. Multi-start is also on the phone, with a full token (decision D15;
fleet-mobile #50).

> **[Screenshot placeholder]** The New session dialog on a ticket, with
> Brief Claude and Also start in.

## Resuming work

Work whose sessions have ended shows in its group's **Done** section and
under ⌘K. For as long as `work.recent_days` (14 by default), work with only
ended sessions keeps a sidebar group of its own. Work moved back out of
done in the tracker shows as **Reopened** in the attention strip, with its
past sessions.

**Resume ▾** is a split button:

| Mode | What it does |
|---|---|
| **Continue last conversation** (`last`, the default when possible) | re-opens the last Claude conversation on its host and branch |
| **Fresh with brief** (`brief`) | a new conversation with a handover brief built from the ticket, the snapshots and the journal |
| **Fresh** (`fresh`) | a new conversation, no brief |

The tooltip and the dialog say which host and branch it will land on; the
host can be overridden, and the brief is editable. The resumed session
re-attaches the ended link (source `resumed`).

Before offering *continue*, fleet **probes the host for the transcript**:
the path the conversation recorded, then
`~/.claude/projects/*/<id>.jsonl`. When the transcript is gone, *continue*
is not offered and *fresh with brief* is the default. When the probe cannot
answer (the host is unreachable), the dialog shows a warning instead of
guessing. A purged conversation is never offered for *continue*, and
purging one that is linked to work warns first.

> **[Screenshot placeholder]** The Resume dialog with its three modes and
> the brief.

## Handover

A handover brief is how the next session learns what the last one did.
There are two kinds:

- **The built brief.** Deterministic, at most 4,000 characters: the item,
  the snapshots, the journal and one read-only git probe, with anything
  written by a third party fenced as untrusted. It is delivered as a
  message from the hub through the hook's context, never typed into a pane.
- **An agent-written handover, on demand.** In a ticket card (Details),
  **Ask for a handover** asks a live, idle session linked to work to write
  the hand-off itself. Fleet sends one prompt; the next Stop hook keeps
  Claude's answer as a journal note, and the next resume brief (and
  `work { action: context }`) shows it first. It is refused while the
  session is busy, stuck or waiting on a dialog, when it has no work, or
  while a request is pending (30 minutes). The session's timeline records
  `handover_requested`, then `handover_written`, `handover_missing` or
  `handover_send_failed`. Fleet never spends a turn on this by itself, not
  even at safe kill (decision D9).
- **A summary of a past session, on demand.** A session that has ended
  cannot write its own hand-off, so each past-work row in the sidebar has
  **Summarise**. It runs one print-mode fork of that session's last
  conversation on the session's own host, under its own Claude account,
  with the model `work.summary_model` names (`haiku` by default). The fork
  has no tools and no MCP servers, fleet's hooks are off for it, and it
  leaves no transcript behind; the original conversation is not touched.
  Claude's answer is shown below the row and kept in the journal (redacted,
  at most 4,000 characters, one per conversation: asking again replaces
  it), and the next resume brief shows it inside the untrusted fence, after
  a live session's own hand-off. It needs the transcript and the directory
  the conversation ran in to still be on the host; otherwise it says so and
  runs nothing. Only one summary runs per host at a time. It is never
  automatic (decisions D10, D30).

## Today and standup

**Today** is the Details panel's empty state, and ⌘⇧T (Ctrl+Shift+T)
opens it over a selected session. It has four sections, cut to the scope
the sidebar shows:

- **Waiting on you**: sessions that need a person;
- **In progress**;
- **Shipped today**: tickets that moved to done and work that ended with a
  PR since your local midnight;
- **Stale**: idle three days, or the ticket is done while a session still
  runs. When tidy-up has suggestions among them, **Tidy up · n** opens the
  Tidy-up sheet narrowed to those sessions.

**Copy standup** puts the same sections on the clipboard as plain text.
Today reads the store and the tracker cache only; it never calls a tracker.

The **ticket card** in Details shows a linked ticket's title, status, link
and acceptance criteria (parsed from the cached description). **Insert into
composer** puts the ticket text, fenced as untrusted, into the conversation
composer. It never sends it.

> **[Screenshot placeholder]** Today with all four sections, and a ticket
> card.

## Tidy-up and auto-tidy

Fleet **suggests** cleaning up; a person confirms. **Tidy up · n** appears
in the attention strip only when there is something to suggest, and never
counts toward *Needs you*. The sheet groups candidates by reason:

| Reason | When | Preselected action |
|---|---|---|
| `done_idle` | the linked ticket has been done `work.tidy_done_days` and the session idle `work.tidy_idle_hours` | Safe kill |
| `pr_merged_idle` | its PR is merged and it is idle | Safe kill |
| `not_planned` | the ticket was closed as won't-do or duplicate | Safe kill |
| `duplicate_worktree` | two sessions work in one worktree | Kill (the worktree stays) |
| `ghost_expiring` | a lost session is a day from being reaped | Resume or let it expire |
| `idle_unlinked` | no work linked, idle and unprompted `work.tidy_idle_unlinked_days` | not ticked |

Each row can be switched to **Archive only** (collapse into the group's
Done; tmux keeps running and the next prompt or attach brings it back),
**Snooze 7 d** or **Never for this work**. **Safe kill** asks Claude to
commit and push first and removes the worktree only if that succeeded.
`j` / `k` move, space toggles, ↵ applies, `esc` closes.

**Idle, no work linked** rows start unticked and have their own **Keep
7 d** and **Safe kill** buttons (Safe kill arms on the first click). With no
work linked, fleet does not guess what uncommitted work is for: the kill is
refused unless the worktree is clean and pushed. **Keep** holds any live
session out of tidy-up for 1–90 days (7 by default).

No setting overrides the protections. These are never suggested and never
touched: a session that is working, blocked, stuck or waiting on a dialog;
one linked to an in-progress ticket; the controller and the operator; a
session prompted or attached to within the hour; and a background agent
with open tasks.

**Auto-tidy** (`work.auto_tidy`, off) lets the GC sweep act on the
candidates whose reason is in `work.auto_tidy_reasons` by itself, with safe
kill or archive only, never a plain kill. `idle_unlinked` can never be
auto-tidied (decision D19). An org can override `work.auto_tidy` for its own
sessions (`on` / `off` / `inherit`). Every action is written to the
session's timeline and its work journal. Settings → Limits → Lifecycle
previews what auto-tidy would act on.

> **[Screenshot placeholder]** The Tidy up sheet with a done ticket and an
> idle unlinked session.

The layers fleet may and may not touch are in
[concepts.md → Lifecycle](concepts.md#lifecycle); the hub's side is in
[hub.md → Tidy-up and auto-tidy](hub.md#tidy-up-and-auto-tidy).

## Retention

The work tables would otherwise grow forever. The GC tick deletes a row only
when it is ended or done, older than its window, and nothing live points at
it. `0` keeps a table forever.

- `work.retention.journal_days` (365): the work journal. Kept regardless of
  age: rows of an open conversation, of a live-linked session, and of work
  that is not done or still has a live link; an undelivered handover and
  one addressed to a live session.
- `work.retention.tracker_items_days` (180): cached tickets in done. Kept
  while any link, live or ended, names one, and while it is the parent of a
  kept ticket.
- The describe cache (`work.describe_cache_secs`'s table, one item's whole
  description) is swept with the tracker items, by the same window — except
  its `0` is never "forever": with `work.retention.tracker_items_days` at
  `0`, the describe cache is still swept at a fixed 30-day floor, so a
  full-text cache never becomes an unbounded copy of every description
  fleet ever fetched.
- `work.retention.timeline_work_events_days` (180): handover, nudge and tidy
  timeline events. The newest of each kind per session stays.
- The write-back outbox (see *Write-back*) follows the journal's window:
  a PR link that was sent, or given up on, goes once it is older than
  `work.retention.journal_days`; one still waiting is never swept.

A sweep deletes at most 2,000 rows per table per tick, 200 per store lock.
Settings → Limits → Retention (standalone desktop) shows the row counts, a
dry-run count and the last sweep; on a hub, `fleet-hub tracker status` or
`work_admin { action: status }` (master), and `work_admin { action:
sweep_now }` runs one sweep. The old `work.journal_days` setting from before
M12.3 is superseded: while `work.retention.journal_days` is unset, an old
`0` still keeps forever and an old window longer than 365 days still stands.

## Organisations and isolation

Organisations are optional. With none, the sidebar's scope selector offers
the GitHub owners of your live sessions (only when there are two or more),
and nothing is fenced.

A named org is two things:

- **A view for people.** The scope selector (⌘⇧O / Ctrl+Shift+O) only
  narrows what is shown, and a session waiting on you in another scope
  still says so ("2 need you in Personal →").
- **A boundary for Claude on a host.** A host placed in an org reads only
  that org's work and unassigned work: tickets, trackers, links, the
  journal and every brief built from it, and other orgs' work is taken out
  of the session rows it receives. Something outside the boundary answers
  exactly as something that does not exist.

A session's org is the most specific matching rule (path prefix, then
`owner/repo`, then owner, then host), else its host's org. A ticket's org
is its tracker's. Linking work of one org to a session of another is
refused for everyone unless forced (the desktop offers **Link anyway**; a
move offers **Move anyway**). Detection never guesses across orgs.
`isolate_sessions` (per org, off) also hides that org's sessions from other
orgs' hosts.

A paired device can be **bound to one organisation** (`fleet-hub pair --org
<id>` or `fleet-hub client bind <name> <id>`): it then reads only that
org's work and sessions — and unassigned ones while the org's
`bound_sees_unassigned` is on (the default, D31; see *Settings*). Another
org's session is hidden from it whatever `isolate_sessions` says. A session
that has tasks of two orgs (a link someone forced across) shows each side
only its own: the other org's task, its title, evidence, conversation and
summary never reach a device bound to the first org, and a device bound to
the other org sees the task without the first org's session.

Orgs are managed in Settings → Work → Organisations on a standalone
desktop (read-only on a paired desktop), or with `fleet-hub org …` on a
hub. **Assign every host of a company before
connecting a second company's tracker.** The details and commands are in
[hub.md → Organisations and isolation](hub.md#organisations-and-isolation).

## Trackers in fleet health

`fleet_health` carries a `trackers` roll-up, read from the cached sync
state and the store. It never calls a tracker or a host. For each tracker:

- `health`: `ok`, `degraded` or `failing`. `auth_failed`, `captcha` and an
  unconfigured tracker are `failing` at once, since the sync stops polling
  them. Three failed passes in a row are `failing` too. A transient state
  (`rate_limited`, `unreachable`) with fewer failures is `degraded`: the sync
  retries it by itself.
- A pass that **skipped items** it could not store (the rest of the pass
  still synced) is `degraded`; three such passes in a row are `failing`,
  because the same item is stuck (decision D25). A clean pass is `ok`
  again.
- `reason`, why it is not `ok`: `credential` (auth_failed, captcha,
  unconfigured), `sync_failed` (passes fail, or rate limited / unreachable)
  or `items_skipped`; empty while `ok`.
- `state`, `consecutive_failures`, `items_failed` (items the last pass
  skipped), `consecutive_partial` (passes in a row that skipped some),
  `last_error` (redacted, one line, at most 300 characters, fenced as
  untrusted; for skipped items, why the last one failed), `last_success_at`,
  `last_pass_at`, its org, and `write_failures` (PR links fleet gave up
  sending, *Write-back*; never part of `health`).

Fleet-wide it also counts `failing`, `degraded`, and the **detection
backlog**: link suggestions on live sessions that have waited more than 7
days for a decision (`detection_backlog`, `detection_backlog_days`). A
per-host token sees only its own org's trackers (a host in no org, only
unassigned ones) and its own host's backlog.

On the desktop:

- The footer shows a one-line summary when there is something to say, for
  example `trackers: 1 failing · 3 suggestions undecided > 7 d`. Clicking it
  opens Settings → Work.
- Each **failing** tracker raises one item in the attention strip,
  **⚠ Reconnect Jira (acme) →**, which opens Settings scrolled to Work.
  Hover it for the error, the failure count and the org. A degraded tracker
  raises none.
- A tracker failing because it keeps **skipping items** raises
  **⚠ Sync skipping items — Jira (acme) →** instead: reconnecting would not
  help. It also opens Settings → Work, where the tracker's last pass reads
  `… · 2 skipped (3 passes in a row)` with the reason. See
  [troubleshooting.md → Sync skips items](troubleshooting.md#sync-skips-items).
- The roll-up is read at startup and every 60 seconds. On a paired desktop
  it is the hub's `fleet_health`.

After you set a new credential and **Test** it, the item goes away once the
tracker is `ok` and a sync pass has succeeded, at the next read after that. The same numbers, with each pass's details, are in `fleet-hub tracker
status` and Settings → Work. See
[troubleshooting.md → Tracker sync fails](troubleshooting.md#tracker-sync-fails).

> **[Screenshot placeholder]** The attention strip with a Reconnect item,
> and the footer's trackers line.

## Usage summary

`work_admin { action: usage, days? }` (on a hub, `fleet-hub work usage
[--days N] [--json]`; on a standalone desktop, Settings → Work → *Usage*)
counts how the work graph is actually used over the last `days` (default
30, 1 to 365). It is read-only and master-only (a per-host or client token
is refused, and a paired desktop shows no Usage section). It records
nothing, sends nothing anywhere, and holds counts and ids only: never a
title, key, path or error text.

| Group | What it counts |
|---|---|
| links | links made, per `source` (`manual`, `started`, `branch`, `resumed`, …); a suggestion a person decided reads `manual` |
| detection | suggestions made, confirmed by a person, promoted by detection itself, rejected, expired (the session ended undecided); the median time from suggestion to a person's decision; classification nudges |
| handover | handovers requested and written, turns that ended without one, requests that could not be sent |
| resume | resumes, with and without a brief |
| journal | briefs queued and delivered, compaction summaries harvested |
| tidy | sessions tidied from Tidy-up, *Keep* answers, auto-tidies per reason |
| trackers | per tracker id: passes, failed passes and items skipped since the syncing process started (not windowed, reset on restart) |

Some things are not stored anywhere, so the answer lists them under
`unrecorded` instead of guessing: suggestions *shown*, handovers refused as
busy, `last` vs fresh resumes, the transcript probe's outcomes, Tidy-up's
suggestions per reason before anything is applied, and multi-start runs.
Suggestions that detection withdrew or let decay leave no row, so
`suggested` is a floor. The counts are bounded by retention and by the
timeline's cap per session (500 events).

Paste it into an acceptance run's record: that gives the decisions real
numbers.

```
$ fleet-hub work usage --days 30
work graph usage, last 30 d
links: 41 made (branch 12, manual 20, resumed 3, started 6)
detection: 18 suggested, 9 confirmed by a person, 4 promoted, 3 rejected, 1 expired; median decision 12 min; 2 nudges
handover: 5 requested, 4 written, 1 missing, 0 send failed
resume: 3 (2 with a brief, 1 without)
journal: 8 briefs queued, 7 delivered; 11 compaction summaries
tidy: 6 applied, 2 kept, 0 auto-tidied (none)
tracker 1: 288 passes, 3 failed, 0 items skipped (since the sync started)
not recorded: suggestions shown (only made, confirmed, rejected and expired are stored)
…
```

## The phone

The phone app (fleet-mobile) reads work from a hub over its paired client
token:

- work groups (per host), the **My work** chip and the row's work chip;
- with a **full** token: Confirm / *Not this* on a suggestion, set or clear
  a link, start work from a ticket (*Start here*) and resume past work;
- **Today** with *Copy standup*, and the ticket card with its acceptance
  criteria, **read-only** (decision D15): the card offers *Copy*, never
  *Send*;
- org labels, an org filter and each row's org colour bar.

- with a **full** token, **Name this work…** for a session with no work,
  and **Rename** for local work (D20; fleet-mobile M13.4a).

- with a **full** token, **multi-start**: several repositories at once,
  behind a confirm sheet; a cross-org start is refused in words (D15;
  fleet-mobile M13.4d).

What stays on the desktop: tracker and org administration, and retention. A **readonly** token
is served `work` but not `work_link`, so it only reads. No client token ever
reaches `work_admin`. Which of these screens your phone shows depends on its
fleet-mobile release; the hub gates each action by the token, not by the
app (a handover request, for instance, is `work_link`, so only a full token
can make one).

## The operator and confirmations

The UX agent (the operator, the agent panel's session) can use the work
tools like any client, with one rule (decision D12): **its starts and kills
always wait for you**, whatever `mcp.confirm_destructive` says. That covers
`work_link` start and resume, new and restarted sessions, safe kill, and
tidy-up kills: each returns `E_CONFIRM_REQUIRED` until you approve it in the
desktop's dialog. On a hub there is nobody to approve, so the operator's
starts and kills are refused (`E_FORBIDDEN`); there it offers archive or
snooze instead. The agent panel's **Tidy up done tickets** chip fills the
operator's composer with that request; it never sends it.

Your own starts and kills from the desktop are not gated by this rule. With
`mcp.confirm_destructive` on, a tidy-up batch that contains a kill still
asks you to confirm, like any kill.

## Settings

Every `work.*` setting, with its default. On a standalone desktop they are
in Settings → Limits (with its *Retention* and *Lifecycle* groups); on a hub, set them with `set_setting` (master token), read
them with `get_settings`. A test (`work_settings_are_in_the_user_guide`)
fails when a `work.*` setting in `service/settings.rs` is missing from this
table.

| Setting | Default | Range | What it does |
|---|---|---|---|
| `work.recent_days` | `14` | 1–365 days | how long ended work with no live session keeps a sidebar group |
| `work.sync_interval_secs` | `300` | seconds, `0` = off | seconds between tracker sync passes; read at start; under a minute is raised to one |
| `work.describe_cache_secs` | `300` | seconds, `0` = off | how long a full ticket description fetched by *Read the full description* (`work { action: describe }`) is reused before fleet asks the tracker again; the cache is swept with the tracker items, at a floor of 30 days when that retention window is `0` |
| `work.trusted_branch_projects` | `[]` | project ids | projects where a sole branch key links automatically; set from the popover, cleared with **Trust none** |
| `work.evidence_snippets` | `true` | on / off | keep a redacted ±40-character prompt snippet around a detected key as evidence |
| `work.session_start_context` | `false` | on / off | SessionStart hands Claude the linked ticket's context (synchronous hook; takes effect on re-provision) |
| `work.classify_nudge` | `false` | on / off | one note per conversation asking Claude to name its work after three unlinked turns |
| `work.summary_model` | `haiku` | `haiku` / `sonnet` / `opus` | the model a dead session's on-demand *Summarise* runs on, on the session's own host and account |
| `work.tidy_done_days` | `2` | 1–365 days | how long a linked ticket must be done before tidy-up suggests its session |
| `work.tidy_idle_hours` | `4` | 1–720 hours | how long a session must be idle before any tidy reason suggests it |
| `work.tidy_idle_unlinked_days` | `7` | 1–90 days | idle and unprompted days before a session with no work is suggested (`idle_unlinked`) |
| `work.auto_tidy` | `false` | on / off | the GC sweep acts on the allowed tidy reasons by itself (safe kill or archive only) |
| `work.auto_tidy_reasons` | `done_idle,pr_merged_idle` | any of `done_idle`, `pr_merged_idle`, `not_planned` | the reasons auto-tidy may act on |
| `work.retention.journal_days` | `365` | 0–3650 days, `0` = forever | work journal retention |
| `work.retention.tracker_items_days` | `180` | 0–3650 days, `0` = forever | retention of done tickets no link names |
| `work.retention.timeline_work_events_days` | `180` | 0–3650 days, `0` = forever | retention of handover, nudge and tidy timeline events |

Per-org settings, set on the org (Settings → Work → Organisations, or
`work_admin { action: "update_org", org_id, … }` on a hub), not here:

| Org setting | Default | Range | What it does |
|---|---|---|---|
| `auto_tidy` | `inherit` | `on` / `off` / `inherit` | overrides `work.auto_tidy` for the org's sessions |
| `isolate_sessions` | `false` | on / off | also hides the org's sessions from other orgs' hosts (D7) |
| `bound_sees_unassigned` | `true` | on / off | devices bound to the org (`fleet-hub pair --org`) also see unassigned work and sessions, as a host does; off, only the org's own (D31) |
