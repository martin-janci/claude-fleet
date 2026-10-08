# Redesign parity checklist

The Orbit Fleet redesign moves functions; it never removes one. This file is
the contract every redesign PR is checked against (ground rule 1 of the
[transition plan](../ux/2026-10-08-orbit-fleet-redesign/transition-plan.md)).
It starts from the "Nothing is removed, only moved" table of the screen
inventory and UX plan, plus the functions the UX critique found with no home
on the boards.

How to use it:

- A redesign PR lists, in its **Parity** section (see the PR template), every
  row below it touches: where the function lives after the PR and the test
  that proves it still works there.
- The PR updates the row here in the same commit: **Now** says where the
  function lives today, **Proof** names the test. A row moves from *Classic*
  to *Both* when New has it, and to *New* only in 13.1, when Classic goes.
- A deleted test needs a replacement that covers the moved function, named
  in **Proof**.
- Step 7.5 (parity sign-off) is done when every row reads *Both* with a
  proof in New, and Martin has used New for a week.

Columns: **Today** is the 0.5.4 function. **Goes to** is its place in the
new layout, from the plan. **Reached by** is how a person gets there, which
must keep working. **Step** is the plan step that moves it. **Now** is
*Classic* (only in the classic layout), *Both* or *New*. **Proof** is the
test file (and test) that covers it today.

## Moved functions

