# Phone parity audit for 14.13

Step 14.13 (phone parity sign-off) is done when the phone analysis
checklists are 100% covered, Martin has used New navigation on the phone for
a week, every screen has been checked in light against MobileLight (Inbox,
question card, My work, More), and New becomes the default. The old
navigation goes one release later. This audit, taken on 2026-10-08, lists what
stands between the checklists and that bar.

Refreshed on 2026-10-10 against fleet-mobile main 1ffaba2 and claude-fleet main 6c0853eb (gap plan step G6.2).

Sources:

- The checklists are the three mobile analysis notes that the plan's M14
  section names (`/mnt/project-files/mobile-redesign/`). They are not checked
  into either repo:
  - `analysis-sessions.md`: pairing (§A), the Sessions tab (§B, §C) and its sheets (§D).
  - `analysis-session.md`: one session (Part A), New session (Part B), and the closing "Parity checklist (nothing may be lost)".
  - `analysis-work-hosts-settings.md`: Work, Files, Hosts, Settings, and the bottom navigation (§G).
- The M14 step rows in claude-fleet
  `docs/ux/2026-10-08-orbit-fleet-redesign/transition-plan.md` (14.1–14.22).
  Their requirements that the notes do not cover are in section 12.
- fleet-mobile `origin/main` at `feef8e6`, including PRs #113–#143.
- Open fleet-mobile PRs: #142 (14.16), #144 (4.10), #145 (14.10 meters,
  stacked on #144) and #146 (11.10 member actions).
- claude-fleet `origin/main` and its open PRs, used for hub steps that phone
  items wait on.

Every claim of coverage was checked by grepping fleet-mobile `origin/main`, or
the open PR's branch, for the screen or string. A PR title alone was not
taken as proof. Paths are relative to
`shared/src/commonMain/kotlin/dev/claudefleet/mobile/`.

Status words:

- **done**: on `origin/main` behind `PhoneLayout.New`.
- **open PR**: in an open fleet-mobile PR.
- **gap**: phone work that nothing blocks.
- **blocked**: waits on hub work, named in the row.

