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
- [The Work view](#the-work-view)
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

In the sidebar, **⋯ → Group by → Work** groups sessions by their
primary work. Work groups come first; sessions with no work stay under
their project. Each group's header rolls up the PR / CI state of its
sessions, and ended sessions show in a collapsed **Done · n** section of
the group. The **Filters** panel has a *Work* group: tracker (with two
or more trackers), status, the tracker's own column names, *Assigned to
me*, and in work mode *Session: Any / Active session / Past only*;
*Include → Archived work* shows or hides archived sessions and past
work. Archived work is hidden by default: the end of the list says *N
archived hidden* with *Show archived*, and *Session: Past only* shows it
regardless. Each filter that is on shows as a chip under the search, with a ×
and *Clear all*; an empty list names the filters that hide it.

> **[Screenshot placeholder]** The sidebar grouped by work, with a Done
> section open and the Filters panel's Work group showing.

## The task list and the task page (2026-09-29)

The Work tab opens in **List** layout; the header, filters and saved views are unchanged, and **List | Grouped** switches to the organisation → group tree described below.

- **List** sorts every task the filters match into **To do**, **Doing** (in progress) and **Done** (the last 7 days, collapsed). A ticket keeps its tracker's own status name in its row; its section follows fleet's effective status, by the same rule as the Board: only a bare key, which has no status, follows its live sessions.
- **+ New task** writes a task in fleet itself (`TASK-<id>`); ▾ opens the **New task** dialog, which ⌘N (Ctrl+Shift+N) also opens anywhere in Work: title, project, notes, and **Start a session for it now**, which opens the new task's start menu next. A new task stays in fleet: fleet creates no ticket in a tracker. Its **Work** button (below) opens a session in that project with the title and notes as its first prompt.
- **The Work button** (2026-10-06) is on every task row. Its main half reads **Open** (a live session: select it), **Continue** (no live session but a past one: resume its last conversation) or **Start**. Start asks fleet first where it would land; when everything is known and nothing is in the way it starts at once, with the brief on — for a Jira, Asana, Linear or GitHub ticket too. Otherwise it opens the *start popover*: pick the **Repo**, the **Host** (an offline one says so), edit the **Branch**, turn the **Brief** off or preview it as text, and see what is in the way: a live session on the task (Start then makes a *parallel* session in a checkout of its own, `<branch>-2`, never the same tree; *Open it* jumps there), a task that is done, a proposal (*Accept & start*), another organisation (tick *Start across organisations* if it is meant). ▾ offers *Start new…* (always the popover), *Continue* one of the last past sessions, *Attach running session…* and *Copy key*; Alt-click on Start opens the popover as well. In the list, `j` / `k` move the selection, `s` runs the selected task's Work button and ⇧S opens its popover. The task page's **Start new** asks the same way.
- **Attach running session…** (2026-10-06) puts a session that is already running on the task. The picker lists every session not on it yet — the same repository first, then sessions with no task, then idle ones — with a filter. A session already on another task offers **Switch** (that task's link ends and this one becomes its primary; the old task keeps the conversation, and *Continue* there resumes it) or **Add** (this task is a secondary link and the other stays primary). Add is the default while the session is mid-turn or when its checkout is named after the other key, Switch otherwise. When the task is already open in another session the picker says so (naming it when it is yours to see, "someone" otherwise) and the button becomes *Attach anyway*. For ten seconds after, *Undo* puts things back: a Switch switches back, an attach to a session with no task removes the link.
- **The task page** adds *Brief*, *Subtasks* (+ Add subtask, Start), *Proposals* (an agent's suggested subtasks — Accept / Reject; rejected ones behind a toggle), *Delegated jobs* with their result, and *Agent steps* — the agent's own `TaskCreate` / `TaskUpdate` todos per session, labelled "per the agent", never a status. Placement and rules sit under *Placement & rules*.
- A subtask started from the list gets the parent ticket's brief (only when you may see that ticket) followed by its own title and notes.
- Every `dispatch_task` job appears as an agent subtask under the requester's task and follows the job's state. The ☑ Tasks popover is gone; a session's jobs are still in its details.
- Tracker items stay read-only: nothing here writes to Jira, Asana or GitHub.
- Hosts pick up step capture after re-provisioning (they read `provision_stale` until then); meanwhile the Stop hook backfills steps from the transcript.

## The Work view

The sidebar has two ways into the same work: **Sessions** (host / project →
session → its tasks, as before) and **Work** (organisation → project or group
→ task → its sessions). Switch at the top of the sidebar or with ⌘⇧W
(Ctrl+Shift+W). Both are views of one graph of tasks, sessions and links; a
session under several tasks is the same session everywhere, and opening any
of its occurrences opens it.

**The tree.** Each organisation has its sections; each section is a
project or group with its count; each task shows its key and title, its
tracker, its status, how many active and past sessions it has, a dot when a
session needs you, **?** when something waits for review, a struck-through
title when the tracker no longer answers for it, and what is wrong with its
tracker when it is not answering (*tracker: token expired or wrong*,
*tracker: not tested yet*, …), which is not the same as having no sessions. A task
with no session at all stays in the tree. Under a task, each session says
what its link is:

| Mark | Meaning |
|---|---|
| ★ | the session's **primary** task (the one Sessions groups it by) |
| (plain) | a secondary task of a live session |
| dashed, `?` | a **suggestion** — fleet's guess, not a link |
| dimmed, *ended* | a **past** session; never shown as active |

Sections load page by page (*Load more*), so a fleet with thousands of
tickets stays quick; expanded sections and the last selection are kept per
view.

**Filters and saved views.** A search, the *Assigned to me* and *To
review* toggles, and a **Filters** panel laid out like the Sessions
list's: Organisation, Tracker (or *Local work* / *Bare keys*), Status
(Open, To do, In progress, Done) and Sessions (Active session, Past only,
No session, Suggested). Each filter that is on shows as a chip with a ×
and *Clear all*, and "No tasks match" names them. The view select saves
the current filters under a name (Save as…, Update, Delete); saved views
live on the hub, so the phone has the same ones. The Sessions list's own
filters (hosts, recency, org scope) do not apply here and step aside.
Archived tasks (done, or every session archived, with nothing running)
are hidden by default; the end of the tree says how many, with *Show
archived*, and the panel has an *Archived tasks* switch. *Status: Done*
shows done tasks regardless, and *Sessions: Past only* shows past work
(which is archived work), as the Sessions list's *Past only* does. A task
is archived only when it is archived for everyone: a session on another
host or in another org that you cannot see still keeps it in the tree.
⌘⇧O cycles the Work view's organisation.

**A task's detail** (select it) shows the tracker's data, where its org and
its group come from, the repositories it ran in, every session with its
state and *why* it is linked (the branch, the ticket URL in a prompt, a
person), the last known outcome of its newest past session, and **Open**,
**Continue** (resume the last conversation) and **Start new**. The ticket's
description shows its first 600 characters; when the ticket holds more, a
line under it says so — *Shown 600 of 6812 characters — open the ticket* —
and *open the ticket* opens it in the tracker. A description that fits has
no such line.

Beside a live session's PR the detail shows the PR's **Result**: *Ready*,
*Waiting*, *Blocked*, *Unknown*, *Merged* or *Closed*. The session's own
details list the reasons behind it: failing checks by name, uncommitted or
unpushed work, a worktree on another commit than the PR, a review or a
draft, merge conflicts. They also say which commit the reading is about and
when it was taken. Fleet explains here, it does not decide: *Ready* only
means nothing it read stands in the way, and a reading older than a
quarter of an hour is *Unknown*, whatever it said. The session row's CI
badge dims in that case too. See
`docs/specs/2026-09-29-result-evidence-design.md`.

**Where things come from.** Every value that fleet did not get from a person
says so:

- **Organisation**: *from the tracker* (a ticket's org is its tracker's),
  *set by a person* (a local task placed in an org), or *inferred from its
  sessions* — a view only, not a boundary. The organisation is the access
  boundary, so moving a task is a separate, previewed step (below).
- **Group**: *placed by a person*, *by a rule* (named), *from the tracker*
  (the Jira project, Linear team, GitHub repository, Asana project), *from
  its repository*, or *from its key prefix*. Placing a task in a group is
  local to fleet: it never changes the Jira or Asana ticket.

**Correcting it.**

- **Place in group…** puts one task in a group (pick one or type a new
  name; a note is optional). Clearing it returns the task to where it would
  sit by itself. If someone else placed it meanwhile you are told, with the
  current value, instead of overwriting it.
- **Make a rule for similar tasks…** is a separate step after a placement:
  choose the conditions (tracker, project/container, key prefix, words in
  the title, repository) and the group, **Preview** the tasks it would move
  (and how many a person already placed, which it leaves alone), then save.
  Rules are listed under the Work view's **⚙** (Rules), where each can
  be edited, turned off (the tasks go back at once) or deleted.
- **Assign org…** (local tasks only) first shows exactly what the move
  changes: the sessions that would then carry a task of another org, the
  hosts and org-bound devices that would stop or start seeing it, and how
  many journal entries and summaries go with it. It is applied only while
  that preview still holds. A ticket's org is its tracker's; move the
  tracker instead (Settings → Organisations, or `fleet-hub org assign-tracker`).

**A session's tasks.** A session's details list all its tasks — primary,
secondary, suggested and past — each with *why*. **Make primary** moves the
primary (every other link stays), **Remove** takes a mistaken link away,
**Work on task…** adds another task — a ticket or one of your own tasks
(`TASK-<id>`) — which becomes primary only if the session has none. When
that task is already open in another session it says so first, with
**Attach anyway**, *Open that one* and *Cancel*. **Show in Work view** jumps
to the task.

**Review.** The Work view's **Review · n** tab collects what needs a person:
suggestions (with their reasons), a session linked to another org's task
(kept deliberately with *Keep*, or removed), a link to a ticket the tracker
no longer shows, and a session with tasks but no primary. **Confirm**,
**Change…** (pick another task), **Reject** and **Keep** act on one item;
select several to decide them together — each item is checked on its own
and the ones that could not be applied stay, with the reason. The last
confirm or reject can be undone.

**Two devices at once.** A decision names the version of the link it was
made on. If the desktop and the phone change the same link or the same
primary, the second one is told what changed and gets the current value,
instead of silently undoing the first. Two starts or resumes of one ticket
make one session; the other device is pointed at it.

> **[Screenshot placeholder]** The Work view with an org, two groups, a task
> with a primary, a secondary and a past session, and the Review tab.

## The Work view (reads)

The hub answers the other way into the work graph — organisation → group →
task → *every* session of the task (primary, secondary, suggested and past),
including tasks with no session at all — as reads of the `work` tool
(work graph M14.1b; the screens above and the phone's *My work* use them):

- `work { action: tree, filters?, cursor?, limit?, per_task? }`: a page of
  tasks with their sessions, the section headers (`groups`, each with its
  count under the filters), and the orgs and trackers the caller sees.
  Filters: `org` (an id or `"none"`), `tracker` (an id, `"local"` or
  `"ref"`), `status`, `mine`, `has` (`active` / `past_only` / `none` /
  `suggested`), `review`, `query`, `group`, `archived`, and (redesign
  step 6.2) `assignee` (one person by name, any case), `status_name` (one
  tracker column, the tracker's own status name, any case) and `group_by`:
  what a section under each org is, `group` (the default: a person, rule,
  tracker container, repo or key), `org` (one section per org), `person`
  (the first assignee), `mission` (named only when the caller may read the
  mission), `account` (the account its sessions run on, an active one
  first) or `repo`. A task with nothing to group by sits in `none`, last;
  `group` then names a section of that grouping. An unknown `group_by` is
  refused; a hub from before 6.2 ignores all three. The Work board's
  panel chips pick several at once: `orgs` (ids or `"none"`, any of them)
  and `stages` (any of `backlog`, `in_progress`, `in_review`, `blocked`,
  `done`, matched against each task's `stage`: done, then blocked (waits
  for another task), then in review (its tracker column says review, or a
  live session has a pull request), then in progress (its tracker says so,
  or a session works on it), else backlog). They apply with `org` and
  `status` when both are set; an unknown stage or org word is refused, and
  an older hub ignores both. `hidden_by_filters` counts the tasks the
  filters hide that would show with none set (the archived switch kept;
  0 on a section's read): the list's "Hidden by filters · Show" row. A task is
  *archived* when it has no active session and is done, or every one of
  its links (at least one of them past) is archived, judged over every
  link of the task, not only the ones the caller sees. The tree hides
  archived tasks only when asked, with `archived: false` (the desktop
  always sends it); absent shows them, so a client from before the archive
  keeps seeing every task. `status: "done"`, a `done` stage and
  `has: "past_only"` show them anyway. `archived_hidden` says how many passed every other filter
  but were hidden that way (over the whole result, not the page). Each
  task carries `archived`; `task` / `session_tasks` / `review` answer archived
  tasks as any other. Pages are a keyset: pass
  `next_cursor` back with the same filters (other filters refuse it). No
  task is repeated across pages while the fleet changes; a task that moved
  meanwhile may be skipped until the next full read. A client (the desktop's
  Work view) can also send `sections` (up to 100 of `{ org_id, group_id,
  limit? }`) and `with_review_total: true`: the same read then answers
  `sections`, each exactly the first page that section's own read
  (`filters.org` / `filters.group`) would give, cursor included, and
  `review_total`, the review inbox's total — so one refresh is one read.
  Neither is in the tool's schema (an assistant pages a section by its
  filters), and an older hub answers without them.
- `work { action: task, task_id }` (`item:<id>` or `ref:<KEY>`): one task
  with every session and why it is linked, its tracker description (at
  most 600 characters, with `description_chars`, the full length fleet
  knows, and `description_truncated` when it shows less), the last known
  outcome, its placement and the rules that match it.
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
desktop's and the phone's screens make them). None of them is a new tool, and an
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

Local work is reached THROUGH the sessions linked to it, which is also how
fleet knows whose it is. So a local item whose last link you removed —
**Clear**, or an unlink — can no longer be renamed or given a status, by
you or by anybody else: with no link there is no record of whose work it
was, and fleet answers as it would for an item id that does not exist. It
still appears in the local-work list, and linking a session to it again
makes it writable again.

**Clear** removes the link without rejecting the key: fleet may propose it
again later. But not from the same evidence. When the session's branch,
its pull request's head branch or a closing reference of its pull request
is what named the key, fleet remembers that you cleared it and does not
link or suggest the key again from that branch or that pull request while
it stays the same (rule R9u). A different branch or pull request is new
evidence and is detected as usual; going back to the very branch you
cleared keeps it cleared. A mention in a prompt, a ticket URL or the pull
request's text can still suggest it. Use **Not KEY** when the key is never
this session's work. Only your *Clear* is remembered: when Claude or the
operator unlinks a key (a per-host token, `work_link { action: unlink }`),
it is a plain unlink.

Claude in the session can link its own work too (`work_link`, source
`agent`), which is how the friendly-name skill records "I'm working on
ABC-123". An agent cannot overturn your rejection: once you said *Not
this* to a key for a session, Claude linking or confirming that key is
refused (`E_FORBIDDEN`) and your rejection stands. Only you can link it
again.

Who decides is recorded by who is asking, not by what they claim. A link,
confirmation or rejection made through a per-host token (the host's own
Claude) or by the operator (the agent panel's session) is recorded with
source `agent`, even if it passes `source: manual`; one made from the
desktop, the master token or a paired phone is recorded as yours
(`manual`). The same holds for **Name this work…** (an agent's naming
records `agent`) and for a ticket **start** (an agent's start records
`agent_started`, a person's `started`). An agent's confirmation or start
therefore never counts as yours: it does not count toward a project's
automatic trust, it is not written back to a tracker, and the usage
summary counts it apart. When an agent
decides the same way you already did, your decision is kept.

### Status of work with no ticket

Work fleet tracks itself — named work with no ticket — carries one of three
states: to do, in progress, done. You never have to set it: fleet marks work
*in progress* while a session is working on it, and *done* once the pull
request it produced is merged. Setting it yourself overrides that for good; the
same work will not flip back because a session started again.

A ticket's status is not yours to set here — it belongs to Jira, GitHub, Asana
or Linear, and fleet would be overwritten on its next sync. Change it there.

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
are never keys. One resolver (`service/work/resolve.rs`, rules R1–R12)
decides what each sighting becomes:

- a **confirmed automatic link**, only for strong, unambiguous signals, for
  example a ticket URL that is the only candidate, or a sole branch key in
  a **trusted** project. A toast says "Linked NAME → KEY (branch) · Undo";
  Undo is *Not this*.
- otherwise a **suggestion**. A suggestion never regroups a session. It is
  shown as a dashed chip with `?` and waits for a person.

A project becomes trusted when you tick **Trust branch keys in this repo**
in the popover, or automatically after you confirmed three branch
suggestions in it (a suggestion only a pull request made, or one an agent
confirmed, does not count). Settings → Limits → Lifecycle shows how many projects are trusted and has a
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

The rule tag (R1…R12) says which resolver rule decided it. With
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

### Jev's proposal (off by default)

With `decide.jev.work_link` on `assist`, the same unlinked conversation can
get a suggestion from the decision model instead, without touching the
conversation: Jev reads the first prompt (keys, links and the branch
removed) and picks one of the same candidates or none. Its answer is a
pre-selected suggestion (rule R12, source `jev`), marked ✦ and "Proposed by
Jev" on the row and in Review, that you confirm or reject. See
[decisions](decisions.md#work_link--the-work-item-of-a-session-no-rule-could-link-j1).

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

> **[Screenshot placeholder]** Settings → Trackers with one connected tracker
> and the Add tracker steps open.

### Per provider

| Provider | Paste | Credential | Notes |
|---|---|---|---|
| **Jira Cloud** | `https://<name>.atlassian.net` or any ticket URL | Atlassian email + API token (id.atlassian.com → Security → API tokens; they expire within a year) | *My work*, *Current sprint*, *Recent*, favourite filters; epics by hierarchy level |
| **Jira Data Center** | the site, with provider `jira_dc` (`--provider jira_dc`) | personal access token | one exact host, https only; the name is resolved and a loopback / link-local address is refused unless `allow_private_network`; an internal CA goes in `extra_ca` (PEM) |
| **GitHub** | `https://github.com/<owner>` or any issue URL, plus a host where `gh` is logged in (`--via-cli <host>`) | **none in fleet**: `gh` on that host uses its own `gh auth login` | `assignee:@me` issues in the owner's repos (`--repo owner/repo` narrows); fleet refuses to store a GitHub token |
| **GitHub Enterprise Server** | an issue URL, provider GitHub, plus the **hostname** (`--hostname ghe.corp.example[:port]`) and `--via-cli <host>` | none: `gh auth login --hostname …` on that host | keys are `host/owner/repo#n`, so the same repo name on github.com is different work |
| **Asana** | `https://app.asana.com[/<workspace>]` or any task URL | personal access token | tasks have no human keys: detection is by URL. Which sections mean *in progress* is guessed on the first test and shown with a **Confirm** button; your confirmed map wins. A section the guess cannot place counts as *to do*; with the experimental `status_map` decision feature on (off by default, [decisions.md](decisions.md#status_map--asana-section-proposals-j3)) the hub proposes a category for it, which you apply with `fleet-hub tracker section-map` or decide one at a time with `fleet-hub decide proposals apply\|reject <run>`; on a standalone desktop in `assist`, Settings → Trackers lists them as *Proposed by Jev (assist)* with the confidence and why, and **Apply**, **Apply as…** or **Not this** |
| **Linear** | `https://linear.app/<workspace>` or any issue URL | personal API key | team keys are the prefixes; *My issues*, *Current cycle*, *Recent* |

A tracker only one machine can reach (a VPN) is read with `curl` on that
host: `--via-host <host>`, the token piped on stdin. A key prefix that two
trackers both claim (`ENG` in Jira and Linear) is never bound automatically.
The full details are in [hub.md → Trackers](hub.md#trackers).

### Write-back: the PR link (Jira, off by default)

For a Jira tracker (Cloud or Data Center), Settings → Trackers has **Link
pull requests** (add a session's pull request to its ticket as a link).
With it on, when a session has a pull request, fleet adds that PR to the
linked ticket once, as a Jira remote link titled `PR: owner/repo#n`. The
write is queued when the PR probe sees the PR (or its state change), when a
person links or confirms work on a session that already has a PR (also when
the sync binds a key typed before the tracker was connected), and when you
turn the setting on, for every PR already open on a session linked to one of
the tracker's tickets. Nothing else is ever written: no transition, no
worklog, no comment (D29), and nothing a transcript or a tracker wrote.

- **Only work a person linked.** The link must be confirmed and made by
  hand or by *Start* (`manual` / `started`). A detection guess, an agent's
  suggestion, and a link, confirmation or ticket start an agent made
  (`agent` / `agent_started`) never write.
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

### Reading a ticket's description

Fleet caches the first 2,000 characters of a ticket's description, and shows
less than that in places with less room (a card, a start brief). When a
description is longer than what a place can show, what Claude is handed ends
with one line naming the cut, for example:

```
[shown 2000 of 6812 chars of the description — work { action: describe, key: "ABC-1" } for the rest]
```

You do not have to do anything about it. The line is there so Claude knows it
is holding part of a requirement rather than all of it, and can ask fleet for
the rest itself — which it does with `work { action: describe }`, one extra
read of that one ticket. Without the line, an agent would work from a third of
a ticket believing it had the whole thing, which is the mistake this exists to
prevent. A description that fits carries no line at all, so a line means
there really is more.

Only Jira (Cloud and Data Center) and GitHub serve a full description on
demand; for Asana and Linear the line says *open the ticket* instead, and the
ticket's URL is in every answer that carries its description. Nothing is
written to the tracker either way — this is a read. `describe` itself stops
at 32,000 characters; a longer description ends with the same kind of line,
saying *open the ticket* for the rest.

What Claude fetches is held briefly (`work.describe_cache_secs`, 300 s by
default) so a second question about the same ticket costs no second request,
and it is never mixed into the ticket's cached excerpt, sent to a phone, or
put in a session's event history. Disconnecting a tracker deletes it.

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
names the session `KEY title` and links it with source `started`
(`agent_started` when an agent — a per-host token or the operator —
started it: the same start, but not your decision). With
**Brief Claude** on (the default), the ticket's context, its description
fenced as untrusted, rides the first hook's context, and a short start
prompt is typed only once Claude's REPL is ready, never into a trust
dialog. The brief is editable before you start.

**Draft with Claude** (beside the brief in New session, and in the start
popover) asks for a brief written from the ticket and from what earlier
sessions on the same task left: commit subjects, first prompts, progress
notes, summaries and hand-offs. Fleet scores each of those by the words it
shares with the ticket and gives the model the best twelve that fit 3,000
characters, most relevant first, fenced as untrusted (Jev J4). The run is
one `claude -p` on the host the start would land on, only when the ticket's
organisation may reach that host, with the model `work.summary_model` names,
no tools, no MCP servers, no hooks and no transcript; its cost is booked as
`brief`. The draft is the field's text: edit it, **Regenerate** it, or
**Clear** it to go back to the ticket brief. Nothing is sent until you
start the session. The wire is `work_link { action: preview_start,
draft_brief: true }`; a hub older than this answers the ticket brief and
the field says it cannot draft yet.

If a live session is already on that key, the dialog says so ("ABC-123
already running on X") and offers **Jump** instead of starting a second
one. The Work tab's start popover offers a **parallel** start instead: a
second session on the key in a checkout of its own (`work_link { start,
parallel: true }`); `work_link { preview_start }` is the preview it
reads, which makes nothing. A hub older than the preview has no such
action, and the Work button then starts as it did before. While another device is still starting or resuming the same key (a
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
| `same_work` | Jev proposed (`decide.jev.related_session`, assist) that two running sessions do the same work; the idler is suggested | not ticked; never automatic |
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
  one addressed to a live session. Work fleet tracks itself keeps its
  journal and handover history whatever its status — marking your own work
  done never puts its history on a clock — the same line the ticket cache
  draws by sweeping only tickets.
- `work.retention.tracker_items_days` (180): cached tickets in done. Kept
  while any link, live or ended, names one, and while it is the parent of a
  kept ticket.
- The describe cache (`work.describe_cache_secs`'s table, one item's whole
  description) is swept with the tracker items, by the same window — except
  its `0` is never "forever": with `work.retention.tracker_items_days` at
  `0`, the describe cache is still swept at a fixed 30-day floor, so a
  full-text cache never becomes an unbounded copy of every description
  fleet ever fetched. It is the one swept table with no liveness rule — a
  cached description goes on age alone, even for a ticket a live session is
  working on, because the next `describe` simply fetches it again. That
  window is also the ceiling on how long an entry is *served*: a longer
  `work.describe_cache_secs` is clamped to it. Disconnecting a tracker
  deletes its items' cached descriptions at once, and so does a sync that
  changes a description — its first 2,000 characters, its length, or the
  ticket's "updated" time at the tracker, so an edit past the excerpt is not
  served stale.
- `work.retention.timeline_work_events_days` (180): handover, nudge, tidy and withdrawn-suggestion
  timeline events. The newest of each kind per session stays.
- The write-back outbox (see *Write-back*) follows the journal's window:
  a PR link that was sent, or given up on, goes once it is older than
  `work.retention.journal_days`; one still waiting is never swept. It has
  its own row (`tracker_writes`, "PR link outbox") in the retention status.

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

Orgs are managed in Settings → Organisations — on a paired desktop too,
when the hub trusts it — or with `fleet-hub org …` on a hub. **Assign every host of a company before
connecting a second company's tracker.** The details and commands are in
[hub.md → Organisations and isolation](hub.md#organisations-and-isolation).

Each org's page opens with an **Overview**: how many of its live sessions
you can see, and how many of them need you. Under *What belongs to it* it
lists the org's asset catalogs next to its rules, hosts and trackers.
**Devices** lists the phones, browsers and desktops bound to it, with
read-only and trusted marked, and binds or unbinds one. Only the hub's
operator sees that list: the master, or the owner's own device bound to no
org.

Settings → **Company** holds the rest of the company's administration:
**Organisations**, **Devices** (pair a phone or a browser — the page shows
the one-time code, its link and a QR to scan — trust it, bind it to an org,
hand it to a person, let it change a catalog, or revoke it) and **People**
(rename, or disable someone, which revokes their devices and the shares
made to them). On a desktop paired with a hub these change the hub's orgs,
devices and people, and need this desktop to be trusted (`fleet-hub client
trust <name>`); none of them can lock out the device you are using. The
**Its own settings** on an org's page let it keep a different value of a
few settings — the classification nudge, the summary model, how long Tidy
up waits for an unlinked session, and its budgets — while every other
session keeps the fleet's. The overview shows what the org's sessions cost
today, over the last 7 days and this month (counted from the upgrade that
added it), and a daily or monthly budget (Settings → Limits → Company
budgets, or per org) raises an Attention item when the org reaches it;
fleet only warns. Spend is shown only to someone who sees every session.
**Members** puts people in the company, each as an admin (administers the
org — its settings, members and their devices), a member (sees its work and
what is shared with it) or a viewer (reads only). A member's devices are fenced
to the org. An org's admin manages it from their own device; the hub's owner
keeps the rules, the trackers, people's names and which company owns the hub.
Share a session with your whole team from the Share sheet (*an org*): it
reaches the people in the org now, not someone who joins later. Removing a
member takes back what was shared with them on the org's sessions; nobody —
no admin either — reads someone else's private session. Details:
`docs/hub.md` → *Companies: members and roles*.

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
  opens Settings → Trackers.
- Each **failing** tracker raises one item in the attention strip,
  **⚠ Reconnect Jira (acme) →**, which opens Settings → Trackers.
  Hover it for the error, the failure count and the org. A degraded tracker
  raises none.
- A tracker failing because it keeps **skipping items** raises
  **⚠ Sync skipping items — Jira (acme) →** instead: reconnecting would not
  help. It also opens Settings → Trackers, where the tracker's last pass reads
  `… · 2 skipped (3 passes in a row)` with the reason. See
  [troubleshooting.md → Sync skips items](troubleshooting.md#sync-skips-items).
- The roll-up is read at startup and every 60 seconds. On a paired desktop
  it is the hub's `fleet_health`.

After you set a new credential and **Test** it, the item goes away once the
tracker is `ok` and a sync pass has succeeded, at the next read after that. The same numbers, with each pass's details, are in `fleet-hub tracker
status` and Settings → Trackers. See
[troubleshooting.md → Tracker sync fails](troubleshooting.md#tracker-sync-fails).

> **[Screenshot placeholder]** The attention strip with a Reconnect item,
> and the footer's trackers line.

## Usage summary

`work_admin { action: usage, days? }` (on a hub, `fleet-hub work usage
[--days N] [--json]`; on a standalone desktop, Settings → Usage → *Work
graph usage*, also linked from Settings → Work & trackers)
counts how the work graph is actually used over the last `days` (default
30, 1 to 365). It is read-only and master-only (a per-host or client token
is refused, and a paired desktop shows no counts: the page says to read
them on the hub). It records
nothing, sends nothing anywhere, and holds counts and ids only: never a
title, key, path or error text.

| Group | What it counts |
|---|---|
| links | links made, per `source` (`manual`, `started`, `branch`, `resumed`, `agent`, `agent_started`, …); a suggestion a person decided reads `manual`, one an agent decided `agent`; a ticket an agent started reads `agent_started` |
| detection | suggestions made, confirmed by a person, confirmed by an agent, promoted by detection itself, rejected, withdrawn (detection took it back: withdrawn or decayed), carried (a resume, fork or inherit carried the same work onto the session and settled it), expired (the session ended undecided); the median time from suggestion to a person's decision; classification nudges |
| handover | handovers requested and written, turns that ended without one, requests that could not be sent |
| resume | resumes, with and without a brief |
| journal | briefs queued and delivered, compaction summaries harvested, session summaries written (`work_link { summarize }`), PR links written back to a tracker |
| tidy | sessions tidied from Tidy-up, *Keep* answers, auto-tidies per reason |
| trackers | per tracker id: passes, failed passes and items skipped since the syncing process started (not windowed, reset on restart) |

Some things are not stored anywhere, so the answer lists them under
`unrecorded` instead of guessing: suggestions *shown*, handovers refused as
busy, `last` vs fresh resumes, the transcript probe's outcomes, Tidy-up's
suggestions per reason before anything is applied, and multi-start runs.
A suggestion that detection withdrew (its branch or PR moved on) or let
decay (an event suggestion not seen again after a conversation boundary)
loses its row, but leaves a `work_suggestion_withdrawn` event on the
session's timeline, holding only ids and rule words; `withdrawn` counts
those, and `suggested` includes them. So does a suggestion fleet settled by
carrying the same work onto the session (a resume, a fork, a review or
worker inheriting its parent's work): its event's reason is `carried`, and
`carried` counts it apart from what detection took back. The counts are
bounded by retention and by the timeline's cap per session (500 events),
so `suggested`, `withdrawn` and `carried` are floors.

Paste it into an acceptance run's record: that gives the decisions real
numbers.

```
$ fleet-hub work usage --days 30
work graph usage, last 30 d
links: 41 made (branch 12, manual 20, resumed 3, started 6)
detection: 18 suggested, 9 confirmed by a person, 1 confirmed by an agent, 4 promoted, 3 rejected, 2 withdrawn, 0 carried, 1 expired; median decision 12 min; 2 nudges
handover: 5 requested, 4 written, 1 missing, 0 send failed
resume: 3 (2 with a brief, 1 without)
journal: 8 briefs queued, 7 delivered; 11 compaction summaries, 2 session summaries, 1 PR links written
tidy: 6 applied, 2 kept, 0 auto-tidied (none)
tracker 1: 288 passes, 3 failed, 0 items skipped (since the sync started)
not recorded: suggestions shown (only made, confirmed, rejected and expired are stored)
…
```

## The phone

The phone app (fleet-mobile) reads work from a hub over its paired client
token:

- work groups (per host), the **My work** chip and the row's work chip —
  though not yet local work's live *in progress* / *done* (native item
  status): the phone's wire model predates that field and reads a bare
  key's status the old way, so a fleet-mobile release must add it before a
  phone shows the same answer the desktop does for work with no ticket;
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

**Brainstorming with the operator.** Ask the operator to brainstorm or plan
something new and it works in four stages: options, a choice, decisions, a
plan. Each stage waits for your answer. When you say yes to the plan it
creates a task (`TASK-<id>`) whose notes hold the plan, and proposes up to
ten subtasks under it. You accept or reject each one; the operator cannot,
and it starts nothing: press **Work** on a subtask when you want it done.
The skill is `skills/fleet-brainstorm/SKILL.md`, written into the
operator's directory when it starts.

**Watching a start, and cancelling it.** After **Work** starts a session,
a strip under the button shows its steps: session, checkout, Claude ready,
brief sent. When Claude asks you to trust the folder it says "Waiting for
you" with an Open button that takes you to the terminal. Until the brief is
in, **Cancel start** ends the session and removes the checkout and branch
the start made. It refuses, and touches nothing, when the checkout was not
made by that start or already has changes or commits of its own.

Your own starts and kills from the desktop are not gated by this rule. With
`mcp.confirm_destructive` on, a tidy-up batch that contains a kill still
asks you to confirm, like any kill.

## Settings

Every `work.*` setting, with its default. On a standalone desktop they are
in Settings → Limits (with its *Retention* and *Lifecycle* groups); on a hub, set them with `set_setting` (master token), read
them with `get_settings`. This table is generated from
`service/settings.rs` (`settings_docs_are_current`); every setting is also
in [the settings reference](settings-reference.md).

<!-- BEGIN GENERATED: settings work. -->
<!-- Generated from service/settings.rs: REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current -->
| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `work.retention.journal_days` | `365` | 0–3650 days, `0` = forever | fleet | Days a work journal row is kept once its conversation ended and its work is done or unlinked. |
| `work.retention.tracker_items_days` | `180` | 0–3650 days, `0` = forever | fleet | Days a done ticket that no session links to is kept in the cache. |
| `work.retention.timeline_work_events_days` | `180` | 0–3650 days, `0` = forever | fleet | Days handover, nudge, tidy and withdrawn-suggestion timeline events are kept; the newest of each kind per session always stays. |
| `work.recent_days` | `14` | 1–365 days | fleet | How long ended work with no live session keeps a sidebar group. |
| `work.sync_interval_secs` | `300` | seconds, `0` = off | fleet | Seconds between tracker sync passes. Under a minute is raised to one. Applies after a restart. |
| `work.describe_cache_secs` | `300` | seconds, shown in minutes, `0` = off | fleet | How long a fetched ticket description is reused before the tracker is asked again; never longer than the done-tickets retention window. |
| `work.trusted_branch_projects` | `[]` | JSON array of ids | fleet | Projects where a sole ticket key in the branch name links automatically; elsewhere it is a suggestion. Set from the work popover. |
| `work.evidence_snippets` | `true` | on / off | fleet | Keep a short, redacted prompt snippet around a detected ticket key as evidence. Off keeps only the matched text. |
| `work.session_start_context` | `false` | on / off | fleet | Give Claude the linked ticket at session start. Makes the start hook synchronous, which can add up to 2 s when the hub is down. Experimental. Applies when the hooks are next installed. |
| `work.classify_nudge` | `false` | on / off | fleet, per org | After three prompts with no ticket, ask Claude once which of your few open tickets it is on. Its answer is only ever a suggestion. Experimental. |
| `work.summary_model` | `haiku` | `haiku` / `sonnet` / `opus` | fleet, per org | The model Summarise runs on for a past session, on that session's own host and account. |
| `work.help_model` | `haiku` | `haiku` / `sonnet` / `opus` | fleet | The model that answers a question asked at a terminal or composer prompt line, on the session's own host and account. |
| `work.draft_commit_messages` | `false` | on / off | fleet | Files tab: Draft writes a commit message from the staged diff with claude -p on the session's host. A draft is text you edit and commit yourself. |
| `work.draft_briefs` | `false` | on / off | fleet | Starting from a ticket: Draft writes the agent's brief from the ticket before the first prompt, on the planned host. |
| `work.draft_release_notes` | `false` | on / off | fleet | Finish: Draft writes a finished mission's release note from its merged PRs, on the mission's planner host. |
| `work.catch_up_summaries` | `false` | on / off | fleet | A watched session offers "Since 13:20": what it did since you last looked, summarised on its own host and account. |
| `work.tidy_done_days` | `2` | 1–365 days | fleet | Days a linked ticket must be done before Tidy up suggests its session. |
| `work.tidy_idle_hours` | `4` | 1–720 hours | fleet | Hours a session must be idle before any tidy reason suggests it. |
| `work.tidy_idle_unlinked_days` | `7` | 1–90 days | fleet, per org | Days a session with no work linked must sit idle and unprompted before Tidy up suggests it. Only ever suggested, never auto-tidied. |
| `work.auto_tidy` | `false` | on / off | fleet | Let the GC sweep act on the allowed tidy reasons by itself, by safe kill or archive only. Off, Tidy up only suggests. An organisation can override it. Asks to confirm. |
| `work.auto_tidy_reasons` | `done_idle,pr_merged_idle` | any of `done_idle`, `pr_merged_idle`, `not_planned` | fleet | The tidy reasons auto-tidy may act on. |
<!-- END GENERATED: settings work. -->

Per-org settings, set on the org (Settings → Organisations, or
`work_admin { action: "update_org", org_id, … }` on a hub), not here:

| Org setting | Default | Range | What it does |
|---|---|---|---|
| `auto_tidy` | `inherit` | `on` / `off` / `inherit` | overrides `work.auto_tidy` for the org's sessions |
| `isolate_sessions` | `false` | on / off | also hides the org's sessions from other orgs' hosts (D7) |
| `bound_sees_unassigned` | `true` | on / off | devices bound to the org (`fleet-hub pair --org`) also see unassigned work and sessions, as a host does; off, only the org's own (D31) |
