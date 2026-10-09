# Jev evaluation: the test map

**Date:** 2026-09-27
**Companion:** `2026-09-27-jev-language-census-design.md` (the verified facts
about Jev, decisions D31–D47, the language census).
**Status legend:** **[verified]** comes from a primary source or from the code.
**[built]** is on the branch. **[proposed]** is a design we agreed on but has
not been measured. **[hypothesis]** is what an experiment has to confirm or kill.

This map says, for every candidate use of a decision model in fleet, how it is
tested before anything is turned on: the scenario, the dataset, the baseline,
the metrics, the acceptance thresholds, the phases, the owning components and
the conditions that turn it off. Thresholds are fixed **before** an experiment
runs. Changing one after seeing results needs a new decision row.

## 1. Principles (apply to every card)

1. **Rules and permissions first.**
   - An exact id (a ticket key, a URL, a tracker's own status category) and
     the org boundary (`OrgScope`, per-host fences, `force_cross_org`) are
     decided before any model runs.
   - The model only ever sees the candidate set the server has already
     filtered.
   - Its answer is checked server-side against that set; an answer outside it
     is `invalid_answer`.
2. **A model answer never grants anything.**
   - No permission, no cross-org link, no risky action.
   - Kills, starts, moves and write-back keep their existing confirmations and
     checks (`confirm_gate`, safe kill, `OPERATOR_CONFIRMS`, write-back only
     for person-confirmed links).
3. **Every path has a defined fallback, and it is always "what fleet does
   today".**
   - Fallback causes: flag off, org not consented, mode off, no key, breaker
     open, budget spent, timeout, HTTP error, rate limited, invalid answer,
     low confidence.
   - Each one is recorded (`decision_runs.fallback`).
4. **Off by default, instantly off.**
   - `decide.jev.enabled` is a kill switch in Settings.
   - Each feature has a mode (`off | shadow | assist`); `auto` exists only for
     a feature and org that passed its gate.
   - Consent is per org (`orgs.jev_allowed`); rows with no org have their own
     switch (`decide.jev.unassigned`).
   - Turning any of these off loses no work: suggestions stay suggestions.
5. **A rule that wins stays.**
   - Every card measures the cases a plain rule decides correctly.
   - The model is only asked where the rule abstains, or where it provably
     beats the rule.
6. **Never on a hook's synchronous path.**
   - Hooks stay fast.
   - Model calls run after the hook returns, or on a tick.

## 2. Phases and their gates

| Phase | What runs | Entry gate | Exit / kill |
|---|---|---|---|
| 0 Offline benchmark | the adapter against a frozen dataset with verified answers; train/dev/test split by time; the answer never in the input | D34 label hygiene built; dataset card written; thresholds registered in this file | Fails its card's offline thresholds → the use case stops or goes back to design |
| 1 Shadow | Jev decides on live inputs; fleet keeps using today's path; both recorded (`answer` vs `baseline_answer`) | Phase 0 passed; org consent; `mode = shadow` | ≥ N live decisions per language cell (card), agreement and latency within bounds, no privacy incident |
| 2 Assist | Jev's answer shown as a suggestion with its reason and confidence; a person confirms, corrects or rejects; the follow-up is recorded | Shadow passed | Correction rate above the card's bound over a rolling window → back to shadow automatically |
| 3 Limited automation | only low-risk decisions, only the cells (use case × language × code bucket × org) that passed; still reversible (a suggestion that is pre-selected, never a destructive act) | Assist passed with the card's `auto` thresholds | Any auto-demote trigger (§7) → assist |
| 4 Wider | per feature and per org, one at a time | 4 weeks in phase 3 without a demote | Same triggers; a model version change (`model_version`) sends every cell back to shadow until re-measured |

## 3. Datasets

| Id | What | How the answer is known | Where it lives |
|---|---|---|---|
| **A** real, stratified | cases from the owner's hub (first prompts, candidate sets as of the decision time, tracker section names), stratified by org, tracker, language bucket, code bucket | a person's decision recorded after D34: `manual` / `started` links, rejections, corrections; D39 hand labels | the hub machine only; never committed |
| **B** paired corpus | the same case in en / sk / cs / de, LLM-translated and spot-checked by a person (D43) | inherited from the source case | synthetic part in the repo; translations of real cases stay local. **[built for J3]**: rows sharing a `pair` id, compared with the `en` row of the pair (`bench::status_map_robust::language_pairs`); the built-in set `status_map_paired.jsonl` (16 boards × 4 languages, 81 pairs, `--paired-fixture`; LLM-written, not yet spot-checked) |
| **C** perturbations | no diacritics, typos, slang and abbreviations, Slovak with English terms, mixed sentences, code blocks kept vs replaced by `[code: <lang>, N lines]` (D42) | inherited | same as B. **[built]** (`bench::perturb`, `--perturb`): J3 `fold`, `typo`, `emoji`, `no-board`, `shuffle-board`; J1 `fold`, `typo`, `code` (the D42 A/B), at the provider's raw pick. Slang, abbreviations and mixed sentences are not generated (they need a person or a model to write them) |
| **H** hand labels (D39) | ~150 sessions with no link, exported with their first prompt and candidates; a person marks the right item or "none" | the person | local file, `0600` |
| **F** fixtures | synthetic cases per card for CI (`testdata/`) | written with the case | repo |

**Splits.**
- Split by time, not at random: train or dev is older, test is newer.
- The test split is looked at once per registered experiment.

**Leakage guard [built for census and for the J1 benchmark: `bench::work_link::redact_prompt`].**
- The input is redacted with the recogniser before any model sees it:
  - every ticket key and URL it finds;
  - the branch name;
  - for `started` links, the `{key}-{slug}` branch slug, which is the ticket
    title.
- A test fails if any label's key, title or slug survives in the state sent
  (`no_case_leaks_its_truth`). The J1 guard also removes the truth's exact
  title and, for `started` links, every slug word by stem (inflections too).

**Language axis (D38, D41, D42, D44).**
- Every case carries:
  - `nl.state` and `nl.candidates`, from `service::nl`;
  - `folded`, `code` density, and `pl.primary` from changed-file extensions.
- A cell is **use case × nl.state bucket (× nl.candidates for J1) × code
  bucket**.
- A cell with fewer than **200** unpaired or **60** paired cases is not judged.
  It routes to fallback.
  - 200 unpaired cases tell 90% from 80% at α 0.05 and power 0.8.
  - For a paired design, McNemar on the B corpus needs roughly a third as
    many.

## 4. Metrics (definitions used by every card)

- **Accuracy on answered:** right answers ÷ answers given (the model did not
  abstain).
- **Coverage:** answers given ÷ cases.
- **Coverage at precision P:** the largest coverage whose accuracy on
  answered is ≥ P, choosing the confidence threshold on dev and reporting on
  test.
- **Wrong automatic decisions:** automated answers a person later corrected.
  In phases 0–2 this is structurally 0; it is measured as "would have been
  wrong".
- **Abstention quality:** on cases whose truth is "none of these",
  abstentions ÷ cases.
- **Calibration per cell:** ECE (10 equal-width bins) and Brier score of
  the stated confidence against whether the answer was right **[built:
  `bench::ece`, `bench::brier`, both benches]**. TypeSafe's own
  advice: thresholds do not carry across question types, and here also not
  across languages.
- **Latency:** p50 and p95 measured on the hub, including the network.
- **Cost:** input tokens × $0.042 / M, from `usage.input_tokens`.
- **Correction rate:** follow-up `corrected` or `rejected` ÷ suggestions
  shown (assist).
- **Time to decision:** a person's seconds from shown to decided (assist vs
  baseline).
