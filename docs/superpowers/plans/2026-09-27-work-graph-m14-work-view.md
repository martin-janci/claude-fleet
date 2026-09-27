# Work graph M14: the Work view, landed (plan)

**Date:** 2026-09-27
**Roadmap:** `../2026-09-24-work-graph-roadmap.md`. M14 is new on `main`; M14.0 adds it there.
**Design (binding):** `../specs/2026-09-27-work-view-design.md`. It lives on the branch `claude/fleet-dynamic-work-view-kwc3r9`, and M14.1 brings it to `main`.
**Depends on:**
- claude-fleet: M0–M13 on `main` (`f10d0b92`: M13.0–M13.2, M13.4a/c/d/e, the review fixes #331 and #332, and #334's tests).
- fleet-mobile: `main` (`8ca9afa`, with #50 and #51).
**Input:** the Work view backend commit `defe19c3` on that branch (69 files, +8,590 lines, one commit).

## Goal

> The work graph's data is complete, but you can only walk it one way: host → project → session → its tasks.
> M14 adds the other direction, the **Work view**:
>
> org → project / group → task → *every* session of that task.
>
> This includes secondary, suggested and past links, and tasks with no session at all. It is one contract read by the desktop and the phone under one set of permissions.
>
> M14 does not redesign it: the design spec exists, and a backend for it has already been built. M14 **lands** it. The pieces are cut into reviewable PRs, rebased on today's `main`, and brought under the same bars as M5–M13: isolation matrix, budget, verdicts, conformance, acceptance. Then the desktop and phone UIs are built on top.

**Value line:**
- A task with three sessions on two hosts is one row that shows all three.
- A task with no session is visible, and can be started from where it is shown.
- Suggestions and conflicts are decided in one Review inbox. Decisions can be done in bulk and undone.
- A phone paired **to one org** sees only that org. This is a new security boundary, and it is proved like M5's.
- Two devices starting the same ticket at once give one session, not two.

## Facts this plan builds on (verified 2026-09-27 at `main` `f10d0b92`)

**The backend branch is stale against `main`:**
- `claude/fleet-dynamic-work-view-kwc3r9` is based on `be0e2bc5` (#327) and is **36 commits behind `main`**.
- Its migration is `063_work_view.sql`, but `main` already has:
  - 061 `tracker_writes`
  - 062 `tracker_webhooks`
  - 063 `row_version_on_visible_change` (#333)
  - 064 `drop_tracker_webhooks` (#330)

  The next free number is **065**.
- It raises `BUDGET_BYTES` to 59,114; `main` has 57,050. The budget must be measured again after the rebase.
- It keeps `CONTRACT_REVISION` at 4. That is still true after the rebase and must stay true: there is no new MCP tool, only new actions.

**The branch's claims vs its content:**
- The roadmap text on the branch says M14 is "built" and marks D31–D35 as "default, built".
- The commit has **no Svelte, TS UI or Kotlin files**. It carries:
  - the backend: `work { tree | task | session_tasks | review | rules | rule_preview | views | org_impact }`, ten new `work_link` actions with compare-and-set, `store/work_view.rs`, `service/work/view.rs` and `structure.rs`;
  - org-bound pairing (`fleet-hub pair --org`);
  - the `work:changed` event;
  - 18 routed Tauri commands;
  - the verdicts, the isolation rows (+614 lines) and a scale test.

  The desktop Work view and the phone's *My work* tab do not exist yet.
- Its acceptance section is **Part R, steps 60–72**. The spec says **Part P**, but on `main` Part P is GHES. Part R is the right one.

**A real bug the spec found, still on `main`:** `work_link start` re-checks only after the SSH spawn. When two devices start the same ticket at once, the loser's session stays up, unlinked. `resume` already avoids this with an in-flight registry.

**Other things on `main` that M14 meets:**
- `work.md` / docs debt from M13:
  - the roadmap's M13 status block is stale (M13.1 and M13.2 are landed);
  - the M9 line still says "D3 none, D10 off";
  - `CLAUDE.md` says "M13 is next" and places M13.4c/e on a branch;
  - `docs/work-graph.md:305` still says multi-start is desktop-only;
  - the acceptance headings for D3 and D10 still say "decided against".
- **M13 is not closed.** Three items remain open:
  - M13.3 waits on the owner's acceptance run (0 of 59 results filled);
  - M13.4b (D5) waits on the remote SessionStart numbers;
  - M13.5, the close-out, is not done.
- **D26 conflicts with M14.** D26 says: after M13 the work graph is *operating*, and new work arrives as issues or small plans, not milestones. M14 is a milestone by the owner's choice; D36 records that.
- **Migration numbers are claimed twice.** The live-instance plans (`2026-09-27-live-instance-fixes-README.md`) reserved 061–065. Those reservations are already overtaken by `main`. M14 and plans A–D will both want 065 or later.

## Design decisions

1. **Land, don't rebuild.** The branch's backend is the starting point. Workers rebase and split it; they do not rewrite it. Any deviation from the spec is written in the PR and in the spec's *Revisions*.
2. **Cut `defe19c3` into reviewable PRs.** One PR of 8,590 lines that adds a security boundary cannot be reviewed. The cut follows the spec's stages:
   - the start race fix, alone;
   - migration + reads + org-bound scope;
   - mutations and compare-and-set;
   - the Tauri commands and events.

   Each PR must be green alone.
3. **Migration numbers are taken at merge time.**
   - Whoever merges second renumbers their file to the next free number, and does it in a merge commit.
   - M14's is written as `0NN_work_view.sql` in the plan and becomes 065 or later.
   - The live-instance README gets one line saying its reservations are void.
4. **The org-bound client is a boundary, proved like M5's.**
   - `OrgScope::Org` is made only by `Caller::org_scope`.
   - Every new action gets a row in the isolation matrix, for three callers: master, a per-host token, and an org-bound client token.
   - `call_tool` redaction covers the new reads.
   - The PR carries a written threat note: what a bound client can see, change and infer.
5. **No new tool, no contract bump.** All reads are `work { … }` actions and all writes are `work_link { … }` actions. `CONTRACT_REVISION` stays 4. Older hubs and older phones keep working: the new actions are simply absent, and the UI hides what the hub does not list.
6. **UI waits for its contract.**
   - The desktop and phone read UIs start once the read PR is on `main`.
   - The edit UIs start once the mutation PR is on `main`.
   - Both clients read the same actions; neither gets a private path.
7. **M13's open items stay M13's.** M14 does not absorb M13.3, M13.4b or M13.5. They keep waiting on the owner, and M14 does not block them.

## Tasks

### M14.0: this plan, the spec on `main`, the truth pass (docs)
- Bring `docs/superpowers/specs/2026-09-27-work-view-design.md` to `main` from the branch, with fixes:
  - acceptance section Part **R**;
  - migration `0NN`;
  - base `f10d0b92`.
- Add **M14** to the roadmap. Add D31–D36 to the decisions table as **proposed defaults, not built**, waiting on the owner (see *Decisions*).
- Fix the M13 truth debt listed under *Facts*: the roadmap's M13 status, the M9 line, CLAUDE.md, `work-graph.md:305` and the acceptance headings.
- Add one line to the live-instance README saying its migration reservations are void (design decision 3).
- Docs only. **Done** when the roadmap, CLAUDE.md and the guide match `main`.

### M14.1: land the backend (claude-fleet, four PRs, in this order)
- **M14.1a: the `start` race fix.**
  - `work_link start` claims the ticket key in the in-flight registry `resume` uses, before the SSH spawn.
  - A test starts the same key twice concurrently and asserts one session and one refusal (`E_BUSY` or the existing busy code).
  - Small, independent of everything else, first.
- **M14.1b: migration + read contract + org-bound clients.**
  - Migration `0NN_work_view.sql`, the additive part only.
  - `work { tree, task, session_tasks, review, rules, rule_preview, views, org_impact }`.
  - Keyset pagination.
  - `fleet-hub pair --org` and `OrgScope::Org`.
  - The isolation rows for every new read.
  - The scale test: its p95 bound is kept from the branch and re-measured on `main`.
  - `BUDGET_BYTES` re-measured, and the reference regenerated.
- **M14.1c: mutations.**
  - Per-link `version` and `expected_*` parameters with `E_CONFLICT`.
  - `link` / `confirm` `{primary: false}`, `set_primary`, `reconsider`, `ack`, `decide_batch` (with per-item results), `place`, `assign_org` (with the impact preview), and rules and views CRUD.
  - Isolation rows for every write, for three callers.
  - A cross-org write needs `force_cross_org` for every caller, as today.
  - The M9.7 confirm rule applies to the operator's starts and kills, unchanged.
- **M14.1d: desktop commands and events.**
  - The routed Tauri commands, each with its `verdicts.rs` row, and `REGEN_HUB_VERDICTS`.
  - The `work:changed` event.
  - A lagged or not-resumed stream leads to a full reload, as the existing `ready.resumed` / `lagged` does.
- Each PR:
  - rebases the branch's code onto `main`;
  - passes `cargo fmt` / `clippy` / `test`, `pnpm check` / `test`, and the hub e2e W leg;
  - carries its part of the spec's *Contracts* section in its body.

### M14.2: the desktop Work view, read (after M14.1b and d)
- The sidebar switch `Sessions | Work` (⌘⇧W / Ctrl+Shift+W).
- The work tree:
  - org → group → task → session occurrences;
  - primary ★, secondary, suggested (dashed), past (dimmed).
- Explicit loading, empty and error states. Paging with *Load more*.
- Filters, and saved views (read).
- Task detail in the center pane, including *Open*, *Continue* and *Start new*.
- In session detail, a *Tasks* section with *Show in Work view*.
- Vitest for the tree building and the store patching on `work:changed`.
- Optimistic merge follows the existing `mergeOne` / `removeOne` rules. No re-fetch storm.

### M14.3: the desktop Work view, edits and Review (after M14.1c)
- *Make primary*, *Remove* and *Add task…* in session detail.
- *Place in group…*, *Assign org…* (with the impact dialog) and *Make a rule…* (with the preview) in task detail.
- The **Review** tab:
  - suggestions and conflicts;
  - multi-select with a count;
  - per-item results;
  - *Undo* for the last decision.
- On `E_CONFLICT`: show the current value with *Reload*, never silently overwrite.

### M14.4: the phone (fleet-mobile, after M14.1b; edits after M14.1c)
- The **My work** tab:
  - saved views as chips;
  - the filter sheet;
  - org → group sections;
  - task cards;
  - *Load more*;
  - the offline banner.
- The task screen and the session screen's *Tasks* section, with the Review sheet.
- Gating:
  - full token for writes;
  - the hub must list the action, as with #50 and #51;
  - no offline queue;
  - a write is shown as saved only after the hub's answer.
- A phone paired with `--org` is tested against a hub fixture: it sees only its org. The source scan (`ToolsTheAppMayCallTest`) is updated for the new actions.
- Two PRs: read, then edits.

### M14.5: acceptance, guide, close-out
- `docs/work-graph-acceptance.md` **Part R** (steps 60–72, from the branch, checked against what was built).
- `docs/work-graph.md` → *The Work view*.
- CLAUDE.md gets one paragraph.
- The hub e2e W leg gets one `work { tree }` and one `set_primary` round trip, so CI covers the new contract end to end.
- The roadmap marks M14 done. D31–D35 read the owner's answers.

## Acceptance (manual)
Part R of `docs/work-graph-acceptance.md`, run by the owner on:
- a real hub;
- a desktop paired with it;
- a phone paired twice, once unbound and once with `--org`.

## Risks
- **Scope creep from the branch.**
  - The branch already holds more than the spec's stages, and its roadmap text says "built".
  - Mitigation: the M14.1 cut. Anything in the branch that no PR above names is left out and listed in M14.0's *Revisions*.
- **Security regression in the org boundary.**
  - An org-bound client is the first paired client that is *not* `All`.
  - Mitigation: design decision 4, plus a security review (`/security-review`) on M14.1b and c before they are opened.
- **Budget.**
  - Eight reads and ten writes add ~2 KB to the master surface.
  - Mitigation: descriptions stay short and derive from enums (as in M4), and the budget is re-measured in each PR, not once at the end.
- **Two lines of work.**
  - M13 ran in two sessions at once, which produced duplicate PRs (#49/#51, the M13.4c/e branches vs #327).
  - Mitigation: M14 has one driver (D36). A worker never starts a stage whose branch or PR already exists; it checks the remote branches first.
- **Migration collisions** with the live-instance plans A–D. Mitigation: design decision 3.

## Decisions (the owner's; defaults if unanswered)

| # | Question | Default |
|---|---|---|
| D31 | May an org-bound client see *unassigned* work and sessions? | Yes, as a host does. To fence everything, assign every host and tracker to an org. |
| D32 | Does a forced cross-org link raise a review item until it is acknowledged? | Yes (`cross_org` review kind, cleared by `ack`). |
| D33 | May a full, unbound phone change a local task's org? | Yes, with the impact preview. Bound clients and hosts may not. |
| D34 | Placement rules only, or also link rules? | Placement only. Link rules would bypass detection's evidence and R9. |
| D35 | Saved views: shared on the hub, or per device? | Shared on the hub. A bound client's views are its org's. |
| D36 | M14 as a milestone, despite D26 ("operating: issues, not milestones")? And who drives it? | Yes, M14 is the last work-graph milestone; D26 applies after it. One driver session, named in M14.0. |

D31–D35 come from the spec. The branch marks them "built", but on `main` they are **proposed** until the owner answers.

## Order and parallelism

```
M14.0 ──► M14.1a ──► M14.1b ──► M14.1c ──► M14.1d
                        │           │
                        ├─► M14.2 (desktop read) ─► M14.3 (desktop edits)
                        └─► M14.4 read (phone) ──► M14.4 edits
                                                        │
                                        all ──► M14.5 (acceptance, guide)
```
- M14.1a can go at once. It is a bug fix on `main` today.
- M14.2 and the phone read PR can run in parallel: different repos, same contract.
- M13.3, M13.4b and M13.5 are independent of all of this.

## Revisions
- 2026-09-27: written at `main` `f10d0b92`, from the spec and the backend commit on `claude/fleet-dynamic-work-view-kwc3r9` (`defe19c3`).
