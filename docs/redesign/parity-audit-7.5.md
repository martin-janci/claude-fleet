# Parity gap audit for 7.5

Step 7.5 (parity sign-off) is done when every row of
[parity.md](archive/parity.md) reads *Both* with a proof in the New layout, the
shortcut freeze test is green, and Martin has used New for a week. This
audit, taken on 2026-10-08 against `origin/main` at `6b2d3e89`, lists what
stands between the checklist and that bar. Every step the rows cite
(0.3, 1.1, 1.4, 1.9, 3.3, 3.4, 3.6, 3.8, 3.9, 3.12, 4.1, 6.2) has merged,
so no row waits on an unmerged step.

Why most rows are already reachable in New: both layouts render the same
`Sidebar`, `QuickSwitcher`, `NewSessionDialog`, `WorkBoard` and status bar
(`src/App.svelte`), and `Sidebar.svelte` has no layout branch. What is
missing is mostly a test that sets `uiLayout` to `new` and proves it.

## Moved to Both in this PR

The row was stale: a merged step already proves it in New.

| Row | Proof in New |
|---|---|
| P2 | `src/lib/WorkTree.test.ts` › New layout: fixed tabs Tasks, Missions and Board, with Review as a count |
| P3 | `src/lib/filter_schema.test.ts` › Group picks List, or Grouped…; `src/App.destination.test.ts` › New layout: the board is a Work view… |
| P13 | `src/lib/ShellHeader.test.ts` › an account pill opens that account… |
| P16 | `src/lib/session_actions.test.ts` › the row menu › opens from ⋯ and from a right-click, only in the New layout |
| P25 | `src/lib/NewSessionDialog.test.ts` › NewSessionDialog in the New layout › … |

## Reachable in New, no New test yet

Each needs one test that renders it with `uiLayout.set('new')`; no product
change.

| Row | Where it lives in New | Test to add |
|---|---|---|
| P3 (board keys) | the same `WorkBoard` as Classic | ← → and e on the board under New, next to `App.destination.test.ts` › the board is a Work view |
| P8 | the same lost/ghost fold in `Sidebar`; Restore in `HostDetail` | the lost-fold tests (`Sidebar.test.ts`) under New, and Restore from the New Hosts detail (`HostsView.test.ts` › Open shows the detail…) |
| P9 | Needs you in the New Filters panel (`filter_schema.test.ts` › Needs you, now in the panel…) | an idle-too-long row counted in Needs you under New |
| P11 | Appearance in Settings and the ⌘K theme command, shared by both layouts; the sidebar theme line is gone in both | the ⌘K theme command and Appearance under New |
| P15 | density, not layout, decides the badges (`SessionRowItem.svelte`) | `SessionRowDensity.test.ts` › Comfortable shows every 0.5.4 badge, repeated under New (New adds the account pill) |
| P21 | the same `TaskWorkSections` | + Add subtask under New (nearest today: `TaskStartButton.test.ts` › a subtask › New: starts through the same split button) |
| P26 | ? sheet: `App.destination.test.ts` › the New status bar ends on Shortcuts…; the board hint is the same one-time `WorkBoard` hint | the board's first-run hint under New |
| H1, H2, H3 | the same `QuickSwitcher` (pin, hide and groups are not layout-gated) | copy the Ctrl+P, Ctrl+Backspace and Ctrl+G / fold tests into its New describe |
| H7, H8 | Friendly names and Row details sit in the ⋯ view options in both layouts (`SidebarFilters.svelte`) | toggle each under New, beside `filter_schema.test.ts` › Group picks the grouping, and Select is in ⋯ |

## Real gap, now closed

| Row | Gap |
|---|---|
| P19 | Complete, Mark failed and Cancel were flat `mission-move-*` buttons in both layouts; the ⋯ menu beside Edit and Pause was never built, and the row's Step 6.2 is the Work filters step. Closed in the PR after this audit: New puts them in a ⋯ menu beside Edit and Pause, each behind a confirm; Classic keeps the buttons. |

## Open PRs that add rows

- P31 (5.8, Shared with me; Share in the header) lands as *Both* with its
  New proofs in the same PR as this audit (#615).

## Freeze test

`src/lib/shortcuts.test.ts` exists (14 tests): every 0.5.4 global chord
resolves to the same action, the registry has no conflicts, each per-view
key table matches its handler, and the scopes match.

## What is left for sign-off

1. The New-layout tests in the table above: test only, one PR.
2. Martin's week on New, which can start now: nothing on the checklist is
   missing from New.
