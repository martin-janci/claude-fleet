# New session: one picker in ⌘K — tickets, pins, suggestions, groups, hidden noise

Status: design v2, 2026-10-06. v1 (a new sidebar popover with Recent /
Popular / Context sections) was reviewed by four competing UX reviews
(keyboard power user, information architecture, accessibility, a rival
designer) and replaced by this. Two phases; phase 1 ships alone.

Mockup (interactive, real data): https://claude.ai/artifact/KzEbmrQvhhBiQq54fDs5Rj

## The problem

"+ New session" opens a sidebar popover listing **every** project
alphabetically by `owner/repo`, in a 240px box, with no search. On the
author's fleet that is 81 rows (2026-10-06); `claude-fleet`, the most-used
project, is row 20. About ten projects are in use; most of the rest have no
session fleet knows of (`last_session_at` null — often because they predate
fleet's tracking, not because they are dead); some are throwaway
(`ppt-epic-145…150`, `test-*`, `tmp-*`, `*-analysis`).

A second surface already exists: the ⌘K quick switcher offers "New session
in <project>" rows, ranked differently (after every session, by
`last_session_at` alone).

## Decisions (from the review)

- **D1 One surface.** The popover is removed. "+ New session", a new ⌘N
  (Ctrl+Shift+N off macOS) and the Hosts view's `n` open the ⌘K switcher in
  a **New session mode**. One ranking, one set of keys.
- **D2 No session record is "unknown", not noise.** Only an explicit Hide or
  a throwaway name unused for 30 days hides a project. Unknown and stale
  projects stay in their group, dimmed and last.
- **D3 Frecency from the person's own picks**, kept locally, plus
  `last_session_at`. No server-side start counting.
- **D4 Recent + Popular + filter-context collapse into one "Suggested"**
  (≤ 7), led by the true context signals: the selected session's project
  and the preferred host.
- **D5 Groups list every member.** Only Pinned/Suggested de-duplicate among
  themselves; a group is never "the leftovers".
- **D6 Start from work.** Up to 3 *My work* tickets with no live session
  head the list — what you are about to do, not just where.
- **D7 Manual groups** in phase 1, chosen from existing groups (or a new
  one) through a combobox; grouping a project implies it is not noise.
- **D8 Pinned and visibility are separate fields**; unpinning never hides.

## Phase 1

### Entry points

| Trigger | Opens |
|---|---|
| Sidebar "+ New session" button | switcher, New session mode |
| ⌘N (macOS) / Ctrl+Shift+N | same |
| Hosts view `n` on a host | same, preferred host = that host |
| ⌘K / ⌘P | switcher, normal mode (unchanged), whose project rows now use the same ranking |

In New session mode the input carries a leading chip **New session in**
and the placeholder `project or ticket…`. Backspace on an empty query
leaves the mode (normal ⌘K). Sessions and hosts are not listed in this mode.

### The list, empty query

1. **Start from work** — up to 3 tickets from *My work* whose
   `live_session_ids` is empty; each row: key chip, title, and
   `→ <project> · <host>` from the existing `placeForTicket`. Only when a
   tracker is configured and tickets loaded.
2. **Pinned** — every pinned project, A→Z.
3. **Suggested** — up to 7, excluding pinned and hidden: first the selected
   session's project (chip `current session`), then projects with a session
   on the preferred host (chip `on <host>`), then by frecency score.
   Pinned + Suggested rows carry ⌘1…⌘9 in order.
4. **Groups** — every non-hidden project, in its group (below). Inside a
   group: active members A→Z, then dormant ones (no session record, or none
   for 90 days), dimmed, with `no sessions yet` / `4mo`. A group starts
   open when any member had a session in the last 30 days, else folded
   (its header shows `N projects`). Headers show a subtitle: the rule
   (`openmarket-* · papayapos`) or `your group`.
5. **Hidden (N)** — folded; each row says why (`hidden by you`,
   `throwaway name`).
6. **Add project…** — always last.

A folded section is a single option row (`Show 14 in Hidden`), so it is
reachable from the keyboard; Enter/→ unfolds, ← folds.

### The list, with a query

Sections disappear except **Tickets** (the existing ticket ranking) first.
Then one ranked list of projects: `fuzzyMatchFields` over
`[owner/repo, repo, group]`, plus boosts **on the fuzzy scale** (fuzzy
substring scores run in the hundreds): pinned +40, frecency up to +60,
current-session +80; hidden −400 and tagged `hidden · <reason>`. Last row:
**Add project “<query>”…** (clone URL prefilled when the query looks like
`owner/repo` or a URL).

### Groups

A project's group, first match wins:

1. the person's group (`project_picks.grp`);
2. **prefix cluster**: per owner, over **all** that owner's projects (so a
   hide never moves a neighbour), the longest dash-separated prefix shared
   by ≥ 3 repos (`sales-twins-app`, `-mobile`, `-revonaut-fixes` →
   `sales-twins`); a single-token repo equal to a cluster's prefix joins it
   (`openmarket`). Leading prefixes only. Name = the prefix, subtitle =
   `<prefix>-* · <owner>`;
3. `More from <owner>` for an owner with ≥ 3 projects;
4. `Forks & others` for owners with fewer.

Group order: owners by their most recent `last_session_at`; within an
owner, clusters and person's groups A→Z, then `More from <owner>`;
`Forks & others` last. Groups are display only — never orgs, never
`org_rules`.

### Hidden

Hidden when `vis = 'hide'`, or all of: `vis` is not `keep`, not pinned, no
person's group, no session in 30 days, and the repo matches
`^(test|tmp|example)-`, `-analysis$` or `-epic-\d+$`. `system` projects
(`fleet/operator`) are excluded from the picker entirely, as today. A
project added through *Add project* is marked `vis = 'keep'`.

