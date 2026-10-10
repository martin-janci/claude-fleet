# Sprints, releases and epics in the native work graph — design

**Date:** 2026-09-28
**Status:** phase 1 (status of native items) landed; the backend of phases 2–3
(`work_buckets`, sprints, releases, provider mapping, adoption, the MCP
actions) landed with migration 108. The board (phase 5) is built: columns
by status over the Work view's filters, scoped to one sprint or to the
backlog with no sprint. The Work view axis (§6a), bulk assignment (§6b) and the
sprint lifecycle dialog (§5, E9) are built on the desktop. Epics (phase
4) are built with a depth of ONE, not §3's three: a local item with a
parent is never a parent, because `item_org` (and every org fence on it)
walks one level; deeper needs that walk changed first.
**Builds on:** `2026-09-24-work-graph-design.md` (§0 is authoritative) and the
work-graph roadmap's decisions D1–D36. This document is a delta against §0: it
adds planning structure to the native side of the work graph and does not
change how detection, linking or trackers behave.

## The goal

Fleet's own task system should carry **sprints, releases and epics**, so a
person can plan in fleet whether or not the work also lives in Jira — and so a
project with no tracker at all is not a second-class citizen. A tracker that
has these concepts can be linked to them; a tracker that does not keeps
working unchanged.

Today none of this exists natively:

| Concept | State on `main` (2026-09-28) |
|---|---|
| Sprint | `WorkItemSnapshot.iteration: Option<String>` + `iteration_active: bool` — a flat string on the item, read-only, tracker-only. No entity, no lifecycle; a local item cannot have one. |
| Epic | `work_items.parent_id` + `hierarchy_level`, written only from a tracker's `parent_external_id`. A local item can never be a parent. |
| Release | Nothing. Neither Jira `fixVersions` nor GitHub milestones are synced. |
| Status of local work | `work_items.status_category` is written **only** by `store::tracker_items`. A local item is created `'todo'` and can never leave it. |

`Caps.iterations` is `true` for Jira Cloud, Jira DC and Linear, and **`false`
for GitHub and Asana** — so degradation covers two of five providers, not an
edge case.

## 0. Decisions taken by the owner (2026-09-28)

| # | Question | Answer |
|---|---|---|
| E1 | Source of truth when Jira also has sprints | **Native owns.** A tracker's sprint/epic/version is observed and may be *adopted*, never authoritative. Keeps "trackers enrich, never gate" and works with no tracker. |
| E2 | What is an epic | **An item with children.** No new entity: a local item may have `parent_id` and `kind = 'epic'`, so an epic has a status, sessions, a branch and a PR — because it is work. Matches how tracker epics already arrive. |
| E3 | What is a release | **A planning target**, with a `shipped_ref` column present from day one but never read automatically. Git-tag observation is explicitly out of scope. |
| E4 | Relationship to `work_placements.group_label` | **Orthogonal axes.** Sprint = *when*, release = *which version*, `group_label` = anything else. Placements and `work_rules` are untouched. |
| E5 | UX shape | Grouping axis in the Work view, **plus** bulk assignment from a multi-select, **plus** a sprint board. The board earns its place as the substitute for a tracker's board when there is no tracker. |
| E6 | Status of a local item | **Derived from evidence, a person's override wins.** Live session → `in_progress`, merged PR → `done`, else `todo`; an explicit setting is final, exactly as resolver rule R1 treats a person's link decision. |
| E7 | Scope of a sprint | **An organisation, across all its repos.** A team's sprint routinely spans repositories, and the existing org fence then applies unchanged. |

## 1. Domain model

One table for sprints and releases: they share their whole shape, and `kind`
discriminates. Migration number: the next free one (main is at `074`).