| # | Today | Goes to | Reached by | Step | Now | Proof |
|---|---|---|---|---|---|---|
| P1 | Sessions · Work switch | Sessions and Work rail items | ⌘⇧W / Ctrl+Shift+W, unchanged | 3.2 | Both | `src/lib/WorkViewSwitch.test.ts`, `src/lib/shortcuts.test.ts`; New: `src/App.destination.test.ts` › Work and Sessions pick the sidebar tree and leave a fleet page |
| P2 | Tasks · Review · Missions tabs | Views at the top of the Work list, same names | Click; Review count kept | 3.4 | Classic | `src/lib/WorkTree.test.ts` |
| P3 | List · Grouped · Board | One segmented control; Board fills the main pane | Click; ← → and e on the board | 3.4 | Classic | `src/lib/WorkTree.test.ts` (layout toggle), `src/lib/WorkBoard.test.ts` |
| P4 | Work ⚙ Placement rules | Work view ⋯ menu, and Settings → Work & trackers → Placement rules… (landed in 7.1) | Both | 7.1 | Both | `src/lib/WorkRuleEditor.test.ts`, `src/lib/WorkSettings.test.ts` › opens the placement rules from Settings… |
| P5 | Link suggestions bar and sheet | An Inbox item plus the attention line; the sheet is unchanged | j / k / y / n, Undo | 3.3 | Both | `src/lib/LinkReview.test.ts`; Inbox: `src/lib/Sidebar.test.ts` › lists only what needs you… (attention line) |
| P6 | Tidy up chip and sheet | An Inbox item; Safe kill, Archive, Snooze 7d, Never unchanged | j / k / space / Enter | 3.3 | Both | `src/lib/TidyReview.test.ts`; Inbox: `src/lib/Sidebar.test.ts` › lists only what needs you… (attention line) |
| P7 | Needs you pill | The Inbox Needs you group, and still a filter chip in Sessions | Click; the rail's Inbox | 3.3 | Both | `src/lib/Sidebar.test.ts` › the "Needs you" queue keeps stuck, safe-kill, ghost and failed rows; › lists only what needs you…; `src/lib/inbox.test.ts`; `src/lib/attention.test.ts`; New Sessions chip, in the Filters panel: `src/lib/filter_schema.test.ts` › Needs you, now in the panel… |
| P8 | Lost and ghost sessions | Folded "5 stopped" row in their group, plus a host item with Restore | Expand the row; Hosts → Restore | 1.1 | Classic | `src/lib/Sidebar.test.ts` › a ghost row stays one line…; `src/lib/SessionRowItem.test.ts` |
| P9 | Idle-too-long nudge | Idle group, still in the Needs you filter | Filter chip | 3.3 | Classic | `src/lib/attention.test.ts` › counts narrower than it filters |
| P10 | Today and Copy standup | Control's Today tab (9.1; the Inbox's second tab from 3.3 until then) | ⌘⇧T / Ctrl+Shift+T, unchanged; Control's Today tab | 3.3, 9.1 | Both | `src/lib/TodayView.test.ts`, `src/lib/today.test.ts`; Control: `src/App.destination.test.ts` › ⌘E opens and closes Control; ⌘⇧T opens its Today tab |
| P11 | theme: auto line | Settings → Appearance and a ⌘K command | Both | 0.3, 1.4, 3.9 | Classic | `src/lib/Sidebar.test.ts` (theme-toggle); Appearance: `src/lib/AppearanceSettings.test.ts`; ⌘K: `src/lib/commands.test.ts` › the theme command names the theme it switches to… |
| P12 | ✦ operator button | Rail item (Control); its Chat tab is the agent, and each of the six blocked states has a next step | ⌘E / Ctrl+Shift+E, unchanged; the rail | 9.1 | Both | `src/lib/AgentFab.test.ts`, `src/lib/shortcuts.test.ts`; Control: `src/App.destination.test.ts` › the rail opens Control…; blocked states: `src/lib/operator.test.ts` › every blocked state offers a next step, `src/lib/AgentPanel.test.ts` |
| P13 | Usage in the status bar | Account pills in the top bar; a click still opens that host | Click | 4.1 | Classic | `src/lib/HostsList.test.ts`, `src/lib/account_usage.test.ts` |
| P14 | Version, Downloads, hub and tracker health | Stay in the status bar | Unchanged | 3.17 | Classic | `src/lib/downloads.test.ts`, `src/lib/app_version.test.ts` |
| P15 | 10 badge kinds on a session row | Status, PR, CI and age on the row; host, context, cost, privacy, sync, related, cross-org in the hover card and inspector | Comfortable density with Row details shows all of them on the row, as today | 3.6 | Classic | `src/lib/SessionRowDensity.test.ts` › Comfortable shows every 0.5.4 badge, `src/lib/SessionRowItem.test.ts`, `src/lib/SessionRowWork.test.ts`, `src/lib/SessionRowOrg.test.ts` |
| P16 | Row hover actions | Same hover strip, plus a ⋯ menu and right-click with the same items | Double-click to rename, unchanged | 3.6 | Classic | `src/lib/SessionRowItem.test.ts`, `src/lib/Sidebar.test.ts` › Enter in label mode saves… |
| P17 | Worktree path | Inspector and hover card, with Copy path | Click; ⌥⌘B / Ctrl+Alt+B shows the inspector | 3.5 | Both | `src/lib/SessionDetails.test.ts`; New: `src/App.inspector.test.ts` › a selected session gets its header, the agent tab and the inspector beside it |
| P18 | Eight quick chips | Three visible, the rest under ⋯; presets still edited in Settings | Click | 5.9 | Both | `src/lib/ConversationPanel.test.ts`, `src/lib/composer_presets.test.ts`, `src/lib/SettingsDialog.test.ts`; New: `src/lib/ConversationPanel.test.ts` › shows three chips and puts the rest under ⋯… |
| P19 | Mission Complete, Mark failed, Cancel | ⋯ menu beside Edit and Pause | Click, with a confirm | 6.2 | Classic | `src/lib/WorkMissions.test.ts` |
| P20 | Session Details actions (about 15) | Inspector actions plus ⋯ menu, all kept; the Details tab when the inspector is hidden | Click; ⌥⌘B / Ctrl+Alt+B | 3.5 | Both | `src/lib/SessionDetails.test.ts`; New: `src/App.inspector.test.ts` › the Details tab takes the column…, › the inspector chord and the header button toggle it… |
| P21 | Empty task sections | Hidden while empty; + Add subtask stays a button | Click | 1.4 | Classic | `src/lib/TaskWorkSections.test.ts` › starts a subtask and adds one under this task |
| P22 | Hosts tab | Accounts & hosts rail item | ⌘I / Ctrl+Shift+H, unchanged | 3.2, 4.1 | Both | `src/lib/HostsView.test.ts`, `src/lib/shortcuts.test.ts`; New: `src/lib/AppRail.test.ts` (the item is current on Hosts and names ⌘I), `src/App.destination.test.ts` › Accounts opens as one more overlay… |
| P23 | Assets | Toolkit rail item: a Skills tab (per-host drift) and an Assets tab with the same workspace; the old entry points open its Assets tab | Click; the rail's Toolkit; the quick switcher's asset rows | 3.16 | Both | `src/lib/AssetsPanel.test.ts`, `src/lib/AssetsWorkspace.test.ts`; New: `src/lib/Toolkit.test.ts`, `src/App.destination.test.ts` › Toolkit (step 3.16) is the Assets screen… |
| P24 | Sessions and Work filters | One engine; every facet of both kept (machine, time, tracker column, background agents, archived, saved views) | Same chips; ⌘⇧O / Ctrl+Shift+O | 3.7 | Both | `src/lib/WorkFiltersBar.test.ts`, `src/lib/work_view_persist.test.ts`, `src/lib/Sidebar.test.ts`; New: `src/lib/filter_schema.test.ts` › holds every 0.5.4 facet under the heading the schema names; › holds every 0.5.4 facet and the saved views… |
| P25 | About 15 ways to start a session | All kept; they open the same flow | ⌘N / Ctrl+Shift+N and every existing button | 1.9, 3.12 | Classic | `src/lib/NewSessionDialog.test.ts`, `src/lib/QuickSwitcher.test.ts` |
| P26 | Board instruction sentence | First-run hint and the ? shortcut sheet | ? | 1.4, 3.8 | Classic | `src/lib/WorkBoard.test.ts`, `src/lib/ShortcutSheet.test.ts` › ? opens the sheet with every live chord |
| P27 | Every 0.5.4 shortcut | The shortcut registry, `src/lib/shortcuts.ts` | Each chord, unchanged on Mac and Linux/Windows | 0.1 | Both | `src/lib/shortcuts.test.ts` |
| P28 | The classic layout itself | Settings → Appearance → Layout (Classic, New) until 13.1 | Settings | 0.3 | Both | `src/App.test.ts` › App layout switch |
| P29 | The answer card (Conversation, session row, ⌘K Approve), 1–9 and Esc | One approval card, the kit QuestionCard, with the command it approves in mono; "Answer in your own words…" dismisses and focuses the composer; after an answer the Inbox's next session opens, with Undo back | Click; 1–9; ⌘K Approve, unchanged | 5.9 | Both | `src/lib/AnswerPrompt.test.ts`, `src/lib/QuickSwitcherPalette.test.ts` (⌘K Approve); New: `src/lib/AnswerPrompt.test.ts` › the New layout draws the kit card…, `src/lib/ConversationPanel.test.ts` › card and agent tab stay in step…, › after Approve, focus moves to the next session… |
| P30 | The control-API confirmation dialog (the agent's starts and kills, M9.7; `mcp.confirm_destructive` calls) | The operator's own requests: a QuestionCard per request above Control's composer, with Approve / Deny and the call in mono; everyone else's, and the operator's whenever no transcript shows: the dialog, unchanged. On a hub both list and answer the hub's queue | Click | 9.2 | Both | `src/lib/McpConfirmDialog.test.ts`; New: `src/lib/ConfirmCards.test.ts` › the operator's kill is a card…, › with no transcript on screen the operator request falls back to the dialog, › on a hub, confirm:changed brings the hub's request in…; hub: `mcp::tools::tests::a_paired_device_lists_and_answers_the_operators_waiting_start` |

## Functions with no home on the boards

The UX critique found these on no canvas board. Each now has a named place,
and the step that lands it must keep it reachable.

| # | Today | Goes to | Reached by | Step | Now | Proof |
|---|---|---|---|---|---|---|
| H1 | Quick switcher pin | ⌘K command registry, New session mode | ⌘P / Ctrl+P on the highlighted project | 3.9 | Classic | `src/lib/QuickSwitcher.test.ts` |
| H2 | Quick switcher hide | ⌘K command registry, New session mode | ⌘⌫ / Ctrl+Backspace on the highlighted project | 3.9 | Classic | `src/lib/QuickSwitcher.test.ts` › Ctrl+Backspace with a query is left to the input… |
| H3 | Quick switcher groups | ⌘K command registry, New session mode | ⌘G / Ctrl+G, folding with ← → | 3.9 | Classic | `src/lib/QuickSwitcher.test.ts` › Ctrl+G moves the project to a group…; › a folded group is one row… |
| H4 | Find in the conversation | The agent tab's conversation view | ⌘F / Ctrl+F | 5.9 | Both | `src/lib/ConversationPanel.test.ts` (the same panel is the New layout's Conversation tab, 3.5) |
| H5 | Turn stepping | The agent tab's conversation view | [ and ] | 5.9 | Both | `src/lib/ConversationPanel.test.ts` (the same panel is the New layout's Conversation tab, 3.5) |
| H6 | Group by Project | The left list's grouping (project, state, host, agent, work) | The Group control on the Filters row (New); ⋯ → Group by (Classic) | 3.6, 3.7 | Both | `src/lib/Sidebar.test.ts` › groups multiple sessions under their project, `src/lib/Sidebar.test.ts` › state groups hold the same rows as the project tree, `src/lib/row_groups.test.ts`; New: `src/lib/filter_schema.test.ts` › Group picks the grouping… |
| H7 | Friendly names | Row title, with the tmux name as secondary text | Double-click to rename | 3.6 | Classic | `src/lib/Sidebar.test.ts` › shows the friendly name by default… |
| H8 | Row details | Comfortable density's Row details | The density setting | 3.6 | Classic | `src/lib/Sidebar.test.ts` › the details pill hides the second row line and persists, `src/lib/SessionRowDensity.test.ts` |
| H9 | Link review keys | The link sheet, opened from its Inbox item | j / k / y / n / Enter / Backspace / Esc | 3.3 | Both | `src/lib/LinkReview.test.ts`, `src/lib/shortcuts.test.ts` (per-view table) |
| H10 | Ten Settings sections (Hosts line, Hub, Projects, Setup guide, Notifications, Conversation composer, Control API, Diagnostics, Work, generated pages) | The one Settings tree (`src/lib/settings_tree.ts`): Appearance (with Setup guide), Accounts & hosts, Hub & sync, Projects, Notifications, Sessions & agents (composer), Control API, Error reports (Diagnostics), Work & trackers; every generated page has a leaf or sits under one | Settings, ⌘, | 7.1 | Both | `src/lib/settings_tree.test.ts` › reaches every generated page; `src/lib/SettingsDialog.test.ts` › each tree leaf shows its own screen…; `src/lib/SettingsDialog.hub.test.ts` |
