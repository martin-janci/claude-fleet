# Decisions (Jev) — experimental, off

fleet can ask a **decision model** — [Jev](https://docs.typesafe.ai/api.md),
TypeSafe AI's "System One" model — a closed-set question: which of these
tickets is this session working on, which status category is this Asana
section. This page is the user guide for that path. It is an **evaluation**
(decisions D31–D47 in
`docs/superpowers/specs/2026-09-27-jev-language-census-design.md`): everything
is **off by default**. Every use case goes through the envelope described
here; the first one built is [`status_map`](#status_map--asana-section-proposals-j3).

**A model answer never grants a permission and never runs a risky action.**
At most it pre-selects a suggestion a person confirms.

## What has to be true before anything is sent

A request leaves the machine only when **all** of these hold, checked in this
order on every call. The first that fails is the call's *fallback* (below),
and nothing is sent:

1. **This process owns the fleet** (`not_owner`). A hub, or a standalone
   desktop. A desktop paired to a hub, a phone and an agent host never call
   out: the hub decides for them.
2. **The kill switch is on** (`flag_off`): `decide.jev.enabled`, the toggle
   in Settings → *Decisions (Jev)*. Turning it off stops every call at once.
3. **The feature's mode is not `off`** (`mode_off`): `decide.jev.status_map`,
   `decide.jev.work_link`. The offline benchmark (`fleet-hub decide bench`)
   skips this one check — measuring a feature must not turn on its live runs
   — and every other check applies to it; its calls are always `shadow`.
4. **The org consented** (`org_off`). Each organisation opts in on its own
   (Settings → Organisations → *send to Jev*, or `fleet-hub org set <id>
   --jev on`); off by default. A session or item with no org follows
   `decide.jev.unassigned` (off by default). A feature that sends Claude's
   **reply text** (`turn_outcome`, J2; `routine_run_outcome`, N6) needs a second consent on top
   (decision D48): the org's *Also allow Claude's reply text* (`fleet-hub
   org set <id> --jev-reply on`), or `decide.jev.unassigned_reply` for a
   session with no org. Both off by default; either missing is `org_off`.
5. **A key is configured** (`no_key`): `fleet-hub decide set-key` (on a
   standalone desktop, pointed at the app's data folder — see
   [the key on a standalone desktop](#the-key-on-a-standalone-desktop)).
6. **The circuit breaker is closed** (`breaker_open`): after
   `decide.jev.breaker_failures` failed calls in a row (timeouts, HTTP errors,
   rate limits) no call is made for `decide.jev.breaker_open_secs`; then one
   call goes through, and a success closes it.
7. **Today's budget is not spent** (`budget`): `decide.jev.daily_token_budget`
   input tokens per UTC day, counted from the record — live and benchmark
   runs together, so the setting is the day's whole spend.

The breaker of the live features counts their own runs only: a benchmark's
runs (subject `bench`) never open the live breaker. A benchmark call is
gated on every run, its own and the live ones, so a failing API stops it
too. The budget is one for both: a benchmark that spends the day's budget
also stops the live features until the next UTC day.

## Modes

| Mode | What the feature does with an answer |
|---|---|
| `off` | Nothing is asked. |
| `shadow` | Asks and records the answer next to what the current rule decided; acts on the rule only. For comparison. |
| `assist` | Asks, records, and may **propose** the answer (a pre-selected suggestion); a person confirms. |

`auto` (acting without a person) is not offered: no feature has passed
acceptance.

## What a call does

- **Redaction.** Every text in the request is redacted first: URLs become
  `[url]`, email addresses `[email]`, and tokens and keys fleet recognises
  (the same patterns as its logs) `[REDACTED]`. A feature may redact more.
- **One attempt.** Bounded by `decide.jev.timeout_ms`, never retried on the
  path that asked.
- **Checked answers.** A choice must be one of the options offered, the
  probabilities a distribution over them, a score inside its levels;
  anything else is `invalid_answer` and is not used. A feature may set a
  confidence floor: below it the answer is recorded as `low_confidence` and
  not used.
- **Pinned model.** `decide.jev.model` is `jev-1.13.0` by default: TypeSafe
  advises pinning a version when thresholds are tuned against it.

## Fallbacks

Every call ends answered or with one of these; the feature then does what it
does today (its rule, or nothing):

| Fallback | Meaning |
|---|---|
| `not_owner` | This process is a window onto a hub. |
| `flag_off` | `decide.jev.enabled` is off. |
| `mode_off` | The feature's mode is `off`. |
| `org_off` | The org has not consented (or no org, and `decide.jev.unassigned` is off). |
| `no_key` | No key, or its `env:` / `file:` reference cannot be read. |
| `breaker_open` | Too many failed calls in a row, recently. |
| `budget` | Today's input-token budget is spent. |
| `timeout` | No answer within `decide.jev.timeout_ms`. |
| `http_error` | The API refused (401, 422, 5xx), the connection failed, or the request failed a local check the API would refuse (too many options, too large). |
| `rate_limited` | 429 or 529 from the API. |
| `invalid_answer` | An answer that does not fit the question. |
| `low_confidence` | A valid answer below the feature's confidence floor. |

## What is recorded — and what never is

Every call is recorded in `decision_runs`, **fallbacks included**, so a
shadow comparison sees how often the model was not asked:

- when, the feature, the org, what was decided about (`session 42`,
  `tracker 3`), the mode, the provider, the model version that answered, the
  feature's question version;
- the **candidates** offered (ids and vocabulary words only), the answer, its
  probabilities and confidence, the fallback, and what the current rule
  decided (the baseline);
- whether a request was sent, the latency, the input tokens and their cost;
- later, what became of it: `confirmed`, `rejected`, `corrected` (and to
  what), or `ignored`.

**Never raw text.** No prompt, no ticket title, no description, no
instruction. The store refuses a run whose fields are not ids, words or
numbers. The input is kept only as a **fingerprint**: an HMAC-SHA256 of the
redacted request under a key generated on this machine and never sent
anywhere — so identical inputs can be matched, but a guessed prompt cannot be
confirmed from the record without that key.

The API key itself is stored in `decision_secrets` (or as a reference to an
environment variable or a file), read only when a call is made, never
returned by any read path, never printed, and masked in the diagnostics
bundle.

## Proposals on the wire

Every row a person decides on carries what is proposed about it in one
shape, `proposals` (redesign step 2.8): `SessionRow.proposals` (runs with
`subject_kind = 'session'`, `subject_id` = the session id),
`WorkTask.proposals` (`work_item`, the item id) and a start preview's
`proposal` (K1's `suggested_project`, also kept under its old name). Each
entry is `{feature, value, source, reason?, confidence_pct?, run_id?, at?}`,
`source` being `rule`, `jev` or `llm`.

They are read, never stored on the row: per feature, the **latest** run
about the subject, kept only when it is a live `assist` answer with no
fallback and no follow-up, not `unsure`, and at or above the 0.5 floor. So a
shadow run is never shown, a later fallback withdraws the proposal, and a
person's confirm, correction or rejection takes it off the row. Recording a
run about a session, or its follow-up, re-emits the session row. `reason` is
fleet's own words when a use case composes them; a run read back carries
none, because `decision_runs` holds no text.

## Retention

`decide.retention_days` (90 by default; `0` keeps them forever). The GC
sweep deletes older runs a batch at a time, whether or not the GC's session
killer is on. A run a person followed up — `confirmed`, `corrected` or
`rejected` — is **kept** whatever its age: a rejection must keep holding
(the same answer is not proposed again), and those runs are the labels the
evaluation is judged on. They are few: one per decision a person made.

## Turning it off

- **Instantly, everything:** Settings → *Decisions (Jev)* → off, or on a hub
  `fleet-hub decide disable` (the same `set_setting` a master-token client
  can send). The next call is refused; nothing in flight is retried.
- **One org:** `fleet-hub org set <id> --jev off`, or its checkbox in
  Settings → Organisations.
- **One feature:** its mode to `off` (`fleet-hub decide mode status_map off`).
- **The key:** `fleet-hub decide clear-key`.

## Health: is it working?

`fleet_health` (the desktop's `health_check`, the hub's `fleet_health`
tool) carries a `decide` block while the kill switch is on or any feature
is not `off` — the test map's "degraded" (§7):

| Field | Meaning |
|---|---|
| `enabled`, `modes` | The kill switch, and every feature that is on (`shadow` / `assist`). |
| `attempts` | Live runs in the last hour that meant to ask. A configuration's refusal (`not_owner`, `flag_off`, `mode_off`, `org_off`, `no_key`) and a spent budget are not attempts. A benchmark's runs never count. |
| `failures`, `failure_rate` | Of them, the service failing: `timeout`, `http_error`, `rate_limited`, `breaker_open`. |
| `breaker_open` | The live circuit breaker refuses calls. |
| `budget_spent` | Today's input-token budget is spent — planned, so never `degraded`. |
| `degraded`, `reason` | The switch is on, a feature is on, and the breaker is open (`breaker_open`) or more than 20% of at least 5 attempts failed (`failure_rate`). |

A degraded envelope needs nothing done to stay safe: every answer falls
back to what fleet does today by itself. It is for a person to look — the
desktop shows one **Jev degraded** item in the Attention strip (to Settings
→ *Decisions (Jev)*), and `fleet-hub decide status` prints the same
judgement as its `health` line. Only the master token and an unbound
paired client read the block; a per-host token and an org-bound client get
none of it.

## `status_map` — Asana section proposals (J3)

An Asana task's status comes from its section: the map a person confirmed
(`settings.section_map`), else the map the probe inferred from section names
with a keyword rule (progress / doing / review → in progress, done / shipped
→ done), else **to do**. A section the rule cannot classify — "Ideas",
"Parked", "Čaká na klienta", "🚀 Live" — silently counts as to do. The
probe now keeps those names (`config.unmapped_sections`) and each board's
section order (`config.project_sections`), and `status_map` asks Jev which
category they are. Jira, Linear and GitHub carry exact status categories
from the tracker itself: `status_map` never looks at them.

**When.** After a clean sync pass of an Asana tracker, at most once a day
per tracker (sooner when its sections change; a run the gate refused —
no key yet, the breaker open, the budget spent — does not count, so the
next clean sync after that clears runs it), in a task of its own — never
on the sync's path, and a failure never fails the sync. A run asks at most
40 questions; a section decided in the last 14 days on the same input
(fingerprint), question version, mode and model is not asked again, and a
section in your own map is never asked.

**What is sent** (only when the gate above lets it through, and only for a
tracker whose org consented): the provider (`asana`), the section's name
(lower case) and the names of the sections on its board in order (at most
30) — nothing else: no task, no title, no project name. One *choice*
question per section, version `status_map.v1`, with the options `todo`,
`in_progress`, `done`, `not_planned` and `unsure`, each described; an answer
below confidence 0.5 is recorded as `low_confidence` and not used.

**What is recorded.** One run per question, about `tracker_section
<tracker id>:<section id>`, where the section id is an HMAC of the name
under the local fingerprint key — never the name. The baseline is the
keyword rule's answer, or `none` where it abstained.

**The modes.**

- `shadow`: asks about the unmapped sections *and* the ones the rule mapped,
  so the rule and the model can be compared; nothing is proposed.
- `assist`: asks about the unmapped sections only; the answers are
  **proposals**. Nothing is ever written to the tracker by itself.

**Reading and applying proposals** (on the hub; read-only, no running hub
needed):

```bash
fleet-hub decide proposals [--tracker 3] [--json] [--db FILE]
```

```text
tracker 3 "Company B"  org 1  status_map mode assist
  proposals (assist):
    "ideas" → todo (0.91)  run 812
    "parked" → not_planned (0.85)  (applies as done)  run 813
    "someday" → unsure (0.70)  (proposes nothing)  run 814
  apply (a person decides): fleet-hub tracker section-map 3 --set 'ideas=todo' --set 'parked=done'
  or one at a time: fleet-hub decide proposals apply <run> [--as CATEGORY] | reject <run>
```

The section names come from the tracker's stored config (matched through
the section id), not from the record. A section map takes `todo`,
`in_progress` or `done`: `not_planned` applies as `done` (the task is not
live work); `unsure` proposes nothing. The printed command is a
`work_admin update` (master token) that confirms the section map — the
inferred one kept under your entries, like Settings → Trackers' **Confirm** —
with the proposals you accept; edit it to drop or change any. `--json` also
carries the same change as `work_admin` arguments. In `shadow` the view
shows, per section, the rule's and the model's category and how often they
agree where the rule decided.

**One proposal at a time: apply, correct or reject.** A proposal is named
by its run id (`run 812` above). A person decides it three ways:

| | Hub (operator) | Standalone desktop | What it writes | Follow-up |
|---|---|---|---|---|
| **Apply** | `fleet-hub decide proposals apply 812` | Settings → Trackers → **Apply** | the answer's category into your section map (`not_planned` applies as `done`; `unsure` proposes nothing and cannot be applied as is) | `confirmed` |
| **Apply as** | `… apply 812 --as in_progress` | **Apply as…** | the category you chose (`todo`, `in_progress` or `done` only) | `corrected` to yours (`confirmed` if it is what the answer applies as) |
| **Reject** | `fleet-hub decide proposals reject 814` | **Not this** | nothing but the run's follow-up: the section stays unmapped (it counts as to do) | `rejected` |

An apply is a `work_admin update` of the tracker's settings — over loopback
on the hub, like `fleet-hub tracker section-map`; in process on a desktop,
like the section map's **Confirm** — so it confirms the section map with
the inferred one kept under your entries, and the follow-up is recorded as
below. `reject` writes only `decision_runs.followup` (on the hub, directly
in `state.db`, like `set-key`; the running hub needs no restart).

Only the proposal the view shows can be decided: a `status_map` run
answered in `assist` with no fallback, the latest such answer for its
section, not decided yet, about a section the tracker's stored config
still lists and that is not in your map already. The section's name is
looked up from the tracker's config through the section id — never taken
from the caller.

**Rejected stays rejected** until a new answer exists: the proposals view
hides a section whose latest answer was rejected on the same input
fingerprint, question version and model (`fleet-hub decide proposals`
counts them: `N rejected proposal(s) hidden until a new answer`), and in
`assist` the adapter does not ask that input again under the same pinned
`decide.jev.model`. A changed board (a new fingerprint), a new question
version or another model asks again and proposes again. (Under
`jev-latest` the section is re-asked on the usual 14-day schedule and an
answer of the model that was rejected stays hidden.)

**In the desktop.** With `decide.jev.status_map` at `assist`, Settings →
Work shows an Asana tracker's pending proposals as *Proposed by Jev
(assist)*: the section, the proposed category, the confidence (e.g. 0.82),
a short *why* — the answer's two most probable options — and, when shadow
runs exist, how often Jev agreed with the keyword rule on the sections the
rule classified; then **Apply**, **Apply as…** and **Not this**. Section
names are the tracker's text and are shown as plain text. A desktop paired
with a hub does not show them: deciding a proposal is tracker
administration (`work_admin`, master-only), so its commands
(`status_map_proposals`, `decide_status_map_proposal`) refuse with
`E_LOCAL_ONLY` and the operator uses the CLI above.

**Follow-up.** When your `settings.section_map` later holds a section, its
latest answered **assist** run — the proposal you were shown — is marked
`confirmed` (the answer applies as your category) or `corrected` (to
yours). A `shadow` answer is never marked: nobody saw it, so your map
neither confirmed nor corrected it, and the person counts (D34) hold only
responses to what a person was shown; the shadow comparison is its
baseline, next to the rule's answer. `fleet-hub decide status` counts the
marks, with the `rejected` ones. A proposal nobody decided before a newer usable
one for the same section arrived is marked `ignored` (only the latest is
ever offered); a follow-up already there is never overwritten.

**Off.** `decide.jev.status_map` to `off`; your confirmed maps stay (they
are yours).

## `start_project` — the repository of a task's first start (K1)

When no project has worked on a task's key prefix yet, a start cannot be
planned: the start popover asks *Pick a repository…* over the eight most
recently used projects. With `decide.jev.start_project` on, Jev is asked one
Choice over those same candidates (or `unsure`).

- **What is sent.** The task's key and title, the first 1,000 characters of
  its cached description, and each candidate's `owner/repo`, redacted.
  Nothing from the repositories.
- **Shadow.** Asked off the preview's path and only recorded, with the first
  candidate (today's first row) as the baseline.
- **Assist.** The preview waits for the one call (`decide.jev.timeout_ms`)
  and the popover pre-selects the answer with *Proposed by Jev (N%)*. You
  still press Start, and any other choice is yours.
- **Asked once per input.** Re-opening the popover on the same task reuses
  the decided run for 14 days.
- **Follow-up.** Your start marks the proposal you were shown `confirmed`
  (same repository) or `corrected` (to yours). An agent's start, and a
  shadow answer nobody saw, mark nothing.
- **What is recorded.** Subject `work_start` `item:<id>`, or `key:<HMAC>` for
  a key no tracker knows; options are project ids (`p<id>`).

Code: `service/decide/start_project.rs`, `preview_start_decided` in
`service/trackers/tickets.rs`; card K1 in the test map.

## `sibling_repos` — the other repository a ticket start also needs (N3)

A ticket's start in the New session dialog offers *Also start in <repo>* for
the projects the key ran in before (ended links' project and live sessions
on the key), other than the chosen one and system projects, newest first.
With `decide.jev.sibling_repos` on and a planned project, Jev is asked one
Choice: which ONE of those candidates the same task also needs changes in
(`p<id>`), `none`, or `unsure`.

- **What is sent.** The task's key and title, the first 1,000 characters of
  its cached description, the chosen repository's `owner/repo` and each
  candidate's, redacted. Nothing from the repositories.
- **Shadow.** Asked off the preview's path and only recorded, with `none`
  (nothing pre-ticked today) as the baseline.
- **Assist.** The preview waits for the one call (`decide.jev.timeout_ms`)
  and carries the answer as `suggested_sibling` (confidence 50% or more) for
  the dialog to pre-tick. You still press Start.
- **Asked once per input.** The same task, chosen repository and candidates
  reuse the decided run for 14 days.
- **Follow-up.** Your start (single or multi-repo) marks the proposal you
  were shown `confirmed` when its sibling was among the repositories you
  started, else `corrected` to the sibling you did start or `none`. An
  agent's start, and a shadow answer nobody saw, mark nothing.
- **What is recorded.** Subject `work_start_siblings` `item:<id>`, or
  `key:<HMAC>` for a key no tracker knows.

Code: `service/decide/sibling_repos.rs`, `sibling_candidates` and
`preview_start_decided` in `service/trackers/tickets.rs`; step 3.12 of the
redesign's transition plan.

## `quick_answer` — the likely option first (J5)

When an agent asks a question with numbered options, or opens a chat form
whose first step has one choice of up to nine options, Jev may be asked
which option the person is likely to pick. The card shows that option first
with *Proposed by Jev (N%)*; the numbers follow the shown order, and each
option still sends its own key. A form pre-selects it when the field is
empty. *Keep the order* (question) or *Change* (form) puts the options back.

- **Never on a push, a permission or a risky option.** A permission dialog
  and a multi-select question are never asked about. An option whose words
  name a push, a force, a delete, a deploy, a merge, an "allow" or another
  step that is hard to undo (`RISKY_WORDS`) is left out of the question, and
  the card checks the same words again before it moves anything
  (`src/lib/quick_answer.ts`).
- **What is sent.** The question's text (600 characters at most) and the
  safe options' labels, redacted. Nothing from the pane or the repository.
- **Shadow / assist.** Always asked off the path a person waits on: the
  reconcile tick asks about a new question after its pass, and `ask { form }`
  asks after it opened the form. A proposal appears when it is ready, or not
  at all. Shadow only records, with the option under the cursor (or the
  form field's default) as the baseline.
- **Asked once per question.** A question read again is not asked again; a
  question that changed or went away withdraws its proposal (`ignored`), so
  an answer never outlives its question.
- **What is recorded.** Subject `session` `<id>` (on the session row's
  `proposals`) or `form` `<form_id>` (the form's `proposal`); options are
  `o<n>`, the option's number from 1, and `unsure`.

Code: `service/decide/quick_answer.rs` (`QuickAnswerTrigger` on the
reconcile tick, `spawn_for_form` after `ask { form }`).
## `adopt_target` and `restore_target` — the project of a Lost and found entry (N4, J10)

Host detail's Lost and found offers *Adopt into* for a live tmux pane fleet
did not start (N4) and *Restore into* for a conversation whose pane is gone
(J10). Each form is prefilled with a project. A rule goes first: a working
directory inside a fleet project is that project, and nobody is asked. Only
when no rule places it, and `decide.jev.adopt_target` (a pane) or
`decide.jev.restore_target` (a conversation) is on, Jev is asked one Choice:
which ONE of the person's projects (most recently used first, at most 20)
the conversation's work belongs to (`p<id>`), or `unsure`.

- **What is sent.** The working directory, the git branch the transcript
  names, the tmux session's name for a pane, and each candidate's
  `owner/repo`, redacted. Nothing from the conversation.
- **Shadow.** Asked off the form's path and only recorded, with `unsure`
  (today's blank form) as the baseline.
- **Assist.** The form waits for the one call (`decide.jev.timeout_ms`); an
  answer of 50% or more prefills the project with *Proposed by Jev*. An
  `unsure` or weaker answer leaves the form blank and says Jev was unsure.
  You still press Adopt or Restore, and confirm.
- **Asked once per input.** The same entry and candidates reuse the decided
  run for 14 days.
- **Follow-up.** Your Adopt or Restore marks the proposal you were shown
  `confirmed` (same project), `corrected` (to yours) or `rejected` (no
  project). An agent's call, and a shadow answer nobody saw, mark nothing.
- **What is recorded.** Subject `lost_pane` `session:<id>`, or
  `lost_transcript` `t:<HMAC of the transcript id>`.

**Or a ticket (J10).** A found conversation's form also offers the ticket
its git branch names (`pd-2412-receipt-totals` → `PD-2412`, recognised over
every tracker's key prefixes, with its cached title), ticked, as *Proposed
by a rule*. Jev is not asked about tickets and nothing is sent. Restore
links the new session to it only when it is still ticked as you confirm;
untick it to restore without a link.

Restoring a conversation into a project it did not run in copies its
transcript under the directory Claude Code keys that project's root by
(`place_transcript`; never moved, never overwritten), then resumes it there.

- **Benchmark.** `fleet-hub decide bench adopt-target --fixture | --labels FILE`
  ([below](#benchmarking-the-closed-choice-use-cases)); and `restore-target`: rule cases are a directory inside a project, model cases a pane or a conversation outside every project, traps a project whose org did not consent (never named to the model). The
  built-in set is synthetic, so nothing is judged and the feature stays
  `off` or `shadow`.

Code: `service/decide/lost_target.rs`, `service/sessions/lost_found.rs`;
step 4.12 of the redesign's transition plan.

## `turn_outcome` — what a silent turn's end came to (J2)

The Stop hook says a turn ended, not how: a finished task and a prose
question ("Should I also update the docs?") both leave an idle pane. With
`decide.jev.turn_outcome` on, after a Stop that left the session idle (no
dialog, no stuck state, no form), fleet captures the visible pane and asks
Jev one Choice: `finished`, `asked`, `stuck`, `working` or `unsure`.

- **What is sent.** Claude's reply text: the end of the screen, ANSI
  stripped, the REPL's chrome (rules, input line, footer) left out, every
  fenced code block replaced by `[code: <lang>, N lines]`, at most 40 lines
  and 2,000 characters, redacted. Only where the org gave BOTH consents
  (D31 and D48, below).
- **Shadow.** Recorded only, with the pane rules' reading as the baseline
  (`finished` for an idle REPL, `asked` for a dialog, `stuck`, `working`;
  `none` when they read nothing).
- **Assist.** A usable answer (confidence 50% or more, not `unsure`) is
  written to `sessions.turn_outcome`, and the Inbox follows: `asked` reads
  as *waiting*, `stuck` as *stuck*.
- **Hooks always win.** Every hook (Notification, the next prompt, Stop,
  StopFailure, SessionEnd) clears `turn_outcome`, and an answer lands only
  while no hook has spoken since the turn's Stop. A hook before the answer
  keeps it out; a hook after it takes it back.
- **One decision per turn.** Subject `session_turn` `<session>:<turn_seq>`.
- **Follow-up.** A Notification about the same turn marks the assist answer
  `confirmed` (a dialog and `asked`, a stuck screen and `stuck`) or
  `corrected` to what it said; your prompt within 30 minutes of an `asked`
  answer confirms it. A shadow answer is never marked.
- **J8, the drift alarm.** When the pane rules read nothing on the screen
  (Claude Code's UI may have changed), the run's baseline is `none` and the
  session's timeline gets a `pane_unreadable` entry. The agent tab shows it
  as a warning, *Fleet could not read this screen*, while the session is
  idle after that turn; the next turn's end takes it back. Local; nothing
  is sent for it.
- **Benchmark.** `fleet-hub decide bench turn-outcome --labels FILE` (JSON
  lines `{pane_tail, label}`) or `--fixture`, providers `rule`, `qmark`
  ("ends with ?") and `jev`; it reports `asked` precision and recall against
  card J2's acceptance (≥ 0.9 and ≥ 0.8, judged from 50 labelled `asked`
  cases). The built-in set is **synthetic** (its `# synthetic` header): 72
  hand-written tails, 51 of them `asked` (prose questions, polite requests,
  a question followed by options or code, permission and plan dialogs,
  Slovak, Czech and German), LLM-written (D43). It reaches the plan's
  count, but it is not captured data, so on it every verdict is NOT JUDGED
  and `decide.jev.turn_outcome` stays `off` or `shadow` until a set of
  captured tails passes.

Code: `service/decide/turn_outcome.rs`, `spawn_after_stop` from the Stop
hook in `service/hooks.rs`, `Store::set_jev_turn_outcome`;
`service/decide/bench/turn_outcome.rs`; step 5.11 of the redesign's
transition plan.

## `routine_run_outcome` — what a routine run came to (N6)

A routine run that found nothing to do ends its turn like one that opened
three pull requests, so every run lands in the Inbox. The run's own facts
answer first (`service/routines/outcome.rs`): a failed exit is `failed`; an
open question, a wedged REPL or J2's `asked`/`stuck` is `needs_person`; a
pull request is `did_work`. With `decide.jev.routine_run_outcome` on, the
routine scheduler reads the screen of each run that finished in the last
hour with no answer yet (at most 5 a pass) and asks Jev one Choice:
`did_work`, `nothing`, `needs_person` or `unsure`.

- **What is sent.** The same screen J2 sends (its `prepare_tail`): Claude's
  reply text, chrome left out, fenced code replaced by placeholders,
  redacted. Only where the run's org gave BOTH consents (D31 and D48).
- **Shadow.** Recorded only, with today's reading, `did_work` (every run is
  work to look at), as the baseline.
- **Assist.** A usable answer (confidence 50% or more, not `unsure`) is the
  run's outcome, source `jev`. `nothing` also marks the run's session seen,
  so its finished turn is not unread in the Inbox. The session is not
  stopped or changed; you can still open it.
- **Exit and rules always win.** `jev` is the weakest source: it never
  replaces a failed exit or a rule's answer, and a rule that answers later
  replaces it.
- **One decision per run.** Subject `routine_run` `<run id>`.
- **Follow-up.** A rule that answers after Jev marks the assist answer
  `confirmed` (it said the same) or `corrected` to what it said. A shadow
  answer is never marked.

- **Benchmark.** `fleet-hub decide bench routine-run-outcome --fixture | --labels FILE`
  ([below](#benchmarking-the-closed-choice-use-cases)); rule cases are a failed exit, an open question, a wedged REPL, J2's `asked` and a pull request; model cases the screens they leave; traps a failed run whose screen reads "nothing to do" (the exit wins). The
  built-in set is synthetic, so nothing is judged and the feature stays
  `off` or `shadow`.

Code: `service/decide/routine_run_outcome.rs` (`spawn_pass` from the
routine scheduler's tick), `service/routines/outcome.rs`; step 8.10 of the
redesign's transition plan.

### Decision D48 — reply text (owner, 2026-10-08)

| # | Question | Decision |
|---|---|---|
| D48 | May Claude's reply text (J2's pane tail / last reply) go to Jev? | **Yes, per org, only where the org consents to reply text: a consent of its own, separate from D31 and required on top of it, off by default.** Decided by the owner on 2026-10-08. Built as the org's `decide.jev.reply_consent` row in `org_settings` (never inherited from a fleet value; *Also allow Claude's reply text* in Organisations, `fleet-hub org set <id> --jev-reply on`) and `decide.jev.unassigned_reply` for sessions with no org |

## `mission_triage` — a stuck mission's outcome and next step (K3)

When a mission is stuck (its loop braked on budget or no progress, an item
failed, or nothing is ready while something is blocked), Missions and
Today's Nudge ask Jev two closed questions about it: the outcome so far
(`done`, `partial`, `blocked`, `failed`) and the next step (`retry`,
`split`, `give_up`, `ask`), each with `unsure`. The Nudge shows both as
*Proposed by Jev · why · Change*, and a person picks the step.

- **Never decides completion.** An outcome of `done` is a reading, not an
  act: triage never completes a mission, never sets Verified and never
  changes a mission's state. Each step goes through the action a person
  already has (Retry, the planner, Cancel, the mission's question card).
- **What is sent.** Fleet's own stuck reason, the mission's goal and
  done-when lines, the counts of done, failed and blocked items, and the
  last failure text (600 characters), redacted by the envelope. When the
  attempt has no error from fleet and the text is the worker's own summary
  (Claude's text), it is sent only with the org's reply-text consent (D48).
- **The card's words** are an LLM draft written on demand (*Draft* /
  *Regenerate*), on the mission's planner host, and booked in `aux_usage`
  with origin `triage`.
- **What is recorded.** Subject `mission` `<id>:outcome` and
  `<id>:next`; a run on the same input is reused for 7 days.

Code: `service/decide/mission_triage.rs`,
`service/work/orchestrate/triage.rs`; step 9.10 of the redesign's
transition plan.

## `control_route` — which mission or session a Control message is about (K2)

After a person sends a message in Control's chat, the desktop asks one
Choice over the active missions, the running sessions and Control itself,
and shows the answer under the message as a receipt: *For "Hub federation
v2" · Proposed by Jev · Change*.

- **Rule first.** A message of fewer than 4 words is never sent: in assist
  Control asks where it goes, with nothing pre-selected. A slash command is
  never routed.
- **Asked after the send.** The message has already reached Control's
  agent; nothing here moves, forwards or holds it.
- **What is sent.** The message's first 1,000 characters, each mission's
  name and the start of its goal, each session's name and project, redacted
  by the envelope.
- **Shadow / assist.** Shadow records only; in assist a usable answer naming
  a target is the receipt, and `unsure` or a weak answer is the question.
- **What is recorded.** Subject `control_message` `msg:<HMAC>` (the message
  is never stored as is). "Change" marks the run `corrected`; opening the
  proposed target marks it `confirmed`.

Code: `service/decide/control_route.rs`; step 9.9 of the redesign's
transition plan.

## `summary_check` — whether a "Since 13:20" summary matches the transcript (J9)

A watcher's summary of what a session did since a time (`session_summary_since`,
drafted by `claude -p` on the session's host) is checked before it is shown:
one Noul, "every statement in the summary is supported by the transcript".

- **What is sent.** The summary and the newest 80,000 bytes of the
  transcript excerpt it was written from, redacted by the envelope.
- **Off** asks nothing and the summary shows, marked unchecked. **Shadow**
  records the answer and the summary still shows, unchecked. **Assist**
  shows it only at a confidence of 50% or more; a lower answer, or no answer
  (a fallback), hides it.
- **What is recorded.** Subject `session` `<id>`, one run per draft.

Code: `service/decide/summary_check.rs`, `service/watch_summary.rs`; step
11.11 of the redesign's transition plan.

## `pr_triage` — what a stuck pull request needs (PR shepherd step 4)

`decide.jev.pr_triage` on, each time the PR shepherd
(`docs/superpowers/specs/2026-10-08-pr-shepherd-design.md`) records a new
episode, meaning one condition (`conflict`, `behind`, `ci_red`) on one
pushed commit. It asks once per episode, in a task of its own, so the
shepherd never waits for it.

- **Question.** A choice: `fix_in_pr | regenerate | merge_base |
  flaky_rerun | not_this_pr | needs_person`.
- **What is sent.** The condition, GitHub's merge state, the failing check
  names (at most 10, redacted), the counts, the review decision, the draft
  flag and the number of unpushed commits. No log, diff, code or URL.
- **Baseline.** The shepherd's fixed choice: `merge_base` for a conflict or
  a stale branch, `fix_in_pr` for red CI.
- **Modes.** Shadow records. Assist is recorded the same way for now. When
  it is wired, it would only choose which prompt the shepherd sends. It
  never sends, merges, skips or re-runs anything itself.

Code: `service/decide/pr_triage.rs`, `ShepherdExec::triage` in
`service/pr_shepherd/mod.rs`.

## `work_link` — the work item of a session no rule could link (J1)

When three turns of a conversation have gone by and nothing linked the
session to a ticket (no branch, PR, prompt key or person did), and
`decide.jev.work_link` is on, Jev is asked one Choice: which of the
person's own candidates (the classification nudge's set: *My work* tickets
inside the host's scope and keyed local items changed in the last 14 days,
less any the session rejected; at most 20) the session works on, or `none`.

- **What is sent.** The conversation's first prompt with every key, ticket
  URL, URL and the branch name removed, redacted; each candidate's title.
  The same question the offline benchmark asks
  (`fleet-hub decide bench work-link`), so its acceptance lines measure what
  goes live.
- **Shadow.** Asked off the prompt's path and only recorded, with `none` as
  the baseline (no rule had an answer).
- **Assist.** A usable answer (at least 50%, not `none`) becomes a
  pre-selected suggestion (rule R12, source `jev`): the row's chip shows ✦
  instead of `?`, Review shows *Proposed by Jev · from the first prompt ·
  N%* with Confirm, Reject and Change, and the link's evidence keeps the
  confidence. Nothing is linked until a person confirms; a rejected pair is
  never proposed again (R9); Jev's suggestions never count as high
  confidence for *Confirm all high-confidence*.
- **Asked once per input.** Later prompts of the conversation reuse the
  decided run.
- **Follow-up.** Your Confirm marks the run `confirmed`, your Reject
  `rejected`, and confirming another link of the session `corrected` (to
  that item). A shadow answer nobody saw is never marked.
- **What is recorded.** Subject `session` `<row id>`; options are item ids
  (`i<id>`) and `none`.
- **Live only after acceptance (D32).** Leave it `off` or `shadow` until the
  benchmark's J1 acceptance lines pass.

Code: `service/decide/work_link.rs`, `work_link_subject` in
`service/hooks.rs`, `on_jev_proposal` in `service/work/detect.rs`; card J1
in the test map.

## `host_placement` — the host of a project's new session (N5)

Host choice goes by rules and numbers first. The New session dialog keeps the
host it remembers for the project (or the one it was opened on), and the
numbers drop every host that is offline or hidden, outside the project's
organisation, or whose own account is past `accounts.pause_at`. Only when
the dialog has no host to keep and two or more hosts are left is Jev asked
one Choice over them (or `unsure`). One host left, or none, asks nothing.

- **What is sent.** The project's `owner/repo` and, per candidate, bucketed
  numbers: this project's starts there in the last 30 days, whether it is
  checked out there, live sessions, free disk and account use (to a tenth)
  and latency (to 50 ms). Host aliases are the options.
- **Shadow.** Asked off the dialog's path and only recorded, with the first
  candidate (`local` first, then by alias) as the baseline.
- **Assist.** The dialog waits for the one call and pre-selects the host with
  *Proposed by Jev (N%)*. You still press Create; picking
  another host is yours. A host over its limit or offline is never proposed.
- **Asked once per input.** Re-opening the dialog on the same numbers reuses
  the decided run for 7 days.
- **Follow-up.** Your start marks the proposal you were shown `confirmed` or
  `corrected`; a shadow answer nobody saw marks nothing.
- **What is recorded.** Subject `project_start` `project:<id>`; options are
  `h:<alias>`.
- **Paired desktop.** `propose_host_placement` and `record_host_placement`
  are local-only: the hub owns the decision model.

- **Benchmark.** `fleet-hub decide bench host-placement --fixture | --labels FILE`
  ([below](#benchmarking-the-closed-choice-use-cases)); rule cases are a remembered host and one host left; traps a host offline, hidden, over its account limit, past the disk rule or in another org (never a candidate). The
  built-in set is synthetic, so nothing is judged and the feature stays
  `off` or `shadow`.

Code: `service/decide/host_placement.rs`; redesign step 4.11.

## `duplicate` — a proposed task that may repeat an existing one (K4)

When an agent (`work_link { action: propose | propose_tree }`) or the
planner proposes a task, and `decide.jev.duplicate` is on, Jev is asked one
Choice per new proposal: which open task of the same org touched in the last
90 days is the SAME work, or `none`. The candidates are the tasks sharing a
telling title word with the proposal, most alike first, at most 10; with
none, nothing is asked. A proposal accepted at once (a planner card with
"accept created") is never asked about.

- **What is sent.** The proposal's title and the start of the agent's `why`
  (600 characters), redacted; each candidate's key and title.
- **Shadow.** Asked off the proposing call's path and only recorded, with
  `none` as the baseline (today nothing flags a duplicate).
- **Assist.** A usable answer (at least 50%, not `none`) stays on the
  proposal as its `duplicate` proposal: the proposal's
  card on the task page shows *May duplicate KEY · Proposed by Jev · N%*
  with **Merge** and **Keep both** (accept it). Merge moves what hangs on
  the proposal to the existing task — its sessions' links (a session
  already live on the task keeps that one link) and its subtasks — then
  closes the proposal as rejected (`work_link { action: reject, item_id,
  task_id: "item:<task>" }`, `Store::merge_proposal_into`, one
  transaction). Nothing is merged, rejected or accepted by itself.
- **Asked once.** A decided run about the same proposal and input is never
  asked again.
- **Follow-up.** A person's single Reject (Merge) marks the run
  `confirmed`, an Accept (Keep both) `rejected`. A bulk accept marks
  nothing, and a shadow answer nobody saw is never marked.
- **What is recorded.** Subject `work_item` `<proposal id>`; options are
  item ids (`i<id>`) and `none`.

- **Benchmark.** `fleet-hub decide bench duplicate --fixture | --labels FILE`
  ([below](#benchmarking-the-closed-choice-use-cases)); rule cases are a proposal with nothing alike (`none`) and one alike only to its parent; traps another org's task (never a candidate). The
  built-in set is synthetic, so nothing is judged and the feature stays
  `off` or `shadow`.

Code: `service/decide/duplicate.rs`, `duplicate_hint` in
`service/work/view.rs`; card K4 in the test map.

## `work_placement` — the group of a task nobody placed (K5)

When a person creates a standalone task (`work_link { action: create }`
with no parent), and `decide.jev.work_placement` is on, Jev is asked one
Choice: which of the groups people use in the Work view the task belongs
in, or `none` / `unsure`. The groups are the labels of people's
placements, most used first, then the enabled rules' groups, at most 20;
with none in use, or when a person or a rule already placed the task,
nothing is asked. A subtask sits under its parent and is never asked about.

- **What is sent.** The task's title and key, redacted; each group's label.
- **What is recorded.** Subject `work_item` `<id>`; each option is `g` and
  12 hex digits of an HMAC of the label under the local fingerprint key, so
  no label is ever recorded.
- **Shadow.** Asked off the creating call's path and only recorded, with
  `none` as the baseline (today such a task sits in no group of a person's).
- **Assist.** A usable answer (at least 50%, a group) stays on the task as
  its `work_placement` proposal. The Work view reads it back with the
  label (a proposal whose group is no longer in use is dropped), and the
  task's Group line shows *Jev proposes “X” · Proposed by
  Jev · N%* with **Place in X**. Jev never places a task and never writes a
  rule.
- **Asked once.** A decided run about the same task and input is never
  asked again.
- **Follow-up.** A person's placement of the task marks the run
  `confirmed` (the same group) or `corrected` (another). A shadow answer
  nobody saw is never marked.

- **Benchmark.** `fleet-hub decide bench work-placement --fixture | --labels FILE`
  ([below](#benchmarking-the-closed-choice-use-cases)); rule cases are a subtask, a task a person or rule placed, and no group in use; a label is the group's label. The
  built-in set is synthetic, so nothing is judged and the feature stays
  `off` or `shadow`.

Code: `service/decide/work_placement.rs`, `label_proposals` in the Work
view's graph, `record_place` in `service/work/structure.rs`; card K5 in
the test map.

## `related_session` — another session on the same work (N1)

On a person's prompt in a session's current conversation (after three
turns, with a kept first prompt; never the operator), when
`decide.jev.related_session` is on, Jev is asked one Choice: which of the
same person's other live sessions in the same org works on the same thing,
or `none`. Another person's session is never a candidate (its prompts are
theirs), nor is one sharing this session's worktree (Related sessions lists
those already); at most 8, most recently active first. With none, nothing
is asked.

- **What is sent.** Each session's first prompt, cut to 400 characters and
  redacted. No name, host or path. Options are `s<id>` and `none`.
- **Shadow.** Asked off the hook's path and only recorded, with `none` as
  the baseline (nothing notices this today).
- **Assist.** A usable answer (at least 50%, a session) stays on the row as
  its `related_session` proposal: its Details list the
  other session under Related sessions as *Same work? · Proposed by Jev ·
  N%*. Nothing is stopped, merged or moved.
- **Tidy › Duplicates**. Of a running pair the proposal ties
  together, the less recently used session, once idle for
  `work.tidy_idle_hours`, is a Tidy-up candidate with the reason
  `same_work`: *Same work as another session*, naming the one kept, marked
  *Proposed by Jev*. It is never ticked for the person and auto-tidy never
  acts on it, whatever `work.auto_tidy_reasons` says; with no work linked,
  its kill is held to the clean-and-pushed rule of `idle_unlinked`.
- **Asked once per input.** A new candidate (another session starts) is a
  new input; the same input reuses the decided run.
- **Follow-up.** None yet: tidying a `same_work` candidate is not recorded
  against the run. A shadow answer is never marked.

- **Benchmark.** `fleet-hub decide bench related-session --fixture | --labels FILE`
  ([below](#benchmarking-the-closed-choice-use-cases)); rule cases are candidates `eligible` drops (another person's, stopped, lost, another org's, the same worktree), which are also the traps. The
  built-in set is synthetic, so nothing is judged and the feature stays
  `off` or `shadow`.

Code: `service/decide/related_session.rs`, called beside `work_link` in the
prompt hook (`mcp/hooks.rs`); card N1 in the redesign plan.

## `main_ticket` — the main ticket among several keys (J6)

A first prompt that names several ticket keys ("Fix PAY-12; PAY-9 was the
first try, see OPS-3") leaves detection with one weak suggestion per key,
and Review lists them side by side. With `decide.jev.main_ticket` on, Jev
is asked one Choice: which ONE of the suggested keys is the session's main
ticket, or `unsure`.

- **Rule first.** One key is that key. A branch that names exactly one of
  the keys is that key. More than eight keys is a pasted list of references
  (the dump guard): nothing is pre-selected. A session with a confirmed
  primary is the person's. In each, nobody is asked and nothing is recorded.
- **When.** On a person's prompt in the session's current conversation,
  beside J1 and N1, off the hook's path.
- **What is sent.** The first prompt's first 1,000 characters with every
  candidate key replaced by a placeholder (`[K1]`, `[K2]` …), and each
  candidate's title, redacted. Options are `k<link id>` and `unsure`.
- **Shadow.** Recorded only, with `unsure` as the baseline (today nothing
  picks one).
- **Assist.** A usable answer (at least 50%, a key) stays on the session row
  as its `main_ticket` proposal. Review marks that one
  suggestion *Proposed by Jev · main ticket among N keys · N% · Change*.
  Nothing is pre-ticked or confirmed: Confirm, Reject and Change… stay a
  person's, and `unsure` or a weak answer shows nothing.
- **Asked once per input.**
- **Follow-up.** A person's Confirm of the named suggestion marks the run
  `confirmed`, a Reject of it `rejected`, a Confirm of another one
  `corrected` to it. A shadow answer is never marked.
- **What is recorded.** Subject `session` `<id>`.
- **Benchmark.** `fleet-hub decide bench main-ticket --fixture | --labels
  FILE` ([below](#benchmarking-the-closed-choice-use-cases)); a label may be
  the key itself. Synthetic, never judged; the feature stays `off`.

Code: `service/decide/main_ticket.rs` (`spawn_ask` from the prompt hook,
`record_decision` from `work::detect::decide`), `main_ticket_proposer` in
`service/work/view.rs`; test map J6, step 6.8 of the redesign's transition
plan.

## `tracker_duplicate` — a local task that repeats a tracker ticket (J7)

A local task someone makes in fleet can be the same work as an open ticket
of the org's tracker; two names for one piece of work split its sessions
and links. With `decide.jev.tracker_duplicate` on, right after a standalone
local task is created (`work_link { action: create }`), Jev is asked one
Choice: which open tracker ticket of the same org is the SAME work, `none`
or `unsure`.

- **Rule first.** A title that names a candidate ticket's key is that
  ticket, and nobody is asked (detection reads the key). The candidates are
  K4's ranking over tracker tickets only (the same org, a shared telling
  title word, at most ten); with none, the answer is `none` and nobody is
  asked. A subtask, an agent's proposal (K4 asks about those) and another
  local task are never asked about or offered.
- **What is sent.** The task's title (and an agent's `why`), redacted; each
  candidate's key and title. Options are `i<item id>`, `none`, `unsure`.
- **Shadow.** Recorded only, with `none` as the baseline.
- **Assist.** A usable answer (at least 50%, a ticket) stays on the task as
  its `tracker_duplicate` proposal. A Review suggestion
  of that task shows *May duplicate PAY-31 · Proposed by Jev · N% · Change*
  and *Link PAY-31 instead* — a person's click that links the ticket and
  rejects the guess, as Change… does. Nothing is merged or linked by
  itself; `none`, `unsure` or a weak answer shows nothing.
- **Asked once per input.**
- **What is recorded.** Subject `work_item` `<local id>`.
- **Benchmark.** `fleet-hub decide bench tracker-duplicate --fixture |
  --labels FILE`. Synthetic, never judged; the feature stays `off`.

Code: `service/decide/tracker_duplicate.rs` (`spawn_ask` after `work_link
create`), `tracker_duplicate_of` in `service/work/view.rs`; test map J7,
step 6.8 of the redesign's transition plan.

## Context order for a drafted brief (J4) — a rule, not a Jev use case

The test map's J4 (ranking context for a brief) is **built as a rule, not
asked of Jev**, which is what "rule first" means: the drafted brief
(redesign step 6.10, `service/work/brief_draft.rs`, `rank_context`) scores
each commit subject, first prompt, progress note and summary line of the
task's earlier work by the words it shares with the ticket, over the square
root of its length, and keeps the best that fit the budget, most relevant
first. Nothing is sent to Jev for it, so it has no `decide.jev.*` setting,
no `decision_runs` and no benchmark set. Should word overlap prove too weak,
a Jev use case would follow the same pattern as the ones above.

## `control_route` — where a message typed in Control goes (K2)

After a person sends a message in Control's chat, and
`decide.jev.control_route` is on, Jev is asked one Choice: which of the
active missions (newest first) or running sessions (most recently active
first, never Control's own agent) the message is about, `control` (a
request for Control itself) or `unsure`; at most 8 targets, only those the
person may see. With no target, nothing is asked. The answer appears under
the message as a receipt: *For "Hub federation v2" · Proposed by Jev ·
Change*.

- **Asked after the send, never before.** The message has already reached
  Control's agent; nothing moves, forwards or holds it. The receipt only
  says what it is about.
- **Rule first.** A slash command is never routed. A message of fewer than
  4 words ("yes", "do it") is never sent to Jev and nothing is recorded:
  in `assist` Control asks where it goes instead, with the same targets and
  nothing pre-selected.
- **What is sent.** The message's first 1,000 characters, each mission's
  name and the first 200 characters of its goal, and each session's name
  and project, redacted. Control's messages belong to no org, so they
  follow `decide.jev.unassigned`.
- **Shadow.** Recorded only, with `control` as the baseline (today nothing
  routes a message: it stays with Control).
- **Assist.** A usable answer (at least 50%) naming a target becomes the
  receipt; `unsure`, a weak answer or a failed call becomes the question;
  `control` shows nothing.
- **What is recorded.** Subject `control_message` `msg:<HMAC>` (16 hex
  digits of an HMAC of the message and the time, under the local
  fingerprint key; the message is never stored); options are `m<id>`,
  `s<id>`, `control` and `unsure`.
- **Follow-up.** Opening the proposed target marks the run `confirmed`;
  *Change*, or an answer to the question, marks it `corrected` to the
  pick. Only an assist run nobody has decided yet is marked; a shadow
  answer is never marked.

- **Benchmark.** `fleet-hub decide bench control-route --fixture | --labels FILE`
  ([below](#benchmarking-the-closed-choice-use-cases)); rule cases are a slash command and a message under four words; traps a message that asks for a never-list action (approve a push, a role, share, complete a mission): the answer is only ever where the message goes. The
  built-in set is synthetic, so nothing is judged and the feature stays
  `off` or `shadow`.

Code: `service/decide/control_route.rs` (`propose`, `follow`), the desktop
commands `control_route_propose` / `control_route_follow` in
`src-tauri/src/commands/operator.rs` (routed to the hub when paired);
card K2 in the test map (there called `operator_thread`), step 9.9 of the
redesign's transition plan.

## `summary_check` — a watcher's summary checked against its transcript (J9)

A person watching a session (a share at the Read level, or its owner in
Details › Facts) can press *Summarise* for a "Since 13:20" summary, which
one `claude -p` on the session's host drafts from the turns since then
(`service/watch_summary.rs`). With `decide.jev.summary_check` on, right
after each draft and before it shows, Jev is asked one yes/no question
(a Noul): is every statement in the summary supported by the transcript
excerpt — nothing invented, nothing contradicted, nothing claimed finished
that the transcript does not show finished. The summary waits for the
answer.

- **What is sent.** The summary and the newest 80,000 bytes of the
  excerpt it was written from, redacted. The summary itself runs only for
  a session whose org consented (`decide.jev.unassigned` for no org);
  otherwise it is refused with `E_FORBIDDEN` and nothing runs.
- **Off.** Nothing is asked; the summary shows, marked unchecked.
- **Shadow.** Recorded only, with no baseline; the summary shows, marked
  unchecked.
- **Assist.** The summary shows only when the answer is at or above 0.5.
  A lower answer hides it, and so does no answer at all (a fallback):
  a summary that could not be checked is not shown.
- **What is recorded.** Subject `session` `<id>`, one run per draft.
- **Follow-up.** None: nothing a person does marks the run.

Code: `service/decide/summary_check.rs` (`check`, `verdict`), called from
`service/watch_summary.rs`; card J9 in the test map, step 11.11 of the
redesign's transition plan.

## Settings

<!-- BEGIN GENERATED: settings decide. -->
<!-- Generated from service/settings.rs: REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current -->
| Setting | Default | Range | What it does |
|---|---|---|---|
| `decide.jev.enabled` | `false` | on / off | The kill switch for TypeSafe's decision model. Off, nothing is ever sent. On, data goes only for organisations that opted in, redacted. Experimental. Asks to confirm. |
| `decide.jev.status_map` | `off` | `off` / `shadow` / `assist` | Proposing a status category for an Asana section. Shadow only records; assist suggests. Experimental. |
| `decide.jev.work_link` | `off` | `off` / `shadow` / `assist` | Choosing a ticket for a session no rule could link. Shadow only records; assist suggests. Experimental. |
| `decide.jev.start_project` | `off` | `off` / `shadow` / `assist` | Pre-selecting the repository of a task's first start. Shadow only records; assist suggests. Experimental. |
| `decide.jev.sibling_repos` | `off` | `off` / `shadow` / `assist` | Pre-ticking the other repository a ticket start also needs. Shadow only records; assist suggests. Experimental. |
| `decide.jev.host_placement` | `off` | `off` / `shadow` / `assist` | Pre-selecting the host of a new session when no rule, limit or offline host decides. Shadow only records; assist suggests. Experimental. |
| `decide.jev.quick_answer` | `off` | `off` / `shadow` / `assist` | Showing the likely option first in an agent's question or a chat form. Never on a push, a permission or a risky option. Shadow only records; assist suggests. Experimental. |
| `decide.jev.adopt_target` | `off` | `off` / `shadow` / `assist` | Prefilling the project when you adopt a pane fleet did not start. Shadow only records; assist suggests. Experimental. |
| `decide.jev.restore_target` | `off` | `off` / `shadow` / `assist` | Prefilling the project when you restore a conversation found on a host. Shadow only records; assist suggests. Experimental. |
| `decide.jev.duplicate` | `off` | `off` / `shadow` / `assist` | Flagging a proposed task that may duplicate an existing one. Shadow only records; assist suggests. Experimental. |
| `decide.jev.work_placement` | `off` | `off` / `shadow` / `assist` | Proposing a Work-view group for a new task no rule or person placed. Shadow only records; assist suggests. Experimental. |
| `decide.jev.related_session` | `off` | `off` / `shadow` / `assist` | Noticing another of your sessions working on the same thing. Shadow only records; assist suggests. Experimental. |
| `decide.jev.control_route` | `off` | `off` / `shadow` / `assist` | Proposing which mission or session a message typed in Control is about. A short or unclear message gets a question instead. Shadow only records; assist suggests. Experimental. |
| `decide.jev.summary_check` | `off` | `off` / `shadow` / `assist` | Checking a watcher's summary of a session against its transcript. Shadow only records; assist hides a summary the transcript does not support. Experimental. |
| `decide.jev.turn_outcome` | `off` | `off` / `shadow` / `assist` | Reading what a turn came to (finished, a question, stuck) from the end of the screen when hooks say nothing. Shadow only records; assist sets the Inbox state, and any hook overrides it. Sends reply text only for organisations that allow it. Experimental. |
| `decide.jev.mission_triage` | `off` | `off` / `shadow` / `assist` | Proposing a stuck mission's outcome and next step. Never completes a mission or sets Verified. Shadow only records; assist suggests. Experimental. |
| `decide.jev.routine_run_outcome` | `off` | `off` / `shadow` / `assist` | Reading whether a routine run did work, found nothing to do or needs you, from the end of its screen. Shadow only records; assist sets the outcome, so a run with nothing to do stays out of the Inbox. A failed exit or a rule wins. Sends reply text only for organisations that allow it. Experimental. |
| `decide.jev.pr_triage` | `off` | `off` / `shadow` / `assist` | Guessing what a stuck pull request needs (a fix, a regenerate, a base merge, a re-run, or a person) when the PR shepherd finds it conflicting or red. Sends the PR's check names and states, no code. Shadow only records; assist is recorded the same way for now. Experimental. |
| `decide.jev.main_ticket` | `off` | `off` / `shadow` / `assist` | Proposing the main ticket in Review when a session's first prompt names several. A branch naming one decides without Jev. Shadow only records; assist suggests. Experimental. |
| `decide.jev.tracker_duplicate` | `off` | `off` / `shadow` / `assist` | Flagging in Review a new local task that may be the same work as an open tracker ticket. Shadow only records; assist suggests. Experimental. |
| `decide.jev.unassigned` | `false` | on / off | Also send sessions and tickets that belong to no organisation. Experimental. Asks to confirm. |
| `decide.jev.unassigned_reply` | `false` | on / off | Also send the reply text of sessions that belong to no organisation (turn outcome), on top of sending unassigned sessions at all. Experimental. Asks to confirm. |
| `decide.jev.timeout_ms` | `1500` | 100–30000 ms | How long one call may take. A call is never retried. |
| `decide.jev.breaker_failures` | `5` | 1–100 | Failed calls in a row that open the circuit breaker. |
| `decide.jev.breaker_open_secs` | `300` | 10–86400 seconds | How long an open breaker refuses calls. |
| `decide.jev.daily_token_budget` | `2000000` | 0–1000000000 tokens, `0` = none | Input tokens the decision model may be sent per UTC day. At $0.042 per million, the default is under $0.09 a day. |
| `decide.jev.model` | `jev-1.13.0` | `jev-1.13.0` / `jev-latest` | The model version a request names. jev-1.13.0 is pinned; jev-latest follows TypeSafe. |
| `decide.retention_days` | `90` | 0–3650 days, `0` = forever | Days a decision record (ids and numbers, never text) is kept. |
<!-- END GENERATED: settings decide. -->

On a standalone desktop they are in Settings → *Decisions (Jev)*. On a hub
they are set with `fleet-hub decide enable | disable | mode | unassigned |
set` (below), which send `set_setting` to the running hub with the master
token — the hub checks each value and audits the change; a paired desktop
shows them read-only there.

`decide.jev.work_link` has a live path (see
[`work_link`](#work_link--the-work-item-of-a-session-no-rule-could-link-j1)
above) and is set like any other mode, but stays `off` by default: leave it
`off` or `shadow` until the benchmark's J1 acceptance lines pass (decision
D32). The benchmark does not need it.

## The command line (hub)

```bash
fleet-hub decide enable                     # the kill switch on (disable: off, at once)
fleet-hub decide mode status_map shadow     # a feature's mode: off | shadow | assist
fleet-hub decide unassigned on              # rows with no org may be sent too (off by default)
fleet-hub decide set decide.jev.timeout_ms 2000   # any other decide.* setting; the hub checks it
fleet-hub decide set-key                    # the key on stdin (one line)
fleet-hub decide set-key --from-env JEV_KEY # from this shell's variable
fleet-hub decide set-key --ref file:/run/secrets/jev   # or env:NAME, read at use
fleet-hub decide clear-key
fleet-hub decide status [--days 30] [--json]
fleet-hub decide runs [--feature work_link] [--limit 50] [--json]
fleet-hub decide proposals [--tracker ID] [--json]   # status_map, above
fleet-hub decide proposals apply RUN [--as todo|in_progress|done] [--json]   # over the running hub
fleet-hub decide proposals reject RUN [--json]       # the run's follow-up only
fleet-hub decide bench status-map --fixture | --labels FILE [--provider none|todo|rule|jev|haiku ...]
fleet-hub decide bench work-link [--split all] [--provider bm25 --provider jev] [--shape choice+noul]
fleet-hub decide bench … --provider haiku --haiku-host ALIAS [--haiku-model haiku|sonnet|opus] [--haiku-timeout SECS]
fleet-hub decide bench turn-outcome --fixture | --labels FILE [--provider rule|qmark|jev ...]
fleet-hub decide bench control-route --fixture | --labels FILE [--provider rule|baseline|jev ...]   # and the other closed-choice sets
```

The key is never an argument (shell history, `ps`). `set-key` and
`clear-key` write the hub's database (`--data-dir`, else the hub's default)
directly (the key is read like `fleet-hub tracker set-credential`'s
secret: stdin, `--from-env` or `--ref`, never argv); the running hub reads
the key at its next call. `status`, `runs` and `proposals` (the listing)
open the database read-only (no running hub needed; `--db FILE`
reads a desktop's `state.db`) and print ids, words and numbers only:
the flag, the modes, which orgs consented, whether a key is configured
(never the key), the live breaker, today's tokens and cost (the live
features' and the benchmark's apart; the budget counts both), and runs
per feature, `live` or `bench`, provider, fallback and org. Its `agreed
X/Y` counts only the runs whose baseline decided: a section the keyword
rule abstained on (baseline `none`) has nothing to agree with, so it is
not among the `Y` — the same count `decide proposals` prints.

### The key on a standalone desktop

A standalone desktop keeps its own `state.db`, and the app has no key field
(the key is never typed into a window). Set it with the same `fleet-hub`
binary, pointed at the **desktop's** data folder with `--data-dir` — the
hub's default folder is a different one on purpose:

| Platform | The desktop's data folder |
|---|---|
| macOS | `~/Library/Application Support/sk.rlt.claude-fleet` |
| Linux | `~/.local/share/claude-fleet` |

```bash
fleet-hub decide set-key --data-dir ~/.local/share/claude-fleet         # the key on stdin
fleet-hub decide set-key --data-dir ~/.local/share/claude-fleet --ref file:/path/to/jev-key
fleet-hub decide status  --db ~/.local/share/claude-fleet/state.db     # read-only
fleet-hub decide clear-key --data-dir ~/.local/share/claude-fleet
```

The desktop may keep running: the key is read at its next call. Prefer the
key itself or a `file:` reference — an `env:NAME` reference is read by the
desktop process, which (started from the Dock or a launcher) usually does
not have your shell's variables. Everything else — the kill switch, modes,
consent — is in Settings.

## Benchmarking work_link

Before `work_link` asks anything live, it is measured offline (the test
map's card J1, phase 0):

```bash
fleet-hub decide bench work-link [--split dev|test|all] [--provider none|bm25|jev|haiku ...]
                                 [--shape choice|choice+noul]
                                 [--haiku-host ALIAS] [--haiku-model haiku] [--haiku-timeout 120]
                                 [--days 365] [--org ID] [--max-cases N] [--max-calls 500]
                                 [--labels FILE] [--db FILE] [--json]
                                 [--perturb fold|typo|code ...]
fleet-hub decide bench work-link --export-unlinked 150 --out FILE
```

**Nothing runs by itself and nothing changes what fleet does.** Without
`--provider jev` the database is opened read-only; without `--provider
jev` or `--provider haiku` nothing is sent.

**The cases (dataset A)** are the links a *person* confirmed — source
`manual` or `started`; never `agent`, `agent_inferred` or a detection
rule's — whose conversation kept its first prompt. The right answer is the
linked item. Prompts fleet typed itself (a start, a resume, a quick reply)
are left out, and so are prompts Claude Code submitted itself — a
`<task-notification>` when a background agent or command finishes, a
slash command's `<command-name>` echo, a `<system-reminder>` — from both
datasets (on the production hub, 6 of the first 25 `--export-unlinked`
rows were task notifications). One test decides it for the benchmark, the
census and work detection (`service/prompt_origin.rs`, through the
detection loop guard): a prompt that *starts* with a harness block and has
nothing after the blocks is not a person's; a person's words after such a
head are kept without it. Since the hook stopped storing that text, the
person's first prompt is the conversation's first; rows stored earlier are
left out as fleet-typed. Each case also becomes a **none-case**: the same prompt, the
candidates without the right item, and "abstain" as the right answer.

**What a provider sees** is the first prompt with the answer taken out
(the leakage guard): every ticket key and ticket link the recogniser finds,
every URL, the session's branch name, the right item's key and exact title,
and — for a `started` link — every word of its `{key}-{slug}` branch slug
(the slug is the ticket's title), inflections included. A test fails if
any of these survives.

**The candidates** approximate what fleet could have offered at the
decision: the items of the case's org (its tracker's, or local items linked
in it) updated at most a day after the decision and not unavailable then,
newest first, at most 50, the right one always among them. The store keeps
an item's latest `updated_at` only, so this is an approximation; the report
says so, and lists every other one.

**Recall first.** Before any model is judged, the report shows how often
the right item would have been in the candidates anyway (without being
added), and how often it was in the set the M4.6 nudge offers — "mine"
tickets inside the host's fence and org, plus recent local items — and
whether that set was small enough for the nudge to fire (1–5). A model
cannot choose an item the candidates never held.

**Split by time.** Cases are ordered by decision time: the oldest 60% are
*dev*, the newest 40% *test* (`--split`, default `test`). Every threshold is
chosen on dev and applied to what is reported.

**Providers.**

| Provider | What it does |
|---|---|
| `none` | Always abstains: what fleet does today when nothing links a session. |
| `bm25` | BM25 over each candidate's title (counted twice) and cached description, against the redacted prompt; lower-cased, diacritics folded (`č` → `c`, `ß` → `ss`), split on anything not a letter or digit, cut to a 6-character stem. Abstains under a score threshold chosen on dev (the one that maximises right answers plus right abstentions); the report names it. |
| `jev` | One Choice over the candidates (option keys are item ids like `i123`, each described by its title) plus `none`, through the envelope: question version `work_link.bench.v1`, subject `bench:<case>`, recorded in `decision_runs` like any call. A case whose org the gate refuses — flag off, no consent, no key, breaker, budget (the feature's live mode is not needed) — is **skipped with that fallback** and nothing is sent for it. At most `--max-calls` calls a run. |
| `haiku` | The same Choice — the same redacted first prompt, candidate ids and titles, and `none` — asked of `claude -p` on `--haiku-host` (below, *The `claude -p haiku` baseline*), on the same cases as Jev. Its pick is the answer (`none` abstains) at its own operating point, its stated confidence the score; an answer outside the options is an abstention counted as `invalid`; a failed call (timeout, SSH, `claude`) is skipped with its reason. A case whose org is not the host's is skipped as `other_org` and nothing is sent for it. Not gated by the envelope and never recorded in `decision_runs`. At most `--max-calls` calls. |

**Question shape** (`--shape`, Jev only). `choice` (the default) is the one
Choice above. `choice+noul` is the test map's skill-suggestion pattern as
the envelope allows it — one question per call: the same Choice, then, when
it picked an item, **one Noul on that item only** ("does this session work
on *title*?", version `work_link.bench.noul.v1`, subject
`bench:<case>:noul`). The answer stands when the noul reaches a threshold
chosen on dev (like BM25's: most right answers plus right abstentions) and
is an abstention below it; that noul is then the score every other
threshold uses. Both calls count against `--max-calls` (a case whose check
no longer fits is skipped as `max_calls`), and a case's latency, tokens and
cost are both calls together.

**Metrics** (test map §4), per provider and dataset: accuracy on answered,
coverage, coverage at precision 0.9 (the lowest threshold whose dev
accuracy reaches 0.9 over at least 5 answers, applied to the reported
cases), abstention quality on none-cases, p50/p95 latency, input tokens and
cost (Jev, haiku), `invalid` answers (haiku), and what was skipped and why.
Differences in accuracy on answered between two providers come with a
bootstrap 95% interval (1000
resamples, fixed seed); an interval across 0 is "no difference". The
breakdown is by org, tracker, language of the prompt × language of the
right item's title (`service::nl`), code density and candidate-set size. A
cell under 5 cases shows as `<5` with no rates; a cell under 200 cases is
marked *not judged* (test map §3).

**Calibration** (Jev, haiku): ECE over 10 equal-width confidence bins and
the Brier score of the Choice's confidence against whether its answer (an
item, or `none`) was right, over every usable case that stated one, and per
breakdown cell (the `ece` next to a cell's rates). With `choice+noul` the noul gets its own
line (the noul against whether the chosen item was right). Under 5 answers
neither number is shown.

**Acceptance (card J1, when `jev` ran)**, per dataset, each line PASS, FAIL
or NOT JUDGED:

1. *Accuracy on answered ≥ 0.90 at coverage ≥ 0.40*, at the precision-0.9
   point (threshold chosen on dev). FAIL when dev never reached 0.90.
2. *≥ 10 points above BM25 at equal coverage.* BM25 answers at its abstain
   threshold; its coverage on dev is the target, and Jev's confidence
   threshold is the one whose dev coverage comes closest to it. Both are
   applied to the reported cases and compared with a bootstrap interval
   (the `at bm25's coverage` line).
3. *Not worse than `claude -p haiku` by more than 3 points*, when both
   `jev` and `haiku` ran (paired). Haiku answers at its own operating point
   (its `none`); its coverage on dev is the target, and Jev's confidence
   threshold is the one whose dev coverage comes closest to it — the same
   construction as line 2. Both are applied to the reported truth cases
   both answered usably and compared with a bootstrap interval (the `at
   haiku's coverage` line); PASS when Jev − haiku ≥ −0.03 over at least
   200 paired cases. NOT JUDGED, with the reason, when haiku did not run,
   when there is no threshold from dev (`--split all`), or on fewer cases.
4. *Abstention quality ≥ 0.85* on none-cases, at Jev's operating point
   (with `choice`, its own `none`; with `choice+noul`, the noul threshold).

The overall verdict is over these four (so it is NOT JUDGED without a
haiku run, unless one fails). Then **one line per language
cell** (prompt × truth title): the cell at Jev's operating point against
`en×en` thresholded to the same coverage; more than 10 points below
English **falls back** (FAIL), otherwise it keeps (PASS). Anything with
fewer than 200 cases — the dataset, the none-cases, a cell or the English
cell — is NOT JUDGED. The thresholds of 1–2 need Jev's dev answers: run
`--split all` (the note says so otherwise).

**Robustness (dataset C, `--perturb`).** Each case a model would be
asked (the reported side of A, and H) is asked once more with its first
prompt perturbed — `fold` (diacritics removed, case kept), `typo` (one
deterministic typo in one word of four letters or more) or `code` (every
fenced code block replaced by `[code: <lang>, N lines]`, decision D42's
A/B) — only when the perturbation changes it, as `<case>.<perturbation>`.
Each variant is compared with its original at the provider's **raw pick**
(before any threshold chosen on dev; a "none of these" case is right when
it abstains): each side's accuracy on answered and coverage, how often the
answer changed, the accuracy difference with its bootstrap interval, and
McNemar's exact test on who was right — the verdict (`worse` / `better` at
p < 0.05, else `no difference`; *not judged* under 60 pairs, test map §3).
A diagnostic: it changes no acceptance line. Each perturbation is its own
pass of at most `--max-calls` calls; a provider with fewer than 5 pairs is
left out of the lines.

**Hand labels (D39, dataset H).** `--export-unlinked N --out FILE` writes N
sessions with a first prompt and no confirmed or suggested link, spread over
the window, one JSON line each: the redacted prompt, the candidates (ids and
titles) and `"label": null`. A person sets `label` to a candidate's id or to
`"none"`; `--labels FILE` adds the labeled rows as dataset H, reported on
its own (a label naming an item outside the row's candidates is counted as a
recall miss). A row's org is read from the database — its session's org
now — never from the file: a row whose `org_id` differs is refused (export
again), and a row whose session is gone is left out and counted, since
nothing can vouch for its org (Jev's consent gate and the haiku org fence
rest on it). **The file holds prompt and title text**: it is created
`0600`, never over an existing file, and stays on the machine. The report
itself never holds a prompt or a title.

## Benchmarking status_map

Before `status_map` is put in `shadow` for an org, it is measured offline
(the test map's card J3, phase 0):

```bash
fleet-hub decide bench status-map --fixture                     # the built-in synthetic set
fleet-hub decide bench status-map --labels sections.jsonl       # the owner's hand set
fleet-hub decide bench status-map --fixture --provider rule --provider jev [--max-calls 500] [--db FILE] [--json]
fleet-hub decide bench status-map --labels sections.jsonl --provider jev --provider haiku --haiku-host ALIAS
fleet-hub decide bench status-map --labels sections.jsonl --provider jev --provider haiku --haiku-host ALIAS \
    [--split dev|test|all] [--question FILE ...] [--floor-sweep]
fleet-hub decide bench status-map --fixture --provider rule --provider jev \
    --perturb fold --perturb typo --perturb emoji --perturb no-board --perturb shuffle-board
fleet-hub decide bench status-map --paired-fixture --provider rule --provider jev   # en / sk / cs / de
fleet-hub decide bench status-map --fixture --split dev --provider jev --question-set --floor-sweep
```

**The cases** are labeled sections, one JSON line each:

```json
{"section": "Čaká na klienta", "project_sections": ["Nové", "V riešení", "Čaká na klienta", "Hotovo"],
 "expect": "in_progress", "lang": "sk", "note": "ambiguous: started, waiting on the client", "org_id": 2}
```

`expect` is `todo`, `in_progress`, `done` or `not_planned`; `lang` is `en`,
`sk`, `cs`, `de` or `mixed`; a `note` containing `ambiguous` marks a case a
reasonable person could label otherwise, reported apart; `org_id` (optional)
is the org whose consent a Jev call needs, and `tracker_id` (optional) the
Asana tracker the section is on. **The org is checked against the database
before anything is sent** (with `--provider jev` or `haiku`): a row with a
`tracker_id` takes that tracker's org and is refused when its `org_id`
says otherwise; a row with only an `org_id` must name an org the database
knows, and when Asana trackers there list the section, one of them must be
of that org (else it is refused: give the row its `tracker_id`). Names are normalised as the
probe stores them (trimmed, lower case; the board de-duplicated), and the
Jev request is **the adapter's own** (`status_map::question_for`: the
section and at most 30 board names), so the benchmark measures what
`shadow` would send.

> **The built-in set is synthetic.** `--fixture` reads
> `crates/fleet-core/src/service/testdata/decide/status_map_sections.jsonl`:
> 391 sections on 91 plausible boards — plain English boards, Slovak, Czech
> and German ones with and without diacritics (`Rozpracované` /
> `Rozpracovane`, `V řešení` / `V reseni`, `Prüfung` / `Pruefung`), emoji
> prefixes (`🚀 Shipped`, `🧊 Icebox`), jokey names (`Parking lot`,
> `Graveyard`, `Victory lap`), names that trap the keyword rule (`Not
> started`, `Almost done`, `Abandoned`, `To be released`) and vague names
> whose meaning depends on where they sit (`Ready` before *In progress* vs
> after *Review*; `Next` or `Waiting` after *Done*). It was **written by an
> LLM (decision D43) and has not been spot-checked by the owner yet**; 60
> labels are marked ambiguous. Its verdicts are indicative: the card's gate
> is the owner's hand set (`--labels`). It has no `org_id`, so a Jev run on
> it is consented by `decide.jev.unassigned`, not by an org, and a haiku
> run on it needs a `--haiku-host` with no org (below).

**Providers.**

| Provider | What it does |
|---|---|
| `none` | Always abstains. |
| `todo` | Always `todo`: what fleet does today with a section the rule cannot classify. |
| `rule` | The keyword rule `infer_section` (progress / doing / review / wip / started / active / testing / qa → in progress; done / shipped / complete / released / closed → done); abstains where it says nothing. |
| `jev` | One Choice through the envelope — question version `status_map.bench.v1` (`status_map.bench.q.<version>` with `--question`, below), subject `bench:<case>`, baseline the rule's answer or `none`, the adapter's confidence floor 0.5. `unsure` and an answer under the floor are **abstentions**; a failed call (timeout, HTTP error, invalid answer) is skipped with its fallback. A case whose org the gate refuses is skipped and nothing is sent; `decide.jev.status_map` may stay `off` (in `shadow` or `assist` it would also start the daily live runs). At most `--max-calls` calls. |
| `haiku` | The adapter's own request — the same redacted section, board and options — asked of `claude -p` on `--haiku-host` (below), at the same floor: `unsure`, an answer under 0.5 and an answer outside the options (counted as `invalid`) are abstentions; an answer that states no confidence stands and is left out of calibration. A failed call is skipped with its reason. A row whose org is not the host's is skipped as `other_org` (the built-in set has no org: it needs a host with no org). Not gated by the envelope; the database is opened read-only for the host's org; nothing is recorded. At most `--max-calls` calls. |

Without `--provider jev` or `--provider haiku` no database is opened at
all (`haiku` opens it read-only, for the host's org). With `jev`, the hub's
database (or `--db FILE`) is opened for writing: it holds the gate's
settings, and every call is recorded in `decision_runs`. No threshold is
tuned on the set — the rule is fixed and Jev runs at the adapter's floor.

**A question file** (`--question FILE`) rewords the question for one run,
so the wording can be tried on the hub (where the Jev key lives) without a
release per attempt:

```json
{"version": "v2-draft1",
 "instructions": "state.section is the name of one section … Choose unsure when …",
 "options": {"todo": "…", "in_progress": "…", "done": "…", "not_planned": "…", "unsure": "…"}}
```

It replaces the instructions and each option's criterion for **both**
`jev` and `haiku`; the option ids and their order, the confidence floor and
the state (the section and its board window, redacted as ever) stay the
adapter's, so what leaves the hub about a section does not change. The file
must hold exactly those five options, no text may be empty, and `version`
must match `^[a-z0-9][a-z0-9._-]{0,39}$`; otherwise the run is refused
before anything is sent. It needs `--provider jev` or `--provider haiku`.
Jev's runs are recorded with question version
`status_map.bench.q.<version>` (never `status_map.bench.v1`, so the two
wordings never mix in `decision_runs`), and the report's header names the
question: `the adapter's status_map.v1` or `file <version>`. Without the
flag nothing changes.

**The dev/test split** (`--split dev|test|all`, default `all`) keeps a
reworded question honest. It is by **board**, never by case: a case's
board is its normalised section names in order (after the de-duplication
above) joined with a newline, and the board is *dev* when the first 8 bytes
of that key's SHA-256, read as a big-endian integer, are 0, 1 or 2 modulo
10 — about 30% of the boards, the same on every run and machine. All
sections of a board land on the same side, so a wording cannot learn a
board's layout on dev and be scored on it in test. The report states the
split and both sides' board and case counts; with `all` that line (and a
note naming the split's purpose) is the only difference. The intended
workflow: iterate the wording with `--split dev --question FILE`; when it
is settled, run it **once** with `--split test` and read that verdict;
then a code change adopts the wording in the adapter
(`status_map::INSTRUCTIONS` / `OPTIONS`) as `status_map.v2`. Dev is kept
small so the test side stays above the 200 cases the haiku line needs (it
is judged over paired cases where the rule abstains). On the built-in set
dev is 28 boards with 129 sections and test 63 boards with 262, 211 of
them where the rule abstains; the owner's hand set is the built-in set
plus the real rows.

**Metrics** per provider: accuracy on answered and coverage over every case,
and the same **where the rule abstains** (the sections that today silently
count as to do); accuracy with `not_planned` counted as `done` (as a section
map stores it); **`done` precision** — of the answers that apply as done
(`done` or `not_planned`), the share labeled so — which is the card's
headline, since a wrong `done` hides live work (a strict column counts
`done` alone); the confusion matrix (label → answer or abstain); ECE (10
bins) and Brier over every category answer Jev gave with a confidence
(under the floor included, `unsure` not); latency, tokens and cost.
Differences in accuracy on answered between providers come with the same
bootstrap interval as J1's, over all cases and (between providers that
answer there) where the rule abstains. The breakdown is by language and by
clear / ambiguous label; a cell under 200 cases is *not judged*.

On the built-in set the rule answers 74 of 391 (accuracy 0.892) and
abstains on 317; its `done` precision is 0.935 over 31 answers ("almost
done" and "to be released" are live work). A test pins these numbers.

**Acceptance (card J3)**, per provider that answers, each PASS, FAIL or NOT
JUDGED (under 200 usable cases): `done` precision ≥ 0.97; accuracy on
answered ≥ 0.90 where the rule abstains; coverage ≥ 0.60 of those sections;
and "beats `claude -p haiku` or ties it at under 1/10 of its latency",
judged when `haiku` ran too: paired over the sections where the rule
abstains (the ones the adapter asks in assist) that both answered usably,
the provider's accuracy on answered minus haiku's with its bootstrap
interval — above 0 **beats** (PASS), across 0 is a **tie** that passes when
the provider's p50 latency over the same cases is under a tenth of haiku's
(an offline provider's is 0) and fails otherwise, below 0 FAILS; NOT
JUDGED under 200 paired cases or without a haiku run (the reason is
printed). The overall verdict is PASS only when all four pass.

### Diagnostics: robustness, languages, the floor, the wording

None of these changes an acceptance line (no card registers a threshold on
them); each is evidence for a decision a person makes. They print after the
breakdown and are in `--json` under `robustness`, `languages` and
`floor_sweep` (absent when not asked for).

**Robustness (dataset C, `--perturb`).** Each reported section is asked
once more as a person or a board might have written it: `fold`
(diacritics removed on the whole board: `rozpracované` → `rozpracovane`),
`typo` (one deterministic typo in the section's name, and in its entry on
the board), `emoji` (a neutral emoji — 📌 🔹 ⭐ 🟣 📁 🌀, none of them a
status — before every name of the board), `no-board` (the section's name
alone) or `shuffle-board` (the board in another order: what is left
without the position). A variant exists only when the perturbation changes
what is sent; its keyword rule is read again on the new name. Each is
compared with its original over the pairs both answered usably: accuracy on
answered and coverage on both sides, `done` precision on both sides, how
often the answer changed, the accuracy difference with its bootstrap
interval, and McNemar's exact test on who was right — the verdict (`worse`
/ `better` at p < 0.05, else `no difference`; *not judged* under 60 pairs).
A lost answer counts as much as a wrong one there, which is why McNemar,
not accuracy on answered, decides. Each perturbation is its own pass of at
most `--max-calls` calls.

**Languages (dataset B).** Rows with the same `pair` id are one section in
several languages, each with its board translated:

```json
{"section": "Hotovo", "project_sections": ["Nápady", "Treba urobiť", "Rozpracované", "Hotovo"],
 "expect": "done", "lang": "sk", "pair": "b01.5"}
```

When the rows carry pairs, the report compares every language with the
`en` row of the same pair (the same numbers and verdict as robustness).
`--paired-fixture` reads the built-in paired set,
`crates/fleet-core/src/service/testdata/decide/status_map_paired.jsonl`:
16 boards (software, support, releases, content, bugs, GTD, hiring,
design, ops, a numbered sprint, events, research, invoices, translation,
a roadmap, legal) in en, sk, cs and de — 81 pairs, 324 rows, 32 marked
ambiguous. Like the main set it is **LLM-written (D43) and not yet
spot-checked by the owner**, and it has no org. A split breaks pairs (the
languages' boards differ), so run it with the default `--split all`.

**The floor sweep (`--floor-sweep`).** The adapter asks at a confidence
floor of 0.5; Jev's answers under it are kept already, so the sweep costs
no call. For `jev` and `haiku` it prints, at every floor from 0 and 0.30 to
0.95 (step 0.05; `*` marks 0.5): answers, coverage, accuracy on answered,
the same where the rule abstains, and `done` precision. With `--split all`
it also names **the lowest floor the dev boards would choose** — `done`
precision ≥ 0.97 (when anything applies as done) and accuracy ≥ 0.90 where
the rule abstains, over at least 20 such answers — and that floor's numbers
on the test boards. The adapter's floor changes only with a code change and
a new decision row; the sweep is the evidence for one.

**The question set (`--question-set`).** Three rewordings of the adapter's
question ship in
`crates/fleet-core/src/service/testdata/decide/questions/`:
`v2-position` (read the name first, then a concrete rule for the position),
`v2-multilingual` (names in any language, with or without diacritics,
emoji and numbering, with examples in sk / cs / de) and `v2-careful-done`
(`done` only when the name says finished; ready, waiting and almost done
are in progress). `--question-set` asks the adapter's question and each of
them — one report each, one pass of at most `--max-calls` calls each —
then prints one comparison of the model providers where the rule abstains
(coverage, accuracy, `done` precision and n, ECE, calls, tokens). **Only
with `--split dev`**: compare wordings on dev, then judge the one you chose
once with `--split test --question FILE`, and only then adopt it in the
adapter as `status_map.v2`. `--question` may be repeated to compare your
own files the same way. With more than one question `--json` prints
`{"reports": [...], "questions": [...]}`.

## Benchmarking the closed-choice use cases

Every Jev use case ships with a benchmark set, and stays `shadow` (or
`off`) until its acceptance lines pass. Besides J1, J2 and J3 above, each
use case that picks one option out of a few has a bench of the same shape:

```bash
fleet-hub decide bench control-route       --fixture | --labels FILE [--provider rule|baseline|jev ...]
fleet-hub decide bench duplicate           …   # K4
fleet-hub decide bench related-session     …   # N1
fleet-hub decide bench work-placement      …   # K5
fleet-hub decide bench host-placement      …   # N5
fleet-hub decide bench routine-run-outcome …   # N6
fleet-hub decide bench adopt-target        …   # N4
fleet-hub decide bench restore-target      …   # J10
fleet-hub decide bench main-ticket         …   # J6
fleet-hub decide bench tracker-duplicate   …   # J7
```

**The cases** are JSON lines `{id, input, label, by?, never?, trap?}`.
`input` holds the use case's own facts (a Control message and its targets;
a proposed task and the open ones; a host list with its numbers; a run, its
session and its screen; a directory and the projects; …). `label` is the
right option in the use case's words, or `unsure` where nothing should be
pre-selected. `by` says who should answer: `rule` or `jev`. `never` lists
options that must never be the answer (a host over its limit, another
org's task, another person's session, a failed run read as `nothing`);
`trap` names the never-list target a case baits (`approve_push`, `role`,
`share`, `priority`, …). `#` lines are comments.

**Each case goes through the use case's own code**: the same rule
functions and request builders the live adapter calls
(`control_route::unclear`, `duplicate::rank`, `related_session::eligible`,
`host_placement::candidates`, `routines::outcome::rule_outcome`, the Lost
and found path rule, `main_ticket::rule`, `tracker_duplicate::rule`). A case
the rules decide never reaches a model; a set whose `by` contradicts the
rules is refused with its line number.

**Providers.** `rule` (the rule layer alone; it abstains where the model
would be asked), `baseline` (what fleet does without Jev: the rule, else the
live adapter's shadow baseline), `jev` (the rule, else the model through the
envelope's gate — a case has no org, so `decide.jev.unassigned` must be on,
and `decide.jev.unassigned_reply` too for `routine-run-outcome`; the
feature's live mode may stay `off`; every call is recorded as a benchmark
run). Without `--provider jev` nothing opens a database.

**Metrics** per provider: rule-decided cases, calls, coverage, accuracy on
answered, proposal precision (over the answers that would show something:
not `unsure`, not `none` / `control`), pre-selects on `unsure` cases, and
answers on a never list. No case text is printed.

**Acceptance (each closed-choice use case alike)**, each PASS, FAIL or NOT
JUDGED:

| Line | Threshold | Judged from |
|---|---|---|
| Proposal precision | ≥ 0.9 | 50 labelled cases the rules leave to the model |
| Pre-selected on an `unsure` case | ≤ 5% | 20 `unsure`-labelled cases |
| An answer on a case's never list | 0 | any trap case |

**The built-in sets are synthetic.** We have no recorded Control chats,
known duplicates, past starts or recorded runs to label yet, so each set
(`crates/fleet-core/src/service/testdata/decide/<use case>_cases.jsonl`,
12–20 cases each) is hand-written from fleet's own test fixtures and
realistic made-up cases, LLM-written (D43), and starts with a `# synthetic`
line. On a synthetic set **every verdict is NOT JUDGED**, whatever the
numbers: it checks that the rules decide what they should without a call,
that no never-list option is ever offered, and that the report runs. No use
case's acceptance has passed; each stays `off` or `shadow` until a set of
recorded cases (no `# synthetic` line) passes.

## The `claude -p haiku` baseline (D33)

`--provider haiku` answers each benchmark question with a Claude model in
print mode, so the cards' haiku lines can be judged. D33 also names it the
later generative fallback for *asynchronous* decisions only; nothing live
calls it yet.

```bash
fleet-hub decide bench status-map --labels sections.jsonl --provider jev --provider haiku --haiku-host gpu1
fleet-hub decide bench work-link --split all --provider bm25 --provider jev --provider haiku \
    --haiku-host gpu1 [--haiku-model haiku|sonnet|opus] [--haiku-timeout 120] [--max-calls 500]
```

**Where the data goes.** Each case's prompt leaves the hub over SSH for
the host `--haiku-host` names and reaches Anthropic through **that host's
Claude account** — the processor its sessions already use, which is why D33
chose it. The host is required (there is no default), and before the first
call the benchmark prints a note on stderr naming it and its org (kept in
the report's notes). Nothing about a haiku call is recorded in
`decision_runs`: that record is the envelope's, for Jev. The call does not
pass the envelope's gate (flag, mode, org consent): naming the host is the
consent — **but never across the org boundary**.

**The org boundary.** The host's org is read from the database the
benchmark opened (`hosts.org_id`, read-only; `--db FILE` like the rest; a
host the database does not know is refused). A case goes to the host only
when **its org is the host's org**, and a case with no org only to a host
with no org — no org on both sides counts as the same; nothing else does.
Every other case is skipped as `other_org`, counted and shown with the
other skip reasons, and nothing is sent for it. So the built-in status-map
set (`--fixture`, no `org_id`) needs a host with no org, and a J1 run
covering several orgs needs one run per org, each on a host of that org
(`--org ID` keeps the cases to one).

**What it is asked.** The Jev request of the case, **redacted exactly as
the envelope redacts it** (URLs, emails, secrets), rendered as plain text:
the state as JSON, the instruction, and each option id with its
description, then *Reply with exactly one line of JSON: `{"choice": "<option
id>", "confidence": <0..1>}`*. Nothing else about the case — no label, no
note, no key or title the leakage guard removed — is in it; a test replays
the J1 leakage guard on what reaches the host.

**How it runs.** One command on the host, `claude -p --model <m>
--output-format json --settings '{"disableAllHooks":true}' --tools ''
--strict-mcp-config --no-session-persistence`, in a fresh temporary
directory with the output capped: no tool, no MCP server, none of fleet's
hooks, no transcript left behind. **The prompt goes on the command's
stdin, never in argv** — `claude -p` reads it there — so the command line
(what `ps` shows on the host) holds only those fixed flags and the
validated model. One call at a time per host, each
under `--haiku-timeout` seconds (10–600, default 120; the host-side
`timeout` stops `claude` 10 s earlier), and `--max-calls` bounds the run.
The host must be reachable over SSH from where the benchmark runs (an
agent-only host is not: its transport cannot pipe stdin). `--haiku-model` is `haiku` (default), `sonnet` or
`opus`.

**How it is read.** The last JSON object in the model's text that has a
`choice`; the choice must be one of the offered option ids (else
`invalid`, an abstention), the confidence is clamped to 0..1 and a missing
one leaves that answer out of calibration. Latency is the whole call, SSH
included. Input tokens (plain + cache) and cost (`total_cost_usd`) come
from the `--output-format json` envelope; a call whose envelope has none is
counted as usage unknown and left out of the totals. A timeout, an
unreachable host, a missing `claude` or an error result is skipped with
that reason (`timeout`, `ssh_error`, `noclaude`, `call_failed`,
`signed_out` when the host's Claude login has expired).

## How to run phase 0

A checklist for the owner, on the hub, before any feature leaves `off`:

1. **Census** — which languages the prompts and titles are in, so the cells
   are known: `fleet-hub census languages [--days 90] [--org ID]`.
2. **Labels** — the hand sets the cards are judged on:
   - J3: write (or correct) the section labels —
     start from the built-in set, spot-check its `expect` and `note`
     columns, and add your own boards' sections with their `org_id`;
   - J1: `fleet-hub decide bench work-link --export-unlinked 150 --out
     h.jsonl`, set each `label` (D39).
3. **Offline baselines** (nothing sent):
   `fleet-hub decide bench status-map --labels sections.jsonl` and
   `fleet-hub decide bench work-link --split all --labels h.jsonl`.
4. **Jev, gated** — the key (`fleet-hub decide set-key`), the kill switch
   on (`fleet-hub decide enable`), and consent for the orgs to be measured
   (`fleet-hub org set <id> --jev on`; `fleet-hub decide unassigned on` for
   rows with no org). Leave the
   features' modes `off`: the benchmark does not need them, and
   `decide.jev.status_map` at `shadow` would also start the daily live runs
   on every consenting org's Asana trackers. Then add `--provider jev` to the same commands (J1 with
   `--split all`, and once more with `--shape choice+noul`); `--max-calls`
   bounds the spend, `fleet-hub decide status` shows it.
   Add `--provider haiku --haiku-host ALIAS` to the same runs (a host whose
   Claude account may see these prompts, of the cases' own org — cases of
   any other org are skipped as `other_org`) so the haiku lines are judged
   on the same cases.
   Then the diagnostics on the same key and budget (each pass bounded by
   `--max-calls`):
   - robustness: `status-map --labels sections.jsonl --provider rule
     --provider jev --perturb fold --perturb typo --perturb emoji --perturb
     no-board --perturb shuffle-board`, and `work-link --split all
     --provider bm25 --provider jev --perturb fold --perturb typo --perturb
     code` (the code placeholder is D42's A/B);
   - languages: `status-map --paired-fixture --provider rule --provider jev`
     (and your own paired rows, `pair` ids);
   - the wording: `status-map --labels sections.jsonl --split dev --provider
     jev --question-set --floor-sweep`, then the chosen wording once with
     `--split test --question FILE --floor-sweep`.
5. **Read the acceptance lines** of each report: PASS / FAIL / NOT JUDGED
   against the thresholds registered in the test map, the calibration, and
   the per-language cells (a cell that falls back stays off for that
   language). A threshold changes only with a new decision row.
   While a feature runs live, watch `fleet_health.decide` (*Health*,
   above): the desktop's **Jev degraded** item, or `fleet-hub decide
   status`'s `health` line.
6. **Shadow per org** — only for a card that passed: set
   `decide.jev.<feature>` at `shadow`, consent only the orgs whose cells
   passed, and leave the rest off. Assist comes after shadow's own exit
   criteria (test map §2).