```sql
CREATE TABLE IF NOT EXISTS work_buckets (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT    NOT NULL,              -- sprint | release
  name        TEXT    NOT NULL,
  org_id      INTEGER REFERENCES orgs(id) ON DELETE SET NULL,
  state       TEXT    NOT NULL,              -- sprint: planned|active|closed
                                             -- release: planned|released
  starts_at   INTEGER,                       -- sprint
  ends_at     INTEGER,                       -- sprint: end; release: target date
  shipped_at  INTEGER,                       -- release
  shipped_ref TEXT,                          -- release: a tag or URL, written by hand (E3)
  goal        TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  version     INTEGER NOT NULL DEFAULT 1     -- expected_version concurrency, as work_placements
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_buckets_name
  ON work_buckets(kind, COALESCE(org_id, 0), name);
```

Membership is N:M with history, not a column on the item — Jira's
`fixVersions` is a list, and an issue's sprint history is several sprints:

```sql
CREATE TABLE IF NOT EXISTS work_bucket_items (
  bucket_id  INTEGER NOT NULL REFERENCES work_buckets(id) ON DELETE CASCADE,
  item_id    INTEGER NOT NULL REFERENCES work_items(id)   ON DELETE CASCADE,
  source     TEXT    NOT NULL,          -- manual | adopted
  added_at   INTEGER NOT NULL,
  removed_at INTEGER,                   -- carry-over is recorded, not deleted
  PRIMARY KEY (bucket_id, item_id)
);
```

`removed_at` rather than a delete, so "this did not finish in sprint 23"
survives as a fact. `source` is what makes adoption withdrawable without
touching a person's decision (§5).

**Invariant:** an item has at most **one** current sprint
(`removed_at IS NULL AND kind = 'sprint'`), and may have several current
releases, because Jira permits several `fixVersions`. Enforced in
`store::work_buckets`, with a test, not by a trigger — the rule needs a
message a caller can act on.

A native bucket may be linked to a tracker's sprint or version:

```sql
CREATE TABLE IF NOT EXISTS work_bucket_refs (
  bucket_id     INTEGER NOT NULL REFERENCES work_buckets(id) ON DELETE CASCADE,
  tracker_id    INTEGER NOT NULL REFERENCES trackers(id)     ON DELETE CASCADE,
  external_id   TEXT    NOT NULL,
  external_name TEXT,
  last_seen_at  INTEGER,
  PRIMARY KEY (bucket_id, tracker_id, external_id)
);
```

The native bucket keeps its own name, dates and state. The ref exists so that
(a) synced items can be placed into the bucket automatically and (b) the UI can
say "Jira: Sprint 24" without pretending Jira owns it.

## 2. Status of a native item

`status_category` stays the stored column and keeps its three values
(`todo | in_progress | done`) — one vocabulary for tracker and native work, so
no view needs to know which it is looking at.

Two new columns:

```sql
ALTER TABLE work_items ADD COLUMN status_set_by TEXT;    -- NULL | 'person' | 'derived'
ALTER TABLE work_items ADD COLUMN status_set_at INTEGER;
```

Precedence, deliberately the same shape as resolver rule R1:

1. `status_set_by = 'person'` → that status is final. Nothing derives over it.
2. `status_set_by = 'derived'` → a stamped `done` (below).
3. Otherwise, live: a confirmed link whose session is working → `in_progress`.
4. Else `todo`.

**Why `done` is stamped and `in_progress` is not.** The merged-PR signal lives
on the **session** (`sessions.pr_signals`, `state: OPEN|CLOSED|MERGED`, read
today by `PrSignals::is_merged`). When that session is retired and swept, the
signal is gone — so a purely computed `done` would silently revert to `todo`
once the work's session disappeared, which is the worst possible behaviour for
the one status a release depends on. `done` is therefore **written once**, with
`status_set_by = 'derived'`, and it survives the session.

