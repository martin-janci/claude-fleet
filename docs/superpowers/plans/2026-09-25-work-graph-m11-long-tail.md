# Work graph M11: the long tail (plan)

**Date:** 2026-09-25
**Roadmap:** `../2026-09-24-work-graph-roadmap.md` (M11 is new; add it there when this plan lands)
**Design:** `../specs/2026-09-24-work-graph-design.md` §0
**Depends on (landed on `main`):**
- M0–M9;
- M10.1 (#279), M10.4 (#278), M10.5 (fleet-mobile #36) and M10.6 (#280);
- M4.6 (#273).

**Runs alongside:**
- M10.2, the work-graph e2e on `claude/cloud-fleet-work-graph-m10-e2e`. M11 items should add a scenario there when it lands.
- M10.3, the written acceptance, which is still to write.

## Goal

> Each milestone M0–M9 ended with a "Not done" line. M10 closed the ones
> it named. M11 takes what is left on those lines. For every item it
> either builds it, or records a decision that it will not be built. The
> result is a roadmap in which every "not done" is either done or
> decided.

**Value line:**
- The operator can name a piece of work that has no ticket ("Name this work…").
- Resume says *before* it runs that a transcript is gone.
- Tidy-up also offers the idle sessions that were never linked to anything.
- A GitHub Enterprise tracker works through `gh`.
- The tool descriptions give back the headroom M0.6 promised.

## Facts this plan builds on (verified 2026-09-25 at `main` d11025c)

**Open "not done" items, per milestone status in the roadmap**

| Milestone | Item | What exists now |
|---|---|---|
| M0 | **M0.6** tool-description budget: pay back the headroom before the work tools land | Never done. `BUDGET_BYTES` has grown from 64,832 (pre-M1) to **71,658** (`mcp/tools/tests.rs:3273`). |
| M1 | **"Name this work…"** on a group header: a local item with a title | `Store::create_local_work_item(key, title)` exists and is used only by tests and hooks. No MCP action and no Tauri command expose it. |
| M2 | **Resume does not probe whether the transcript file still exists** on the host | `service/work/resume.rs` only checks the purge flag, reachability and the held conversation (lines ~320–331). |
| M6 | **GitHub Enterprise Server**, `acli`, per-provider metrics | The GitHub provider goes through `gh` on a host and assumes github.com. There is no `--hostname`. There are no per-provider sync metrics in `work_admin`. |
| M7 | **Tidy reason `idle_unlinked`** | Not built. An unlinked session has no link to archive under. |
| M10.6 | **First sync of a new tracker floods the ring** (reach 155 s) | The report recommends accepting it. The alternative, suppressing per-item frames on the first pass, is a design change for the user to decide. |
| M4.5 | **D5**, SessionStart context on by default | Still open. It waits for the remote measurement (`scripts/measure-session-start.sh`, M10.4). |

**Constraints that stay**
- No new MCP tool. Add actions to `work` / `work_link` / `work_admin`; the action enum is generated from the parser tables (M8.0).
- Every new action needs an isolation-matrix row (`mcp/tools/tests_isolation.rs`).
- No `CONTRACT_REVISION` bump: phones use `MAX_HUB_CONTRACT = 4`. New wire fields get `#[serde(default)]`.
- Every Tauri command gets a `verdicts.rs` row plus `REGEN_HUB_VERDICTS=1`.
- `mark_untrusted` / `fence_untrusted` apply to any third-party text.
- The operator's starts and kills are always confirmed (M9.7).

## Design decisions

1. **"Decided against" counts as done.** Every item below ends as either *built* or *decided against (Dn)*. Nothing stays open without a decision.
2. **Probes are cheap and host-side.** The transcript probe uses the existing per-host SSH ControlMaster path: one `test -f`, quoted with `shell::quote`, with a short timeout. It is never run on the list path, only when a resume is planned.
3. **`idle_unlinked` only suggests.** It never archives, because there is no link to archive. It offers the M7 *safe kill* (confirm-gated) or "keep" (snooze per session). The M7 hard-coded protections apply unchanged.
4. **The M0.6 payback is measured, not guessed.** Shorten the descriptions, measure with the existing budget test, and lower `BUDGET_BYTES` to the new measurement plus 100 B headroom. Behaviour must not change, and the generated reference is regenerated.

## Tasks

### M11.0: this plan
Commit this file. Add M11 to the roadmap's milestones, critical path and *Revisions*.

### M11.1: "Name this work…" (local work items)
- **Hub action:** `work_link { action: name, session_id | link_id, title, key? }` creates a local item with `create_local_work_item` and links the session to it (manual, confirmed).
  - The title and key are validated: length caps, no control characters, and `canonical_key` for a key.
  - A per-host token may name work only for its own host's sessions, inside its org.
  - Add an isolation-matrix row.
- **Rename:** `work { action: local_items }` lists local items. Renaming one is `work_link { action: name, item_id, title }`.
- **UI:**
  - "Name this work…" on a group-by-work header for sessions that have no work (and on a row's `#` menu).
  - A small dialog with an optional key.
  - A Routed Tauri command and a verdict row.
- **Phone:** out of scope, consistent with D15 (the phone stays read-only).
- **Tests:** store, service, the isolation row, Vitest for the dialog and the menu.

### M11.2: Resume probes the transcript
- In `resume.rs` planning, when *continue* is chosen and the host is reachable, run one host-side `test -f` on the transcript path the conversation row records.
  - The path is built only from stored, validated components and quoted.
  - If the file is missing, the plan falls back to "start fresh with the brief", and the reason reads "the conversation's transcript is no longer on {host}".
- The probe is skipped for a hub-routed call when the host is unreachable, so the existing "unreachable" reason still wins.
- **Tests:** FakeSsh with the file present, absent, and a timed-out probe (treated as unknown, which keeps *continue* but adds a warning).

### M11.3: Tidy reason `idle_unlinked`
- **Planner:** `service/gc/tidy.rs` gets a new reason, `idle_unlinked`: a live Claude session with no live or suggested link, idle longer than `work.tidy_idle_unlinked_days` (default 7), clean worktree, not the operator's session, and not protected.
- **Actions:** safe kill (confirm-gated, as `tidy_apply` already is), or "keep for N days" (a per-session snooze, stored in the timeline, with no new column if avoidable; if a column is needed, it is migration 057).
- **Auto-tidy:** never applies `idle_unlinked`, even when `work.auto_tidy` is on (decision D19 default).
- **Scope:** a per-host token sees only its host's and org's candidates, as for M7.
- **Tests:** planner table rows, isolation, UI rows in the Tidy-up sheet.

### M11.4: GitHub Enterprise Server and sync metrics
- **GHES:**
  - The GitHub tracker gains a `hostname` setting in `trackers.settings` (M6 migration 051).
  - `gh` is called with `--hostname <h>`. The value is validated as a hostname, admin-fenced like Jira DC, and passed through `shell::quote`.
  - Keys become `host/owner/repo#n` only when the host is not github.com. The recogniser (Rust and TS, shared fixture) learns this form.
- **Metrics:**
  - `work_admin { action: status }` gains per-tracker counters for the last pass: duration, items listed, items changed, frames emitted, and the last error.
  - They are kept in memory (reset on restart) and shown in Settings → Work.
- **`acli`:** decided against by default (D17): `gh` and the REST providers cover the use.
- **Tests:** conformance suite for the GHES transport (`conformance_suite!`), provider isolation, recogniser fixture rows.

### M11.5: Pay back the tool budget (M0.6)
- Tighten the descriptions of `work`, `work_link` and `work_admin`, and of the older tools where the text repeats what the schema already says.
  - Measure before and after with the budget test.
  - Lower `BUDGET_BYTES` to the new measurement plus 100 B.
  - Regenerate `docs/control-api-reference.md` (`REGEN_DOCS=1`).
- No behaviour change. The action enum and parameter names stay the same, because phones and scripts depend on them.
- Record the recovered bytes in the roadmap's *Revisions* and mark M0.6 done.

### M11.6: Close the recorded decisions
- **First-sync ring flood:** record D18. The default is *accept*, as the M10.6 report recommends. If the user picks *suppress*, it is a separate task: check the phone's reducer first.
- **D5:** when the user has run `scripts/measure-session-start.sh`, put the numbers into the M4 plan and decide D5. Until then it stays open. This is the one decision M11 cannot close alone.
- **Roadmap:** every milestone's "Not done" line is rewritten to *done*, *decided against (Dn)* or *waits on the user (Dn)*.

## Acceptance (manual)
Add these to M10.3's acceptance document:
- name a piece of work from a group header;
- resume a conversation whose transcript was deleted on the host;
- see an idle unlinked session in Tidy-up and keep it;
- connect a GHES repository.

## Risks
- **The transcript probe adds SSH latency to resume planning.** It is one command on an existing ControlMaster, with a 3 s cap, and only on the resume path.
- **An `idle_unlinked` false positive.** A session a person is actively using, but without a link, must never be offered. The planner requires both idle time and no recent prompt. Protections come first.
- **The M11.5 rewording drifts from behaviour.** Descriptions come from the enums (pane_intel, action tables) where possible, and the reference test catches drift.
- **GHES hostname as an SSRF vector.** `gh` runs on the user's host with the user's `gh` auth, but the hostname is still admin-fenced and validated. This matches the Jira DC treatment.

## Decisions (defaults if unanswered)

| # | Question | Options | Default |
|---|---|---|---|
| D17 | Support `acli` (Atlassian CLI) as a Jira transport? | yes · decided against | decided against: REST covers it |
| D18 | First sync of a new tracker: suppress per-item frames? | accept the flood · suppress (design change) | accept (M10.6 report) |
| D19 | May auto-tidy ever act on `idle_unlinked`? | never · per org | never |
| D20 | Local work items on the phone (name / rename)? | no (read-only phone) · yes | no, consistent with D15 |

## Order and parallelism
```
M11.1 ─┐
M11.2 ─┤ independent of each other; each adds an e2e scenario once M10.2 lands
M11.3 ─┤
M11.4 ─┘
M11.5    last: it rewrites descriptions that M11.1/M11.3/M11.4 touch
M11.6    docs; any time, finalised after M11.1–M11.5
```

## Revisions
- 2026-09-25: first version.
- 2026-09-25: M11.5 built on `claude/cloud-fleet-work-graph-m11-budget`. The master surface measured 71,590 B before and 54,646 B after (16,944 B paid back, M0.6); `BUDGET_BYTES` is 54,746. Only description and parameter-doc wording changed: no tool, action or parameter renamed, no schema shape changed, and every confirm gate, untrusted marker, host fence and "never" clause kept. The `work` / `work_link` / `work_admin` texts, already cut in M5.1 and M8.0, were left as they are so M11.1 / M11.3 / M11.4 merge cleanly. The final `BUDGET_BYTES` must be re-measured once the M11.1, M11.3 and M11.4 branches land: whichever lands second merges and re-measures. The roadmap's *Revisions* and M0.6 status wait for the M11.6 docs pass.
- 2026-09-25: M11.2 built. The probe checks the path a conversation row recorded (when it still validates), then `"$HOME"/.claude/projects/*/<uuid>.jsonl`, since a conversation row is gone once its session is (cascade) and the cwd slug is not always derivable; "absent" means absent from both. It runs in `resume_plan` when `last` is possible, and in `resume` only for mode `last`; an unknown answer adds `ResumePlan.warnings` (new, `#[serde(default)]`), shown in the Resume dialog.
- 2026-09-25: M11.4 built (branch `claude/cloud-fleet-work-graph-m11-ghes`), no migration. GHES: `trackers.settings.hostname` (admin-fenced DNS name, optional port; `gh --hostname`, only `https://<host>/api/graphql`); keys `host/owner/repo#n`, recognised only for configured GitHub trackers' hosts; R3u is per GitHub instance. Metrics: `work_admin { action: status }`, in memory; Settings → Work, `fleet-hub tracker status`; new LocalOnly command `tracker_sync_metrics` (174 commands).
- 2026-09-25: M11.3 built (`claude/cloud-fleet-work-graph-m11-idle-unlinked`). No migration: "keep" is a `tidy_kept` timeline event (latest wins, ignored before the row's `created_at`), written by a new `tidy_apply` item action `keep` (1–90 days). `idle_unlinked` needs a work session with its own, unshared tracked worktree, no live link but rejections, `idle_since` and every prompt / attach / turn / creation stamp older than `work.tidy_idle_unlinked_days` (default 7, 1..=90). It is never preselected in the sheet. Its kill is refused, not safe-killed, unless the worktree inspects clean and pushed, and only while the fresh plan still names it. D19 is enforced in the planner (`TidyReason::auto_allowed`), in `auto_selection` and in the executor. No new tool, no contract bump; +5 B on the tool surface (54,939 B merged over M11.4, `BUDGET_BYTES` 55,039).
