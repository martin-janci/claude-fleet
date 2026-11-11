# Human in the loop: UX audit and target design (2026-11-10)

**Looked at:** claude-fleet `6dcaecb1` (0.6.4 plus `main`) and fleet-mobile `b9d19e1`.
**The question:** how well do the desktop and the phone let one person supervise many
agents: notice an ask, understand it, decide, check the result, and set how much runs
without asking?

| Path | What it is |
|---|---|
| `README.md` | This audit: findings with evidence, the target design, a plan |
| [`boards/`](boards) | Eight target boards (`*.dc.html` + `canvas.json`), the same format as `docs/ux/2026-10-08-orbit-fleet-redesign/canvas`. Live canvas: <https://claude.ai/artifact/XJx4FE3DVFx8YeEroWDpU3> (private until shared) |
| [`images/`](images) | `today-*.webp`: real screenshots of the apps. `board-*.webp`: renders of the boards |
| [`tools/`](tools) | Capture and render scripts, to take the same screenshots again |
| [`HANDOVER.md`](HANDOVER.md) | How the next agent starts on the plan |

No code was changed. Each finding names the file and line, the fix, its size, and a
check, as the 2026-09-21 audit did.

---

## 1. Verdict

The small interactions are careful, more than in any comparable product I
read (§10). The answer card re-reads the pane before it presses a key, ignores a press in its
first 600 ms, takes two presses to save a folder rule, never highlights Approve, and
phone notifications never carry Approve. One attention table is tested on both the
hub and the desktop. Keep all of it (§3).

The weak points are one level up, in how the loop is put together:

1. **The leash is off.** Fleet starts every session with permission prompts
   skipped, and no setting changes that. The card the product is designed around
   almost never appears, and "push to main: always a person" is enforced only for
   mission workers ([H-01](#h-01)).
2. **The loop is open at the "done" end.** A finished turn is not an ask: it is not
   counted, not notified, and stops being "Done" the moment it is looked at. A
   question asked in prose reads as Idle. On the live fleet 38 of 43 sessions were
   idle, so the real queue is the one the Inbox does not show ([H-05](#h-05), [H-06](#h-06)).
3. **Asks live in nine places** with different counts, lifetimes and notifications.
   Some expire unseen after 10 minutes; some are never counted ([H-08](#h-08)).
4. **Being told is not dependable.** Desktop notifications are off by default and
   silent while the window has focus. Nothing is announced twice. The phone has no
   push ([H-07](#h-07)).
5. **The decision is thin and the review is weak.** Rows say "Needs you" without
   saying why; the Inbox opens on an empty pane; an edit is approved without seeing
   it; the diff shows a blank line after every line ([H-11](#h-11), [H-13](#h-13),
   [H-18](#h-18), [H-25](#h-25), [H-26](#h-26)).

**First ten fixes, in order:**

| # | Fix | Finding | Size |
|---|---|---|---|
| 1 | Remove the blank line after every diff row | H-25 | S |
| 2 | Make the row say why: line two is the ask, the failure or the result, and the dot is the state | H-11, H-12 | S |
| 3 | Keep names visible: the selected row and the session header | H-15, H-16 | S |
| 4 | Count and announce every ask: review requests, operator confirms, a new ask while the window has focus | H-08, H-09 | S–M |
| 4b | A confirm card never gives Approve focus, and Escape never denies (done) | H-57, H-58 | S |
| 5 | Ask before a person's step in every session, through the guard that already exists | H-01 | M |
| 6 | Give finished work a lane: Review, with its own count, cleared by a verdict | H-05 | M |
| 7 | Show a turn's last words on the row, and treat a closing question as an ask by rule | H-06 | M |
| 8 | Open the Inbox on the first ask and answer in place | H-18 | M |
| 9 | Tell twice and reach the phone: one re-notify, push, a boot receiver | H-07 | M–L |
| 10 | One page for autonomy, one for rules, one answer log | H-02, H-03, H-04 | L |

---

## 2. Method and limits

- **The real UI.** The desktop frontend ran in Chromium against the repo's own fake
  hub (`e2e/federation`), with fake answers added for the commands a populated fleet
  needs. About 480 screenshots: Inbox, Sessions, every conversation state, the
  answer cards, Files, dialogs, Control, Work, Settings, light and dark, 1100 to
  1920 px. The phone's shared Compose UI was drawn by its JVM preview tests: 185
  renders of 63 states, including 320 dp and font scale 1.3.
- **The code.** Two maps with `file:line` for every ask, guard, count and
  notification (desktop and hub; phone). I re-read every line cited below for a C or
  H finding.
- **The documents.** `docs/status.md`, `docs/decisions.md`, the redesign plan, both
  canvas-gap audits, the 2026-09-21 and 2026-09-27 audits, the phone's pager spec.
  Known items are not reported again (§9).
- **The design intent.** All 90 canvas boards were rendered and read, so a finding
  says whether the gap is in the build or in the design.
- **Outside sources.** The references in §10 were checked against the papers and
  the vendors' documentation on 2026-11-10; each carries a mark.

**Limits.** No live fleet, no real users, no real hook timing. Fake data can
mislead, so nothing became a finding until the code agreed with the picture. The
phone was drawn on the JVM (other fonts) and never ran on a device. Latency numbers
are read from constants, not measured.

---

## 3. What already works

Keep these. Several are ahead of the products in §10.

| What | Where |
|---|---|
| An answer is a key, re-checked twice: a fresh pane read and a backend `E_CONFLICT` at the press. "That dialog is gone. Nothing was sent." | `src/lib/answer_send.ts:59-90`, `crates/fleet-core/src/service/sessions/prompt.rs:700-714` |
| A new card ignores presses for 600 ms | `src/lib/answer_guard.ts:14` |
| "Don't ask again … in this folder" takes a second press that says what is saved and where | `src/lib/AnswerPrompt.svelte:377-402` |
| Approve is never drawn primary; Jev never proposes on a permission or a risky option | `src/lib/kit/QuestionCard.svelte:1-11`, `src/lib/quick_answer.ts:101-102` |
| One attention table, checked by both test suites; an honest badge | `src/lib/attention_states.json`, `crates/fleet-core/src/service/attention.rs` |
| A permission becomes "Needs you" at once (Notification hook), with its options 0.3 to 3.5 s later | `crates/fleet-core/src/service/hooks.rs:1589-1603`, `service/sessions/activity.rs:220` |
| "Paused · 5-hour limit on acme" with Switch account and Wait on the row | `src/lib/SessionRowItem.svelte:1031-1037` |
| A mass loss is one line: "12 stopped on trn · Restore" | `src/lib/Sidebar.svelte:1813-1816` |
| After an answer the view moves to the next ask | `src/lib/ConversationPanel.svelte:903-917` |
| Link review: `j` `k` `y` `n`, "Confirm all likely", "Undo last" | `src/lib/WorkReview.svelte` |
| Phone notifications never answer, enforced by type | `fleet-mobile …/notify/NeedsYouContent.kt:44-71` |
| The phone's card writes its own headline ("Allow Bash: cargo fleet-test?") and says what Send does ("Send says No to the question and gives your words instead.") | `fleet-mobile …/ui/QuestionCard.kt`, `SessionScreen.kt:3221` |
| Evidence names its source, commit and time; "Unknown is a first-class answer" | `docs/specs/2026-09-29-result-evidence-design.md:52-62` |
| Contrast and target sizes pass where measured: status words 6.4 to 8.7:1, muted text 5.7 to 6.8:1, no target under 24 px in Sessions or Inbox | measured in the page, both themes |

---

## 4. Findings

**Severity:** C misleads or blocks at the core of the loop · H high · M medium · L low.
**Size:** S one PR (about 300 lines) · M two to four PRs · L a milestone.

### 4.1 Steer: how much runs without asking

<a id="h-01"></a>
#### H-01 · C · The leash is off, and no setting turns it on

**Seen.** A fleet-started pane runs `cl`, which falls back to `ag claude --yolo` or
`claude --dangerously-skip-permissions` (`crates/fleet-core/src/tmux.rs:1907`).
Provisioning installs `cl=claude --yolo` on every host
(`crates/fleet-core/src/service/provision.rs:129`). `ClaudeLaunch` carries a model,
an effort and a profile, and no permission mode (`tmux.rs:1959-1966`).

So in a default fleet nothing a session runs is ever put to a person. The Main
board's example, "Push to origin needs your OK", cannot happen.

The one exception shows the fix. A mission's worker passes through a
`PreToolUse(Bash)` hook, and the hub refuses a person's step there: `gh pr merge`,
`gh pr ready`, a push to `main`, a tracker write
(`service/work/orchestrate/guard.rs:1-13`, `service/hooks.rs`
`pre_tool_use_denial`). For "any other session" the hub answers nothing. The rule
"PR ready, merge, push to default: always a person"
(`docs/superpowers/specs/2026-10-07-autonomous-orchestration-projects-design.md:309-322`)
is therefore true for workers and untrue for the session a person started by hand.

**Why it matters.** With prompts skipped, a person is in the loop only before the
work and after it. That can be a sound choice: people accept 93% of permission
prompts anyway (Anthropic, 2026), and a prompt that is always approved protects
nothing. But then two things must hold: the few steps that cannot be taken back
still ask, and the result is reviewed well. Neither holds today (this finding, and
H-05, H-25, H-26).

**Do.** Turn the worker guard into the leash.

1. One setting per session, with a fleet default and a project override: *Plan
   only · Ask before changes · Free in its worktree · Asks before a person's step ·
   Unattended* ([board: Autonomy](images/board-Autonomy.webp)).
2. The hook answers one of three things for every fleet-started session: nothing
   (run), **ask**, or **deny**. `ask` raises Claude Code's own permission dialog,
   which the existing card answers. Per Claude Code's documentation a `PreToolUse`
   hook may return `allow`, `deny` or `ask`, and `ask` and `deny` rules still apply
   in bypass mode; check both on the version the hosts run.
3. Workers keep `deny`. Everyone else gets `ask` for the four person's steps the
   guard already knows (`guard::deny_reason`).
4. The inspector says which leash a session is on, in one sentence.

The hook is fail-open today (`|| true`): if the hub does not answer, the command
runs. Say so in the UI, and keep branch protection as the backstop.

**Size:** M for steps 2 to 4 with one fixed level; L with the five levels.
**Check:** in a non-worker session, `git push origin main` raises a permission card;
the same command in a worker is refused; `pnpm test` raises nothing.

<a id="h-02"></a>
#### H-02 · H · Autonomy is told three ways and covers only Control

**Seen.**
- The dial gates only the operator's control-API calls
  (`crates/fleet-core/src/mcp/guard.rs:2024-2032`). Any other caller is gated by
  `mcp.confirm_destructive`, which is off (`docs/settings-reference.md:334`): an
  agent holding a host token can call `kill_session` with no card.
- The shipped settings read as **Custom**, which acts as Ask first
  (`docs/status.md:1152-1154`). A new user's first sight of the dial is a state
  the dial does not name.
- Each level is a paragraph. Autopilot's is 60 words with a nested "unless … then"
  (`src/lib/autonomy.ts:30-57`).
- The confirm card says "The agent's starts and kills always wait for you"
  (`src/lib/ConfirmCards.svelte:115`). At Autopilot with session care, restarts,
  retirements and starts inside a grant do not (`guard.rs:2010-2020`).
- The docs say "there is never an auto mode"; the dial's third position is named
  Autopilot.
- On the phone a tap on a level writes up to four fleet-wide settings at once,
  with no confirm and no undo (`fleet-mobile …/ui/AutonomyChip.kt:73-90`).

**Do.** One Autonomy page with two tables: what a session runs alone, and what
Control does without a card ([board](images/board-Autonomy.webp)). Rows are things
done; columns are levels; a cell is *runs*, *asks* or *no*. Ship a named default.
A change that lets Control do more asks for a confirm, on both devices. Make the
card's sentence follow the level.
**Size:** M. **Check:** the table is generated from `autonomy.rs` and
`guard.rs`, and a test fails when they drift, as `attention_states.json` does.

<a id="h-03"></a>
#### H-03 · M · Nobody can see what was answered, or what ran without asking

A dialog answer is stored as `keys_sent` with the key only: not who, not which
option, not which question (`service/sessions/prompt.rs:337-343`). A denied
operator call leaves no record. There is no page that lists decisions.

**Do.** An answer log: who, device, when, the question's fingerprint, the option's
label, how long it waited. And "Done without asking, last 24 hours" on the
Autonomy page. Both feed H-04 and §7. **Size:** M (one migration).

<a id="h-04"></a>
#### H-04 · M · A rule made by "don't ask again" disappears from view

The second press saves a rule in the project's `.claude/settings.local.json` on
that host. After that fleet shows it nowhere, cannot remove it, and does not carry
it to the next host, where the same question is asked again.

**Do.** A Rules page: built-in rules, the person's rules with a use count, rules
found on hosts with Adopt and Remove, a "Try a command" box, and an offer after
five same answers with none denied ([board](images/board-Rules.webp)). The offer is
a count, not Jev, so it fits "a rule decides first". The plan already has the
default: "Confirmations before fleet offers a rule: Five"
(`transition-plan.md:640-660`).
**Size:** L. Rules buy fewer interruptions at some cost in control: one 2026
preprint found person-written allow/ask/never policies stopped less overreach than
approving each action (Yan, n = 113). So rules are for what stays in a worktree,
and a person's step is never a "runs" rule.

### 4.2 Notice: learning that something waits

<a id="h-05"></a>
#### H-05 · C · Finished work is not an ask

**Seen.**
- `done_unread` folds into Done, which the badge does not count
  (`src/lib/attention_states.json:24`), and `notify.desktop` leaves `done` out by
  default (`docs/settings-reference.md:348`).
- It ends when the session is on screen: `touch_session_viewed` stamps it
  (`src/lib/session_viewed.ts`). A glance clears it. In one capture the same
  session read **Done** in the conversation strip and **Idle** in its header and
  row ([image](images/today-03-done.webp)).
- The live-fleet analysis counted 38 of 43 sessions idle and one blocked
  (`docs/ux/2026-09-27-live-instance-analysis/ux.md:18-22`).

With the leash off (H-01), checking the result is the only control left, and it has
no queue. The Inbox says "Nothing needs you" while a dozen results wait.

**Why it matters.** Anthropic's own agent view groups sessions as *Ready for
review*, *Needs input*, *Working*, *Completed*. Warp's "Waiting on you" has *PR
review* items and states that opening is not resolving. Both treat a finished
result as work for a person.

**Do.** A Review lane in the Inbox ([board: Inbox](images/board-Main.webp)).
- In it: a finished turn that changed something (a commit, a diff, a PR) and has
  no verdict.
- Out of it: by a verdict (*Reviewed*, *Send notes*, *Later*), not by a look. A
  turn that only answered in chat still clears on view.
- Counted on its own ("3 to review"), never in the amber badge, so the badge stays
  single-digit and means "an agent is waiting".

The six status words stay; the lane is a grouping, the word is still Done.
**Size:** M (one column for the verdict, hub and both clients).
**Check:** open a Done session and leave it: it is still in Review. Press
Reviewed: it leaves.

<a id="h-06"></a>
#### H-06 · C · A question asked in prose reads as Idle

**Seen.** No hook says "the turn ended with a question". The only reader is Jev's
`turn_outcome`, which is off and has not passed acceptance
(`docs/settings-reference.md:286`, `docs/status.md:921-926`). So the row reads Done
or Idle, is never counted and never notified. A phone render shows it: the agent
asks "Should the guide cover the phone at all, or only the desktop?" under a strip
that says "idle since 50 min", with quick replies "Yes", "Continue", "Run tests"
([image](images/today-09-phone-inbox-and-cards.webp), fourth phone).

**Do, rules first** (the order the design system asks for):
1. **Show, do not classify.** The Stop hook already receives
   `last_assistant_message` and keeps its first 200 characters as the `turn_done`
   detail (`service/hooks.rs:1269-1278`). Keep the last 200 too, on the row. Line
   two of a finished row becomes "Done: *its first sentence*". A person can triage
   from the list with no classifier at all.
2. **One rule.** If the last sentence ends in a question mark, the lead is "Asks:"
   and the row sits in the Answer lane, uncounted, marked "by rule".
3. **Ask Claude Code.** Its Notification hook has `idle_prompt`,
   `agent_needs_input` and `agent_completed`; fleet subscribes to none of them
   (`service/hooks_install.rs:124`). Their exact triggers are thinly documented.
   Test them on a host; if `agent_needs_input` fires here, it is a hook, and "hooks
   always win".
4. Jev stays the fourth reader, for what the rules miss.

**Size:** M. **Check:** a turn ending "Which one should I use?" shows that sentence
on its row within one hook, with Jev off.

<a id="h-07"></a>
#### H-07 · H · Being told is not dependable

| Where | What happens | Evidence |
|---|---|---|
| Desktop, window focused | A new ask raises no toast, no sound, no notification. Only the rail number changes. A stuck session does raise a toast. | `src/lib/Attention.svelte:112`; capture `attention--after-blocked` vs `attention--after-stuck` |
| Desktop, window in the background | Nothing unless the device switch "OS notifications" is on. It is off by default and its key is `notify.stuck-os`, though it gates every state. | `src/lib/notify.ts:24` |
| Any device | A waiting session is announced once. No reminder, no escalation. | `Attention.svelte:140-146`; `fleet-mobile …/notify/NeedsYou.kt:61-88` |
| Desktop | "Routine … failed" cannot fire: the list refreshes only while focused, the notification only while not. | `src/lib/window_hidden.ts:46-60` against `Attention.svelte:112` |
| Desktop | Mission waits, operator confirms, review requests, autofix findings and access requests never notify. | `Attention.svelte:124-205` watches sessions and routines only |
| Phone | No push service. Android holds a socket in a foreground service, off by default, with no boot receiver. iOS checks when the system allows, no sooner than 15 minutes. | `fleet-mobile …/android/NeedsYouService.kt:423`, `AndroidManifest.xml:43-48`, `iosMain/…/NeedsYouRefresh.kt:26` |
| Phone | The Welcome screen promises "from the lock screen to the answer in two taps". No notification button answers. | M1 notes §4 |

**Why it matters.** An agent that waits costs time every minute it waits. Paging
tools escalate until someone acknowledges (PagerDuty: default 30 minutes). An
alert that changes nothing while a person is busy is right; one that never comes
back is not. Deferring to a breakpoint lowers frustration (Iqbal & Bailey 2008),
and a negotiated interruption did best overall (McFarlane 2002).

**Do** ([board: Notifications](images/board-Attention.webp)).
1. Decide on the hub, not in each window: one pipeline, one table of kind × device.
2. In the app, a new ask shows a quiet toast with Open.
3. One reminder (15 or 30 minutes by kind), saying what the ask holds up.
4. The cause once: "acme reached its 5-hour limit · 4 sessions paused".
5. The phone only when the desk is away (no key or pointer for 2 minutes). Claude
   Code's own mobile push does the same.
6. Real push (FCM, APNs) and a boot receiver. Until then, a "Delivery" table that
   says when each device last took a notification, with "Send a test".

**Size:** S for 2, and for fixing the routine notification; M for 1, 3, 4, 5; L for push.

<a id="h-08"></a>
#### H-08 · H · Asks live in nine places

| Ask | Listed in the Inbox | In the badge | Desktop notification | Lifetime |
|---|---|---|---|---|
| Session dialog (permission, question) | yes | yes | yes, when unfocused | until answered |
| Chat form | yes, as a chip "FORM WAITING" | yes | yes | 24 h |
| Mission waiting on a person | yes | yes | **no** | until answered |
| Agent action (reviewer, support) | yes | yes | **no** | durable |
| Failed routine | yes | yes | dead path (H-07) | until fixed |
| Review asked by a colleague | only when something else is listed; header can read "Nothing needs you" | **no** | **no** | until Done |
| Operator confirm (kill, start, move) | **no**: Control's chat or a modal | **no** | **no** | **10 minutes, in memory, lost on a hub restart** |
| Autofix finding | **no**: Control's panel | **no** | **no** | 3 tries a day |
| Access request | **no**: a count on the Share badge | **no** | **no** | until answered |

Evidence: `src/lib/inbox.ts:224-255` (the count), `src/lib/Sidebar.svelte:1124-1126`
and `:1779-1780` (review requests sit inside a section that may be absent),
`crates/fleet-core/src/mcp/guard.rs:33` and `:2535` (confirm TTL and store),
`src/lib/ControlViewsPanel.svelte:294`. A spec already says the 10-minute confirm
"is the wrong tool for an approval that may wait a day"
(`docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md:173-175`).

On one Control screen the words "need you" carry four numbers: 3 in the thread
list, 5 on the panel's tab, 4 in "Welcome back", 3 on the strip
([image](images/today-08-control.webp)).

**Do.** Do for asks what step 0.4 did for attention: one table. Every kind of ask
has a row that says where it is listed, whether it counts, whether it notifies,
when it expires, who may answer. Both test suites read it. Then:
- every kind is listed in the Inbox and counted;
- an operator confirm is durable (the `agent_actions` queue is the model) and its
  expiry is announced;
- a number that means "Control's threads" says so.

**Size:** S to list and count review requests and confirms; M for the table and a
durable confirm.

<a id="h-09"></a>
#### H-09 · M · The same reason is filed differently on the two devices

The phone files stuck, host down, limit, signed out, context full, stale working
and failing CI as `blocked` (`fleet-mobile …/notify/FleetNotify.kt:27-32`). The
desktop files context full, stale and stuck as `needs_you`, failing CI as `failed`
(`src/lib/notify.ts:150-161`). One `notify.*` setting, two behaviours. A stuck
session is filed `blocked`, so with the default `notify.sound` it never chimes
(`Attention.svelte:82-91`).
**Do.** Put the notify row in `attention_states.json`, next to the state. **Size:** S.

<a id="h-10"></a>
#### H-10 · M · One cause, many rows

An account at its limit shows as one row and one notification per session. A down
host does the same. The mass-loss line shows the pattern is known. **Do.** A cause
row: "Weekly limit on tech.silvester · 4 sessions" with *Switch all 4…* and *Wait*.
**Size:** M.

### 4.3 Understand: what is being asked

<a id="h-11"></a>
#### H-11 · H · The row says "Needs you" without saying why

**Seen** ([image](images/today-01-inbox.webp)). In a populated Inbox of 17:
- six rows carry the question ("Waiting for you: What VAT ID should the sample
  invoi…"), the whole line in bold amber;
- eight read `Needs you · Claude Code · htz-1 · <the last prompt>`. These are the
  stuck, stale, context-full and form rows. The reason is not on the row;
- the permission row reads "Waiting for you: Do you want to proceed?", which is
  Claude Code's wording for every permission and names nothing;
- a failed row shows the raw payload: `API Error: 529 {"type":"ov…`;
- a form row carries a boxed "FORM WAITING" in capitals and no title.

The kit's own row says line two is "what it waits on or did last, never the last
prompt" (`src/lib/kit/SessionRow.svelte:1-4`). The shipped row is a different
component (`src/lib/SessionRowItem.svelte`) and shows the last prompt.

The phone has the right shape and weak fallbacks: "Waiting for you: Needs you",
"Waiting for you: waiting for input", "Failed: Failed · stuck", and "Waiting for
you: hetzner-3 is down" for a host nobody is waiting on
(`fleet-mobile …/ui/PhoneSessions.kt:107-128`, `waitsOn` and `phoneLead`).

**Do** ([board: Status language](images/board-Status.webp)). Line two always says
the reason:

| State | Line two |
|---|---|
| Needs you | `Waiting for you:` the command, the question, or "a form, 3 fields" |
| Failed | `Failed:` why in words ("stuck on the trust prompt", "the API was overloaded (529)") and the one fix |
| Paused | `Paused ·` what it waits on and until when |
| Working | `Working:` what it is doing |
| Done | `Done:` the first sentence of its last message, then the PR and checks |

Only the lead is coloured. For a permission, write the headline from the tool call,
as the phone does. **Size:** S. **Check:** no row in the capture world reads
"Needs you" with no reason; a lint fails on a `{`.

<a id="h-12"></a>
#### H-12 · H · The dot is green on every row

The desktop dot shows the tmux session's lifecycle: `.status-dot.status-running`
is `--status-done` (`src/lib/SessionRowItem.svelte:688`, `:1220`). So Needs you,
Failed and Paused rows all carry a green dot. The design manual says "the dot
carries the colour, the word carries the meaning", and the phone does that.
**Do.** The dot is the state; tmux and host health move to the inspector. Back the
colour with a shape: filled, a ring for paused, dashed for lost. **Size:** S.

<a id="h-13"></a>
#### H-13 · H · The card shows the command, not what it does; an edit is approved unseen

**Seen** ([image](images/today-11-answer-cards.webp)).
- A push to `main` and `ls` get the same card: "Do you want to proceed?", the tool
  call, **1 Yes**.
- An edit shows "Do you want to make this edit to pricing.md?" and
  `Edit(docs/pricing.md)`. The change itself is only in the terminal. The card's
  own rule is that "the person approves what they can read".
- Option 2 is cut at the folder: "…for git push commands in
  /home/martin/src/billing-api…". The cut part is the rule's reach.
- The card has no age, though the board draws "asked 2m ago".
- A kill asked by Control shows `kill_session host=mac name=flt-219-settings` and
  Approve. The person's own Kill dialog checks for unsaved work and offers Clean
  up (`src/lib/KillDialog.svelte:88-126`). The agent's kill is approved on less.

**Why it matters.** A warning that looks the same every time stops being read:
people clicked through 70% of one browser's SSL warnings and a tenth of another's
malware warnings; the design made the difference (Akhawe & Felt 2013). Making
people touch the part that changed resisted habituation (Bravo-Lillo et al. 2014).

**Do** ([board: Decision card](images/board-DecisionCard.webp)).
- A headline in the app's words; the command stays in mono.
- Two or three facts read by rule, each with its source: "Pushes 3 commits to main.
  CI has not run on them." For an edit, the diff. For a kill, the Kill dialog's
  check.
- "A person's step" takes two presses, and the second button names the act. It is
  the step "Save rule and allow" already is. No session-wide "don't ask again" is
  offered for it.
- The age, amber past 15 minutes, and what waits behind the ask.

**Size:** M.

<a id="h-14"></a>
#### H-14 · M · The terminal's rows leak into the card

"Type something." and "Chat about this" are drawn as numbered answers beside the
real ones. A free-text question offers "1 Type something.", "2 Chat about this",
"Answer in your own words…" and "Open terminal", and no text field
([image](images/today-04-free-text-question.webp)). Labels carry "(esc)" and
"(shift+tab)". "Answer in your own words…" leaves "✓ Sent: Dismissed" on the card.
On the phone the hidden folder option makes the numbers read 1, 3.
**Do.** Only real options get a number. The rest becomes one text field in the
card and one link. Strip the key hints from labels. **Size:** S–M.

<a id="h-15"></a>
#### H-15 · M · The name is the first thing to go

- Session header: the title shrinks before anything else. "Invoic…", "Settings
  se…", and no title at all once "Ada watching" shows
  ([image](images/today-02-permission-card.webp)).
- Selected row: eight hover icons and a PRIVATE chip cut the name to "Stripe w…".
  A click in the middle of a hovered row lands on Restart and opens "Restart
  claude?" ([images](images/today-06-row-hover-and-bulk-bar.webp),
  [and](images/today-07-row-click-restart.webp)).
- Phone header: "Quiet hour…" beside four icons; four or five letters at 320 dp.

**Do.** The title keeps 24 characters; actions fold into a menu first. Two hover
icons and a menu, placed after the name, never over it. **Size:** S.

<a id="h-16"></a>
#### H-16 · M · Counts and words disagree on one screen

- Inbox: a red 17 by the title, an amber 17 on the rail, "17 NEED YOU", then
  "NEEDS YOU 15", then "17 waiting".
- Sessions search: tab "All 8", footer "24 sessions".
- Status bar: "oci-arm still connecting · All systems OK · … · oci-arm offline".
- One limit, three wordings: "5-hour limit · 20:27", "5h LIMIT · resets 20:28",
  "5h 0% left·week 29% left".
- A confirm card reads "asked now ago".
- "Continue" is a card button and a reply chip, one above the other. "Not waiting"
  is a button and the first words of the footer.
- Phone: the bar badge leaves review requests out; the Inbox header counts them.

**Do.** One count per screen, in amber, named once. "All systems OK" only when it
is. **Size:** S.

<a id="h-17"></a>
#### H-17 · M · "Paused" names three states

A limit (counted), a lost or ghost session (not counted), and a **failed safe
kill**, which means work that could not be committed or pushed
(`src/lib/attention.ts:328-331`). The last is not counted and not notified.
**Do.** A failed safe kill is Failed. Lost reads "Paused · lost when mac restarted".
**Size:** S.

### 4.4 Decide: answering

<a id="h-18"></a>
#### H-18 · H · Every ask costs a trip

- The Inbox opens on "No session open" with 17 waiting.
- Rows take an answer only in Comfortable density
  (`src/lib/SessionRowItem.svelte:1003`); the default is Compact
  (`src/lib/prefs.ts:135`).
- Digits answer only in the Conversation, not from the list.
- After an answer the view jumps within 0.1 s to the next session, which may be a
  stuck one or a colleague's. What the answered session did next is never shown;
  only a toast.

**Do** ([board: Inbox](images/board-Main.webp)).
- The Inbox selects its first ask. The main pane is the decision: the card, the
  facts, the agent's last words, the diff in question. Open the session is one key.
- `j` `k` move, `1`–`9` answer, `l` later, from the list.
- After an answer the card stays for a moment with "✓ You approved it", then moves
  on; "Next:" names where.
- Group by kind of ask: Approve, Answer, Fix, Review. The order inside each is
  oldest first, by rule.

**Size:** M. **Check:** clearing three asks takes three keys.

<a id="h-19"></a>
#### H-19 · M · The same ask, many times

Three sessions that ask to run the same test command are three cards. A plan of
steps from Control is "Confirm N" or Cancel, a kill and a start in one press.
**Do.** Group asks with the same command that stay in a worktree: "Yes for 3 · No
for 3 · One by one". Never group a person's step. In a plan, each step can be
unticked. **Size:** M.

<a id="h-20"></a>
#### H-20 · M · Later means four things

Control's cards have Later, kept in prefs. The phone's Later only withdraws the
notification. "Wait until 20:19" is kept in memory and does nothing on the hub
(`src/lib/account_limits.ts:92-96`). The Inbox has none.
**Do.** One Later: a time or a condition ("when I am back at the desk"), kept on
the hub, shown on the row, counted apart. incident.io asks for a reason and logs
it; a time is enough here. **Size:** M.

<a id="h-21"></a>
#### H-21 · M · The phone's lock covers one approval of many

The fingerprint is asked before the session's answer card
(`fleet-mobile …/App.kt:3407`). It is not asked before "Answer in your own words…",
an agent action's Approve or "Approve all N", Control's Approve, a chat form, a
mission decision, or a setting proposal. A notification's Retry runs while the app
is locked (`App.kt:1424-1432`).
**Do.** One gate for every send that approves. **Size:** S–M.

<a id="h-22"></a>
#### H-22 · L · Small holes in the guards

- ⌘K "Approve" presses option 1 with no 600 ms arm (`src/lib/commands.ts:345-358`).
- Confirm cards answer to `1` and `2` through the card's default, outside the
  shortcut registry (`src/lib/kit/QuestionCard.svelte:65-73`).
- Escape does not dismiss the card (`src/lib/AnswerPrompt.svelte:280-294`), though
  the parity list says it does.
- The toast's "Undo" after an approval only goes back to the session. Call it
  "Back to it".
- Escape does not leave select mode; the bulk bar's buttons overlap at 340 px.

### 4.5 Verify: checking the result

<a id="h-25"></a>
#### H-25 · H · The diff shows a blank line after every line

**Seen** ([image](images/today-05-diff.webp)). Measured in the page: rows start
40.5 px apart, where the type asks for 16.5. About 16 lines fit where 39 should.

**Cause.** `.diff` has `white-space: pre` (`src/lib/DiffView.svelte:130`). Since
review notes landed (`bfd90f1c`) each row is followed by `{#if below && nl}`, and
Svelte keeps the space between them as a text node. Inside `pre` that space is a
line. The compiled template is `<div>…</div> <!>`. The note button also makes each
row 24 px tall.

**Do.** Move `white-space: pre` to `.text`. **Done** in Wave 0: rows now follow
each other with no gap. Rows with a note button stay 24 px tall: that is the
app's 24 px target floor (`src/app.css`, WCAG 2.5.8), kept on purpose. A denser
review diff would need the note target moved off the line number (the whole row,
or a gutter button that appears on hover). **Size:** S.

<a id="h-26"></a>
#### H-26 · H · Review has no home

The pieces exist in five places: the diff in Files; `PrResult` in Details; the
task result panel in Work; Finish checks in a mission; CI as a chip. None of them
is where a finished session lands. There is no "viewed" per file, no count per
file, no verdict. The reply chips after a finished turn are "Continue · Run the
tests · Open a PR · Explain", with "Open a PR" offered for a session whose PR is
approved ([image](images/today-03-done.webp)).

**Why it matters.** The owner asked for exactly this: "a claim of completion can
be traced to evidence and the summary admits failed or skipped checks"
(`docs/superpowers/specs/2026-09-29-ai-task-system-brainstorming.md:87`). People
with an AI assistant wrote less secure code and were more sure of it (Perry et
al. 2023); experienced developers were 19% slower with AI tools and believed they
were 20% faster (METR 2025). The claim and the evidence must sit side by side.

**Do** ([board: Review](images/board-Review.webp)). One screen for a finished
session:
- *What it says it did*: its words, marked "not checked".
- *What fleet read*: tests, checks, uncommitted files, each with source, commit
  and time. "2 commits since that run were not tested." "Type check: no run found."
- *What the ticket asks for*: its criteria, ticked by the person.
- *Worth a look*, by rule: a deleted test, a migration, a change past 400 lines.
- The diff, with viewed per file and notes.
- A verdict: Send notes · Ask a colleague · Run a review · Mark PR ready… ·
  Reviewed.

**Size:** L; the first slice (the lane, the packet from data the hub already has,
the verdict) is M.

<a id="h-27"></a>
#### H-27 · M · The diff is hard to read for long

Three rows of tabs and two toolbars stand before the first line (about 260 px).
`diff --git`, `index`, `---` and `+++` are drawn as rows. The type is 11 px. There
is no next-hunk key on the desktop; the phone has one. **Do.** One toolbar; the
file header as a line; 12 px; `n` `p` for hunks, `v` for viewed. **Size:** S–M.

<a id="h-28"></a>
#### H-28 · M · Notes are lost with the device

Drafts are kept on the device (`docs/review-notes.md:14-16`), and in memory only on
the phone (`fleet-mobile …/ui/ReviewNotesViewModel.kt:42-45`). **Do.** Keep drafts
on the hub, per person. **Size:** M.

<a id="h-29"></a>
#### H-29 · L · A person's steps have no button for the person

Merge and "ready" are a person's (H-01), and the app offers neither. **Do.** "Mark
PR ready…" and "Merge…" in the Review verdict, with the two presses of H-13.
**Size:** M.

### 4.6 Recover and learn

<a id="h-31"></a>
#### H-31 · M · Nothing happens when nobody answers

A session waits for ever. The only "nobody answered" path is Control's
auto-continue (`crates/fleet-core/src/service/control_continue.rs:21-36`). An
operator confirm vanishes after 10 minutes with no word.
**Do.** The reminder of H-07; an expiry that is announced; and on the row, what
waits behind the ask ("2 tasks held"). **Size:** S–M.

<a id="h-32"></a>
#### H-32 · M · The attention a fleet costs is not measured

The owner's measure is "time to a useful result and the human attention needed,
not the number of tasks or agents"
(`docs/superpowers/specs/2026-09-29-ai-task-system-brainstorming.md:78`). Nothing
records an ask's wait or a review's wait. **Do.** §7. **Size:** M, after H-03.

### 4.7 Phone

Beyond H-06, H-07, H-11, H-15, H-21:

| Id | Sev | Finding | Evidence | Fix |
|---|---|---|---|---|
| H-33 | H | The question card cuts off its own answers at 360×780. "Chat about this" and the multi-select's Continue are clipped; a long command is printed twice and leaves only "1 Yes". At 320 dp or font 1.3, two answers show. | `session--multiple-choice`, `session--multi-select`, `session--permission-long-command` | The answers scroll, the question does not; print the command once |
| H-34 | H | An Inbox row cannot be answered; the pager spec planned answer chips. | `fleet-mobile …/ui/InboxViewModel.kt:76-98` | A question row opens into its card in the list ([board](images/board-Phone.webp)) |
| H-35 | M | Control's Approve and Deny are pushed off screen by three escalation cards; no chat turn is visible. Confirm cards are polled every 4 s only while the tab is open, are not in the Inbox and never notify. | `control--chat-confirm-escalations`; `ControlChat.kt:520-570` | A confirm is an ask (H-08); escalations fold to one line |
| H-36 | M | At font 1.3 the notes bar fills the diff and no code shows; "Sessions" wraps in the bottom bar. | `files-tab--diff-with-notes-dark-font13`, `bottom-bar--each-tab-dark-font13` | `maxLines`; the notes bar on one line |
| H-37 | M | Drawn targets under 48 dp: quick replies 29, Control's pills 30, the autonomy chip 22, a diff line 18. | measured in the renders | `touch-min` on custom clickables |
| H-38 | M | The pull request list cuts each row before its CI and review state. | `pull-requests--open-ci-states` | State first, title second |
| H-39 | L | Enabled primary buttons ("Approve", "Retry the last turn") are drawn in the muted grey; "Reject" beside them is accent. | `inbox--missions-agents-reviews` | One button style per role |
| H-40 | L | A shared session that waits on its owner reads "Waiting for you:". The Android group summary says "N sessions need you" for failures and done. | `inbox--proposed-and-shared` | "Waiting for Ada:" |

### 4.8 Craft

- **Type.** In the Sessions list 98 of 131 text elements are 11 px, the floor. Line
  two of a row should be 12 px (`text-xs`).
- **Settings.** Checkboxes are 13×13 px. The only notification switch a device has
  is labelled "OS notifications" under a heading about stuck sessions.
- **1100 px.** The status bar wraps and is cut.
- **Federated fleets.** Their rows use another anatomy: the name, then five text
  buttons always shown, and "needs you" in lower case (`src/lib/FleetInbox.svelte`).

### 4.9 Settings, sizes and first run (later captures)

From a second capture set (166 screenshots, axe-core on five screens). File names
are in the capture set, not in `images/`.

| Id | Sev | Finding | Fix |
|---|---|---|---|
| H-41 | H | One world, seven "need you" numbers: rail badge 5, "5 NEED YOU", section 4, footer "3 waiting", Control 3, dashboard widget 2, org page 2. | Part of H-08 and H-16: one count, from the ask table |
| H-42 | M | Usage reads "% left" in the header (`5h 39%`) and "% used" in the dashboard widget (`5h 61%`), neither labelled. The limit card says "No other login has room" while each paused row offers Switch account. The header says 5-hour limit, the rows say weekly. | "75% left" everywhere; hide Switch when nothing has room |
| H-43 | M | Pause all asks nothing and stops everything in one click; paused routines still show "next in 8h" and Run now. | A confirm that lists what stops; paused rows say Paused |
| H-44 | M | At 150% text size the header items overlap, the session title goes and answer buttons are cut. At 1100×720 the title reads `cl…` and the composer `Effor`. | The title rule of H-15; the header folds into a menu |
| H-45 | M | In Settings, Tab walks into the hidden session pane and stops on `terminal-ime` with no ring: 5 of 20 stops are invisible. Sessions and Inbox: 20 of 20. | `inert` on what a dialog covers |
| H-46 | M | axe-core: a form field without a label (`.port`, critical), a tablist without tabs (Terminals, critical), `aria-label` on status dots where it is not allowed (serious). Settings checkboxes are 13×13 px (23 of them). | Fix the three rules; 24 px targets |
| H-47 | M | First run: the Welcome dialog covers the Get started card, a coach mark sits on the tour, and tour steps 3 to 6 point at an empty pane. | Show one at a time; tour steps only where their target exists |
| H-48 | L | Missing spaces and clipped text: `(paired as`, `limituntil`, `timed out.Sessions`, `Sessions3`, `Orbi`; 12- and 24-hour clocks on one page. | A copy pass |

Measured and fine: every status word and all muted text is at least 4.5:1 in both
themes (lowest 5.22:1); no target under 24 px in Sessions or the Inbox.

### 4.10 Review flow and consequential dialogs (later captures)

From a third capture set (178 screenshots, focus and IPC recorded per dialog).

| Id | Sev | Finding | Fix |
|---|---|---|---|
| H-49 | H | Tidy-up opens with destructive rows already ticked: 3 of 4, Kill and Clean up among them, and the button reads "Tidy 3". A suggestion should start unticked, as the design says for agent proposals. | Nothing ticked; the button counts what the person ticked |
| H-50 | M | The Kill dialog lists 6 uncommitted files and 3 unpushed commits, and Kill still sends at once. Cancel has focus, which is right; the safer path is not the first offer. | With unsaved work, Clean up is the first button and Kill takes the two presses of H-13 |
| H-51 | M | Enter in New session's Name field starts the session (`start_work`); the "can't be cancelled" line then overlaps Cancel. | Enter submits only a one-field form, as `keyboard.md` says; ⌘↵ starts |
| H-52 | M | A review note's editor does not take focus: typing after clicking a line number goes nowhere. | Focus the textarea when it opens |
| H-53 | M | One state, four labels: Failed in the header and row, Done in Details › Status, "Failed · cannot merge" in Result, "completed" in ⌘K. | One status word from the attention table, everywhere |
| H-54 | M | A host goes down and the status bar still reads "All systems OK"; its session's terminal is blank with no message. | Part of H-16; the pane says the host is down |
| H-55 | L | The sender of "Ask for review" sees only a toast; no screen lists the asks they sent. | A "Sent" view in Review |
| H-56 | L | "Move to host…" opens a sheet titled "Transfer"; Ctrl+Shift+N is listed for New session and New task; the Clean up dialog opens with nothing focused; git write controls on a paired hub give their reason only in a tooltip. | A copy and focus pass |

The blank line in the review-notes diff (H-25) shows here too, against 16.5 px rows in a commit's diff from the same component; removed lines also print the old number in both gutters.

### 4.11 Control, missions and Work (later captures)

From the fourth capture set (177 screenshots, focus and IPC recorded).

| Id | Sev | Finding | Fix |
|---|---|---|---|
| H-57 | H | When an operator confirm card arrives, focus moves to its first answer, **Approve**, so Enter approves a kill or a force push. The card component focuses its first answer on arrival (`QuestionCard` `enter`, `src/lib/kit/QuestionCard.svelte:51-62`), and `ConfirmCards` lists Approve first. The modal for the same request starts on Deny, which is the fix the 2026-09-21 audit made (UX-124). | Focus the card, not an answer; or Deny first, as the modal does |
| H-58 | H | Escape on a focused confirm card leaves Control and sends `mcp_confirm approved:false`: a navigation key denies the request. | Escape only leaves; Deny is a button |
| H-59 | M | After Approve the card disappears; after Deny nothing remains. The only trace is "5 handed off" turning to "6". | The "after the answer" card of the Decision card board, kept in the chat |
| H-60 | M | A kill gets the raw call `kill_session host=mac name=…`; a start gets a title, facts and a brief. | H-13: the riskier the call, the more the card says |
| H-61 | M | A mission waiting on a person reads "Failed · stuck" in triage beside a "Needs you" chip. Finish checks run label into value ("ci:buildbuild passed"); Done-when shows open circles on a completed mission. | One state word; a copy pass |
| H-62 | M | The Work tab strip overlaps itself at the default width ("Tasks 24Missions 2oard"); board cards are clipped at the column edge; one agent is "client:ux-agent", "ux-agent" and "Control". | Tabs wrap or scroll; one agent name |
| H-63 | L | Clicking a mission row in Control's views froze the renderer, also with the fixture from `ControlMissionView.test.ts`. Not confirmed in a real build. | Check in a real build |
| H-64 | L | Today has no page of its own; it lives in Control's right panel, where ⤢ is disabled. | Let it open full width |

---

## 5. Target design

Eight boards, in the canvas format the redesign uses. Each keeps the hard rules:
the six status words, amber only for waiting, the rail order, the left list and
its filters, every existing shortcut, no Approve on a notification, and nothing
decided by AI. What is new is a grouping, a pane, two pages and a log.

| Board | What changes | Findings |
|---|---|---|
| [Inbox](images/board-Main.webp) | A queue of decisions. Lanes by kind of ask (Approve, Answer, Fix, Review). Rows say the ask. The main pane is the decision: card, facts, last words, the diff in question. `j` `k`, `1`–`9`, `l`. "Next:" names where an answer leads. | H-05, H-10, H-11, H-18, H-19 |
| [Review](images/board-Review.webp) | A finished session: the claim, what fleet read, the ticket's criteria, what is worth a look, the diff with viewed and notes, a verdict. | H-25 to H-29 |
| [Rules](images/board-Rules.webp) | Rules offered from your answers by count, built-in rules, rules found on hosts, "Try a command". | H-03, H-04 |
| [Autonomy](images/board-Autonomy.webp) | Two tables: what a session runs alone, what Control does without a card. "Always a person." "Done without asking." | H-01, H-02 |
| [Decision card](images/board-DecisionCard.webp) | Nine variants: in the worktree, a person's step, an edit with its diff, a choice, free text, several at once, after the answer, answered elsewhere, read-only. | H-13, H-14, H-19, H-22 |
| [Status language](images/board-Status.webp) | One row per state; what each word promises; today against then; the header, the selected row, the count, the status bar. | H-11, H-12, H-15, H-16, H-17 |
| [Notifications](images/board-Attention.webp) | Kind × device × "still waiting"; five notifications; Focus; quiet hours; a delivery table. | H-07, H-09, H-10, H-31 |
| [Phone](images/board-Phone.webp) | The lock screen names the kind; a question opens in the list; a person's step takes a hold; a review is triaged, not done. | H-21, H-33 to H-35 |

Three of the boards differ from the current canvas on purpose:

- **Approve is not primary.** The canvas draws it in the accent on every board and
  moves focus to it (`canvas/Motion.dc.html:84`). `ai.md` and the code say it is
  never pre-selected. The boards here follow the rule.
- **The Inbox groups by kind before age.** The manual says "sorted by when it
  asked". Kind then age is still a rule, and it lets a person clear the yes/no asks
  in one pass. It is a choice for the owner (§8).
- **Done has a count.** Not in the badge; beside it.

---

## 6. Plan

Sizes as above. Each row is a PR or a short series.

### Wave 0: this week, no migration

| Step | What | Finding | Check |
|---|---|---|---|
| 0.1 | Diff: no blank line after each row (done) | H-25 | rows follow each other with no gap |
| 0.2 | Row line two says the reason, in every state; only the lead is coloured; no raw payload | H-11 | no bare "Needs you" in the capture world |
| 0.3 | The dot is the state | H-12 | a Failed row's dot is `--status-failed` |
| 0.4 | The title keeps 24 characters; two hover icons and a menu, after the name | H-15 | a click at the row's centre opens the session |
| 0.5 | One count per screen; "All systems OK" only when true; "asked just now" | H-16 | — |
| 0.6 | Review requests and operator confirms are listed and counted in the Inbox | H-08 | the header never reads "Nothing needs you" with a review waiting |
| 0.7 | A quiet toast for a new ask while the window has focus; the routine notification can fire | H-07 | — |
| 0.8 | A failed safe kill is Failed | H-17 | attention table case |
| 0.9 | The card: headline from the tool call, the age, the option's full text on wrap; no numbers on the terminal's rows | H-13, H-14 | — |
| 0.10 | Phone: leads and fallbacks; the card's answers scroll; `maxLines` in the bar | H-11, H-33, H-36 | renders at 320 dp and font 1.3 |
| 0.11 | The confirm card's sentence follows the level; the bulk bar wraps; Escape leaves select mode | H-02, H-22 | — |

### Wave 1: two to three weeks

| Step | What | Finding |
|---|---|---|
| 1.1 | The leash, first level: the guard asks before a person's step in every fleet-started session; the inspector says so | H-01 |
| 1.2 | Last words on the row; a closing question is an ask by rule; try `agent_needs_input` | H-06 |
| 1.3 | The Review lane with a verdict and its own count (one column; contract revision) | H-05 |
| 1.4 | The ask table: one row per kind, read by both test suites; a durable operator confirm | H-08 |
| 1.5 | The Inbox selects the first ask; the decision pane; answers from the list; "Next:" | H-18 |
| 1.6 | One reminder; the cause once; the phone only when the desk is away | H-07, H-10 |
| 1.7 | Android boot receiver; the Delivery table with "Send a test" | H-07 |
| 1.8 | The card: facts by rule, the edit's diff, two presses for a person's step, a text field | H-13, H-14 |
| 1.9 | One lock gate on the phone for every approving send | H-21 |

### Wave 2: four to eight weeks

| Step | What | Finding |
|---|---|---|
| 2.1 | The answer log and "Done without asking" (migration) | H-03 |
| 2.2 | The Autonomy page: both tables, a named default, confirm on more | H-02 |
| 2.3 | The five leash levels, per project and per session | H-01 |
| 2.4 | The Rules page: offers by count, rules found on hosts, "Try a command" | H-04 |
| 2.5 | The Review packet: evidence, criteria, viewed per file, notes kept on the hub | H-26 to H-28 |
| 2.6 | Push from the hub (FCM, APNs) | H-07 |
| 2.7 | Lanes, grouped asks, Later with a time | H-18 to H-20 |
| 2.8 | Focus and the digest | H-07 |
| 2.9 | The phone: answer in the list, a hold for a person's step, review triage | H-34 |
| 2.10 | The numbers of §7, on Today and the Autonomy page | H-32 |

**Order matters in one place.** 1.1 before 2.3: one level proves the hook path on
every host before five levels are offered. And 2.1 before 2.4: rules are offered
from the log.

**Make the audit repeatable.** The captures here came from a small wrapper around
`e2e/federation`: a hook `window.__FLEET_EXTRA__` for extra fake answers, and a
world with every attention state. Worth landing as a second scenario pack, so each
wave ends with the same screenshots taken again.

---

## 7. What to measure

All of it comes from the answer log (H-03) and the row's stamps. None of it leaves
the hub.

| Number | Meaning | Where to show it |
|---|---|---|
| Time to unblock | From ask to answer, median and p90, by kind | Autonomy page, Today |
| Waiting share | Of an agent's day, the part spent waiting on a person | Today |
| Asks per day, and per agent hour | The attention the fleet costs | Autonomy page |
| Answered by rule | Asks a rule answered, and asks that could be a rule | Rules page |
| Review wait | From a finished turn to a verdict | Today |
| Sent back | Reviews that ended in notes, and how many rounds | Review, Today |
| Quick approvals of a person's step | Approvals under 2 s. A sign the card is not read | Autonomy page |
| Notifications acted on | Of those sent, the share opened within 5 minutes | Notifications |

A useful bound from alarm management: one alert per 10 minutes is manageable for
long, two is the ceiling, ten is a flood (ISA-18.2, via secondary sources). If asks
per hour pass 6, the answer is a rule or a looser leash, not a faster card.

---

## 8. Decisions for the owner

| # | Question | Recommendation |
|---|---|---|
| 1 | The default leash: stay Unattended, or ask before a person's step? | Ask before a person's step. It adds a handful of asks a week and makes the "always a person" rule true. |
| 2 | Does finished work count? | Yes, on its own: "3 to review", never in the amber badge. |
| 3 | Order in the Inbox: age only, or kind then age? | Kind then age, as a grouping option that is the Inbox's default. |
| 4 | Reminders: one, at 15 and 30 minutes? | Yes, once, and never in quiet hours. |
| 5 | Claude Code's own *auto* mode as a leash level? A classifier approves in place of the person; Anthropic reports it misses 17% of overeager actions. | Offer it as a named level, off by default. It is stricter than Unattended, and it is still a model deciding. |
| 6 | Two presses on the desktop and a hold on the phone for a person's step? | Yes. |
| 7 | One answer for several sessions asking the same thing? | Yes, for asks that stay in a worktree. |
| 8 | Push: keys on a self-hosted hub, or a relay? | Keys on the hub; the app is yours. A relay only if others install it. |
| 9 | "Autopilot", beside "never an auto mode"? | Keep the level, rename it "On its own, inside a grant", or drop the sentence from the README. |
| 10 | Should the hook fail open? | Keep it open, say so on the Autonomy page, and show hosts where the hook is missing. |

---

## 9. Known and not reported again

- The 2026-10-10 canvas audit's open rows: Control threads by topic, form receipts
  without device and time, "Trust now", Wake host, the Inbox's empty state naming
  the next routine, phone quiet hours per phone, auto-continue on the phone
  (`docs/redesign/canvas-gaps-2026-10-10.md`).
- Live-instance F-04, bulk dismiss of ghosts
  (`docs/ux/2026-09-27-live-instance-analysis/ux.md:90-104`).
- The task-result leftovers "left for the owner"
  (`docs/superpowers/reviews/2026-10-15-task-result-handover-review.md:49-62`).
- Jev's acceptance: every use case is off and every benchmark set is synthetic
  (`docs/status.md:921-926`). H-06 is built so that it does not wait on it.

Contradictions between documents that bear on this audit, for whoever edits them:

| Topic | One document | Another |
|---|---|---|
| Operator starts on a hub | "refused" `docs/work-graph.md:1591-1593` | "wait for the owner's paired device" `docs/status.md:562-565` |
| Default level | "L0 globally" `task-to-session-flow-design.md:596` | `orchestrator.max_level` 1 `docs/settings-reference.md:50` |
| Approve on a notification | never, `design-system/README.md:66` | planned, `fleet-mobile …/2026-09-21-phone-as-pager-design.md:345-353` |
| "Paused" | a limit, `design-system/README.md:10` | ghost or lost on the wire, `docs/control-api.md:1553-1557` |
| Amber | "nothing else is amber" `design-system/README.md:20` | account pills, the login card, the context strip |
| Answer keys | "1/2/3" `keyboard.md:39` | "1–9" `transition-plan.md:408` |
| Touch size | 48 px `design-system/README.md:65` | 44 dp answers, `canvas/MobileSession.dc.html` |

---

## 10. References

Marks: **V** read in the source or the publisher's abstract · **P** through a
secondary source · checked 2026-11-10.

**Supervising automation**
- Amershi et al. 2019, *Guidelines for Human-AI Interaction*, CHI. G3 "Time
  services based on context", G11 "Make clear why the system did what it did",
  G16 "Convey the consequences of user actions", G17 "Provide global controls". V
- Horvitz 1999, *Principles of Mixed-Initiative User Interfaces*, CHI: weigh the
  cost of deferring against the cost of distracting; scope what is done to what is
  known. V
- Bainbridge 1983, *Ironies of Automation*: the more is automated, the less
  practised the person who must step in. V
- Parasuraman & Riley 1997, *Use, Misuse, Disuse, Abuse*; Parasuraman & Manzey
  2010 on complacency. V (abstracts)

**Being interrupted**
- Iqbal & Horvitz 2007, CHI: an alert costs about 10 minutes, then 10 to 15 more
  before focus returns; 27% led to more than two hours away. V
- Iqbal & Bailey 2008, CHI: notifying at breakpoints lowers frustration. V (abstract)
- McFarlane 2002, *HCI*: negotiated interruption did best overall. V
- Mark, González & Harris 2005, CHI: interrupted work resumed after 25 min 26 s on
  average. The often-quoted "23 minutes" is from an interview, not a paper. V
- ANSI/ISA-18.2 and EEMUA 191, alarm rates. P (the standards are paywalled)

**Warnings that stop being read**
- Akhawe & Felt 2013, USENIX Security: 70.2% click-through on one SSL warning, a
  tenth on another browser's malware warning. V
- Bravo-Lillo et al. 2013 and 2014, SOUPS: attractors, and touching the changed
  field. V
- Anderson et al. 2016, *JMIS*: warnings that vary are read longer. V
- Anthropic Engineering, 25 Mar 2026, *How we built Claude Code auto mode*: users
  accept 93% of permission prompts; the classifier misses 17% of real overeager
  actions. V (vendor data)
- Yan 2026, arXiv 2608.27443: person-written policies blocked 20.1 points less
  overreach than approving each action. V (abstract; preprint)

**Reviewing**
- Sadowski et al. 2018, ICSE-SEIP: at Google the median change is 24 lines and
  review takes under 4 hours. V
- Bacchelli & Bird 2013, ICSE: about one review comment in eight is about a
  defect. V
- SmartBear/Cisco: 200 to 400 lines per review. P (vendor)
- Perry et al. 2023, CCS: with an assistant, less secure code and more confidence. V
- METR 2025: 19% slower, believed 20% faster; METR's 2026 follow-up calls its new
  data an unreliable signal. V
- DORA 2024: stability fell 7.2% for each 25% more AI adoption. V

**Sizes and contrast**
- WCAG 2.2: 2.5.8 target size 24×24 CSS px; 1.4.3 contrast 4.5:1. Android 48 dp;
  iOS 44 pt. V

**Other products, from their documentation**
- Claude Code: permission modes, rules, hooks, sandbox; the agent view's groups
  *Ready for review · Needs input · Working · Completed*; mobile push is skipped
  while you are at the machine. V
- Warp, "Waiting on you": asked question, PR review, blocked, failed; "Mark as
  resolved" with Undo; opening is not resolving. V
- Amp: "Snooze Until I'm Needed". V
- Factory: commands rated low, medium, high against an autonomy level. V
- Zed: "Always for <pattern>" only when a safe pattern can be drawn. V
- GitHub Copilot agent: the person who asked cannot approve the pull request. V
- Jules: a plan is approved by a timer if you leave. V
- PagerDuty: acknowledge stops escalation; a timeout resumes it. incident.io: a
  snooze takes a reason and is logged. V

---

## Appendix: evidence

| Image | What it shows |
|---|---|
| [`today-01-inbox`](images/today-01-inbox.webp) | The Inbox with 17 asks: green dots, rows without a reason, an empty main pane |
| [`today-02-permission-card`](images/today-02-permission-card.webp) | A push to `main` in the conversation: "Do you want to proceed?"; the header has no title |
| [`today-03-done`](images/today-03-done.webp) | A finished session: "Idle" in the header, "Done" in the strip, generic reply chips |
| [`today-04-free-text-question`](images/today-04-free-text-question.webp) | A question with no options and no text field |
| [`today-05-diff`](images/today-05-diff.webp) | The diff with a blank line after every row |
| [`today-06-row-hover-and-bulk-bar`](images/today-06-row-hover-and-bulk-bar.webp) | The list beside select mode: bulk buttons overlapping |
| [`today-07-row-click-restart`](images/today-07-row-click-restart.webp) | "Restart claude?" from a click at a row's centre |
| [`today-08-control`](images/today-08-control.webp) | Control: four "need you" numbers on one screen |
| [`today-09-phone-inbox-and-cards`](images/today-09-phone-inbox-and-cards.webp) | Phone: Inbox, a permission card, a prose question under "idle since 50 min" |
| [`today-10-phone-control-diff-settings`](images/today-10-phone-control-diff-settings.webp) | Phone: a choice, Control with the confirm off screen, a diff with a note, notification settings |
| [`today-11-answer-cards`](images/today-11-answer-cards.webp) | The answer card's five shapes today |