Why most rows are already covered: each 14.x PR (#114–#141) carried its own
parity table against these notes, and the Classic screens' view models are
reused under New. Examples are `SessionsViewModel` and `BulkViewModel` (14.3),
`RepoViewModel` (14.4), `NewSessionUiState` (14.6) and `MyWorkViewModel`
(14.9). Since gap plan G5.4 New is the default (`ui/PhoneLayoutPref.kt:13-14`,
`loadPhoneLayout` returns Classic only when the stored value is `classic`).

## 1. Bottom navigation (analysis-work-hosts-settings.md §G; step 14.2)

| Item | Status | Evidence |
|---|---|---|
| Bar Inbox · Sessions · Control · Work · More behind the "New navigation" switch | done | `ui/Navigator.kt:88`, `ui/kit/BottomBar.kt:55`; #115 |
| Only Inbox carries a badge | done | `kit/BottomBar.kt` takes one `BottomBarBadge`; `KitTest` (#114), `NewLayoutTest` (#115) |
| One New session button; the ✦ agent button becomes Control | done | `App.kt:1423-1432` ("Chat with Control"); #115, #120 |
| Files, Hosts and Settings move into More, each with a live line | done | `App.kt:1438-1478` (`MoreEntry` rows); #115 |
| Usage → More › Accounts and usage; Company → More › Organisations | done | `App.kt:1453`, `App.kt:1470` |
| Today → Inbox header | done | `ui/InboxScreen.kt:76` |
| Missions → Control › Missions and More › Automation | done | `App.kt:1438`, `App.kt:1456` |
| Back from a pushed screen returns to More | done | `NewLayoutTest`, `BackGestureTest` (#115) |
| New becomes the default; Classic is removed one release later | done | `ui/PhoneLayoutPref.kt:9-14`: New is the default, and Classic only for a phone that chose it (gap plan G5.4). Classic goes one release later, as planned |

## 2. Pairing (analysis-sessions.md §A, "Pairing parity list"; step 14.11)

| Item | Status | Evidence |
|---|---|---|
| QR scan entry (primary) | done | `ui/PairScreen.kt:215` "Scan the code"; #117 |
| Typed hub address | done | `ui/PairScreen.kt:291` |
| Typed 8-character code | done | `ui/PairScreen.kt:312`, folded under "Enter the code by hand" (`:187`) |
| Pair button | done | #117 (primary once a code is typed) |
| In-progress state | done | `ui/PairScreen.kt:264` "Contacting …", with Cancel |
| Error banner with reason and Dismiss | done | `ui/PairScreen.kt:141` "Couldn't pair" |
| Signed-out banner with reason and Dismiss | done | `ui/PairScreen.kt:135` (also "You forgot this hub") |
| Success screen: hub URL, device name, access level | done | `ui/PairScreen.kt:337` |
| "Open the fleet" | done | `ui/PairScreen.kt:446` |
| Expiry and one-shot note; revocation is on the hub | done | `ui/PairScreen.kt:195` |
| Camera scanner | done | `ui/PairScreen.kt:161` (`QrScannerView`) |
| Paste for the code (14.11) | done | `ui/PairScreen.kt:182` "Paste a pairing link" |
| Notification-permission step (14.11) | done | Paired screen Allow / Not now; #117 |
| Re-pair keeps the hub address (14.11) | done | `data/Session.kt:24`; `PairingTest` (#117) |
| Hex field while the first fleet check runs (14.11) | done | `ui/FleetCheck.kt:56` |

## 3. Sessions tab (analysis-sessions.md §B, §C; step 14.3)

| Item | Status | Evidence |
|---|---|---|
| Connection subtitle: live, offline, reconnecting | done | `ui/PhoneSessions.kt:307` |
| Search with clear, and scopes | done | `ui/PhoneSessions.kt:168` (All, Sessions, Projects, Hosts, Tickets) |
| ⋮: Today, Tickets, Missions, Select sessions | done | `ui/PhoneSessions.kt:471-474` |
| "Needs you" chip with count | done | moved to the Inbox badge and `ui/SessionFiltersSheet.kt:124` "Only sessions that need you" |
| Filters chip opens the sheet | done | `ui/SessionFiltersSheet.kt:56` "Filters and grouping" |
| Group by project, urgency or work (plus host) | done | `ui/PhoneSessions.kt:148` |
| Pinned "N sessions need you" | done | moved to the Inbox, sorted by when each asked (`model/Triage.kt:139`); `PhoneSessionsTest` (#120) |
| Host heading: fold, name, "N need you", count, unreachable | done | `ui/PhoneSessions.kt:516`, `:531` |
| Project subheadings, and ticket subheadings under Work | done | #120 (kept under the host in the Project and Work views) |
| Row: status dot, title, line two (activity or what it waits on), age | done | `ui/kit/PhoneRow.kt:43`, `ui/PhoneSessions.kt:88-94` |
| Row: unlabelled two-segment bar under the title | gap | still not on the New row (`ui/kit/PhoneRow.kt`, `ui/PhoneSessions.kt`). The context % is still in the session's strip (`components/StatusStrip.kt:130`). Drop it on purpose or bring it back |
| Row: ticket chip | done | work chip, #120 |
| Row: CI badge | done | `ui/PhoneSessions.kt:123` "PR #476 ✓" |
| Row: external and shell rows | done | `model/Triage.kt:66-70` |
| Long-press enters select | done | `ui/PhoneSessions.kt:350` |
| Pull to refresh | done | `ui/PhoneSessions.kt:311` (`OrbitPullToRefresh`) |
| Bulk select: count, Send, Kill, ✓ per row | done | `ui/PhoneSessions.kt:733-757` |
| Bulk outcome: per-failure name and reason, what stays picked; Retry (14.3) | done | `ui/PhoneSessions.kt:808`, `:839`; `BulkViewModel.kt:53`, `:173` |
| Search: project hit starts a session | done | `ui/PhoneSessions.kt:682` |
| Search: filters and grouping still apply | done | #120 |
| Offline: stale strip with its age | done | `components/Banners.kt:48-52` |
| Reconnecting: attempt count and reason | done | `components/Banners.kt:80-81` |
| Empty state with New session | done | `ui/PhoneSessions.kt:324` |
| Error banner with Dismiss | done | `ui/PhoneSessions.kt:308` |
| Filters sheet: Active within / Idle beyond, durations | done | `model/SessionFilters.kt:20`, `:36-37` |
| Filters sheet: State, host, project, work status | done | the same sheet as Classic (#55, #120) |
| Grouping and filters survive a restart (14.3 verify) | done | `NewSessionsTabTest.grouping_and_filters_survive_a_restart` (#120) |

## 4. Sessions sheets (analysis-sessions.md §D; steps 14.3, 14.15, 14.16)

| Item | Status | Evidence |
|---|---|---|
| Today: "Since midnight · …" and refresh | done | `ui/TodaySheet.kt:251-259` |
| Today: summary chips as filters, Clear | done | #65; the same sheet under New |
| Today: Waiting on me by ticket, → opens the session; agrees with the Inbox | done | `App.kt:1677` |
| Tidy: reason headers, candidates, per-row check and action, Apply to N | done | `ui/PhoneTidy.kt:76`, `:85` |
| Tidy: nothing ticked; "Suggested by rule" (14.15) | done | `ui/PhoneTidy.kt:192`; `TidyTest` (#135) |
| Tidy result with reasons; Undo and Retry (14.15) | done | #135 |
| Tickets: search or paste a key or URL, Filters, Sort, sections with counts | done | `ui/TicketsSheet.kt:119`, `:208` |
| Ticket detail inline with criteria and Resume | done | `App.kt:1065` (`confirmResume`) |
| Ticket filters: lists, status, tracker column, Sessions | done | `model/TicketFilters.kt:12` |
| One ticket with its tasks as one sheet (14.15) | done | `ui/PhoneTicketSheet.kt:36-48`; the ticket chip and the Tasks chip both open it (`ui/SessionScreen.kt:705`) |
| Missions list with progress, state and Refresh | done | `ui/MissionsSheet.kt:85`, reached from Control (#115) |
| Missions list in Orbit style (Running, Paused, Drafts, Done this week) | done | `ui/OrbitMissionsScreen.kt:177`, `:216-218`; #142 merged |
| Pause all | done | `ui/MissionsSheet.kt:98`, `net/HubClient.kt:1131`. It says what stops before it stops anything (`ui/OrbitMissionsScreen.kt:157`, `:224-235`; #142) |
| Mission detail: goal, meta, autonomy, spent, Waiting for you, Next steps with Go | done | `ui/MissionsSheet.kt:168`, `:177`, `:201`, `:204`, `:238` |

## 5. One session (analysis-session.md Part A and its parity checklist; steps 14.4, 14.5, 14.14)

| Item | Status | Evidence |
|---|---|---|
| Header: back, name, host, find, refresh, ⋮ | done | the same `SessionBar` (#119); `ui/SessionScreen.kt:1336` |
| Status and label, ctx %, cost, model, ↑/↓ turns | done | `components/StatusStrip.kt:130`, `ui/TurnNav.kt` |
| Ticket chip (solid or dashed) and "Tasks · N" | done | `ui/SessionScreen.kt:1450` |
| User bubbles and markdown | done | #67 |
| Edit, Run, Read and Search rows, running and failed | done | `ui/ToolCalls.kt` (#67) |
| Explore subagent card; per-turn ⋮ | done | `model/Conversation.kt:321`; `ui/ReplyActions.kt` (#78) |
| "Older turns are not shown." | done | `ui/SessionScreen.kt:2590` |
| Question card: title, tool, command, numbered options, none pre-selected | done | `ui/QuestionCard.kt:51`; `QuestionCardNeverListTest` (#119) |
| Question card: Enter/Esc, Show terminal | done | the keys are on the agent tab, labelled; `ui/QuestionCard.kt:157` "Show in <agent>" |
| Question card: composer hint | done | `ui/QuestionCard.kt:154` "Answer in your own words…" |
| Trust card: Enter/Esc, Show terminal | done | `ui/QuestionCard.kt:64` |
| Composer: schedule (send later) | gap | send later works and the hub holds it (`deferred_prompts`): `ui/SessionLater.kt:233-243`, time choices at `:152-155`, `model/QueuedPrompts.kt:9`. It is reached from ⋮ "Send later…" (`ui/SessionMenu.kt:81`), not a clock in the composer. The clock on main is still "Draft history" (`ui/SessionScreen.kt:2752`) |
| Composer: field, Send (Queue while working), Stop | done | `ui/Recovery.kt:77`, `ui/SessionScreen.kt:2812` |
| Composer: sending caption, Not sent (Retry, Edit), read-only caption, quick replies | done | `ui/Recovery.kt:80`; #122 |
| States: working, waiting, stuck on trust, idle, failed | done | `ui/kit/StatusWord.kt`; failed card in `ui/Recovery.kt:116-145` |
| States: hub unreachable, read-only, sending, send failed, empty | done | `ui/SessionScreen.kt:653` |
| State: loading (skeleton after 400 ms, 14.12) | done | `ui/SessionScreen.kt:751-752`, `:2620`; `ui/kit/PhoneStates.kt:245-246` waits before it shows |
| Repair report: steps, warning | done | in the conversation, `ui/Recovery.kt:83-86` (#122) |
| Ticket sheet: id, state, title, why, criteria, Copy, Open in browser, Clear, Ask for a handover, Rename | done | `components/TicketCardBody.kt:53`, `ui/WorkSheet.kt:96`, `:182` |
| Suggestion sheet: reason, Confirm, Not this | done | `model/SessionRow.kt:78`, `net/HubClient.kt:1362` |
| Tasks sheet: Add task…, Active, primary ★, Open task, Make primary, Remove | done | `ui/SessionTasksSheet.kt:68`, `:97`, `:101` |
| Details: Refresh, worktree link, facts, tags, Same worktree, timeline, tasks | done | Details tab, `ui/SessionDetailsSheet.kt:130` (#119) |
| Move sheet: targets, carries, warnings, Keep the source running, When it is idle, Move now; no host pre-selected (14.5) | done | `ui/MoveSheet.kt:94-100`, `:167` |
| Files tab: Changes (M/A/D, staged) | done | `RepoBody` in the Files tab (#119) |
| Files tab: History (hash, author, time, refs, Older commits) | done | `ui/RepoScreen.kt:343` |
| Files tab: Files (Find a file) | done | `ui/RepoScreen.kt:409` |
| Files tab: Diff (hunks, File link, line numbers) | done | #119 `a_diff_numbers_its_lines_from_each_hunk` |
| Files tab: Commit (body, files, Copy hash) | done | `ui/RepoScreen.kt:646` |
| Files tab: File (Send to Downloads) | done | `ui/RepoScreen.kt:669` |
| Files tab: +/− counts and ahead/behind on Changes (board) | done | per-file counts `model/Repo.kt:19-27`, `ui/RepoScreen.kt:298-305`; "N ahead of main · N behind main" `model/Repo.kt:103-117`, `ui/RepoScreen.kt:165` |
| Agent tab named after the session's agent (14.4) | done | `ui/SessionTabs.kt:49-61` reads the row's `agent` (contract 11). The Files tab's "Ask Claude Code to commit" still uses `DEFAULT_AGENT_NAME` (`ui/RepoScreen.kt:249`) |
| Terminal view behind "Show terminal" | done | agent pane (#119); full screen (#136) |
| ⋮ menu: Rename, Ticket and tasks, Move, Repair, Recreate, Copy tmux attach, Details, Archive, Kill last (14.14) | done | `ui/SessionMenu.kt:68-87` |

## 6. New session (analysis-session.md Part B; step 14.6)

| Item | Status | Evidence |
|---|---|---|
| Host choice with an unreachable state | done | #118 "Signal lost · cannot start there now" |
| Search projects, choose one | done | #118 |
| Add a project on <host>… | done | `ui/NewSessionScreen.kt:186`; the wizard in `ui/AddProjectWizard.kt` (#137) |
| Change project (N) | done | `ui/NewSessionWizard.kt:380` |
| Worktree switch; Branch with validation; Base branch; Name with help | done | `ui/NewSessionWizard.kt:456`, `:465` |
| Start stays off on an invalid branch (14.6 verify) | done | `an_invalid_branch_holds_project_and_keeps_start_off` (#118) |
| Create, Creating…, Pulse after 400 ms | done | `ui/kit/Pulse.kt:38` |
| Pulse ticks off the real steps | done | `ui/NewSessionWizard.kt:327-336` ticks the hub's `start:progress` steps (`model/StartProgress.kt`). Ticket mode and older hubs fall back to the fixed names |
| Background agent on <host>… | done | `ui/NewSessionScreen.kt:389`; the wizard's Where and Review steps |
| Start <ticket>; Also start in (up to 7, org rule) | done | `ui/NewSessionWizard.kt:531` |
| Multi-start confirm: org, list, Start N, Cancel | done | `ui/MultiStartSheets.kt:89` |
| Multi-start result: status per project, Open, Done | done | `ui/NewSessionViewModel.kt:639` |
| Board extras: Jev host proposal, drafted branch, Start from a branch, first message, account row | blocked | Start from a branch, first message and the account row have landed (`ui/NewSessionWizard.kt:418`, `:723-729`, `:689-701`; G5.6). The drafted branch shows "Drafted from KEY" and Clear but has no Regenerate (`:379-381`). The Jev host proposal waits on the hub: `propose_host_placement` is desktop-only (claude-fleet `src-tauri/src/backend/verdicts.rs:1331`) |

## 7. Work (analysis-work-hosts-settings.md §A; step 14.9)

| Item | Status | Evidence |
|---|---|---|
| Title and count; search "Key or title" | done | `ui/PhoneWork.kt:273` |
| Review with count | done | moved to the "To review N" chip, `model/FilterFacets.kt:140` |
| Saved views; Filters | done | `App.kt:1144`; #130 |
| Assigned to me and To review toggles | done | in the sheet, `model/FilterFacets.kt:138-140` |
| Organisation header (fold, name, count, swatch); group subheader | done | #130 |
| Task row: dot, key, title, meta in status words, needs you; tap opens detail | done | `ui/PhoneWork.kt:125-175` |
| "N archived tasks hidden · Show archived" | done | `ui/PhoneWork.kt:533` |
| Placement rules sheet | done | `ui/PhoneWork.kt:406` |
| Empty | done | `ui/PhoneWork.kt:536` "No tasks here" |
| Offline "as of" | done | `ui/PhoneWork.kt:330` (`HubBanner`) |
| Task detail: status, assigned, org, group by rule, note, description, last, PR | done | `ui/PhoneWork.kt:199-201`, `:636` |
| Continue, Start here, Place in group… | done | `ui/PhoneWork.kt:566`, `:717`; `App.kt:1921` |
| Active, Suggested (Link or Not this in place), Past with Summarize and Open PR | done | `ui/PhoneWork.kt:765`, `:774-831` |
| Place in group: group, note, existing groups, back to the rule's group | done | `ui/PhoneWork.kt:718` |
| Summary inline, "✎ Drafted by … · Regenerate · Clear" | done | `ui/PhoneWork.kt:216-222`, `:818-822` |
| To review: Confirm, Reject, Change…; cross-org as a warning outside the bulk action | done | `ui/PhoneWork.kt:224`, `:762`, `:903`, `:948` |
| Pull requests and Blocked (6.7) | done | #134 |

## 8. Files (analysis-work-hosts-settings.md §B; step 14.10)

| Item | Status | Evidence |
|---|---|---|
| Count and total size | done | `filesHeadline` in `ui/MorePlacesScreen.kt` (#128) |
| Size, host, session, age and note per file | done | #128 |
| Transfer with real size (Progress ring) and Cancel | done | `ui/MorePlacesScreen.kt:467`, `ui/kit/ProgressRing.kt:30` |
| Failure reason; Retry when the host is back | done | `ui/MorePlacesScreen.kt:469` |
| Remove with the every-device warning | done | in the row's ⋮ (#128) |
| Actions: Save, Share, Open | done | #128 |
| Empty | done | `ui/MorePlacesScreen.kt:407` |

## 9. Hosts (analysis-work-hosts-settings.md §C; step 14.10)

| Item | Status | Evidence |
|---|---|---|
| Name, reachability, last seen, session count, transport, hidden, versions, not probed yet | done | `ui/MorePlacesScreen.kt:69`, `:200`; #128 |
| Signal lost with Try again; Recovery plan | done | `ui/MorePlacesScreen.kt:211`; `ui/HostsViewModel.kt:124` |
| Version drift ("behind") | done | `behindHosts`, #128 |
| The old Refresh | done | moved to Try again and Check now per row, plus pull to refresh (`ui/HostsViewModel.kt:45`, `:75`) |
| Host sheet: pinged, versions, Re-probe, sessions | done | `ui/HostDetailSheet.kt:80` |
| After a reboot: Check again, lost sessions, untracked conversations | done | `ui/HostDetailSheet.kt:94`, `:103`, `:137-138` |
| Empty | done | `ui/HostsScreen.kt:81` |

## 10. Accounts and usage (analysis-work-hosts-settings.md 94/95; steps 14.10, 4.10)

| Item | Status | Evidence |
|---|---|---|
| Subtitle, 24 h / 7 days / 30 days, by host, by day, by session | done | `ui/MorePlacesScreen.kt:238`, `:300`, `:307`; `ui/UsageViewModel.kt:20` |
| Claude accounts | done | #128 |
| Quota meters with their numbers (14.10 verify) | done | `ui/MorePlacesScreen.kt:257-323`, `:360`; #145 merged |
| Paused row names the window and when it resets | done | `ui/PhoneSessions.kt:136-148`; #144 merged |
| Switch account and Wait on a paused row | done | Inbox row `ui/InboxScreen.kt:405-415`; in the session `ui/SessionLater.kt:87-136`. `check_account_headroom` now reaches phones (`net/HubCapabilities.kt:383`, `net/HubClient.kt:549`) |

## 11. Settings and organisations (analysis-work-hosts-settings.md §D; steps 14.11, 14.17)

| Item | Status | Evidence |
|---|---|---|
| Hub, client name, access, app version, hub version | done | summary on top, and under General (#117) |
| Usage and Company rows | done | `App.kt:1453`, `App.kt:1470` |
| Every hub page under General / Sessions / Work / Organisations / System | done | `ui/SettingsGroups.kt:12-16`; `SettingsGroupsTest` |
| Proposed changes with their count; as an Inbox item | done | `ui/OrbitSettingsScreen.kt:164`; `ui/OrbitOrgsScreen.kt:305` |
| Automation, Limits, Projects, Work graph pages with History | done | `App.kt:1555`; #64, #89 |
| Decisions (Jev): off, shadow, assist, never auto; who opted in | done | #133 (`offeredOptions` drops `auto`) |
| Forget this hub, asked first; after a forget, Pair says why | done | `ui/OrbitSettingsScreen.kt:96`; `ui/PairScreen.kt:135` |
| Company: orgs, role, counts | done | `ui/OrbitOrgsScreen.kt:55` |
| Org detail: sessions (opens Sessions filtered), spend, hosts, members, devices, trackers, budget meter | done | #133 |
| This phone: notification kinds, Dark / Light / System theme | done | `ui/PhoneSettings.kt:14-16` |
| New navigation switch at the foot of Settings | done | #117 |
| Quiet hours | done | fleet-mobile #153 after claude-fleet 11.9 (#633). The phone follows the hub's Phone column and quiet hours. The Phone column's default includes Blocked, as before the matrix, so a stuck session, a host down or an account at its limit still reaches the phone unless someone unticks it |
| Done notifications; fingerprint lock | done | Done kind `notify/NotifyKinds.kt:15`, off by default; lock `ui/PhoneSettings.kt:71`, `ui/PhoneLock.kt:40-51` (Android and iOS actuals), switch at `App.kt:2058` |
| Organisation automation playbooks (MobileOrgsSettings) | blocked | claude-fleet 8.4 has merged. The phone shows playbooks as plain hub settings fields; the board's toggles with "Ran N times this week" need a per-playbook run count the hub does not send |
| Member actions on Company | done | `ui/MembersSheet.kt:41-56` (change role, remove with the share choices); #146 merged |
| Share and watch | done | `ui/ShareSheet.kt:63`, Narrow to watch at `:184`, `model/Sharing.kt`; opened at `App.kt:2917` (contract 15) |

## 12. Step requirements the notes do not cover (14.5, 14.7, 14.8, 14.12, 14.14, 14.16, 14.18–14.22)

| Item | Status | Evidence |
|---|---|---|
| 14.5 failed card, Not sent with Retry, repair result, Move with no host chosen | done | `ui/Recovery.kt`; `ui/MoveSheet.kt:167`; `MobileRecoveryTest` (#122) |
| 14.7 coordinator chat in the Control tab | done | the Control tab is the coordinator's conversation with its header, handoff chips, other sessions' forms and confirm cards (`App.kt:1774-1797`, `ui/ControlChat.kt:340-499`) |
| 14.7 ChatForm answered, declined and expired states; one-step form as an inline card | done | `model/ChatForms.kt:36-60`; `ui/ChatFormCard.kt` (#121, 10.8) |
| 14.7 multi-step form full screen | done | a long or secret form opens paged, one step at a time, at full height with "‹ Chat" back (`ui/OrbitChatForm.kt:74-85`, `:124-126`, `:237-250`, `:378`). It is a full-height sheet, not a pushed screen |
| 14.7 building state with a skeleton and Atom | done | `ui/kit/Loaders.kt:496` (`Atom`); `ui/ChatFormDraft.kt:60-68`, `:135` streams the draft with "Writing the form · reading …" |
| 14.7 handoff chips | done | `ui/ControlChat.kt:528`, `:562`; drawn above the composer at `App.kt:1793` |
| 14.8 no Approve action; lock screen hides the command; the tap lands on the card | done | `notify/NeedsYouContent.kt:42-47`; `NotificationsNeverAnswerTest` (#124) |
| 14.12 Signal lost and Gravity well banners on every screen | done | `HubBanner` on Sessions (`ui/PhoneSessions.kt:351`), the session (`ui/SessionScreen.kt:693-694`), Hosts and Files (`ui/MorePlacesScreen.kt:180`, `:508`), New session (`ui/NewSessionWizard.kt:170`) and Work. `ConnectionBanner` is left on Classic screens only. `ReconnectingPanel` is still Classic only (`ui/SessionsScreen.kt:849`) |
| 14.12 Hex field for the fleet check | done | `ui/FleetCheck.kt:56` |
| 14.12 Hex field after a repair | blocked | the Hex field and step list are now shown (`ui/RepairWait.kt:22-41`, `App.kt:2747-2749`), but nothing ticks: the hub answers a repair in one call with no steps |
| 14.12 Radar while adding a host | done | `ui/AddHostScreen.kt:34-48`, `ui/kit/Loaders.kt:301`; opened at `App.kt:1626`. Contract 13 lets the owner's trusted phone call `add_host` (claude-fleet `crates/fleet-core/src/mcp/guard.rs:2839`) |
| 14.12 Galaxy for the first import | done | `ui/FleetCheck.kt:107-119` with real counts (`:82-89`); shown at `App.kt:2279` |
| 14.14 shells 0..N with a key bar (Esc, Tab, ⌃C) | done | `ui/Terminals.kt:98`, `:414` (#127) |
| 14.14 / 14.21 Ctrl, arrow, ⇧Tab and Alt keys | blocked | arrows, ⇧Tab and the Ctrl row are built where the hub lists them (`ui/Terminals.kt:108-117`, `ui/Landscape.kt:147-160`, `net/HubCapabilities.kt:409`). Alt is left: the hub refuses every Meta chord |
| 14.14 find with scopes | done | `ui/TurnNav.kt:72` (#127) |
| 14.16 spend ask with Approve and Deny, neither pre-selected | done | `ui/OrbitMissionDetail.kt:220`, `:461-494` (two outlined buttons, nothing pre-selected); `ui/MissionsViewModel.kt:176`, `:200` |
| 14.16 background agent screen (project, agent, read-only, stop-after) | blocked | project, read-only, stop after and a spend limit are built, as a sheet (`ui/NewSessionScreen.kt:466-530`). The agent is always Claude: the hub refuses a Codex background agent (`model/Recovery.kt:72-77`) |
| 14.18 update card, Progress ring, signature check, wordmark, hub-older banner | done | `ui/UpdateScreens.kt`, `update/Updates.kt`; `UpdatesTest` (#123) |
| 14.19 welcome and "I don't have a hub yet" | done | `ui/FirstInstall.kt:160` (#141) |
| 14.19 install fleet-agent on a found host, Pulse and Sonar to the first heartbeat | done | `ui/FirstInstall.kt:339`, opened from Hosts and Add a host (`App.kt:1592`, `:1634`, `ui/AddHostScreen.kt:52-61`). Contract 13 lets the owner's trusted phone call `install_agent` (claude-fleet `crates/fleet-core/src/mcp/guard.rs:2839`) |
| 14.19 installing tmux | done | the install job's `tmux` step installs it when missing (`ui/FirstInstall.kt:243-247`) |
| 14.20 Add a project in three steps with Data rain | done | `ui/AddProjectWizard.kt`, `ui/kit/Loaders.kt:393` (#137) |
| 14.20 "A folder already on the host" source | done | `ui/AddProjectWizard.kt:446-455`. A folder is added on the hub's own machine only (`:122-125`) |
| 14.20 Connect a tracker with a secret field | done | `ui/OrbitTrackersScreen.kt:80`, `:321-322`; `net/HubClient.kt:640-652` (`work_admin` for the owner's trusted phone, contract 13) |
| 14.21 two panes, split diff, split terminals, agent full screen | done | `ui/Landscape.kt:52`, `:80`; `MobileLandscapeTest` (#136) |
| 14.21 hide the status bar on iOS | done | `shared/src/iosMain/.../ui/SystemBars.ios.kt`, `MainViewController.kt:90`, `iosApp/iosApp/ContentView.swift:29` (6be202a) |
| 14.22 help picker, tour, tips, practice fleet, Learn, guides with Undo | done | `ui/help/Help.kt:11-12`; `HelpNeverActsTest` (#125) |
| 14.22 a lesson inside Control puts its prompts in the composer | done | `ui/help/Help.kt:166-171`, `ui/help/HelpScreens.kt:428-441`; a tap fills the composer and sends nothing (`App.kt:2119`) |

## 13. Light check and Martin's week (14.13 verify)

| Item | Status | Evidence |
|---|---|---|
| Inbox, question card, My work and More checked in light against MobileLight | gap | the theme switch exists (`ui/PhoneSettings.kt:15`), and `LightCheckTest` (commonTest `ui/theme/LightCheckTest.kt`, G5.9) holds every text colour at 4.5:1 on every ground. No screen-by-screen check against MobileLight is recorded |
| Martin uses New on the phone for a week | gap | not started on record |

## Counts

| Status | Rows |
|---|---|
| done | 184 |
| open PR | 0 |
| gap | 4 |
| blocked | 5 |

## Gaps

What is left before Martin's phone sign-off, grouped by who has to act.

### Lane M (fleet-mobile), no hub work needed

1. Decide on the row's unlabelled two-segment bar (§3): drop it on purpose or bring it back.
2. A clock in the composer for send later (§5). Send later itself works and the hub holds it; it is reached from ⋮ only.
3. Regenerate on the drafted branch in New session (§6).

### Open fleet-mobile PRs to land

None. #142, #144, #145 and #146 have merged.

### Blocked on claude-fleet

1. A tool or contract change with no plan step yet:
   - Jev's host proposal for phones (`propose_host_placement` is desktop-only; New session, 14.6);
   - a per-playbook run count (organisation automation playbooks, 14.17);
   - repair step progress (the Hex field after a repair ticks nothing, 14.12);
   - Meta chords in `send_prompt { keys }` (the Alt key, 14.14 and 14.21);
   - a Codex background agent (the Agent choice on the background agent sheet, 14.16).

### Waiting on Martin

1. Whether the blocked rows above may stay open at sign-off (they are hub work, not phone parity), or must close first.
2. The light check against MobileLight: Inbox, question card, My work, More. `LightCheckTest` covers contrast only.
3. A week on New. New is already the default; Classic goes one release later.
