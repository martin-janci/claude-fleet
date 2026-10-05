# The New session project picker: context, recent, popular, groups, and the noise folded away

Status: design, approved in conversation 2026-10-05. Two phases; phase 1
ships alone.

## The problem

"+ New session" (and *New session on host* from the Hosts view) opens a
popover in the sidebar footer (`Sidebar.svelte`, `showProjectPicker`) that
lists **every** project, alphabetically by `owner/repo`, in a 240px box with
no search. On the author's fleet that is 81 rows (2026-10-05):

- about ten are in use (a session in the last few days);
- ~55 never had a session at all (`last_session_at` null);
- many are noise: `ppt-epic-145…150` (0 worktrees), `test-*`, `tmp-*`,
  `example-*`, `*-analysis`, `stw-fix2/3`, forks of other people's repos.

The one you want is found by scrolling and reading. The picker does not know
what you are doing, what you used last, or what you use most.

## Goals

1. Open the picker and the project you want is in the first screen, most of
   the time without typing.
2. Typing finds any project, the hidden ones included.
3. Projects that are noise are folded away, never deleted, and a person can
   always override the classification.
4. Jev (the decision model, `service/decide`) proposes groups and noise for
   what the rules cannot classify; a person confirms. Never automatic.

Non-goals: changing `NewSessionDialog` itself; changing orgs or `org_rules`
(the work graph's security boundary); deleting or un-registering projects.

## Phase 1: the picker

### Layout

The popover gains a search field at the top (autofocused; ↑/↓ move, ↵ opens
`NewSessionDialog` on the highlighted project, Esc closes). `＋ Add project…`
stays as the first row. With an empty query the picker shows sections, in
this order; **a project appears once**, in the highest section that claims
it:

| # | Section | Contents | Cap |
|---|---------|----------|-----|
| 1 | Pinned | `pick = pin` | none |
| 2 | For this context | projects the current context points at (below), each with a one-line reason | 5 |
| 3 | Recent | by `last_session_at` desc | 5 |
| 4 | Popular | by `starts_30d` desc, `starts_30d ≥ 2` | 5 |
| 5 | Groups | one heading per group (below), alphabetical inside; a group over 8 rows starts folded | none |
| 6 | Other (N) | noise (below); folded by default | none |

An empty section is not rendered. "For this context" is not rendered when the
context says nothing.

With a query, sections disappear: one list ranked by `fuzzy.ts` over
`owner/repo` and the group name, with a small additive boost for context,
recency and popularity, and a penalty (not a filter) for noise — a hidden
project is still found, only lower.

Each row has Pin / Hide (hover buttons, and the context menu). On an `Other`
row, Hide becomes **Keep** ("this is not noise").

### Context

A pure function over what the app already holds; strongest signal first:

1. **Preferred host** (*New session on host*): projects with a session on
   that host (fleet's local worktree rows cover only the `local` host, so
   sessions are the signal that works for every host). Reason: `on <host>`.
2. **Selected session** (`selectedSession`): its project. Reason:
   `current session`. (Expanding to "same group as" is left out of phase 1.)
3. **Active sidebar filters** (`SessionFacetInput`: scope, host, search,
   work/tracker filters): projects of the sessions the filtered list shows.
   Reason: `from filter: <facet label>`.

The filters are read through a small adapter (`pickerContextFromFilters`) so
the picker does not depend on the shape of the session filter model; a
rewrite of that model (`2026-09-25-session-filter-model`) only changes the
adapter.

### Groups (phase 1, rules only)

A project's group, first match wins:

1. the person's group (`project_picks.grp`);
2. *(phase 2)* a confirmed Jev proposal (also stored in `project_picks.grp`);
3. **prefix cluster**: among one owner's non-noise repos, a leading or
   trailing dash/underscore token shared by at least two multi-token repos
   forms a cluster (`openmarket-*`, `*-mcp`, `sales-twins-*`); a repo
   matching exactly one cluster joins it (a single-token repo joins the
   leading cluster of its own name: `openmarket` → "openmarket"); a repo
   matching two is ambiguous (left to the owner, and to Jev in phase 2). The
   cluster's name is its token;
4. the owner.

The project's org is not a phase-1 rule: the frontend's project rows carry
no org (only sessions do), and resolving `org_rules` for projects is work
graph territory.

Groups are **display only**. They are not orgs, carry no permission, and are
never written into `org_rules`.

### Noise (phase 1, rules only)

A project is noise when `pick` is not `keep` or `pin`, and any of:

- `pick = hide`;
- `last_session_at` is null (no session ever);
- `starts_30d = 0` and the repo name matches `^(test|tmp|example)-`,
  `-analysis$` or `-epic-\d+$`.

`system` projects (`fleet/operator`) are excluded outright, as today.

Two rules from the first draft are dropped: "0 worktrees" (the joined
worktree rows are the `local` host's only, so on a hub without a local host
every project would read as noise), and "registered over 30 days ago"
(projects have no registration time). In their place: a project added
through *Add project* is marked `keep` at once — adding it is the person
saying it matters.

Forks of other owners are **not** noise by rule (some are used); they are left
to Jev.

### The ranking module

All of the above is one pure module, `src/lib/project_rank.ts`:

```ts
rankProjects(input: {
  projects: ProjectTreeRow[];
  picks: Map<string, ProjectPick>;      // keyed `owner/repo`
  context: PickerContext;
  query: string;
  now: number;
}): PickerView   // sections, or one ranked list when query is non-empty
```

`Sidebar.svelte`'s popover renders a `PickerView` (through a new
`ProjectPicker.svelte`); `quick_switcher.ts`'s project rows (`New session in
<project>`) drop noise from the empty-query list with the same `isNoise`, so
both surfaces agree on what is noise. Typing still finds a noise project
there too.

### Data

Project rows are deleted and re-created, and `project_id` is re-derived on
every pass (review C22, `050_orgs.sql`). Everything this feature stores is
therefore keyed by **text** (`owner`, `repo`), like `org_rules`.

Migration `102_project_picker.sql`:

```sql
CREATE TABLE project_picks (
  owner TEXT NOT NULL,
  repo  TEXT NOT NULL,
  pick  TEXT CHECK (pick IS NULL OR pick IN ('pin','hide','keep')),
  grp   TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (owner, repo)
);
CREATE TABLE project_starts (
  owner      TEXT NOT NULL,
  repo       TEXT NOT NULL,
  host_alias TEXT NOT NULL,
  tmux_name  TEXT NOT NULL,
  at         INTEGER NOT NULL,
  UNIQUE (host_alias, tmux_name, at)
);
CREATE INDEX idx_project_starts_project ON project_starts(owner, repo, at);
```

(The number is the next free one at the time of writing; take the next free
one when implementing.)

- **`project_starts`**: reconcile runs `INSERT OR IGNORE` for every live
  session with a project on every pass, next to where `last_session_at`
  moves; `at` is the tmux session's `created_at`. The UNIQUE key makes the
  repeat a no-op, so no "first insert" detection is needed, and sessions
  started outside fleet count too. The GC sweep deletes rows older than 90
  days.
- **The picker state is NOT on `ProjectRow`.** `list_projects_joined` reads
  the worktree columns at fixed offsets after the project columns, and
  reconcile emits `project:updated` with a bare `ProjectRow` that the
  frontend swaps in whole — a pick carried there would be wiped by every
  pass. Instead, two commands of their own:
  - **`project_picks`** → `[{ owner, repo, pick, grp, starts_30d }]`, one
    per non-system project;
  - **`set_project_pick { owner, repo, pick, grp }`** (full replace; both
    null deletes the row) → the updated row.

  Both are hub tools with `Access::PersonDevice` (a person's preference:
  their paired devices only, never a host's token, not served to the master
  — which keeps the master's description budget untouched), and both
  commands are `Routed` in `verdicts.rs` (`REGEN_HUB_VERDICTS=1`,
  `REGEN_DOCS=1`). No event: the picker reloads `project_picks` each time it
  opens and patches its store from `set_project_pick`'s answer. A hub older
  than this feature answers with an error, and the picker works from the
  rules alone. `CONTRACT_REVISION` does not move.

### Phase 1 tests

- `project_rank.test.ts`: section membership and order, one-row-once,
  caps, each context signal and its reason, prefix clusters (incl. the
  ambiguous case), every noise rule and the `keep`/`pin` overrides, query
  ranking with noise penalised but present.
- Store: `project_starts` written once per session however many passes see
  it, survives a project row being re-created, `starts_30d` window,
  retention.
- `project_picks` / `set_project_pick`: round trip, validation, access rows,
  hub routing cases.
- Component: search + keyboard flow, Pin/Hide/Keep, `Other` folded.

## Phase 2: Jev `project_group`

Built on the decision envelope exactly as `status_map`
(`service/decide/status_map.rs`): gate, redact, one bounded call, a
`decision_runs` row in every case.

- `Feature::ProjectGroup`, setting `decide.jev.project_group`
  (`off | shadow | assist`), **off** by default; behind the kill switch, org
  consent (`orgs.jev_allowed`, `decide.jev.unassigned`), key, breaker, budget.
- **Subject**: `project` `<owner/repo>` fingerprinted with the local key, as
  `status_map` does for section names.
- **Two Choice questions per project**:
  - *group*: one of a closed set = the person's groups ∪ orgs ∪ prefix
    clusters ∪ `none` ∪ `unsure`. Jev never invents a name (the envelope is
    closed-set); a person creates a group in one click and the next run
    offers it.
  - *noise*: `keep | noise | unsure`.
- **Sent**: `owner/repo` (redacted), worktree count, days since the last
  session, `starts_30d`, adopted, whether the owner is one of the person's
  owners (owns ≥ 1 project with a session in 30 days), and the group
  candidates. Nothing from the repository's contents.
- **Shadow**: also asks about projects the rules classified, to measure
  agreement (`decide status`, Settings → Decisions).
- **Assist**: asks only where the rules are silent or ambiguous and the
  person has set nothing (`pick` null, `grp` null). An answer at confidence
  ≥ 0.5 other than `unsure`/`none` is a **proposal**. The picker shows one
  line above the sections — *Jev suggests: 9 projects into groups, 7 into
  Other · Review* — opening a batch review: confirm (writes `grp` or
  `pick = hide`), correct (writes the person's choice), reject (`pick = keep`
  for noise; a rejected group is not proposed again on the same input).
  Follow-ups `confirmed / corrected / rejected / ignored` are recorded as in
  `status_map::record_followups`.
- **When**: from the background tick at most once a day, at most 40
  questions a run, a project not re-asked for 14 days on the same input,
  question version, mode and model. Never on the picker's open path.
- **Isolation**: the proposals action gets a row in
  `mcp/tools/tests_isolation.rs`; a per-host token sees only its org's
  projects' proposals.

### Phase 2 tests

The `status_map` test set, mirrored: gate refusals per fallback, shadow asks
classified projects and records a baseline, assist asks only unclassified
ones, closed-set validation rejects an invented group, re-ask window,
follow-ups, the isolation matrix row.

## Rollout

Phase 1 is one PR (frontend module + popover, migration, `ProjectRow`
fields, `set_project_pick`). Phase 2 is a second PR, shipped `off`; turned to
`shadow` on the hub, read agreement after a week, then `assist`.
