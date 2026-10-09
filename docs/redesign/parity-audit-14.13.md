# Phone parity audit for 14.13

Step 14.13 (phone parity sign-off) is done when the phone analysis
checklists are 100% covered, Martin has used New navigation on the phone for
a week, every screen has been checked in light against MobileLight (Inbox,
question card, My work, More), and New becomes the default. The old
navigation goes one release later. This audit, taken on 2026-10-08, lists what
stands between the checklists and that bar.

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
(14.9). The Classic bar is still the default (`ui/PhoneLayoutPref.kt`,
`loadPhoneLayout` returns Classic unless the stored value is `new`).

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
| New becomes the default; Classic is removed one release later | gap | `ui/PhoneLayoutPref.kt`: Classic is the default. This is 14.13's own last step |

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
| Row: unlabelled two-segment bar under the title | gap | not on the New row. The context % is still in the session's strip (`components/StatusStrip.kt:130`). Drop it on purpose or bring it back |
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
| One ticket with its tasks as one sheet (14.15) | gap | #135 left it for a follow-up |
| Missions list with progress, state and Refresh | done | `ui/MissionsSheet.kt:85`, reached from Control (#115) |
| Missions list in Orbit style (Running, Paused, Drafts, Done this week) | open PR | #142 (`ui/OrbitMissionsScreen.kt` on its branch) |
| Pause all | done | `ui/MissionsSheet.kt:98`, `net/HubClient.kt:1131`. #142 adds the list of what stops |
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
| Composer: schedule (send later) | blocked | the hub cannot hold a prompt until later, and no plan step builds it (#127). The clock on main is "Draft history" (`ui/SessionScreen.kt:2752`) |
| Composer: field, Send (Queue while working), Stop | done | `ui/Recovery.kt:77`, `ui/SessionScreen.kt:2812` |
| Composer: sending caption, Not sent (Retry, Edit), read-only caption, quick replies | done | `ui/Recovery.kt:80`; #122 |
| States: working, waiting, stuck on trust, idle, failed | done | `ui/kit/StatusWord.kt`; failed card in `ui/Recovery.kt:116-145` |
| States: hub unreachable, read-only, sending, send failed, empty | done | `ui/SessionScreen.kt:653` |
| State: loading (skeleton after 400 ms, 14.12) | gap | `ui/kit/PhoneStates.kt:194` `ConversationLoading` is defined and used nowhere |
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
| Files tab: +/− counts and ahead/behind on Changes (board) | blocked | `repo_changes` does not send them (#119) |
| Agent tab named after the session's agent (14.4) | gap | `ui/SessionTabs.kt:50`: `agentName` always returns "Claude Code". Contract 11 is accepted (#129), and hub 2.1 and 5.1 have merged (claude-fleet #514, #596) |
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
| Pulse ticks off the real steps | blocked | `new_session` has no step events. claude-fleet 5.13 has not merged |
| Background agent on <host>… | done | `ui/NewSessionScreen.kt:389`; the wizard's Where and Review steps |
| Start <ticket>; Also start in (up to 7, org rule) | done | `ui/NewSessionWizard.kt:531` |
| Multi-start confirm: org, list, Start N, Cancel | done | `ui/MultiStartSheets.kt:89` |
| Multi-start result: status per project, Open, Done | done | `ui/NewSessionViewModel.kt:639` |
| Board extras: Jev host proposal, drafted branch, Start from a branch, first message, account row | blocked | no hub support for the phone yet (#118). The account row needs the headroom read, which the hub keeps desktop-only |

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
| Quota meters with their numbers (14.10 verify) | open PR | #145, stacked on #144 (`MorePlacesScreen.kt:280-320` on its branch) |
| Paused row names the window and when it resets | open PR | #144 (`PhoneSessions.kt:123-134` on its branch) |
| Switch account and Wait on a paused row | blocked | `check_account_headroom` is desktop-only (#139, #144) |

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
| Done notifications; fingerprint lock | gap | not built (#117) |
| Organisation automation playbooks (MobileOrgsSettings) | blocked | claude-fleet 8.4 has not merged (#133) |
| Member actions on Company | open PR | #146 (list, change role, remove with the three share choices); hub 11.2 merged in #606 |
| Share and watch | blocked | claude-fleet 11.7, contract 15 (#143) |

## 12. Step requirements the notes do not cover (14.5, 14.7, 14.8, 14.12, 14.14, 14.16, 14.18–14.22)

| Item | Status | Evidence |
|---|---|---|
| 14.5 failed card, Not sent with Retry, repair result, Move with no host chosen | done | `ui/Recovery.kt`; `ui/MoveSheet.kt:167`; `MobileRecoveryTest` (#122) |
| 14.7 coordinator chat in the Control tab | gap | there is no 14.7 PR. Control is a list of entries that opens the agent session (`ui/MoreScreen.kt:62-68`, `App.kt:1423-1432`) |
| 14.7 ChatForm answered, declined and expired states; one-step form as an inline card | done | `model/ChatForms.kt:36-60`; `ui/ChatFormCard.kt` (#121, 10.8) |
| 14.7 multi-step form full screen | gap | `ui/ChatFormCard.kt:112` draws every step inline in one card |
| 14.7 building state with a skeleton and Atom | gap | `ui/kit/` has no Atom loader |
| 14.7 handoff chips | blocked | claude-fleet 9.8 (contract 14) |
| 14.8 no Approve action; lock screen hides the command; the tap lands on the card | done | `notify/NeedsYouContent.kt:42-47`; `NotificationsNeverAnswerTest` (#124) |
| 14.12 Signal lost and Gravity well banners on every screen | gap | `HubBanner` is used only in Work (`ui/PhoneWork.kt:330`, `:593`). Sessions, the session, Hosts, Files and New session still draw `ConnectionBanner` (`ui/PhoneSessions.kt:307`, `ui/SessionScreen.kt:653`, `ui/MorePlacesScreen.kt:163`, `:392`, `ui/NewSessionWizard.kt:155`) |
| 14.12 Hex field for the fleet check | done | `ui/FleetCheck.kt:56` |
| 14.12 Hex field after a repair | blocked | the hub answers a repair in one call with no steps (#116) |
| 14.12 Radar while adding a host | blocked | the phone has no add-host; `add_host` is Master-only |
| 14.12 Galaxy for the first import | gap | `FullscreenLoader` is used only by `ui/FleetCheck.kt`. #116 assigned it to 14.19, and #141 did not wire it |
| 14.14 shells 0..N with a key bar (Esc, Tab, ⌃C) | done | `ui/Terminals.kt:98`, `:414` (#127) |
| 14.14 / 14.21 Ctrl, arrow, ⇧Tab and Alt keys | blocked | `send_prompt { keys }` takes only Enter, Escape, Tab, C-c and digits (#127, #136) |
| 14.14 find with scopes | done | `ui/TurnNav.kt:72` (#127) |
| 14.16 spend ask with Approve and Deny, neither pre-selected | blocked | claude-fleet 9.8 (contract 14) and 8.6 have not merged (#142) |
| 14.16 background agent screen (project, agent, read-only, stop-after) | blocked | `new_bg_session` takes only a host, name and prompt (#142) |
| 14.18 update card, Progress ring, signature check, wordmark, hub-older banner | done | `ui/UpdateScreens.kt`, `update/Updates.kt`; `UpdatesTest` (#123) |
| 14.19 welcome and "I don't have a hub yet" | done | `ui/FirstInstall.kt:160` (#141) |
| 14.19 install fleet-agent on a found host, Pulse and Sonar to the first heartbeat | blocked | built (`ui/FirstInstall.kt:339`), but the hub lists `install_agent` only to Master tokens, so a phone never sees it. This needs Martin's decision on an additive hub change (#141, claude-fleet #577) |
| 14.19 installing tmux | blocked | the hub's install job does not install tmux (#141) |
| 14.20 Add a project in three steps with Data rain | done | `ui/AddProjectWizard.kt`, `ui/kit/Loaders.kt:393` (#137) |
| 14.20 "A folder already on the host" source | gap | claude-fleet 6.11 has merged (#629); the follow-up is named in #137 |
| 14.20 Connect a tracker with a secret field | blocked | client tokens never get `work_admin`; needs a contract decision (#137) |
| 14.21 two panes, split diff, split terminals, agent full screen | done | `ui/Landscape.kt:52`, `:80`; `MobileLandscapeTest` (#136) |
| 14.21 hide the status bar on iOS | gap | needs the Swift view controller (#136) |
| 14.22 help picker, tour, tips, practice fleet, Learn, guides with Undo | done | `ui/help/Help.kt:11-12`; `HelpNeverActsTest` (#125) |
| 14.22 a lesson inside Control puts its prompts in the composer | gap | the steps only say what to ask (#125) |

## 13. Light check and Martin's week (14.13 verify)

| Item | Status | Evidence |
|---|---|---|
| Inbox, question card, My work and More checked in light against MobileLight | gap | the theme switch exists (`ui/PhoneSettings.kt:15`), and the `kit-previews` CI artifact renders both themes (#114, #117, #120). No screen-by-screen check against MobileLight is recorded |
| Martin uses New on the phone for a week | gap | not started on record |

## Counts

| Status | Rows |
|---|---|
| done | 156 |
| open PR | 4 |
| gap | 16 |
| blocked | 17 |

## Gaps

What is left before Martin's phone sign-off, grouped by who has to act.

### Lane M (fleet-mobile), no hub work needed

1. Agent tab named after the agent (14.4). `agentName` is still hard-coded, although contract 11 and hub 5.1 have landed.
2. Adopt the 14.12 states on the remaining screens: `HubBanner` (Signal lost, Gravity well) on Sessions, the session, Hosts, Files and New session, and `ConversationLoading` in the session.
3. Control as the coordinator chat (14.7): the chat in the tab, a multi-step form full screen, and the building state with Atom. There is no 14.7 PR yet.
4. Galaxy for the first import (14.12 / 14.19).
5. One ticket with its tasks as one sheet (14.15 follow-up).
6. "A folder already on the host" in Add a project (14.20). Hub 6.11 is merged.
7. Done notifications and a fingerprint lock in This phone (14.11 board items).
8. Hiding the status bar on iOS in full screen (14.21).
9. A lesson in Control that fills the composer (14.22).
10. Decide on the row's unlabelled two-segment bar (§3): drop it on purpose or bring it back.

### Open fleet-mobile PRs to land

1. #144 (4.10 when a paused row's limit resets), then #145 (14.10 quota meters).
2. #142 (14.16 Orbit missions list and Pause all asked first).
3. #146 (11.10 member actions on Company).

### Blocked on claude-fleet

1. 9.8 (contract 14): Control handoff chips (14.7). 9.8 with 8.6: the spend ask with Approve and Deny (14.16).
2. 8.4: organisation automation playbooks (14.17).
3. 11.9 (open, #633): quiet hours (14.11).
4. 11.7 (contract 15): share and watch on the phone (11.10).
5. 5.13: Pulse ticking off the real start steps (14.6).
6. A tool or contract change with no plan step yet:
   - a headroom read for phones (Switch account, Wait, and the New session account row);
   - send later held on the hub (composer schedule, 14.14);
   - more `send_prompt` keys (Ctrl, arrows, ⇧Tab, Alt; 14.14 and 14.21);
   - repair step progress (Hex field after a repair);
   - richer `new_bg_session` (background agent screen, 14.16);
   - `repo_changes` counts and ahead/behind;
   - `work_admin` for client tokens (Connect a tracker, 14.20);
   - the hub's install job installing tmux.

### Waiting on Martin

1. Whether to offer `install_agent` and `add_host` to phone tokens. This unblocks 14.19's install path and the Radar.
2. Whether the blocked rows above may stay open at sign-off (they are hub work, not phone parity), or must close first.
3. The light check against MobileLight: Inbox, question card, My work, More.
4. A week on New. After that, New becomes the default, and Classic goes one release later.
