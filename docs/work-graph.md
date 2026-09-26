# Work: tickets, workstreams and the sessions doing them

Fleet knows **what work** each session is doing: a ticket such as `ABC-123`,
or a workstream you named yourself. With that it groups sessions by work,
lets you start a session from a ticket, resumes old work with its context,
and tidies finished work away without destroying anything.

You do not have to set anything up. With no tracker, no organisation and no
`gh`, keys in branch names already group sessions. A tracker (Jira,
GitHub Issues, Asana, Linear) only adds titles, status and a ticket list.

This page is the user guide. For the tool reference see
[control-api.md](control-api.md) (`work`, `work_link`, `work_admin`); for
running a hub see [hub.md](hub.md).

> **Screenshots.** Places marked *📷 Screenshot placeholder* are for the
> maintainer to fill in with a capture of the UI described next to them.

**Contents**

1. [What "work" is](#what-work-is)
2. [Linking and detection](#linking-and-detection)
3. [Trackers](#trackers)
4. [Start, multi-start and resume](#start-multi-start-and-resume)
5. [Handover](#handover)
6. [Today and standup](#today-and-standup)
7. [Tidy-up and auto-tidy](#tidy-up-and-auto-tidy)
8. [Organisations and isolation](#organisations-and-isolation)
9. [The phone](#the-phone)
10. [The operator and confirmations](#the-operator-and-confirmations)
11. [Every setting](#every-setting)

---

## What "work" is

- **A work item** is the thing being worked on. It is one of:
  - a **ticket** from a connected tracker, with its title and status;
  - a **local item**, a workstream you named yourself ("Name this work…");
  - a **bare key**, such as `ABC-123` seen in a branch name before any
    tracker knows it. It groups sessions all the same, and binds to its
    ticket by itself once a tracker that owns the key is connected.
- **A link** ties a work item to a session. A link is *confirmed* (a person,
  a start from a ticket, or the in-session agent said so), *suggested* (fleet
  guessed, and a person has not decided) or *rejected* ("Not this", which is
  sticky: that key is never proposed for that session again). A session has
  at most one **primary** link: the one its chip shows and it is grouped
  under.
- **Links follow the session's identity**, not its tmux name: a rename, a
  move to another host, a restore or a recreate keeps them.
- **When a session ends, its link ends with a snapshot** (host, branch,
  worktree, PR, conversations), and the **work journal** keeps what its
  conversations did. That is what makes a ticket resumable weeks after its
  session was killed.
- **A guess never groups a session.** Only a confirmed link does; a
  suggestion shows as a suggestion until you decide.

---

## Linking and detection

<!-- LINKING -->

---

## Trackers

A tracker makes work items real tickets. Sessions then show their ticket's
**title and status** on the chip and the group header, ⌘K lists your
tickets, and you can start a session from a ticket in one step. Trackers are
**read-only** (fleet never writes to them), **polled** (no webhooks), and
**never gate anything**: with a tracker down or its token expired, every
screen answers from the cache.

Supported: **Jira Cloud, Jira Data Center, GitHub Issues (github.com and
GitHub Enterprise Server), Asana, Linear.** They behave the same everywhere —
in ⌘K, on the chips, in start and resume, in detection. With trackers of two
or more kinds, a small provider badge tells them apart.

### Where trackers are configured

Trackers are fleet administration, so they live with the process that owns
the fleet:

- **A standalone desktop** (no hub): **Settings → Work → Trackers**.
- **A hub**: on the hub, with `fleet-hub tracker …` (below) or the master
  token's `work_admin`. A desktop paired with the hub shows the hub's
  trackers read-only and says so; the phone never configures them.

Only that process syncs. A paired desktop never polls a tracker itself.

### Connect a tracker

**Paste any ticket or issue URL.** Fleet infers the provider and the site
from it, then asks only for what that provider needs:

<!-- CONNECT -->

From a hub:

```sh
fleet-hub tracker add https://acme.atlassian.net/browse/ABC-123
fleet-hub tracker set-credential 1 --email you@acme.com < jira-token.txt
fleet-hub tracker test 1      # account, key prefixes, sprints, views
fleet-hub tracker list
fleet-hub tracker status      # each tracker's last sync pass, and retention
fleet-hub tracker remove 1    # its tickets stay, marked unavailable
```

A token is read from **stdin**, from an environment variable of that command
(`--from-env JIRA_TOKEN`), or stored as a **reference** the hub resolves on
each sync (`--ref env:NAME`, `--ref file:/run/secrets/jira`). It is never a
command-line argument, so it never lands in `ps` or shell history. Docker
secrets and the other options are in [hub.md → Trackers](hub.md#trackers).

After the first successful test, fleet binds every bare key it already saw
(in branch names, prompts, PRs) to its ticket, and offers to review the
sessions that mention the tracker's keys.

### Per provider

| Provider | What to paste | Credential | What fleet reads |
|---|---|---|---|
| **Jira Cloud** | any ticket URL, or `https://<name>.atlassian.net` | your **email + an API token** (id.atlassian.com → Security → API tokens; they expire within a year) | *My work*, *Current sprint* (projects with sprints), *Recent*, your favourite filters; status category and resolution; epics |
| **Jira Data Center** | the server's URL, provider *Jira Data Center* (`--provider jira_dc https://jira.corp.example[/jira]`) | a **personal access token** (Profile → Personal Access Tokens) | the same views and filters as Cloud, over API v2; the Epic Link field |
| **GitHub Issues** | any issue URL, or `https://github.com/<owner>`, plus the **host whose `gh` reads it** (`--via-cli <host>`, optionally `--repo owner/repo`) | **none in fleet**: `gh` on that host, logged in with its own `gh auth login` | issues assigned to you in the owner's repositories; open → to do (in progress when a branch or closing PR is linked), closed → done / not planned / duplicate |
| **GitHub Enterprise Server** | an issue URL on the instance, saying it is GitHub, or `--hostname ghe.corp.example[:port]` | none in fleet: `gh auth login --hostname …` on the host | as GitHub; keys are `host/owner/repo#n`, so the same repo name on github.com is never the same work |
| **Asana** | any task URL, or `https://app.asana.com[/<workspace>]` | a **personal access token** (Settings → Apps → Developer apps) | *My tasks*, one view per project your tasks are in, *Recent* (Premium); completed → done, otherwise the task's **section** through the section map |
| **Linear** | any issue URL, or `https://linear.app/<workspace>` | a **personal API key** (Settings → Security & access) | *My issues*, *Current cycle*, *Recent*; team keys are the key prefixes; canceled → not planned |

Provider notes:

- **GitHub** reads through `gh` run over SSH on the host you name. Fleet
  never reads, stores or sends a GitHub token. A host without `gh`, or with
  `gh` logged out, makes the tracker *unreachable* with the fix in its error.
- **Asana has no human keys.** A task's reference is its URL; the UI shows a
  short `Asana …123456`. Which **sections** mean *in progress* or *done* is
  guessed from their names on the first test and shown in Settings → Work
  with a **Confirm** button; once you confirm, your map wins.
- **Linear and Jira keys look alike** (`ENG-123`). A key belongs to the
  tracker whose probed prefixes (Jira projects, Linear team keys) include it.
  A prefix two trackers both claim is never bound automatically.
- **Jira Data Center** is fenced harder, because its site is whatever an
  admin types: https only, one exact host, and the hub refuses a site that
  resolves to a loopback or link-local address unless the tracker's
  `allow_private_network` is set. An internal CA goes into `extra_ca`.
- **A tracker only one machine can reach** (a VPN) is read with `curl` on
  that host: `--via-host <host>`. The token reaches the host on stdin and is
  never in an argument, an environment variable or a log. See
  [hub.md → Reaching a tracker from a host](hub.md#reaching-a-tracker-from-a-host-via_host).

### Sync and tracker states

The sync runs every `work.sync_interval_secs` (default 300 s; `0` turns it
off). Each pass reads every view from its watermark, refreshes every linked
ticket by id, and looks up keys typed before the tracker was connected. A
ticket that disappears is marked **unavailable** — deleted or no longer
visible to you; a tracker cannot say which — never deleted, and its links
stay.

| State | Meaning | What to do |
|---|---|---|
| `ok` | the last pass worked | — |
| `rate_limited` | the tracker said slow down (429, Linear's complexity limit, GitHub's quota) | nothing: it waits out `Retry-After` and retries |
| `unreachable` | network, DNS, `gh` missing or logged out on the host | fix the cause if it persists; it retries by itself |
| `auth_failed` | the token expired or was refused | set a new credential, then **Test**; polling stops until then |
| `captcha` | the site wants one browser login | log in once in a browser, then **Test** |
| `unconfigured` | no credential yet | set one, then **Test** |

### Trackers in fleet health

`fleet_health` (the footer on the desktop, `fleet_health` over the Control
API, the phone) carries a **tracker roll-up**, read from the sync's
in-memory state and the store — never a live call to a tracker:

- per tracker: **ok**, **degraded** (rate limited, briefly unreachable, one
  or two failed passes) or **failing** (a token refused or missing, a
  CAPTCHA, or three failed passes in a row), with the failures in a row, the
  last error and the last successful sync;
- fleet-wide: the **detection backlog** — suggestions nobody has decided on
  for 7 days or more.

The desktop's footer shows a `trackers: …` line when something is not ok,
and a **failing** tracker raises one Attention item per tracker, such as
**⚠ Reconnect Jira (acme) →**, that opens Settings scrolled to Work. A
degraded tracker raises none: it recovers by itself. A per-host token (the
Claude in a session) sees only its own organisation's trackers.

> 📷 **Screenshot placeholder:** the attention strip with a "Reconnect Jira
> (acme)" item, and the footer's `trackers:` line.

### Secrets

A token never leaves the tracker's own secret row: no answer, event, log
line, diagnostics bundle or error report carries it (a row shows only
`…abcd`), and an error is redacted before it is stored. Each provider's
site is fenced, and redirects are never followed.

---

## Start, multi-start and resume

<!-- START -->

---

## Handover

<!-- HANDOVER -->

---

## Today and standup

<!-- TODAY -->

---

## Tidy-up and auto-tidy

Fleet keeps the sidebar about current work. It **suggests** cleaning up and
never destroys anything useful by itself.

**Tidy up · n** appears in the attention strip only when there is something
to suggest. It opens a sheet grouped by reason; each row is preselected with
its default action, and **Tidy n** applies them. Keyboard: `j` / `k` move,
space toggles, Enter applies, Esc closes. Clicking a row narrows the sidebar
to that session so you can look before you tidy.

| Reason (as the sheet names it) | When | Preselected action |
|---|---|---|
| **Done and idle** | the linked ticket has been done for `work.tidy_done_days` (2) and the session idle for `work.tidy_idle_hours` (4) | Safe kill |
| **PR merged, idle** | the session's PR is merged and the session idle | Safe kill |
| **Won't do / duplicate** | the ticket was closed as won't-do or duplicate | Safe kill |
| **Duplicate on one worktree** | two sessions work in one worktree | Kill (the other session keeps the worktree) |
| **Lost session about to expire** | a lost session is a day from being removed | nothing destructive: its own **Resume**, or Snooze |
| **Idle, no work linked** | a work session with its own worktree, no link, idle and unprompted for `work.tidy_idle_unlinked_days` (7) | unticked; its own **Keep 7 d** and **Safe kill** (armed on the first click, applied on the second) |

A row with a link also offers **Archive only**, **Snooze 7 d** and **Never
for this work**.

- **Safe kill** asks Claude to commit and push first, and removes the
  worktree only after the push succeeded. A session with no work linked is
  killed only when its worktree is clean and pushed.
- **Archive** collapses a live session into its work group's *Done*. tmux
  keeps running, and the next prompt or attach brings it back.
- **Protected, always:** a session that is working, blocked, stuck or
  waiting on a dialog; one linked to an in-progress ticket; the controller
  and the operator; anything prompted or attached to within the hour; a
  background agent with open tasks. No setting overrides this.
- **Reopened · n** (in the attention strip) lists work that came back after
  it was done — a ticket moved out of done — with its past sessions and
  **Resume**. It stays until resumed, done again or dismissed.

> 📷 **Screenshot placeholder:** the Tidy-up sheet with two reasons and the
> "Tidy n · Cancel" footer.

### Auto-tidy

Off by default. With `work.auto_tidy` on, the GC sweep acts by itself on the
candidates whose reason is in `work.auto_tidy_reasons` (default
`done_idle,pr_merged_idle`; `not_planned` may be added). It only ever
**safe-kills** (or archives a session with no worktree fleet can inspect) —
never a plain kill — so a duplicate worktree or a lost session stays a
suggestion, and *Idle, no work linked* is never automatic. Every action is
written to the session's timeline and to its work journal. An organisation
can turn auto-tidy on or off for its own sessions (on / off / inherit).

### Retention

Fleet's own records of work are kept for a while and then swept, but never
while something still points at them:

| Setting | Default | What it keeps |
|---|---|---|
| `work.retention.journal_days` | 365 | the work journal (behind resume and the handover brief) |
| `work.retention.tracker_items_days` | 180 | cached tickets in *done* that no link names |
| `work.retention.timeline_work_events_days` | 180 | handover, nudge and tidy events on the timeline (the newest of each kind per session always stays) |

`0` keeps forever. A journal row of an open conversation, of a live-linked
session, or of work that is not done is kept whatever its age. The sweep
deletes at most 2,000 rows per table per tick. See
[hub.md → Work retention](hub.md) for the exact rules and
`work_admin { status | sweep_now }`.

---

## Organisations and isolation

Organisations are optional. With none, the sidebar's scope selector offers
the GitHub owners of your live sessions (only when there are two or more)
and nothing is fenced. Name an organisation to merge or split owners, to
attach a tracker to it, or to make it a **boundary** for the hosts you put
in it.

An organisation is two things:

- **A view for you.** The scope selector (**⌘⇧O** / **Ctrl+Shift+O**)
  narrows the sidebar, Today and ⌘K to one scope, with the org's colour bar.
  It never hides a session that needs you: "2 need you in Personal →"
  switches to that scope.
- **A boundary for the Claude on a host.** A host placed in org A reads only
  org A's work and unassigned work — tickets, trackers, links, the journal,
  briefs and the SessionStart context. An id outside the boundary answers
  exactly like one that does not exist. A host in no org reads unassigned
  work only.

**Which org a session is in:** the most specific matching rule — a path
prefix, then `owner/repo`, then `owner`, then a host-only rule — else its
host's org. A ticket's org is its tracker's.

**Set it up** in **Settings → Work → Organisations** on a standalone
desktop, or on a hub:

```sh
fleet-hub org add "Company A" --color '#e11d48'
fleet-hub org rule add 1 --owner acme
fleet-hub org rule add 1 --path /home/me/work/acme
fleet-hub org assign-host hetzner-a 1
fleet-hub org assign-tracker 2 1
fleet-hub org set 1 --isolate-sessions on       # optional, see below
```

- **Assign every host of a company before you connect a second company's
  tracker.** A key linked on an unassigned host belongs to no org, so any
  org's tracker may bind it.
- **Linking across orgs is refused for everyone** unless you confirm "Link
  anyway" (`force_cross_org`). Moving a session to a host of another org is
  refused the same way ("Move anyway"). Detection never guesses across orgs.
- **Sessions themselves are not fenced by default.** `isolate_sessions`
  (per org, off) also hides that org's sessions from other orgs' hosts. It
  can break a controller that dispatches across companies, which is why it
  is off.

Details: [hub.md → Organisations and isolation](hub.md#organisations-and-isolation).

---

## The phone

<!-- PHONE -->

---

## The operator and confirmations

<!-- OPERATOR -->

---

## Every setting

<!-- SETTINGS -->

---

## See also

- [concepts.md](concepts.md) → *Work*, *Lifecycle*
- [control-api.md](control-api.md) — the `work`, `work_link` and `work_admin`
  tools, and the generated [reference](control-api-reference.md)
- [hub.md](hub.md) → *Trackers*, *Organisations and isolation*, *Tidy-up
  and auto-tidy*
- [troubleshooting.md](troubleshooting.md) → *Work and trackers*
