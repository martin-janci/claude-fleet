# Classic deletion map (step 13.1)

Step 13.1 deleted the Classic layout and its `ui.layout` switch. One row per
Classic file, or Classic part of a shared file, that went: what replaces it in
the one layout left, and the test that proves the replacement. The archived
checklist is [archive/parity.md](archive/parity.md).

| Classic file (or part) | New replacement | Proof |
|---|---|---|
| `src/lib/prefs.ts`: `uiLayout`, `UiLayout`, the `ui.layout.v2` pref | No switch; `ui.layout`, `ui.layout.v2`, `layout.center-collapsed` and `cf:session-ui` are cleared once at load | `src/lib/AppearanceSettings.test.ts` › has no Layout row (13.1), and forgets the retired layout keys at load |
| `vitest.setup.ts`: the Classic pin | Every test renders the one layout | the whole suite |
| `AppearanceSettings.svelte`: the Layout row | Removed with the switch | `src/lib/AppearanceSettings.test.ts` › has no Layout row (13.1)… |
| `App.svelte`: the center Details pane, its resizer and collapse | The Details tab, and Details filling the column with no session; the inspector | `src/App.inspector.test.ts`; `src/App.test.ts` › renders the sidebar and terminal panes, and no center pane (13.1) |
| `App.svelte`: the Session / Files / Hosts / Assets tabs and the Conversation / Terminal segment | The rail and the session tabs (`SessionTabs.svelte`) | `src/App.test.ts` › App: the Conversation tab; `src/App.hosts.test.ts` |
| `App.svelte`: the overlay flags (`filesMode`, `hostsMode`, `assetsMode`, `boardMode`, `accountsMode`, `detailsMode`, `controlMode`, `newLayout`) | The `destination` store read directly | `src/App.destination.test.ts` › keeps the terminal mounted, and one overlay at most… |
| `App.svelte`: the board overlay and its Esc | The board as a Work view | `src/App.destination.test.ts` › the board is a Work view, with no close and no Esc |
| `App.svelte` + `AssetsPanel` as an overlay of its own | Toolkit's Assets tab | `src/App.destination.test.ts` › Toolkit (step 3.16) is the Assets screen… |
| `AgentFab.svelte` (the floating ✦ button) | The rail's Control item and ⌘E; the `agent-fab` hint anchors on the rail item | `src/App.destination.test.ts` › the rail opens Control…; `src/App.hosts.test.ts` › ⌘E toggles Control…; `src/lib/AppRail.test.ts` › anchors the agent hint on Control… |
| `AgentPanel.svelte`: the floating sheet (close, Esc, grip, maximize); `agent_panel_size.ts`; `operator.ts`: `agentPanelOpen`, `openAgent`, `closeAgent`, `toggleAgent` | Control's Chat tab (the panel inline); `ensureAgent` keeps the re-entrancy guard | `src/lib/AgentPanel.test.ts` › Control's chat has no close or grip…; `src/lib/operator.test.ts` › ensureAgent re-entrancy |
| `today.ts`: `todayOpen` | Control's Today tab | `src/App.destination.test.ts` › ⌘E opens and closes Control; ⌘⇧T opens its Today tab |
| `session_ui.ts` (per-session center width) | Gone with the center pane | — (state cleared at load, see the first row) |
| `AddHostPicker.svelte` | The Add host wizard, which reads the SSH config | `src/lib/HostsView.test.ts` › + Add host opens the wizard, which reads the SSH config for hosts; `src/lib/AddHostWizard.test.ts` |
| `HostsView.svelte`: the list-only view and its empty-detail skew sentence | The Hosts table first; the skew sentence on the empty table (`HostsTable` `emptyText`) | `src/lib/HostsView.test.ts` › the empty table shows the connection banner's sentence… |
| `composer_overflow.ts` and ConversationPanel's More row | Three chips, the rest under ⋯ | `src/lib/ConversationPanel.test.ts` › shows three chips and puts the rest under ⋯… |
| `ConversationPanel.svelte`: the pane spinner as the indicator's text | "Thinking · what runs"; with nothing running, the spinner line is the label's tooltip | `src/lib/ConversationPanel.test.ts` › a working row shows the indicator and polls the pane for the spinner text |
| `AnswerPrompt.svelte`: the chip-row card | The kit QuestionCard | `src/lib/AnswerPrompt.test.ts`; `src/lib/Sidebar.test.ts` › offers the numbered choices… |
| `SidebarFilters.svelte`: the two-row header (search, scope select, Filters, Needs you, Select), the Group by segments | One Filters row: search, Filters panel (Needs you, scope), Group select, ⋯ (Select several) | `src/lib/Sidebar.test.ts` › the "Needs you" pill…, › select mode…, › the Group select offers…; `src/lib/ScopeSelector.test.ts` |
| `Sidebar.svelte`: shared sessions kept in the tree | The Shared with me group | `src/lib/Sidebar.test.ts` › the New layout lifts shared sessions into their own group |
| `WorkFiltersBar.svelte`: the views row and its own panel | The Filters section; saved views in its panel | `src/lib/WorkFiltersBar.test.ts` |
| `WorkTree.svelte`: the Review tab and the List / Grouped chips | The Review count; the Group select | `src/lib/WorkTree.test.ts` |
| `WorkTaskDetail.svelte`: Open, Continue and Start new buttons | The action bar's split button (`WorkButton`), which now also offers Open it on E_EXISTS | `src/lib/WorkTaskDetail.test.ts` › the action bar opens the live session…, › Continue refused because the work is live…; `src/lib/resume_gates.test.ts` |
| `TaskWorkSections.svelte`: a subtask's Start button | The subtask's split button | `src/lib/TaskWorkSections.test.ts` › starts a subtask through its split button… |
| `WorkMissions.svelte`: flat Complete / Mark failed / Cancel buttons | ⋯ menu beside Edit and Pause | `src/lib/WorkMissions.test.ts` › the ⋯ menu for the moves that end a mission |
| `NewSessionDialog.svelte`: the Type toggles and profile field | Agent picker, account and profile picker, Run row | `src/lib/NewSessionDialog.test.ts` › NewSessionDialog in the New layout |
| `ResumeButton`, `WorkButton`: "Resume", "Start" | "Continue", "Start new" | `src/lib/TaskStartButton.test.ts` › the words |
| `OnboardingCard.svelte` (the sidebar's setup checklist) and its Sidebar mount; `hub_disabled.test.ts`'s setup-checklist cases; `App.hosts.test.ts` › the onboarding card's Add a host | Get started and the first-run tour (`FirstRun.svelte`, step 10.5), mounted unconditionally by Sidebar | `src/lib/GetStarted.test.ts` (its host row opens the Hosts view) |
| `HostDetail.svelte`: no panes outside fleet, and the old words ("path is not a fleet worktree", "no fleet project for this path") for a conversation it cannot resume | Lost and found with proposals (4.12): Adopt and Restore into | `src/lib/HostDetail.test.ts` › lists a pane fleet did not start…, › Restore into copies the conversation…, › a candidate in a project but not at a resumable path has no Resume, only Restore into |
| `StartPopover.svelte`: no start-rule offer and no "Picked by a start rule" | The rule offer (8.11) | `src/lib/start_rules.test.ts` › offers the rule and adds it, › says when a rule picked the repository |
| `ResourcePage.svelte`: Settings › Devices' inline create form | The pair_device wizard (10.12); the inline form's test folded its code, link, Halo and QR checks into the wizard's | `src/lib/pages/DevicesPage.test.ts` › opens the pair_device wizard… |
| `SettingsDialog.svelte`: no Take the tour | Take the tour (10.5), which calls `startTour` | `src/lib/tour.test.ts` (no test clicks the Settings button itself) |
| `TerminalView.svelte`: the session bar's Terminals tab ignored | The Terminals tab (5.3) | `src/lib/TerminalView.shells.test.ts` › the session bar's Terminals tab |
| Classic-only gates main added after 13.1 branched, in `SessionDetails` and `WatchView` (WatchSummary, 11.11), `TodayView` (MorningBrief, 9.11), `WorkMissions` (ReleaseNote, 9.11), `SessionRowItem` (the starting row, 5.14), `AgentPanel` (HandoffCards, ControlRouteReceipts, `thinkingAs`; 9.3, 9.6, 9.9, 9.13), `NewSessionDialog` (Run in background), `WorkTree` (the review count after the tablist) | Their New branch, now unconditional | each component's own test file |
| Classic-only branches in `FileViewer`, `FilesPanel`, `ForkSheet`, `HostDetail`, `MicToggle`, `OrgMembers`, `QuickSwitcher`, `ResumeDialog`, `SessionRowItem`, `StartPopover`, `TerminalView`, `TidyReview`, `ToolLine`, `TransferSheet`, `WorkReview`, `attention_facts.ts` | Their New branch, now unconditional | each component's own test file |