**Where it is written.** At the point the merged fact is recorded —
`Store::set_pr_signals`, which the reconcile pass calls whenever the PR probe
reads `MERGED`. Not (only) the tick that computes `pr_merged_idle` for tidy-up:
that tick is `service::work::tidy::auto_tidy`, which returns before it reads the
fleet unless `work.auto_tidy` is on — and that is off by default (D2). A stamp
living only there would be lost for good by the ordinary sequence "merge the PR,
then kill the session", because the session's `pr_signals` go with its row.
`Store::tidy_sessions` stamps too, because it is free there and a person opening
Tidy-up should see delivered work as `done` in that same answer; the write is
idempotent, so two sites are harmless. Both go through
`Store::stamp_derived_done_for_session`, so neither can drift from the other: it
stamps the session's live, confirmed, **primary** link's item — the work the
session is *on*, not everything it is attached to (a merged PR says less about a
secondary link, an epic or a ticket the session also touched). That narrows the
reach; it does not make the stamp selective about the kind of item, so an epic
that is itself the primary work is stamped like any other.

And **only when the stored signal changed.** A stale merged signal must never
stamp work the session is pointed at later: pick different work on a session
still sitting on its merged branch and the next probe would otherwise mark the
new item `done` for good. The case the change-gate cannot cover — a link
confirmed after the merge was first seen, since `resolve_session` runs after
`set_pr_signals` — is covered by a second call from the reconcile pass, once
that same changed signal's links have settled.

`in_progress` stays computed in the read path, because it is transient by
nature: it means "a session is working on this right now", and there is nothing
to preserve after that stops.

A stamped `done` is not undone by new work: if a session starts on a done item,
the row keeps saying `done` and a person may set it back. Flip-flopping on
resumption would make the status untrustworthy for exactly the reader that
matters — a release asking what shipped.

**Who may be overridden.** The override applies to `source = 'local'` items
only. For a tracker item the tracker owns `status_category` — `store::
tracker_items` writes it on every sync, so a person's setting would be silently
reverted on the next pass, which is worse than refusing it. A `set_status` on a
tracker item is therefore `E_INVALID` with a message naming the ticket: change
it in the tracker, or link the work to a local item. Whether tracker items
should gain a *separate* fleet-local status is E11, and its default is no.

`blocked` is deliberately **not** a status. It is a property of a session
(`claude_status = 'blocked'`) that the row already shows, and adding it to the
item's vocabulary would create a second state machine for the same word.

**The stamp must not widen auto-tidy.** Tidy classifies an idle session by a
ranked reason (`TidyReason::RANKED` puts `DoneIdle` above `PrMergedIdle`), and
which reasons may act automatically is a user's list
(`work.auto_tidy_reasons`, default both). Once a merged PR stamps an item
`done`, a session that used to classify as `PrMergedIdle` classifies as
`DoneIdle` instead — so someone who allowed `done_idle` and deliberately
*excluded* `pr_merged_idle` would silently start getting sessions safe-killed
on a merged PR, which is the trigger they opted out of.

Therefore: when the `done` came from the stamp (`status_set_by = 'derived'`),
tidy classifies the session as `PrMergedIdle`, not `DoneIdle`. The status stays
truthful and the reason stays faithful to its origin, so a reason allow-list
keeps the meaning its author chose. A person's `done` keeps classifying as
`DoneIdle`, because that is what it is.

`PrMergedIdle` is therefore decided by the **stamp**, not by the live
`pr_signals`. It has to be: the signal is deleted with its session row, so a
rule that needed it would leave a stamped `done` matching neither reason —
`DoneIdle` is refused it and `PrMergedIdle` would no longer see the merge — and
the session would linger in tidy-up forever. The stamp is the durable half of
the same fact.

## 3. Epics

No new entity (E2). Two changes:

- `work_items.parent_id` becomes settable for `source = 'local'` items. Today
  it is written only from a tracker's `parent_external_id`.
- `work_items.kind` accepts the native value `'epic'`.

Constraints: a parent must be visible in the caller's org scope; a cycle is
refused (`E_INVALID`); depth is capped at 3 levels, so a roll-up read never
walks an unbounded tree.

Roll-up (`done children / total children`) is computed, not stored. **A fully
done set of children does not close the epic** — an epic routinely carries
integration work beyond its children, and auto-closing would assert something
nobody verified.

## 4. Providers: mapping and degradation