### Frecency

A local preference `newsession.frecency`:
`{ "<owner>/<repo>": { "score": number, "at": unix } }`. Picking a project
(Enter, ⌘↵, click, ⌘1…9) adds 1 after decaying the stored score with a
7-day half-life. The ranking score is that decayed score ×10 plus a
recency term from `last_session_at` (`20 · 2^(−days/7)`). At most 200 keys
are kept (lowest dropped). Per device by design (like Raycast / Alfred).

### Keyboard and accessibility

The existing switcher pattern: `input[role=combobox][aria-activedescendant]`
over `PickerList`'s `role=listbox`. The highlight is tracked **by key**,
never by index. The ranking is **snapshotted when the switcher opens** and
recomputed only when the query changes or the person acts (pin, hide,
group, fold) — data arriving mid-open never moves the highlight.

| Key | Action |
|---|---|
| ↑ / ↓ | move, wrapping; fold rows and Add are reachable |
| Enter | open the dialog for the project / ticket; on a fold row, unfold |
| ⌘↵ | start with the last settings (dialog autostarts) / start the ticket |
| ⌘1…⌘9 | open the numbered Pinned/Suggested row |
| ⌘P | pin / unpin the highlighted project |
| ⌘⌫ | hide / unhide, with an Undo toast |
| ⌘G | the group menu for the highlighted project |
| ⇧F10, context-menu key, right-click | the actions menu (`role=menu`) |
| ⌘Z | undo the last pin / hide / group change |
| Esc | close a menu; else clear a non-empty query; else close |

Hover shows icon buttons (pin, group, hide) on a project row; they are
`tabindex=-1` and `aria-hidden`, a mouse convenience only — every action
is on the keys and in the menu. Visuals use the app tokens: the active row
is `PickerList`'s accent wash plus 2px accent bar; secondary text is
`--fg-muted` (5.3:1), never a lighter grey; the modal stays 560px wide;
the list's max height is `min(70vh, 34rem)`.

### Data

Everything stored is keyed by `owner` + `repo` TEXT, never `project_id`
(project rows are deleted and re-created; review C22, `050_orgs.sql`).

Migration `102_project_picks.sql` (next free number when implementing):

```sql
CREATE TABLE project_picks (
  owner      TEXT    NOT NULL,
  repo       TEXT    NOT NULL,
  pinned     INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  vis        TEXT    CHECK (vis IS NULL OR vis IN ('hide', 'keep')),
  grp        TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (owner, repo)
);
```

- The picker state is **not** on `ProjectRow` (`list_projects_joined` reads
  worktree columns at fixed offsets, and reconcile's `project:updated`
  swaps the frontend row whole).
- **`project_picks`** → `[{ owner, repo, pinned, vis, grp }]`, one per
  non-system project. **`set_project_pick { owner, repo, pinned, vis, grp }`**
  (full replace; all default deletes the row) → the row.
- Both are hub tools with `Access::PersonDevice` (a person's own paired
  devices; never a host token; not served to the master) and `Routed`
  commands. `CONTRACT_REVISION` does not move; an older hub answers with
  an error and the picker runs on rules alone.
- The frontend loads picks at startup (and when the hub connection
  changes) and again in the background each time the switcher opens; a
  write patches the store optimistically and rolls back with an error
  toast on failure.

### Starting with the last settings

`NewSessionDialog` gains `autostart`: once its host's worktree list is
ready, it submits once with the remembered per-project choices
(`newsession.project.<id>`). Anything that would need a person (a new
worktree with no name, a blocked hub action, an error) leaves the dialog
open as today.

### Phase 1 tests

- `project_rank.test.ts`: hidden rules and reasons, dormant, clusters
  (≥ 3, longest prefix, single-token join, stability under hide), group
  order and folding, Suggested order and chips, ⌘ numbering, search boosts
  and hidden penalty, ticket block filtering.
- `frecency.test.ts`: decay, record, cap.
- Store / tools / routing: round trip, validation, access rows, cases.
- `QuickSwitcher` new mode: entry, chip, Backspace exits, keys, snapshot
  stability, undo, Add row, fold rows.
- `PickerList`: dim, chip, kbd, group subtitle, row actions are not
  focusable.
- `NewSessionDialog`: autostart submits once / stays open when blocked.

## Phase 2: Jev places what the rules cannot

Built on the decision envelope as `status_map`
(`service/decide/status_map.rs`): gate, redact, one bounded call, a
`decision_runs` row in every case; `Feature::ProjectGroup`, setting
`decide.jev.project_group` (`off | shadow | assist`), off by default.

- **Group question** (Choice), only for projects in `More from <owner>` or
  `Forks & others` with no person's group: one of the closed set = the
  person's groups ∪ prefix clusters ∪ `none` ∪ `unsure`. Jev never invents
  a group. Example: `stw-fix2` → `sales-twins`, `pos-frontend` →
  `papayapos`.
- **Hide question** (Choice `keep | hide | unsure`), only for dormant
  projects (no session in 90 days) that are not pinned, kept or grouped by
  the person.
- **Sent**: `owner/repo` (redacted), days since the last session, worktree
  count, adopted, and the candidate groups. Nothing from the repository.
- **Shadow** measures agreement on projects the rules did place; **assist**
  turns confident answers into proposals, shown as one line at the top of
  the switcher (*Jev has 8 suggestions…* · Review) and decided in a batch
  dialog (Accept / Other group… / No; Hide / Keep). A rejection is final on
  the same input. Background tick, at most once a day, ≤ 40 questions, never
  on the open path; isolation-matrix row for the proposals action.