- **Statistics:** bootstrap 95% intervals on every gap. A gap whose interval
  crosses 0 is "no difference", and the simpler system wins.
- **Paired diagnostics (B, C) [built]:** McNemar's exact test on who was
  right (a lost answer counts as a wrong one) decides `worse` / `better` at
  p < 0.05, the accuracy-on-answered interval beside it; under 60 pairs not
  judged. They are evidence, never an acceptance line.

## 5. Use-case cards

### J3 — Asana section → status category (`status_map`) — first live use case (D32)

| Field | Plan |
|---|---|
| Decision | For an Asana section name the keyword rule cannot classify (`infer_section` returns none → today `todo`), propose `todo / in_progress / done / not_planned` |
| Input | provider, section name, the project's other section names in order (position is a strong signal, at most 30), nothing else — no task text **[built]** |
| Candidates | closed: the four categories plus `unsure` |
| Reference | the section map a person confirms (`settings.section_map`), plus a hand-labeled set of ~300 section names (English, Slovak, German, emoji-prefixed, jokey) |
| Baseline | the keyword rule `infer_section`; "always todo" (today's effective behaviour); `claude -p haiku` (D33) **[built]**: `--provider haiku --haiku-host ALIAS` |
| Metrics | accuracy on answered; coverage where the rule abstains; `done` precision (a wrong `done` hides live work in Today and can make tidy-up suggest a kill — still behind confirmations) |
| Breakdown | language of the section name, emoji / symbol prefix, project size |
| Safety | leaves the hub: section and project names only, and only for orgs that consented. Wrong choice: an item shows in the wrong column until a person corrects the map. Tidy-up never kills from it without its own protections and confirmation |
| Fallback | today's behaviour (`todo`) on every fallback cause |
| Acceptance (assist) | on the hand set: `done` precision ≥ 0.97; accuracy on answered ≥ 0.90 at coverage ≥ 0.60 of rule-abstained sections; beats haiku or ties it at < 1/10 of its latency |
| Auto | never automatic: a proposal is applied only when a person confirms it (`work_admin update settings.section_map`) |
| Rollback | set the mode to `off`; confirmed maps stay (they are the person's) |
| Components | **[built]** the `service/decide` envelope; the probe keeps `config.unmapped_sections` and `config.project_sections` (`service/trackers/asana.rs`); the adapter `service/decide/status_map.rs` — `propose_for_tracker` (question `status_map.v1`, subject `tracker_section <tracker>:<HMAC section id>`, ≤ 40 asks a run, no re-ask within 14 days on the same fingerprint / version / mode / model), `StatusMapTrigger` after a clean sync pass (`spawn_tracker_sync`, at most daily per tracker, off the sync's path), `record_followups` in `work_admin update`; `fleet-hub decide proposals` (read-only) and `fleet-hub tracker section-map` to apply; the section map UI that exists. Phase 2 *Assist* in the product **[built]**: `decide_proposal` (by run id: `apply` — `not_planned` as `done` — / `apply_as` a correction, both through `work_admin update` so the run is `confirmed` / `corrected`, and `reject` → `rejected`, the section left unmapped); only the latest usable assist answer of a section the tracker's stored config still lists, not decided and not in the person's map (`pending_proposal`; the name never comes from a caller); a rejected answer hidden until a new one (another fingerprint, question version or model) and not re-asked in assist under the same pinned model (`RunReport.skipped_rejected`); proposals carry the top two probabilities; the standalone desktop's Settings → Work *Proposed by Jev (assist)* rows (section as plain text, category, confidence, why, shadow agreement with the rule; Apply / Apply as… / Not this) over the commands `status_map_proposals` / `decide_status_map_proposal` (`LocalOnly` on a paired desktop, like `update_tracker`); `fleet-hub decide proposals apply <run> [--as C]` (over loopback `work_admin update`) and `reject <run>` (the follow-up only). Tests: `service/decide/status_map_tests.rs`, `fleet-hub` `decide::tests`, `src/lib/WorkSettings.test.ts`, `src/lib/trackers.test.ts`. The phase-0 benchmark **[built]**: `fleet-core::service::decide::bench::status_map` — labeled sections (`--labels FILE`, or the synthetic fixture `service/testdata/decide/status_map_sections.jsonl`: 391 sections on 91 boards, en / sk / cs / de / mixed, 60 marked ambiguous, LLM-written per D43 and **not yet spot-checked by the owner**), normalised like the probe and asked with the adapter's own `status_map::question_for`; providers `none`, `todo`, `rule` (`infer_section`) and `jev` through `decide()` (`status_map.bench.v1`, subject `bench`, the adapter's floor; a fixture row has no org → `decide.jev.unassigned`); accuracy on answered, coverage, coverage where the rule abstains, `done` precision (applied and strict), confusion matrix, ECE / Brier, bootstrap CIs, by language and ambiguous / clear, and the acceptance above as PASS / FAIL / NOT JUDGED (`bench::Verdict`); `fleet-hub decide bench status-map`. Tests: `service/decide/bench/status_map_tests.rs` (the rule's numbers on the fixture are pinned). The haiku baseline (D33) **[built]**: `service::decide::haiku` — the adapter's own request, redacted as the envelope redacts it, rendered as a plain-text closed-set prompt, run as `claude -p --model haiku|sonnet|opus --output-format json` with no tools / MCP / hooks / transcript on the operator-named `--haiku-host` (the prompt on stdin, never in argv; one call at a time, a per-call wall clock, `--max-calls`), never across the org boundary (a case goes only to a host of its own org, `hosts.org_id`, no org on both sides equal; else skipped `other_org`), never recorded in `decision_runs`; provider `haiku` at the adapter's floor (an answer outside the options is an `invalid` abstention); the "beats haiku or ties it at < 1/10 of its latency" line judged paired over the rule-abstained sections (`status_map::haiku_criterion`); tests with a scripted SSH transport, the org fence, and the prompt reaching a fake `claude` verbatim on stdin through real bash |
| Kill | a `done` precision under 0.95 on any 100 consecutive proposals |

### J1 — choosing a work item for a session no rule linked (`work_link`)

| Field | Plan |
|---|---|
| Decision | For a session with no confirmed or suggested link after N turns, pick one item from the allowed candidates or abstain |
| Input | first prompt(s), redacted (§3 leakage guard); project and repo name; recent commit subjects (hosts, later). Code blocks replaced by a placeholder if the D42 A/B says so |
| Candidates | closed, ≤ 50: the session's `OrgScope` "mine" + local items changed in 14 days, minus rejected. **Recall check first:** the same fence as the nudge (`tickets.rs` `allowed()`) drops "mine" tickets never linked on the host. Measure the recall of the candidate set against the truth before blaming the model |
| Question shape | one Choice over the candidates plus one Noul per candidate ("does this session work on X?") to decide whether to suggest at all — TypeSafe's skill-suggestion pattern |
| Reference | A (person-decided links, D34-clean) + H (D39) |
| Baselines | none (today); BM25 over titles; the M4.6 nudge (in-session Claude); `claude -p haiku` on the host **[built]**: `--provider haiku --haiku-host ALIAS`; a person |
| Metrics | accuracy on answered; coverage at precision 0.9; abstention quality on "none" cases; p95 latency; correction rate (assist) |
| Breakdown | org, tracker, `nl.state` × `nl.candidates` (cross-language pairs are the census's `pairs`), code bucket, candidate-set size, task kind |
| Safety | leaves the hub: a redacted prompt excerpt and candidate titles of a consenting org only. Wrong choice: a wrong suggestion (never a confirmed link, never grouping, never write-back — D34 keeps an agent/model decision out of `manual`) |
| Fallback | today's path (nothing, or the nudge if on) |
| Acceptance (assist) | accuracy on answered ≥ 0.90 at coverage ≥ 0.40; ≥ 10 points above BM25 at equal coverage; not worse than haiku by more than 3 points; abstention quality ≥ 0.85; a language cell more than 10 points below English at equal coverage falls back |
| Auto (pre-selection only) | accuracy on answered ≥ 0.97 at coverage ≥ 0.25, correction rate ≤ 3% over 200 assisted suggestions in that cell |
| Rollback | mode `off`; suggestions made by it decay like any R6 suggestion |
| Components | envelope; `work_link` adapter (after detection, off the hook path) **[proposed]**; the offline benchmark **[built]**: `fleet-core::service::decide::bench::work_link` (dataset A from `store::bench_work_link` — person-decided `manual`/`started` links; none-cases; the leakage guard `redact_prompt`; candidate recall incl. the nudge fence; time split; providers `none`, `bm25` (`bench::bm25`) and `jev` through `decide()` (`work_link.bench.v1`, subject `bench`); the question shape `--shape choice|choice+noul` (the Noul is a second call on the chosen item only, `work_link.bench.noul.v1`, kept above a dev-chosen threshold — the envelope sends one question per call); metrics, breakdown, bootstrap CIs, calibration (ECE / Brier per cell, `bench::Calibration`) and the acceptance above as PASS / FAIL / NOT JUDGED — Jev against BM25 at BM25's dev coverage (`EqualCoverage`) and the per-language-cell rule), `fleet-hub decide bench work-link` with `--export-unlinked` / `--labels` for D39 (dataset H); the haiku baseline (D33) **[built]**: provider `haiku` asks Jev's Choice (the same redacted first prompt, candidates and `none`) through `service::decide::haiku` on the same cases as Jev, and "not worse than haiku by more than 3 points" is judged at haiku's coverage (`VsHaiku`: Jev's threshold chosen on dev to match haiku's dev coverage, the gap ≥ −0.03 over 200+ paired cases); a test replays the leakage guard on what reaches the host; desktop chip / popover and the phone sheet show source `model`, confidence and the reason **[proposed]** |
| Kill | correction rate > 10% over 100 in a cell; any cross-org candidate reaching the model (a test and a runtime assertion) |

### J2 — outcome of a turn after Stop (hypothesis)

| Field | Plan |
|---|---|
| Decision | finished / asks a question / blocked on error / paused mid-work, from the last assistant message; feeds Today and attention (`done_unread` is hard-wired false today) |
| Input | the last assistant message, truncated, code replaced by placeholders — not stored today (needs the Stop hook payload) |
| Candidates | closed, 4 |
| Reference | hand labels of ~300 turns; weak signals: the next prompt (a person answering means "asked") |
| Baselines | "ends with ?" heuristic; the Notification hook `idle_prompt`; haiku |
| Acceptance | "asks a question" precision ≥ 0.9 and recall ≥ 0.8 (a false "finished" hides a waiting session) |
| Safety | sends Claude's reply text: the most sensitive input of any card. It needs its own consent line (D31 covers prompts and titles, not replies) → **decided as D48** (owner, 2026-10-08): only where the org also consents to reply text, off by default |
| Components | **[built]** (step 5.11) `service/decide/turn_outcome.rs`: setting `decide.jev.turn_outcome` (off / shadow / assist), question `turn_outcome.v1` over finished / asked / stuck / working / unsure, input the visible pane tail (ANSI stripped, chrome dropped, code blocks as placeholders, ≤ 40 lines / 2,000 chars, redacted), baseline the pane rules (`none` where they read nothing), floor 0.5, subject `session_turn` `<session>:<turn_seq>`; asked after a Stop that left the row idle (`spawn_after_stop`), shadow recorded, assist written to `sessions.turn_outcome` (attention: `asked` → waiting, `stuck` → stuck); every hook clears it and an answer lands only while no hook spoke since the Stop; follow-ups from a later Notification and a person's prompt; J8's `pane_unreadable` timeline entry; the bench `fleet-hub decide bench turn-outcome` (providers rule, qmark, jev; synthetic fixture `service/testdata/decide/turn_outcome_tails.jsonl`, 72 tails of which 51 `asked`, LLM-written, D43, never judged); J8's warning in the agent tab. Not built: a trigger for hosts without hooks (no turn counter to key one decision per turn), the haiku baseline |
| Status | **[built, off]**; phase 0 waits on hand labels |

### J4 — ranking context for the brief and the ticket card

- Score each journal entry, commit subject or summary for relevance to the
  task, and keep the top-k within the brief's budget.
- Baseline: the deterministic template.
- Acceptance: the person's "was this handover useful" rating (new) goes up,
  or tokens go down by ≥ 30% at an equal rating.
- **[hypothesis]**: the brief is short today, so the gain may not be
  measurable. Run it only if J1 lands.

### J5 — ranking quick replies for a blocked session

- Choose over the chips given the pending question.
- Baseline: fixed order.
- Metric: tap-through on the first chip.
- A person always taps; the model never answers for them.
- **[hypothesis]**, low risk, phone-visible.

### J6 — the subject among several keys (dump guard, R4)

- A Choice over the keys found.
- Low volume. Run it only as an extension of J1's harness.
- **[built]** (redesign 6.8, off) as `main_ticket`: rule first (one key, a
  branch naming one, more than eight keys is a dump: unsure), then a Choice
  over the suggested keys with the prompt's keys masked as `[K1]`…; Review
  marks the proposed suggestion. Bench `fleet-hub decide bench main-ticket`
  (synthetic, not judged).

### J7 — local item vs tracker item (duplicate identity)

- A Score for "same work?" plus Nouls for the fields that disagree
  (TypeSafe's entity-alignment pattern).
- Low volume. Hypothesis.
- **[built]** (redesign 6.8, off) as `tracker_duplicate`, a Choice rather
  than a Score: a new standalone local task against the org's open tracker
  tickets (K4's ranking), the key rule first; Review shows *May duplicate
  KEY* with a person's *Link KEY instead*. Bench `fleet-hub decide bench
  tracker-duplicate` (synthetic, not judged).

### J8 — pane status when the rules say nothing

- A Choice over `ClaudeStatus` / `StuckKind`.
- As a **drift detector only**: it alerts when Claude Code's UI changed and
  the rules start returning none.
- Pane text can contain secrets, so it is redacted and local-first.
- **[hypothesis]**. Weak as a classifier; the rules are exact on today's UI.

### J9 — checking a cheap output

- A Noul for "is this summary supported by the transcript?"
- Result: escalate haiku → sonnet, or flag the summary.
- The transcript leaves the machine and a 32k state limit applies.
- **[hypothesis]**.

### J10 — matching lost transcripts to projects

- A Noul per project.
- Low volume, weak.
- **[built]** (redesign 4.12) as a Choice over the person's projects rather
  than a Noul per project, behind `decide.jev.restore_target`, with N4's
  pane case behind `decide.jev.adopt_target`: `service/decide/lost_target.rs`
  (questions `restore_target.v1` / `adopt_target.v1`), only where no
  directory rule places the entry. See `docs/decisions.md`.

### K1–K5 — candidates found after the test map (owner, 2026-10-07)

A survey of the features landed or designed after 2026-09-27 found five
more closed-set decisions. The owner accepted all five on 2026-10-07, in
this order: the phase-0 runs first (they gate every cell), then K1, K5 with
the picker's phase 2, K2 and K3 as shadow slots in their designs, K4 last.
Each becomes a full card above when its adapter is built.

#### K1 — the project for a task's first start (`start_project`)

| Field | Plan |
|---|---|
| Decision | A start of a task whose key prefix no project has worked on yet: today `start_work` answers `E_AMBIGUOUS` with `missing: "project"` and the 8 most recently used projects (`service/trackers/tickets.rs`), and the person picks in `StartPopover`. Jev pre-selects one of those candidates or abstains |
| Input | the task's title and description excerpt (redacted), each candidate's `owner/repo`; nothing from the repositories |
| Candidates | closed: the preview's candidate projects plus `unsure` |
| Reference | free: the project the person starts in (a confirmed or corrected run) |
| Baselines | the most recently used project (today's first row); BM25 of the title over `owner/repo` |
| Acceptance (assist) | accuracy on answered ≥ 0.85 at coverage ≥ 0.50, ≥ 15 points above "first row" |
| Safety | a pre-selection only; the person still presses Start. Never on the start path: shadow asks after the preview has answered |
| Setting | `decide.jev.start_project` (`off / shadow / assist`), off |
| Components | **[built]** `service/decide/start_project.rs` (question `start_project.v1`, subject `work_start` `item:<id>` / `key:<HMAC>`, options `p<id>` + `unsure`, floor 0.5, a decided run reused for 14 days); `tickets::preview_start_decided` (shadow spawned off the path, assist awaited → `suggested_project`), wired in the hub's `work_link { preview_start }` and the standalone desktop's `preview_start_work`; `record_start` marks the assist proposal `confirmed` / `corrected` on a person's start; the popover pre-selects it (`suggestedProjectId`, *Proposed by Jev*). Tests: `service/decide/start_project_tests.rs`, `src/lib/start_preview.test.ts`, `src/lib/WorkTaskDetail.test.ts`. Not built: a phase-0 bench over past first starts |

#### K2 — which operator thread a prompt belongs to (`operator_thread`)

- Where: Mode B (`2026-10-06-task-to-session-flow-design.md` §3.2), for a
  prompt that names no task key.
- A Choice over the current thread, the other open threads (title and
  task key) and `new_topic`.
- Asked after the prompt is sent, never before; the answer is at most the
  banner *New topic …? / Send in its thread?*. Its buttons are the labels.
- Weak spot: short prompts and indirection ("do the same for the other
  one"); `unsure` must be cheap.
- Waits on Mode B; built into it as a shadow slot from its first slice.

#### K3 — triage in the mission loop (`mission_triage`)

- Where: the orchestration loop (`2026-10-07-autonomous-orchestration-projects-design.md` §5.3, §6).
- (a) The worker's outcome `done / partial / blocked / failed` when its
  JSON report is missing; (b) a failure's next step `retry / split /
  give_up / ask` as an early exit before the `claude -p` planner.
- The state carries fleet's own evidence (CI, commits, the last error) next
  to the worker's text, which is marked untrusted. Jev never proposes
  `complete` and never marks a `done_when` line verified.
- Labels: the planner's and the person's decisions in `orchestration_events`.
- Waits on O2; built as a shadow slot there.

#### K4 — is a new task a duplicate (`task_duplicate`)

- Where: tasks proposed by the brainstorm (C0/C1), the planner's
  `create_item` and `work_link create`.
- A Noul "same work as X?" over the top 5 BM25 candidates of the same org;
  extends J7. The answer is a *possible duplicate of …* line in the
  approval sheet, never a merge.
- Waits on bulk creation (C1 / O2).

#### K5 — a task's group in the Work view (`work_placement`)

- Where: placement (`service/work/structure.rs`), for a task no rule and no
  person has placed.
- A Choice over the person's groups ∪ `none` ∪ `unsure`; labels from
  `set_work_placement`. Jev never writes a rule.
- Built with the project picker's phase 2 (the same "which group" adapter).

#### Not for Jev (found by the same survey)

| Candidate | Why not |
|---|---|
| A local task's org (`assign_org`) | moves the org boundary behind a preview token |
| A mission's `complete` and `done_when` checks | evidence (CI, tests, commits), never an opinion |
| Decomposing a goal into tasks | generative; the planner's job |
| The harness for a task | low volume, no labels |
| A task's size or priority | numbers are a known weak spot; `work_items` has no priority |

### Weak candidates: not planned, with the reason

| Candidate | Why a rule or another tool wins |
|---|---|
| Org suggestion for a new repo | once per repo, at the org boundary; the rule plus a click is enough |
| Safe-kill READY check | must be code: `git status` plus unpushed commits (a bug to fix, not a decision) |
| Model choice for summaries | tiny volume; D27 stands |
| Sync, latency and cost anomalies | numbers; Jev is weak on numbers; statistics over `SyncMetrics` |
| Prompt-injection guard for mail and tracker text | Jev itself can be steered by adversarial content (TypeSafe's jaggedness list); never a security control |
| Operator tool routing | the operator is Claude behind `confirm_gate`; no measurable gain |

## 6. Batching, caching, early exit

- **Early exit [proposed]:**
  - an exact key → no call;
  - one candidate → only the Noul;
  - an unchanged fingerprint + candidate set + question version → no call.
  - Each of these is counted, so the share of cases where the rule wins is
    visible.
- **Batching [hypothesis]:**
  - J1 and J2 share the Stop context; one request with both questions only
    when both are on for that org.
  - Measure the latency and token delta against two calls. Parallel
    questions are free in Jev's pricing; the state is paid once.
- **Caching:** by `input_fp + candidates + question_version + model_version`.
  A model version change invalidates it.

## 7. Monitoring and automatic demotion (static routing, D41)

- The routing table is `(feature, nl bucket, code bucket, org) → jev |
  haiku | rule/person`.
- It is changed only by a reviewed commit.
- **Automatic changes only ever go down (conservative):**
  - correction rate over the card's bound on a rolling window → that cell
    to assist (or shadow);
  - fallback rate > 20% for an hour (breaker, 429, timeouts) → the feature
    shows "degraded" in `fleet_health`; answers fall back by themselves
    **[built]**: `fleet_health.decide` (`service::decide::health`: over at
    least 5 live attempts, or the breaker open; master and unbound clients
    only), the desktop's *Jev degraded* Attention item, `fleet-hub decide
    status`'s `health` line;
  - `model_version` differs from the one the cell was measured on → the cell
    to shadow;
  - daily token budget reached → fallback until midnight UTC.
- A person re-enables a demoted cell.

## 8. What is recorded, and what never is (D37)

- **Recorded:** `decision_runs` — feature, org, subject id, mode, provider,
  model version, question version, the HMAC fingerprint of the redacted
  input (a local key, so short inputs cannot be recovered by guessing),
  candidate ids, answer, probabilities, confidence, fallback, the baseline
  answer, latency, tokens, cost and the follow-up (confirmed, rejected,
  corrected-to, ignored).
- **Never recorded:** prompt text, titles, reply text.
- **Local benchmark captures:** raw text for a benchmark is only ever a local
  file the owner exports (`0600`, never overwritten, never committed).
- **Retention:** `decide.retention_days` (90). Counts reach `fleet-hub decide
  status` and never leave the machine.

## 9. Who owns what

| Component | Owns |
|---|---|
| hub `service/decide` | consent and flag checks, redaction, the call, validation, breaker, budget, recording, fallback |
| per-use-case adapters | question wording (versioned), candidate sets (after `OrgScope`), mapping answers back to today's structures |
| `fleet-hub census` / `decide bench` | offline measurement; never mutates |
| desktop Settings | the kill switch and the per-feature modes; per-org consent in Organisations |
| desktop and phone UI | showing a suggestion with its source, reason and confidence; collecting confirm, correct and reject — the phone never toggles hub settings |
| a person | every confirmation; any threshold change (a new decision row) |

## 10. What is still open

- **D32 is decided:** J3 first (built as a shadow/assist adapter), J1 offline only (phase 0 built; no live adapter until its acceptance lines pass).
- ~~**J2's consent for reply text**~~ **decided: D48** (owner, 2026-10-08;
  `docs/decisions.md` → *Decision D48*): reply text goes only where the org
  consents to reply text, a consent separate from D31 and required on top
  of it, off by default. J2 is built (step 5.11); its phase-0 run waits on
  ~300 hand-labeled pane tails.
- **The owner's census run and D46 labels:** they decide the language
  cells.
- **The owner's D39 hand labels:** needed for J1's "none" cases
  (`fleet-hub decide bench work-link --export-unlinked 150 --out FILE`, then
  `--labels FILE`).
- **The owner's J1 phase-0 run** on the real hub: recall first, then `bm25`,
  then `jev` for consenting orgs, and `haiku` on a named host
  (`--provider haiku --haiku-host ALIAS`) on the same cases.
- **Diagnostics built, waiting on the owner's runs:** robustness
  (`--perturb`, both cards), the paired languages (`--paired-fixture`, and
  `pair` ids in the owner's own rows), J3's floor sweep (`--floor-sweep`)
  and wording set (`--question-set`, dev only; three drafts in
  `service/testdata/decide/questions/`). The order is in `docs/decisions.md`
  → *How to run phase 0*, step 4.
- **The owner's J3 phase-0 run:** spot-check the synthetic section labels
  (D43; `service/testdata/decide/status_map_sections.jsonl`), add the real
  boards' sections, then `fleet-hub decide bench status-map --labels FILE
  --provider rule --provider jev --provider haiku --haiku-host ALIAS`. The order of the whole phase is the
  checklist in `docs/decisions.md` → *How to run phase 0*.