| Provider | Sprint | Release |
|---|---|---|
| Jira Cloud / DC | Sprint — `iteration` / `iteration_active` exist | `fixVersions` — **new**, a list |
| Linear | Cycle — exists | Project/Milestone — **open decision E8** |
| GitHub | — degrades | Milestone (due date + closed state) — **new** |
| Asana | — degrades | — degrades (sections are not time-boxed) |

`Caps` gains one field beside `iterations`:

```rust
/// The provider has release-like containers with a target date (Jira
/// fixVersions, GitHub milestones). Without it, a native release still works
/// — it simply cannot adopt one, and the UI offers no "adopt from tracker".
#[serde(default)]
pub versions: bool,
```

`WorkItemSnapshot` gains `versions: Vec<String>` beside `iteration`, and the
conformance suite gains one scenario **gated on the cap**, in the shape
`describe` established: a defaulted `Harness` hook, so adapters without the
capability compile unchanged and are asserted to report none.

**The degradation rule:** with `iterations: false` or `versions: false`, a
native sprint or release behaves identically. Only adoption is absent. This is
the point of the whole design — a provider's poverty never limits fleet's own
planning.

## 5. Lifecycle

**Sprint.** `planned → active → closed`. Closing asks what happens to
unfinished members: carry to a named sprint (the default) or leave them with no
sprint. Either way `removed_at` is stamped on the closed sprint's rows, so the
carry-over is visible afterwards. At most one `active` sprint per org is a
**warning, not a constraint** — overlapping sprints are a real practice, and
refusing them would make fleet wrong about the world.

**Release.** `planned → released`. Releasing stamps `shipped_at`.
`shipped_ref` is typed in by a person (E3).

**Adoption and its withdrawal.** When a native bucket holds a
`work_bucket_refs` row, a synced item reporting that sprint or version is added
with `source = 'adopted'`. When the tracker stops reporting it, that membership
**ends** (`removed_at`) — the same rule R7 already applies when a state signal
changes and ends the auto link it made. A `source = 'manual'` membership is
never touched by a tracker. One rule for "a person outranks a machine",
everywhere in the work graph.

`work_rules` are **not** extended to buckets. A rule *invents* membership;
adoption *observes* it. That is the same distinction for which D34 allowed
placement rules and refused link rules.

## 6. UX information architecture

Three surfaces, in the order they should be built.

**a. A grouping axis in the Work view.** `Group by ▾` gains `Sprint`,
`Release` and `Epic` beside the existing free `group_label`. A group header
carries the roll-up: dates, state, `4/9 done`, live sessions. Costs no new
screen and answers "what is in sprint 24" at a glance.

**b. Bulk assignment from the list.** Multi-select in the Work view, then
`Add to sprint ▾ · To release ▾ · Under epic ▾`. This is the fast path for
planning; it is deliberately **not** a separate planning panel, because a panel
hides membership one level down and answers nothing until it is opened.

**c. A sprint board.** Columns by `status_category`, cards draggable to change
status — the one place status is set by dragging. A card shows its **live
session and host**, which no tracker's board can ever do.

A mixed sprint (local work beside Jira tickets) shows both, but only local
cards drag: a tracker item sits in the column its tracker reports, and dragging
it is refused with a message pointing at the ticket (§2, E11). The board must
make that visible on the card rather than letting a drag fail silently.

The board is the one place this design accepts a new surface, against §0.1/5
("nothing is a separate app"). The justification is E5: with no tracker there
is no other board, and fleet is then the only place the sprint exists. It is
available for tracker-backed projects too — gating it on the absence of a
tracker would give one concept two mental models.

**Rejected:** a planning panel as the primary surface (hides state, buries
adoption); replacing `group_label` with typed buckets (removes working M14
behaviour and forces a free label to carry dates); burndown charts and sprint
capacity or velocity (YAGNI — no one has asked what fleet's velocity is, and
story points do not exist in this model).

## 7. Agent and MCP surface

Reads join `work`; writes join `work_link`, following the existing rule that a
new action never becomes a new tool (C21).

