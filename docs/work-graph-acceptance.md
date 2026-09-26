# Work graph: the manual acceptance (M10.3)

This is the one written, manual acceptance run of the work graph, for the
fleet's owner to walk through on a **real** installation: a real hub, real
hosts, a real tracker and a real phone. It gathers what the M3–M9 plans,
M11's and M12's *Acceptance (manual)* sections, and the M12.6 review
(*What M10.3 should capture*) asked to be checked by hand, in the order the
work is actually done.

The automated half is `scripts/hub-e2e.sh` (hub W, M10.2), which runs the
same flows against a fake tracker and a fake Claude. This document is the
half that no script can do: your tracker, your repositories, your phone,
your judgement.

**The user runs it; agents do not claim it.** An agent may help you run a
step, but only a person ticks a box.

How to use it:

1. Copy this file (or work in it directly on a branch), fill in the
   *Run record* below, and go through the steps in order. Later steps use
   what earlier ones made (a started session, a handover, a done ticket).
2. For every step tick `pass` or `fail` and write what you saw under
   *Notes*, even for a pass when something surprised you.
3. A step marked **(optional)** can be skipped when its prerequisite is
   missing (no second org, no GitHub Enterprise); write "skipped" and why.
4. Answer the questions in [Evidence for the decisions](#evidence-for-the-decisions)
   as you go: they are why the run exists as much as the pass / fail marks.
5. Finish with the [Summary](#summary), the [Found issues](#found-issues)
   table and [After the run](#after-the-run).

The feature itself is explained in [work-graph.md](work-graph.md); the hub's
side in [hub.md](hub.md); tool parameters in
[control-api.md](control-api.md).

## Run record

| | |
|---|---|
| Date(s) of the run | |
| Who ran it | |
| Hub version (`fleet-hub --version`) | |
| Version upgraded from | |
| Desktop version (About) | |
| Hub deployment (Docker / bare binary) | |
| fleet-mobile release on the phone | |
| Hosts (alias, org, SSH or agent) | |
| Orgs | |
| Trackers (provider, id, org) | |

## Prerequisites

Tick each before starting.

- [ ] **Version.** The release under test is **v0.3.0 or newer**, for the
  hub and the desktop. You also have the release you run today if it is
  older (step 1 upgrades from it).
- [ ] **Backups.** A way to back up `state.db` for the hub and for the
  desktop, as in [RELEASING.md → Upgrading into the work graph](RELEASING.md#upgrading-into-the-work-graph)
  and [hub.md → Upgrade and rollback](hub.md#upgrade-and-rollback). Step 1
  takes them; keep them until the run is recorded.
- [ ] **One hub**, reachable from the desktop and the phone, and a desktop
  **paired with it** (Settings → Hub). `mcp.confirm_destructive` is **off**
  on the hub (its default; see [hub.md → Security notes](hub.md#security-notes)).
- [ ] **At least two hosts** provisioned from the hub, each with tmux,
  git and Claude Code logged in. Call them *host A* and *host B* below.
- [ ] **Ideally two organisations**: host A's work belongs to one company
  (*org A*) and host B's to another (*org B*). Without a second org, the
  isolation steps (Part O) are skipped.
- [ ] **One tracker** you can use for real: a Jira Cloud site (email +
  API token) or GitHub (`gh auth login` on one host). You can move a
  ticket's status and revoke the token or log `gh` out during the run.
  At least **four tickets** assigned to you, not done, in a project whose
  repository fleet knows. Call them *T1*–*T4* (for example `ABC-101` …).
- [ ] **One key that ran in two repositories before** (a branch named
  after it in each), for multi-start (step 16).
- [ ] **A phone** with fleet-mobile installed, able to reach the hub.
- [ ] **On the hub machine:** `curl` and `jq`, for the API steps.
- [ ] **(optional)** A GitHub Enterprise Server instance and a host whose
  `gh` can log in to it (Part P).
- [ ] **For Part M:** a machine or user account with **no SSH access to
  your hosts**, to run a throw-away test hub on a copy of the database.

### The API helper

Several steps read the hub's answers directly. On the hub machine, once:

```sh
export HUB=https://fleet.example.com        # your hub's public URL
docker compose exec -T fleet-hub fleet-hub token show   # bare binary: fleet-hub token show
read -rs TOKEN; export TOKEN                # paste the master token printed above

call() {  # call TOOL ['{"json":"arguments"}']
  curl -s --compressed "$HUB/mcp/json" \
    -H "Authorization: Bearer $TOKEN" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":${2:-"{}"}}}" \
  | jq '(.result.content[0].text | fromjson?) // .'
}

call fleet_health | jq '{db_ready, version}'
```

`call` prints a tool's JSON answer, or the whole response when the answer
is an error. The token never goes into an argument or the shell history.

---

## Part A: upgrade (M12.1)

### Step 1: back up both databases

**Action.**
- Hub (Docker): `cd ~/fleet-hub && docker compose stop fleet-hub && docker compose cp fleet-hub:/var/lib/fleet-hub/state.db "state.db.$(date +%F).bak"`.
  Bare binary: stop the service and copy `<data-dir>/state.db` with any
  `state.db-wal` / `state.db-shm`.
- Desktop: quit the app and copy `state.db` (macOS
  `~/Library/Application Support/sk.rlt.claude-fleet/`, Linux
  `~/.local/share/claude-fleet/`) with its `-wal` / `-shm` files.

**Expected.** Two copies exist, with a non-zero size; nothing else changed.

Result: [ ] pass / [ ] fail

Notes:

### Step 2: upgrade the hub

**Action.** Follow [hub.md → Upgrade and rollback](hub.md#upgrade-and-rollback):
point `image:` at the release under test (no `v` in the tag), then
`docker compose pull fleet-hub && docker compose up -d fleet-hub`, then
`docker compose exec fleet-hub fleet-hub --version` and `docker compose ps`.
Time from `up -d` to `healthy`.

**Expected.** `--version` names the release under test; STATUS reaches
`healthy` within a minute; `docker compose logs fleet-hub` shows the
migrations and no error. `call fleet_health` answers `db_ready: true` and
the new `version`.

Result: [ ] pass / [ ] fail

Notes (time to healthy, the migration lines):

### Step 3: upgrade the desktop

**Action.** Install the release under test on the desktop and start it.

**Expected.** It starts without a schema error and reconnects to the hub
(Settings → Hub shows it connected).

Result: [ ] pass / [ ] fail

Notes:

### Step 4: nothing was lost

**Action.** Compare what you had before with what you see now: the host
list, the session list (live and lost), a session's timeline and its
Conversations panel, your paired clients (`fleet-hub client list`).

**Expected.** Every host, session, conversation and client is still there
and unchanged. No session was killed or restarted by the upgrade.

Result: [ ] pass / [ ] fail

Notes:

## Part B: work with no tracker

### Step 5: grouping by branch keys

**Action.** Before any tracker is connected (or with them all
disconnected, if you already had one): make sure at least two live
sessions sit on branches named after a ticket key (for example
`abc-101-login`). In the sidebar choose **View ▾ → Work**.

**Expected.** Those sessions are grouped under their key (a bare key, no
title); sessions with no key stay under their project; nothing asks for a
tracker. Each group's header rolls up its sessions' PR / CI state.

Result: [ ] pass / [ ] fail

Notes:

### Step 6: linking by hand

**Action.** On a session with no work, open the row's **#** menu and set
T1's key. On another, pick **Not KEY** for its recognised branch key. Then
**Clear** the first one.

**Expected.** The first session moves into T1's group with a solid chip,
then back out on Clear. The rejected key is not suggested again for that
session (it stays out after a hub restart too).

Result: [ ] pass / [ ] fail

Notes:

## Part C: connect a tracker, reading the guide cold (M12.5)

### Step 7: connect it using only the guide

**Action.** Open [work-graph.md → Connecting one](work-graph.md#connecting-one)
and follow it, and only it (plus the links it gives), to connect your
tracker **on the hub**:

- Jira Cloud: `fleet-hub tracker add https://<site>.atlassian.net/browse/<T1>`,
  then `fleet-hub tracker set-credential <id> --email <you> < token.txt`
  (or `--from-env` / `--ref`), then `fleet-hub tracker test <id>`.
- GitHub: `fleet-hub tracker add https://github.com/<owner> --via-cli <host>`
  on a host where `gh auth status` is logged in, then `fleet-hub tracker test <id>`.

(With Docker, prefix each with `docker compose exec fleet-hub`, and `-T`
when piping a token in.) Write down every place you had to guess, look
elsewhere, or ask.

**Expected.** `tracker test` reports your account, the key prefixes and
the views (*My work*, *Current sprint* where there is one, *Recent*,
favourite filters). `fleet-hub tracker list` shows it `ok`. The token never
appeared on a command line or in the output (only a `…abcd` hint).
On the desktop, Settings → Work lists the tracker **read-only** and says
it is configured on the hub.

Result: [ ] pass / [ ] fail

Notes (every guess or detour, with the section of the guide):

## Part D: sync and tickets

### Step 8: the first sync

**Action.** Wait for the first sync pass (at most `work.sync_interval_secs`,
300 s by default, after the tracker is `ok`), then run
`fleet-hub tracker status`.

**Expected.** The pass is listed with its duration, items listed and
changed, event frames, and no error. The groups from step 5 now carry the
tickets' titles and statuses; a key typed before the tracker was connected
is bound to its ticket.

Result: [ ] pass / [ ] fail

Notes (pass duration, items, frames):

### Step 9: a status change reaches fleet

**Action.** In the tracker, move T4 to another status (for example *In
Progress*). Note the time. Watch its chip in the sidebar (open ⌘K → My
work if T4 has no session).

**Expected.** Within one sync interval plus the pass time the new status
shows. The desktop never blocked or showed an error while waiting.

Result: [ ] pass / [ ] fail

Notes (seconds from the change to fleet showing it; this is D13's number):

### Step 10: tickets in ⌘K

**Action.** Press ⌘K (Ctrl+K). Open **My work**, then *Current sprint* /
*Current cycle* if your tracker has one, *Recent*, and a favourite filter.
Type part of T2's title, then T2's key, then paste T2's URL.

**Expected.** My work lists your open tickets with titles and statuses;
each search finds T2; a ticket already running shows its session.
Nothing waits on the tracker (the lists come from the cache).

Result: [ ] pass / [ ] fail

Notes:

### Step 11: only the tracker's prefixes are keys

**Action.** In a session with no work, type a prompt that mentions
`GPT-4`, `COVID-19` and `UTF-8`, and a key-shaped word whose prefix no
tracker of yours has (for example `ZZZ-1`).

**Expected.** None of them becomes a suggestion or a link: with a tracker
connected, a key must carry one of its prefixes (the ones `tracker test`
reported in step 7).

Result: [ ] pass / [ ] fail

Notes:

## Part E: start and multi-start

### Step 12: start from ⌘K

**Action.** ⌘K, type T2's key, ↵.

**Expected.** Fleet starts a session at once (or, when the project is
ambiguous, opens the New session dialog on T2), and selects it:
- the branch and worktree are `slug(key + title)`;
- the session is named `T2-KEY title`;
- its chip is solid (source `started`);
- with **Brief Claude** on, a short start prompt is typed only once
  Claude's REPL is ready (never into a trust dialog), and Claude's first
  answer shows it knows the ticket.

Result: [ ] pass / [ ] fail

Notes:

### Step 13: the duplicate guard

**Action.** ⌘K on T2 again; then open **New session** and put T2 in the
*Work* field.

**Expected.** ⌘K jumps to the running session instead of starting a second
one; the dialog says "T2 already running on …" and offers **Jump**.

Result: [ ] pass / [ ] fail

Notes:

### Step 14: start from the dialog with an edited brief

**Action.** **New session**, *Work* = T3, edit the brief (add one line of
your own), **Start work**.

**Expected.** The session starts on T3 like step 12, and Claude's context
contains your added line.

Result: [ ] pass / [ ] fail

Notes:

### Step 15: the brief is fenced

**Action.** In T3's session ask Claude: "Quote exactly what you were given
about this ticket, including any markers around it."

**Expected.** The ticket's description arrives marked as untrusted
third-party text, not as an instruction from you.

Result: [ ] pass / [ ] fail

Notes:

### Step 16: multi-start

**Action.** **New session**, *Work* = the key that ran in two repositories
before. Under **Also start in**, tick the other repository. **Start work**.
Then repeat with the same key.

**Expected.**
- One session per repository, on the **same branch name**, each linked
  `started`, and each brief names its siblings.
- Each sibling gets its start prompt.
- On the repeat, a repository where the key already runs is **skipped**,
  not refused, and the result says which.
- Any failure is listed per repository (a warning toast), not a silent
  single start.

Result: [ ] pass / [ ] fail

Notes (did you want to do this from the phone? see D15 below):

## Part F: detection

### Step 17: a suggestion from a prompt

**Action.** In a session with **no work yet**, type a prompt that mentions
T4's key (for example "look at T4-KEY before we start").

**Expected.** The row shows a **dashed chip with `?`**; the session is
**not** regrouped. The attention strip shows **N link suggestions ·
Review**.

Result: [ ] pass / [ ] fail

Notes:

### Step 18: the popover and its evidence

**Action.** Click the dashed chip.

**Expected.** The popover lists the suggestion with one evidence line per
signal, each with its rule tag (for example "mentioned T4-KEY in a prompt
at 10:12 (reference) · R6"). Hovering the line shows a redacted
±40-character snippet (with `work.evidence_snippets` on, the default).
The buttons are **Confirm**, **Not this** and **Pick another…**.
**Pick another…** accepts a pasted ticket URL.

Result: [ ] pass / [ ] fail

Notes:

### Step 19: a suggestion from a branch

**Action.** In a project that is not trusted, start a plain session and
`git checkout -b <T1-key>-try` in it (or rename its branch).

**Expected.** A suggestion (R3), not a link; the popover offers **Trust
branch keys in this repo**. With the box ticked and a later branch in the
same repository, the key links automatically with a toast "Linked NAME →
KEY (branch) · Undo". Settings → Limits → Lifecycle shows the trusted count
and **Trust none**.

Result: [ ] pass / [ ] fail

Notes:

### Step 20: batch review

**Action.** Make three or more suggestions (steps 17 and 19 on several
sessions). Click **N link suggestions · Review**. Use `j` / `k`, `y` on one,
`n` on another, click a row, `esc`.

**Expected.** `y` confirms (the session joins the group, solid chip), `n`
rejects it for good; the next suggestion of the same session comes up;
clicking a row narrows the sidebar to that session. The pill disappears
when nothing is left.

Result: [ ] pass / [ ] fail

Notes:

### Step 21: the Undo of an automatic link

**Action.** On the toast of an automatic link (step 19), press **Undo**.

**Expected.** The link is gone and that key is never suggested again for
the session (Undo is *Not this*).

Result: [ ] pass / [ ] fail

Notes:

## Part G: name this work (M11.1)

### Step 22: name local work from a group header

**Action.** In work mode, on a project group whose sessions have no work,
open the header's menu → **Name this work…**, type a title ("Spike: cache
warm-up") and an optional key (`OPS-1`), **Name**.

**Expected.** The project's sessions without work join a new local work
group with that title; the chip is solid; sessions that already had work
are unchanged.

Result: [ ] pass / [ ] fail

Notes:

### Step 23: rename it, and the collisions

**Action.** Row **#** → **Name this work…** on one of those sessions,
change the title, **Rename**. Then try naming new work with T1's key.

**Expected.** The rename shows everywhere at once. The key of a ticket you
can see is refused with a hint to link the ticket instead.

Result: [ ] pass / [ ] fail

Notes:

## Part H: handover (M9.3)

### Step 24: ask for a handover

**Action.** Select T2's session while it is idle. In Details, on the
ticket card, press **Ask for a handover**.

**Expected.** A toast says the handover was asked; the card shows
"Handover asked … waiting for the reply". Claude gets one prompt and
writes the hand-off. When the turn ends the card shows "Handover written
…; the next session's brief shows it". The session timeline has
`handover_requested` then `handover_written`, and neither the timeline nor
the journal shows the marker lines.

Result: [ ] pass / [ ] fail

Notes:

### Step 25: the refusals

**Action.** Press **Ask for a handover** again at once (while the request
is pending); then on a busy session; then on a session with no work.

**Expected.** Each is refused with a reason (pending, busy, no work); the
button is disabled for a session that is not an idle Claude session.

Result: [ ] pass / [ ] fail

Notes:

## Part I: resume (M2, M11.2)

### Step 26: resume ended work, three modes

**Action.** Kill T2's session (safe kill, or kill after committing). Find
T2 in its group's **Done** section (or ⌘K). Open **Resume ▾** and try
each mode on a separate run: **Continue last conversation**, **Fresh with
brief**, **Fresh**.

**Expected.**
- The dialog names the host and branch it will land on; the host can be
  changed; the brief is editable.
- *Continue* re-opens the same Claude conversation.
- *Fresh with brief* starts a new conversation whose brief shows **the
  handover from step 24 first**, fenced, then the ticket, snapshots and
  journal (at most 4,000 characters).
- Every resumed session re-attaches the ended link (source `resumed`).

Result: [ ] pass / [ ] fail

Notes (was the brief enough to continue? see D10 below):

### Step 27: resume when the transcript was deleted

**Action.** End a session linked to T3. On its host, **move** (do not
delete) its transcript out of the way:
`mv ~/.claude/projects/*/<conversation-uuid>.jsonl /tmp/` (the uuid is in
the session's Conversations panel). Open **Resume ▾** on T3.

**Expected.** *Continue last conversation* is not offered; **Fresh with
brief** is the default. Move the file back: *Continue* is offered again.
With the host unreachable (optional: stop its SSH), the dialog shows a
warning instead of guessing.

Result: [ ] pass / [ ] fail

Notes:

## Part J: Today, standup and the ticket card (M9.1, M9.2)

### Step 28: Today

**Action.** Clear the selection (Details' empty state), or press ⌘⇧T
(Ctrl+Shift+T) over a selected session.

**Expected.** **Today** with **Waiting on you**, **In progress**,
**Shipped today** (tickets done and work that ended with a PR since your
local midnight, with *PR* / *Open ticket*), and **Stale** (idle three days,
or the ticket done while a session runs). It follows the sidebar's scope
(⌘⇧O). **Refresh** re-reads it.

Result: [ ] pass / [ ] fail

Notes:

### Step 29: Copy standup

**Action.** Press **Copy standup** and paste into a text editor.

**Expected.** Plain text with the same sections and items as the screen;
the button reads *Copied*.

Result: [ ] pass / [ ] fail

Notes:

### Step 30: Stale opens Tidy-up narrowed (M10.4)

**Action.** When Stale has tidy candidates, press **Tidy up · n** in it.

**Expected.** The Tidy up sheet opens narrowed to those sessions ("From
Today's Stale · n of m") with **Show all**.

Result: [ ] pass / [ ] fail

Notes:

### Step 31: the ticket card

**Action.** Select T3's live session (or start one). Read the ticket card
in Details, then press **Insert into composer**.

**Expected.** Title, status, link and the acceptance criteria parsed from
the cached description. Insert puts the ticket text, fenced as untrusted,
into the composer and **does not send it**.

Result: [ ] pass / [ ] fail

Notes:

## Part K: the operator's confirmations (M9.7)

### Step 32: the operator cannot start or kill on a hub

**Action.** In the agent panel, ask the operator: "Start work on T4" and
then "Kill the session on T3".

**Expected.** Both are refused (`E_FORBIDDEN`, no approver on a hub), and
the operator says so and suggests doing it from the sidebar, or archive /
snooze for a tidy. Nothing started or died.

Result: [ ] pass / [ ] fail

Notes:

### Step 33: Tidy up done tickets fills, never sends

**Action.** In the agent panel press the **Tidy up done tickets** chip.

**Expected.** The operator's composer is filled with the request; nothing
is sent until you press send.

Result: [ ] pass / [ ] fail

Notes:

### Step 34 (optional): the confirmation dialog on a standalone desktop

**Action.** On a standalone (unpaired) desktop with a test host, with
`mcp.confirm_destructive` **off**, ask the operator to start work on a
ticket.

**Expected.** The desktop shows the confirmation dialog anyway (D12);
nothing starts until you approve it; declining leaves nothing behind.

Result: [ ] pass / [ ] fail

Notes:

## Part L: tidy-up and auto-tidy (M7, M11.3)

### Step 35: a done ticket becomes a candidate

**Action.** Move T3 to done in the tracker while a session on it is idle.
To avoid waiting two days, lower the thresholds for the run:
`call set_setting '{"key":"work.tidy_done_days","value":"1"}'` and
`call set_setting '{"key":"work.tidy_idle_hours","value":"1"}'`. Come back
after a day (the minimums are 1 day and 1 hour).

**Expected.** **Tidy up · n** appears in the attention strip (never counted
in *Needs you*). The sheet lists T3's session under `done_idle` with
**Safe kill** preselected. The protections hold: no working, blocked or
dialog-waiting session, no session on an in-progress ticket, nothing
prompted or attached to in the last hour, and not the operator.

Result: [ ] pass / [ ] fail

Notes:

### Step 36: the per-row choices

**Action.** In the sheet, switch one row to **Archive only**, one to
**Snooze 7 d**, and one to **Never for this work**; ↵ to apply.

**Expected.** The archived session collapses into its group's **Done**
while tmux keeps running (the next prompt or attach brings it back); the
snoozed and never rows leave the sheet. Each action is on the session's
timeline.

Result: [ ] pass / [ ] fail

Notes:

### Step 37: safe kill

**Action.** Apply **Safe kill** to a candidate with uncommitted changes in
its worktree.

**Expected.** Claude is asked to commit and push first; the worktree is
removed only if that succeeded. If Claude refuses, the recorded failure is
Claude's own reason, and the worktree stays.

Result: [ ] pass / [ ] fail

Notes:

### Step 38: idle, no work linked, and Keep

**Action.** Lower the threshold for the run:
`call set_setting '{"key":"work.tidy_idle_unlinked_days","value":"1"}'`.
Leave a work session in its own worktree, with no work linked, untouched
(no prompt, attach or turn) for a day. Open Tidy up.

**Expected.**
- It is listed under **Idle, no work linked**, **unticked**, with its own
  **Keep 7 d** and **Safe kill** buttons.
- **Keep 7 d** takes it out of the sheet for 7 days.
- On another such session, **Safe kill** arms first (**Confirm safe
  kill**), then is refused while the worktree is dirty or unpushed, and
  goes ahead only when it is clean and pushed.

Result: [ ] pass / [ ] fail

Notes:

### Step 39: auto-tidy

**Action.** `call set_setting '{"key":"work.auto_tidy","value":"true"}'`.
Wait for the next GC sweep (`gc.sweep_interval_secs`). Then set it back to
`false`. (For an org override: `fleet-hub org set <id> --auto-tidy off`
first, to check that org is left alone.)

**Expected.** The sweep safe-kills (or, with no inspectable worktree,
archives) only candidates whose reason is in `work.auto_tidy_reasons`
(`done_idle`, `pr_merged_idle` by default), never with a plain kill. An
`idle_unlinked` session is **never** touched (D19). Each action is on the
session's timeline (`gc_tidied` or `gc_failed`) and in its work journal. An
org set to `off` is untouched.

Result: [ ] pass / [ ] fail

Notes:

### Step 40: put the thresholds back

**Action.** Set `work.tidy_done_days` to `2`, `work.tidy_idle_hours` to
`4`, `work.tidy_idle_unlinked_days` to `7` (or your own values), and check
`call get_settings | jq 'with_entries(select(.key|startswith("work.")))'`.

**Expected.** The values are what you set, and `work.auto_tidy` is
`false` unless you want it on.

Result: [ ] pass / [ ] fail

Notes:

## Part M: retention on a test hub (M12.3)

Retention deletes rows. Run this part on a **throw-away copy**, never on
the real hub with a 1-day window.

### Step 41: start a test hub on a copy

**Action.** On a machine or user account **without SSH access to your
hosts** (check: `ssh <host A> true` fails there), with the same release:

```sh
mkdir -p ~/fleet-test && chmod 700 ~/fleet-test
cp state.db.<date>.bak ~/fleet-test/state.db && chmod 600 ~/fleet-test/state.db
fleet-hub serve --data-dir ~/fleet-test --bind 127.0.0.1 --port 4190
```

In a second terminal: `export HUB=http://127.0.0.1:4190`, then the master
token from `fleet-hub token show --data-dir ~/fleet-test` (the copy's, which
is the real hub's: keep it private), and the `call` helper.

**Expected.** The copy starts and answers `call fleet_health`
(`db_ready: true`). It cannot reach any host (hosts read unreachable),
which is what keeps the copy harmless.

Result: [ ] pass / [ ] fail

Notes:

### Step 42: note what must survive

**Action.** Pick a key with a **live** link (for example T1) and a done
ticket named by an **ended** link (for example T3). Save:

```sh
call work '{"action":"context","key":"T1-KEY"}' > before-live.json
call work '{"action":"card","key":"T3-KEY"}'    > before-done.json
call work_admin '{"action":"status"}' | jq .retention > before-status.json
```

**Expected.** Both answers carry the ticket, its links and (for T1) the
journal; `retention` lists row counts, a dry-run count and the last sweep
per table.

Result: [ ] pass / [ ] fail

Notes:

### Step 43: a 1-day window, one sweep

**Action.**

```sh
for k in journal_days tracker_items_days timeline_work_events_days; do
  call set_setting "{\"key\":\"work.retention.$k\",\"value\":\"1\"}" >/dev/null
done
call work_admin '{"action":"status"}' | jq .retention   # the dry run now
call work_admin '{"action":"sweep_now"}'
call work_admin '{"action":"status"}' | jq .retention
```

**Expected.** The dry-run counts before the sweep equal what the sweep
deleted (at most 2,000 rows per table per sweep; repeat `sweep_now` until
the counts reach 0). Nothing errors.

Result: [ ] pass / [ ] fail

Notes (rows per table before / swept / after):

### Step 44: linked work survived

**Action.**

```sh
call work '{"action":"context","key":"T1-KEY"}' > after-live.json
call work '{"action":"card","key":"T3-KEY"}'    > after-done.json
diff before-live.json after-live.json; diff before-done.json after-done.json
```

**Expected.** T1's context (live link, journal) is unchanged, and T3's
card still resolves (a done ticket that a link names is kept). Only rows
that were ended or done, older than a day, and pointed at by nothing live
are gone.

Result: [ ] pass / [ ] fail

Notes:

### Step 45: tear the test hub down

**Action.** Stop the test hub; `rm -rf ~/fleet-test`. On the real hub,
check `call get_settings | jq '."work.retention.journal_days"'`.

**Expected.** The real hub still has its own windows (365 / 180 / 180 by
default): the 1-day values lived only in the copy.

Result: [ ] pass / [ ] fail

Notes:

## Part N: tracker health (M12.4)

### Step 46: revoke the token

**Action.** Revoke the tracker's credential at its source: Jira, delete
the API token at id.atlassian.com → Security → API tokens; GitHub, run
`gh auth logout` on the via-cli host. Wait for the next sync pass (or run
`fleet-hub tracker test <id>`). Watch the desktop.

**Expected.**
- `call fleet_health | jq .trackers` shows the tracker as `failing` at
  once for Jira (`state: auth_failed`), or `degraded` and then `failing`
  after 3 failed passes for GitHub (`unreachable`), with
  `consecutive_failures`, `last_success_at` and a `last_error` that is
  one line, redacted and fenced (no token in it).
- Within 60 s of that, the desktop's attention strip shows **⚠ Reconnect
  Jira (name) →** (one item per failing tracker); hovering shows the
  error, the count and the org; the footer shows `trackers: 1 failing …`.
- Clicking the item opens Settings scrolled to Work.
- Nothing else stopped working: chips and ⌘K answer from the cache.

Result: [ ] pass / [ ] fail

Notes (time from revoke to the Reconnect item):

### Step 47: reconnect

**Action.** Create a new token and `fleet-hub tracker set-credential <id>
…`, then `fleet-hub tracker test <id>` (GitHub: `gh auth login` again, then
`test`).

**Expected.** After the next successful pass (and the next 60 s read), the
Reconnect item and the footer line are gone; `fleet_health.trackers` shows
`ok`, `consecutive_failures: 0`.

Result: [ ] pass / [ ] fail

Notes:

### Step 48: the detection backlog

**Action.** Leave one suggestion undecided for more than 7 days (or note
the number at the end of the run): `call fleet_health | jq
'.trackers | {detection_backlog, detection_backlog_days}'`.

**Expected.** Suggestions on live sessions older than 7 days are counted;
the footer mentions them ("… suggestions undecided > 7 d").

Result: [ ] pass / [ ] fail

Notes:

## Part O: organisations and isolation (M5)

Skip this part without a second org; write "skipped".

### Step 49: set up two orgs

**Action.** On the hub, following [hub.md → Organisations and isolation](hub.md#organisations-and-isolation):

```sh
fleet-hub org add "Org A" --color '#e11d48'
fleet-hub org add "Org B" --color '#2563eb'
fleet-hub org rule add <A> --owner <org A's GitHub owner>
fleet-hub org assign-host <host A> <A>
fleet-hub org assign-host <host B> <B>
fleet-hub org assign-tracker <tracker id> <A>
fleet-hub org list
```

**Expected.** `org list` shows both orgs with their rules, hosts and the
tracker. On the desktop the rows carry the org colour bar, and the scope
selector (⌘⇧O) offers both; a session waiting on you in the other scope
still says so ("n need you in Org B →").

Result: [ ] pass / [ ] fail

Notes:

### Step 50: host B's token sees none of org A's work

**Action.** On host B: `jq -r '.mcpServers["claude-fleet"].headers.Authorization' ~/.claude.json`
prints `Bearer <token>`. On the hub machine, in a **separate** shell, set
`TOKEN` to that token (without `Bearer `) and run:

```sh
call work '{"action":"tickets"}'
call work '{"action":"context","key":"T1-KEY"}'
call list_sessions '{"summary":false}' | jq '[.[] | {tmux_name, host_alias, work}]'
call fleet_health | jq .trackers
call work_admin '{"action":"list"}'
```

Then, in a Claude session on host B, ask: "Use the claude-fleet work tool
with action tickets and tell me what you see."

**Expected.** No org A ticket, tracker, link or journal line anywhere:
T1 answers as a key nothing is linked to; session rows of host A carry no
`work`; `fleet_health.trackers` lists no org A tracker; `work_admin` is
refused. Claude on host B sees the same. Back in the master shell,
everything is visible.

Result: [ ] pass / [ ] fail

Notes:

### Step 51: the event stream is fenced too

**Action.** With host B's token:
`curl -sN "$HUB/events" -H "Authorization: Bearer $TOKEN"` and, meanwhile,
confirm a suggestion or change a link on a host A session (and let a sync
pass run).

**Expected.** No `work:` frame and no org A work in any `session:` frame on
host B's stream.

Result: [ ] pass / [ ] fail

Notes:

### Step 52: a cross-org link is refused

**Action.** On a host B session (org B), set T1's key (an org A ticket)
from the **#** menu.

**Expected.** Refused with an explanation and **Link anyway**; **Link
anyway** links it. Detection never suggests across orgs.

Result: [ ] pass / [ ] fail

Notes:

### Step 53 (optional): isolate sessions

**Action.** `fleet-hub org set <A> --isolate-sessions on`; repeat
`call list_sessions '{"summary":false}' | jq '[.[] | {tmux_name, host_alias}]'`
with host B's token. Turn it back off if you do not
want it.

**Expected.** Org A's sessions disappear from host B's list; host B still
sees its own sessions.

Result: [ ] pass / [ ] fail

Notes:

## Part P: GitHub Enterprise Server (M11.4, optional)

### Step 54: connect a GHES repository

**Action.** On a host: `gh auth login --hostname <ghe.host[:port]>`. On the
hub:
`fleet-hub tracker add https://<ghe.host>/<owner> --via-cli <host> --hostname <ghe.host[:port]>`,
then `fleet-hub tracker test <id>`.

**Expected.** The test reports your account; after a sync, an issue
assigned to you shows in ⌘K → My work as `<ghe.host>/<owner>/<repo>#n`,
distinct from a same-named repository on github.com. A URL on that
instance pasted into ⌘K starts work on it.

Result: [ ] pass / [ ] fail

Notes:

## Part Q: the phone (M8.6, M10.5)

The phone screens depend on the fleet-mobile release; the hub gates each
action by the token. Record the release in the *Run record*.

### Step 55: pair and see work

**Action.** On the hub: `fleet-hub pair --name phone-acceptance` and scan
the QR with fleet-mobile.

**Expected.** The phone lists the sessions with their work chips, groups
by work, and shows org labels and an org filter (Part O).

Result: [ ] pass / [ ] fail

Notes:

### Step 56: Today on the phone

**Action.** Open Today on the phone and use its standup share / copy.

**Expected.** The same four sections as the desktop's Today (step 28), and
the same plain text as the desktop's **Copy standup**.

Result: [ ] pass / [ ] fail

Notes:

### Step 57: the ticket card on the phone

**Action.** Open a session linked to a ticket and its card.

**Expected.** Title, status, link and acceptance criteria, read-only, shown
as plain text (no link opens by itself). It offers *Copy*, never *Send*.

Result: [ ] pass / [ ] fail

Notes:

### Step 58: a handover from the phone

**Action.** With the full token, on an idle session linked to work, use
*Ask for a handover* (if the release has it; M8.6.3).

**Expected.** Same as step 24: the desktop's card shows "Handover written
…" once the turn ends.

Result: [ ] pass / [ ] fail

Notes:

### Step 59: a readonly token only reads

**Action.** `fleet-hub pair --name phone-ro --mode readonly`, pair a second
install (or re-pair), and try confirm, start and handover.

**Expected.** Work, Today and cards are shown; every write (confirm, set a
link, start, handover) is refused or not offered. Then `fleet-hub client
revoke phone-ro`.

Result: [ ] pass / [ ] fail

Notes:

---

## Evidence for the decisions

The questions of the M12.6 review
([decisions revisited](superpowers/reviews/2026-09-26-work-graph-decisions-revisited.md#what-m103-should-capture)),
answered from this run. Write what happened, not what you expect.

### D3: write-back to trackers (decided against)

How often, during the run, did a ticket's status or PR have to be updated
by hand in the tracker after a fleet start or PR? Did that feel like
friction?

| Times updated by hand | Which steps | Friction? (none / some / a lot) |
|---|---|---|
| | | |

Answer:

### D10: summaries of dead sessions (decided against)

For each resume of a **dead** session (steps 26, 27): was the built brief
enough to continue, or did you open the transcript by hand? One example of
what was missing:

| Resume (step, key) | Brief enough? | Transcript opened by hand? | What was missing |
|---|---|---|---|
| | | | |

Answer:

### D13: inbound webhooks (decided against)

- The longest wait between a status change in the tracker and fleet showing
  it (step 9; expected at most one `work.sync_interval_secs` plus the pass
  time): ____ s.
- Did any step have to wait on it? Which?
- Is the hub reachable at a public URL at all? (yes / no)

Answer:

### D15 / D20: the phone (multi-start and naming stay on the desktop)

- fleet-mobile release on the phone: ____ ; *Ask for a handover* there
  (M8.6.3)? (yes / no)
- Did any step make you want to **start in several repositories** from the
  phone (step 16)? Which?
- Did any step make you want to **name a piece of work** from the phone
  (step 22)? Which?

Answer:

### D17: `acli` as a Jira transport (decided against)

Does any organisation you work with forbid API tokens (which would bring
`acli` back)?

Answer:

### Tracker health: the quota baseline

After the run, paste `fleet-hub tracker status` and
`call fleet_health | jq .trackers`:

| Tracker | Failures in the run | `rate_limited` passes | Typical pass duration | Items per pass |
|---|---|---|---|---|
| | | | | |

```
(paste here)
```

### D5: the SessionStart context, measured from a remote host

`work.session_start_context` is off until the remote numbers are under
~300 ms p95 (M10 plan). From your machine, against a provisioned **remote**
host:

```sh
scripts/measure-session-start.sh --ssh <host A> --runs 20
scripts/measure-session-start.sh --ssh <host A> --runs 20 --pane <%N of a throwaway session linked to a ticket>
scripts/measure-session-start.sh --ssh <host A> --runs 20 --hub-stopped   # stop the hub when it asks
```

Paste the rows it prints (they also go into the M4 plan's *Remote host*
table):

| Hub (remote host) | min / median / p95 / max, ms | curl exit |
|---|---|---|
| up, answering | | |
| up, answering, with `--pane` (real context) | | |
| down, port refused | | |
| host unreachable (packets dropped) | | |
| up but not answering | | |
| hub stopped, tunnel alive | | |

Is up to the "not answering" number acceptable at every session start on a
bad day? (yes / no) Your D5 answer:

---

## Summary

| Part | Steps | Pass | Fail | Skipped |
|---|---|---|---|---|
| A. Upgrade | 1–4 | | | |
| B. No tracker | 5–6 | | | |
| C. Connect a tracker (guide cold) | 7 | | | |
| D. Sync and tickets | 8–11 | | | |
| E. Start and multi-start | 12–16 | | | |
| F. Detection | 17–21 | | | |
| G. Name this work | 22–23 | | | |
| H. Handover | 24–25 | | | |
| I. Resume | 26–27 | | | |
| J. Today, standup, card | 28–31 | | | |
| K. Operator confirmations | 32–34 | | | |
| L. Tidy-up and auto-tidy | 35–40 | | | |
| M. Retention (test hub) | 41–45 | | | |
| N. Tracker health | 46–48 | | | |
| O. Orgs and isolation | 49–53 | | | |
| P. GHES | 54 | | | |
| Q. Phone | 55–59 | | | |
| **Total** | **59** | | | |

## Found issues

One row per problem, including the small ones. Severity: **blocker**
(wrong data, a leak across orgs or tokens, lost work, a kill that should not
have happened), **major** (a flow does not work), **minor** (works, but
confusing or wrong in a detail), **docs** (the guide was wrong or missing).

| # | Step | Description | Severity |
|---|---|---|---|
| 1 | | | |

## After the run

1. **Commit the filled-in document as the record** of the run: this file
   with every box ticked and every answer written, on a branch, in a
   `docs(work): record the M10.3 acceptance run` commit. It is the evidence
   later decisions cite, so leave failures and odd notes in.
2. Add a line to the roadmap's *Revisions*
   (`superpowers/2026-09-24-work-graph-roadmap.md`) and the M10 plan's
   *Revisions*: the date, the hub version, the totals and the failures, and
   mark M10.3 run.
3. **Update the decisions table** in the roadmap (*Decisions still open*)
   from [Evidence for the decisions](#evidence-for-the-decisions): D3,
   D10, D13, D15 / D20, D17 and D5. A "yes" becomes its own milestone
   (M13+), not a change folded into another one. Paste the D5 rows into the
   M4 plan's *Remote host* table.
4. Every **blocker** or **major** in *Found issues* becomes an issue or a
   fix branch before the next release; fix the **docs** rows in
   [work-graph.md](work-graph.md) in the same pass.
