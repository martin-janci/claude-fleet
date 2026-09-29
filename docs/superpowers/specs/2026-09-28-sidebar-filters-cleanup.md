# Sidebar filters: one model for Sessions and Work

Status: landed on the desktop. fleet-mobile's *My work* should follow it (see
the last section).

## Problem

The sidebar mixed filters, display settings and actions in one row of pills:
host pills sat next to Tasks and Settings, "🤖 bg on" hid rows but looked like
a display toggle, and "⧉ by work" decided which filters existed. The Work view
used selects where the Sessions list used pills, and the two named the same
things differently ("any tracker" / "all trackers", "past only" / "past
sessions only"). Nothing showed that the list was narrowed, and the empty
state said "No active sessions" over a fleet that had some.

An audit turned up filters that silently did nothing, or did something
nobody could see:

| # | Bug | Fix |
|---|-----|-----|
| 1 | Host pills and the org scope were shown in the Work view, which reads neither | The Sessions list's filters are hidden in the Work view. It has its own org filter. |
| 2 | A work group's *Done* ignored the host and scope filters; past-only groups honoured them | Both use `pastVisible` |
| 3 | Needs you left past work and Outside fleet in the list | Both are filtered out, since an ended session never waits on you |
| 4 | Search skipped Other sessions and Outside fleet | Search now covers them too |
| 5 | The empty states never named the filters that hid rows | *No sessions match Host: x, Last 1d.* with **Clear filters**. The same in Work. |
| 6 | A remembered host that was later removed or hidden left the list empty, with no pill active | `effectiveHostFilter` falls back to `all` |
| 7 | A tracker filter stayed on after its chips were hidden (one tracker left) | The chips show while the filter is on |
| 9 | Focus from Link or Tidy review, *View host sessions* and the *Review* toast changed the Sessions list while the Work view stayed up | Each of them switches to Sessions |
| 12 | "Mine" persisted on emptied the list until the hub's `mine` view loaded | `withMineReady` applies it once it has loaded |
| – | Two refresh buttons in the Work view | The sidebar's ↻ now re-reads the Work tree too |

## Follow-ups (same PR)

- **Archived is hidden by default, never lost.** Archived live sessions and
  past work (Sessions list) and archived tasks (Work view: done, or every
  session link archived, with nothing running) stay out of the list. The end
  of each list says *N archived hidden · Show archived*, and the filter
  panel has an *Archived* switch. An explicit *Status: Done* (Work) or
  *Past only* (*Session:* in Sessions, *Sessions:* in Work) shows them
  regardless. The Work tree gets `filters.archived` and `archived_hidden`
  on the page; both are additive. The hub hides archived tasks only when a
  client asks with `archived: false` (the desktop always sends the field):
  absent means "show", so a phone from before this change, which never
  sends it and has no *N hidden* row, still sees its done tasks, and no
  contract revision bump is needed. An older hub ignores the filter and
  shows everything. Archived-ness is judged over every link of a task, not
  only the caller's: a per-host or org-bound caller must not see a task as
  archived while another host or org still works on it. The Sessions pref
  moved to `sidebar.work-filters.v2`: every install had stored the old
  default `archived: true`, so the rest of the filters carry over field by
  field (a field missing from an older pref takes its default) and the new
  archived default applies.
- **Last active is one rule.** Every row is judged by its own time: a
  session by its last activity, a past link by when it ended. It used to
  weigh only a project's newest session, and only in the project tree.
- **Selection follows the filters.** A row that a filter hides leaves the
  selection, so bulk Kill and Send reach only what you can see.
- **⌘⇧O in the Work view** cycles the Work view's own organisation filter.

## Layout

```
R0  [ Sessions ¹ | Work ]            ☑  ⚙  ↻  ‹     view, then global actions
R1  [org ▾] [ search…            ] [⏷ Filters ²]    Sessions list only
R2  [⚠ Needs you 1] [Select]                  ⋯     quick filters, then view options
R3  Host: gpu-box ×  Last 1d ×  Clear all            only while something narrows the list
    ┌ Filters panel (inline, pushes the list down) ┐
    │ SCOPE    Machine: Any · local · gpu-box …    │
    │ TIME     Last active: Any time · 8h · 1d …   │
    │ WORK     Tracker · Status · Tracker column · │
    │          Assignee · Session (group by work)  │
    │ INCLUDE  ◉ Background agents  ◉ Archived work│
    │ Clear all                             Done   │
    └──────────────────────────────────────────────┘
```

- **Filters vs. view options.** Anything that hides rows is a filter and lives
  in the panel. That includes *Background agents* and *Archived work*: they
  are switches that read "include". The old "hide archived" wording was
  inverted. Friendly names, row details and *Group by* never hide rows. They
  are view options, and live in the ⋯ menu.
- **The strip.** Each filter held in the panel appears as a removable chip,
  plus *Clear all*. Search, Needs you, *Assigned to me* and *To review* show
  their state in their own controls, so they get no chip. The empty state
  still names them.
- **The Work view.** It has the same shape: saved views, then search plus
  *Filters*, then the *Assigned to me* and *To review* toggles, then the strip,
  then a panel of chip groups (Organisation, Tracker, Status, Sessions).
  While the Work view is up, the Sessions tab carries the Needs-you count.
- **Controls.** Every control is `.btn` / `.btn--chip` / `.btn--toggle` from
  `controls.css`, with 24 px minimum targets, `aria-pressed` on chips and
  `role="switch"` + `aria-checked` on switches. Colours come from tokens
  (`--usage-crit` for Needs you and Kill).
- **Labels.** Sentence case everywhere, with *Any* as every group's reset:
  `Status: To do · In progress · Done`, `Session: Active session · Past only`.

## Code

- `src/lib/filter_facets.ts` builds the chip text for both views
  (`sessionFacets`, `workFacets`) and the patches that clear one chip. It is
  pure: strings and ids only.
- `ActiveFilters.svelte` is the strip, and `FilterChipGroup.svelte` is one
  labelled single-choice group.
- `SidebarFilters.svelte` holds R0 to R3 and the panel. `WorkFiltersBar.svelte`
  is the Work view's bar.
- Stores and pref keys are unchanged (`host-filter`, `scope-filter`,
  `recency`, `work.filters`, …), except the Sessions work filters, which
  moved from `sidebar.work-filters` to `sidebar.work-filters.v2` (see
  *Archived is hidden by default* above; v1 is read once, field by field),
  so a user's filters survive the upgrade.

## fleet-mobile

*My work* and the Sessions filters follow the same model. The work is on
fleet-mobile's `claude/serene-faraday-7loasz` branch:

- `model/FilterFacets.kt` is a port of `filter_facets.ts` and carries the
  same labels.
- `ui/FilterControls.kt` holds the pieces both screens share: the
  *Filters (n)* chip, the strip of removable chips with *Clear all*, and
  the *N archived hidden · Show archived* row.
- Archived items are hidden by default. The filter prefs move to
  `sessions.filters.v2`, migrated the same way as on the desktop.
- The Work tree reads the new `archived` filter and `archived_hidden`
  count, and sends `archived: false` to hide (absent shows everything).