- `work { action: buckets }` — sprints and releases in scope, with roll-ups.
- `work { action: bucket, id }` — one bucket with its members.
- `work_link { action: bucket_add | bucket_remove, item_id, bucket_id }`
- `work_link { action: set_status, item_id, status }` — the person override of §2.
- `work_link { action: set_parent, item_id, parent_item_id }` — §3.
- Bucket creation, state changes and tracker adoption are **`work_admin`**, not
  `work_link`: they reshape what every client sees, the same reasoning that
  keeps rules and saved views away from a per-host token.

A per-host token — an agent inside a session — may read buckets in its org and
may set its **own** item's status and parent. It may not create, close or adopt
a bucket: a session does not decide the team's plan.

Budget: `BUDGET_BYTES` is `63_573` on `main` today. These actions must be
measured and the constant raised deliberately, with the number in the commit
message.

## 8. Org scope and isolation

`work_buckets.org_id` puts buckets inside the existing fence. A bucket with
`org_id IS NULL` is unassigned and visible under the same rule as unassigned
work — which, since D31 was revised, means **bound clients do not see it by
default**. A cross-org membership (an item from org A in a bucket of org B) is
refused, and forcing it requires the same `force_cross_org` path and raises the
same `cross_org` review item as a forced link (D32).

## 9. Open decisions for the owner

| # | Question | Default if unanswered |
|---|---|---|
| E8 | Linear: is a release a Project or a Milestone? | Milestone — it carries a target date; Projects are closer to epics |
| E9 | May closing a sprint carry items automatically, or must a person confirm the list? | Confirm; the dialog preselects everything unfinished |
| E10 | Should a release's members be derivable from its items' merged PRs, so `shipped_ref` gains meaning without git observation? | No for now; it is the thin end of E3's rejected scope |
| E11 | Should a tracker item carry a *separate* fleet-local status, so it can be dragged on the board even though the tracker owns `status_category`? | **No.** A tracker item's column on the board comes from the tracker, and dragging it is refused with a message pointing at the ticket. Two statuses for one item is a lie waiting to be believed, and write-back stays out (D3/D29). Revisit only if the board proves unusable for mixed sprints |

## 10. Roadmap

Five phases, each shipping usable value on its own. One plan per phase, this
document as their shared spec — the structure the work graph itself used.

| Phase | Content | Value |
|---|---|---|
| 1 | Status of native items: two columns, derivation, `set_status`, the override | `done` finally means something for work with no ticket |
| 2 | `work_buckets` + sprints: model, Work view axis, bulk assignment | planning without a tracker |
| 3 | Releases: `Caps.versions`, Jira `fixVersions`, GitHub milestones, adoption | "what goes into 0.3.0" |
| 4 | Epics for local items: `parent_id`, `kind='epic'`, tree and roll-up | structure in a large backlog |
| 5 | The sprint board | the substitute for a tracker's board |

Phase 1 is a prerequisite for 2, 3 and 5 — without a settable status the board
has no columns, the roll-ups are meaningless, and a release cannot say what
shipped. Phase 4 is independent and may land at any point after 1.

## 11. Risks

- **A fourth grouping axis.** The sidebar already groups by
  `Project · Work · Host · Flat` and the Work view by `group_label`. Three more
  axes is the real UX risk of this design, and the mitigation is that they all
  live behind one `Group by ▾` rather than becoming separate modes.
- **The live half of status derivation is in a hot path.** `in_progress` is
  computed on every Work view and board read (`done` is stamped, so it costs
  nothing there). `scale_work_view` already guards p95 on `tree(..)`; the
  derivation must be one join, never a per-row query, and it must be measured
  against that test before phase 1 merges.
- **`ux_work_buckets_name` and orgs.** A uniqueness index on
  `(kind, org_id, name)` means two orgs may both have "Sprint 24". That is
  wanted; a global unique name would be wrong.
- **Adoption churn on the replay ring.** Adopted membership changes on every
  sync pass that sees a different sprint. Emit only on real change, and never a
  frame per member.
