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
   `decide.jev.unassigned` (off by default).
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

## Retention

`decide.retention_days` (90 by default; `0` keeps them forever). The GC
sweep deletes older runs a batch at a time, whether or not the GC's session
killer is on. A run a person followed up — `confirmed`, `corrected` or
`rejected` — is **kept** whatever its age: a rejection must keep holding
(the same answer is not proposed again), and those runs are the labels the
evaluation is judged on. They are few: one per decision a person made.

## Turning it off

- **Instantly, everything:** Settings → *Decisions (Jev)* → off, or on a hub
  `set_setting { key: "decide.jev.enabled", value: "false" }` with the
  master token. The next call is refused; nothing in flight is retried.
- **One org:** `fleet-hub org set <id> --jev off`, or its checkbox in
  Settings → Organisations.
- **One feature:** its mode to `off`.
- **The key:** `fleet-hub decide clear-key`.

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
per tracker (sooner when its sections change), in a task of its own — never
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
inferred one kept under your entries, like Settings → Work's **Confirm** —
with the proposals you accept; edit it to drop or change any. `--json` also
carries the same change as `work_admin` arguments. In `shadow` the view
shows, per section, the rule's and the model's category and how often they
agree where the rule decided.

**One proposal at a time: apply, correct or reject.** A proposal is named
by its run id (`run 812` above). A person decides it three ways:

| | Hub (operator) | Standalone desktop | What it writes | Follow-up |
|---|---|---|---|---|
| **Apply** | `fleet-hub decide proposals apply 812` | Settings → Work → **Apply** | the answer's category into your section map (`not_planned` applies as `done`; `unsure` proposes nothing and cannot be applied as is) | `confirmed` |
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

## Settings

| Key | Default | What it does |
|---|---|---|
| `decide.jev.enabled` | `false` | The kill switch. Off: nothing is ever sent. |
| `decide.jev.status_map` | `off` | `off` / `shadow` / `assist` for Asana section → status category proposals. |
| `decide.jev.work_link` | `off` | `off` / `shadow` / `assist` for choosing a work item for an unlinked session. |
| `decide.jev.unassigned` | `false` | Sessions and items with no org may be sent too. |
| `decide.jev.timeout_ms` | `1500` | One call's whole budget (100–30000 ms). |
| `decide.jev.breaker_failures` | `5` | Failed calls in a row that open the breaker (1–100). |
| `decide.jev.breaker_open_secs` | `300` | How long an open breaker refuses calls (10–86400 s). |
| `decide.jev.daily_token_budget` | `2000000` | Input tokens per UTC day (`0` = none). At $0.042 per million, the default is under $0.09 a day. |
| `decide.jev.model` | `jev-1.13.0` | `jev-1.13.0` (pinned) or `jev-latest`. |
| `decide.retention_days` | `90` | Days a run is kept (`0` = forever). |

On a standalone desktop they are in Settings → *Decisions (Jev)*. On a hub
they are set over the API with `set_setting` (master token), like the
`work.*` settings; a paired desktop shows them read-only there.

`decide.jev.work_link` has no live path yet (decision D32: J1 is measured
offline only, until it passes its acceptance lines), so Settings shows it
read-only, with whatever value it holds; the benchmark does not need it.

## The command line (hub)

```bash
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
```

The key is never an argument (shell history, `ps`). `set-key` and
`clear-key` write the hub's database (`--data-dir`, else the hub's default)
directly, like `fleet-hub tracker
webhook`; the running hub reads the key at its next call. `status`,
`runs` and `proposals` (the listing) open the database read-only (no running hub needed; `--db FILE`
reads a desktop's `state.db`) and print ids, words and numbers only:
the flag, the modes, which orgs consented, whether a key is configured
(never the key), the live breaker, today's tokens and cost (the live
features' and the benchmark's apart; the budget counts both), and runs
per feature, `live` or `bench`, provider, fallback and org.

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
| `jev` | One Choice through the envelope — question version `status_map.bench.v1`, subject `bench:<case>`, baseline the rule's answer or `none`, the adapter's confidence floor 0.5. `unsure` and an answer under the floor are **abstentions**; a failed call (timeout, HTTP error, invalid answer) is skipped with its fallback. A case whose org the gate refuses is skipped and nothing is sent; `decide.jev.status_map` may stay `off` (in `shadow` or `assist` it would also start the daily live runs). At most `--max-calls` calls. |
| `haiku` | The adapter's own request — the same redacted section, board and options — asked of `claude -p` on `--haiku-host` (below), at the same floor: `unsure`, an answer under 0.5 and an answer outside the options (counted as `invalid`) are abstentions; an answer that states no confidence stands and is left out of calibration. A failed call is skipped with its reason. A row whose org is not the host's is skipped as `other_org` (the built-in set has no org: it needs a host with no org). Not gated by the envelope; the database is opened read-only for the host's org; nothing is recorded. At most `--max-calls` calls. |

Without `--provider jev` or `--provider haiku` no database is opened at
all (`haiku` opens it read-only, for the host's org). With `jev`, the hub's
database (or `--db FILE`) is opened for writing: it holds the gate's
settings, and every call is recorded in `decision_runs`. No threshold is
tuned on the set — the rule is fixed and Jev runs at the adapter's floor —
so there is no dev/test split.

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
that reason (`timeout`, `ssh_error`, `noclaude`, `call_failed`).

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
   on, and consent for the orgs to be measured (`fleet-hub org set <id>
   --jev on`; `decide.jev.unassigned` for rows with no org). Leave the
   features' modes `off`: the benchmark does not need them, and
   `decide.jev.status_map` at `shadow` would also start the daily live runs
   on every consenting org's Asana trackers. Then add `--provider jev` to the same commands (J1 with
   `--split all`, and once more with `--shape choice+noul`); `--max-calls`
   bounds the spend, `fleet-hub decide status` shows it.
   Add `--provider haiku --haiku-host ALIAS` to the same runs (a host whose
   Claude account may see these prompts, of the cases' own org — cases of
   any other org are skipped as `other_org`) so the haiku lines are judged
   on the same cases.
5. **Read the acceptance lines** of each report: PASS / FAIL / NOT JUDGED
   against the thresholds registered in the test map, the calibration, and
   the per-language cells (a cell that falls back stays off for that
   language). A threshold changes only with a new decision row.
6. **Shadow per org** — only for a card that passed: set
   `decide.jev.<feature>` at `shadow`, consent only the orgs whose cells
   passed, and leave the rest off. Assist comes after shadow's own exit
   criteria (test map §2).
