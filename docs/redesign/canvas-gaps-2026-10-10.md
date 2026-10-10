# Orbit Fleet canvas vs the current apps (2026-10-10)

Step G6.1 of the Orbit Fleet gap plan (M15): every gap row of the [2026-10-09 audit](canvas-gaps-2026-10-09.md) checked again, after batches 1 and 2 of the plan landed. Code checked: claude-fleet `main` 6c0853eb (after #812 and #814) and fleet-mobile `main` 1ffaba2 (after #197, #198 and #199). Each row was checked in code; evidence is file:line on current main or "no match".

Totals: **233 of 396 rows closed. Still open: 36 missing, 127 partial.** On 2026-10-09 there were 204 missing and 177 partial (plus 12 in PR #779, now on main).

| Section | Rows | Closed | Missing | Partial |
|---|---|---|---|---|
| 1. Forms, desktop (Anatomy, Work, Automation, Toolkit) | 54 | 39 | 5 | 10 |
| 2. Forms, desktop (Accounts, Org, Session) | 35 | 20 | 2 | 13 |
| 3. Forms, phone | 29 | 19 | 3 | 7 |
| 4. Desktop: Inbox, Work, Missions, Automation, Control | 89 | 62 | 2 | 25 |
| 5. Desktop: sessions, files, hosts, toolkit, settings | 60 | 35 | 3 | 22 |
| 6. Desktop: kit, wizards, states, motion, AI, orgs, federation | 63 | 31 | 10 | 22 |
| 7. Phone | 66 | 27 | 11 | 28 |
| **Total** | **396** | **233** | **36** | **127** |

The plan's bar for G6.1 is "0 open rows outside the left-out list". It is not met: the six left-out items (Wake host, liquid orbit for rebase, database upgrade ring, waiting for the other hub to sign, Bedrock and Vertex accounts, hub settings follow an org) are still open as expected, and 157 other rows are open too. Most of those are the second half of a row whose first half landed (a "partial"), not untouched screens.

## What is still open

**Cut while building, waiting on an owner decision**
- Tracker write-back: New task Tracker field, editing a Jira ticket from Fleet, the conflict line (decision D3/D29).
- Placement and start rules that name a host, account, model or agent (needs a migration on `start_rules`).
- Layers that apply by organisation; read-only share links; releases installed per device; Google Drive in the Library; agent per task and drag in the plan card.

**Desktop**
- Control: chat threads by topic, "# link a task", mission receipts that hand the message on, "from <mission>" on a PR.
- Task detail: Comments tab, Brief drafting in the task (only in the start popover), subtask "2 / 4".
- Sharing wording still watch / answer / drive in the Share sheet (Read / Answer / Steer only for recipients); no "Trust now".
- Host detail still one page, not tabs; Accounts has no page-wide refresh or Wait until; Settings › Hub has no last sync, device count or Unpair; no "Sync while away".
- Forms kit: no "Shown because …" note, no pre-submit host check, no shortcut hint on submit, receipts without device and time.
- Orgs: no "+ Person", no device kind or app version, member summary and "you are an admin" missing.
- States: Host offline has no Move sessions; Downloads has no Pause; Inbox empty does not name the next routine.

**Phone**
- No voice input; no "Thinking" Atom in the conversation; no Jev proposals in Control's plan or New session step 1.
- Place work sheet: no ticket header, counts, New group row or Cancel; Link a ticket has no "Did you mean".
- Files: no Share or search in a file, no Open on GitHub or Ask to push on a commit.
- Decisions (Jev) shows no weekly counts; wizards do not resume on another device; no one-line install command.

## Contents

The sections below keep the board headings of the 2026-10-09 audit. Each board lists only the rows still open, then how many closed and which.

---

# 1. Forms, desktop (Anatomy, Work, Automation, Toolkit)

Checked against claude-fleet main 6c0853eb.

## Form anatomy (FormsAnatomy.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Saved" -> dialog closes and toast offers Undo (generic) | partial | kit helper `savedWithUndo` (src/lib/forms/form_frame.ts:77) used by EditTaskDialog.svelte:184, NewTaskDialog.svelte:81, RenameLabelSheet.svelte:87, SendLaterSheet.svelte:82; no Undo in WorkRuleEditor.svelte, AssetEditor.svelte, WorkPlaceDialog.svelte, RoutinesPanel.svelte, SecretsPanel.svelte |

Closed since 2026-10-09: 13. Disabled with reason (forms.rs:164, FormWizard.svelte:525), Drafted field (forms.rs:168, FormWizard.svelte:441,519-521), invalid checked on blur (FormWizard.svelte:87-97,363), disabled verb reason under it (DialogSheet.svelte:87,133-134), error banner at top (DialogSheet.svelte:106, WizardDialog.svelte:63), Enter / Cmd+Enter submit (shortcuts.ts:297-298, form_frame.ts:41), refused by hub banner with ask an admin (form_frame.ts:64-69), Discard changes? (DiscardAsk.svelte, DialogSheet.svelte:114), scope pill (FieldRow.svelte:473-476), changed from + Reset (FieldRow.svelte:479-487), batched Save bar (PageView.svelte:268-272), typed-name confirm (forms/DestructiveConfirm.svelte:85-95), safer way out (DestructiveConfirm.svelte:75-81, RoutinesPanel.svelte:1001)

## Work forms (FormsWork.dc.html)
| Item | Status | Evidence |
|---|---|---|
| New task: Tracker field ("Fleet only" / create in a tracker) | missing | NewTaskDialog says the task stays in Fleet, no Tracker field by decision D3 (src/lib/NewTaskDialog.svelte:7-8,88) |
| Edit task for a TRACKER ticket (changes go back to Jira) | missing | tracker ticket still "edit it there. Fleet writes nothing back" (src/lib/EditTaskDialog.svelte:222); cut in 6cd4f07e pending owner on D3/D29 |
| Edit task: tracker conflict line ("Jira changed the title ... see theirs") | missing | follows from the above; no match for a tracker conflict in EditTaskDialog.svelte |
| Name this work: Drafted title | partial | drafted name offered as a "Use the drafted name" link, not prefilled as on the board; Drafted label + Undo once used (src/lib/NameWorkDialog.svelte:61-76,187-195) |
| Name this work: "Also name the N other sessions on this branch" | partial | session boxes only when the caller passes several (NameWorkDialog.svelte:210-224); row entry still passes one session (src/lib/SessionRowItem.svelte:460), no same-branch lookup |
| Placement rule: Host and Account (sessions start here) | missing | no match for host/account in src/lib/WorkRuleEditor.svelte; cut in 63f0a381 (rules store no start target, needs a migration) |
| Placement rule: "Delete rule" inside the editor | partial | delete still only in the list (src/lib/WorkRules.svelte:115-122); no match for delete in WorkRuleEditor.svelte |

Closed since 2026-10-09: 8. New task dialog on Cmd+N (shortcuts.ts:302, NewTaskDialog.svelte), "Start a session for it now" (NewTaskDialog.svelte:138), Open in Jira link (EditTaskDialog.svelte:212), Place work combobox with counts and + New group (WorkPlaceDialog.svelte:179,204,224), live "Matches N open tasks now" (WorkRuleEditor.svelte:135-138), move to org loses-access and spend lines (WorkOrgDialog.svelte:200-210), one-step "Move to <org>" verb (WorkOrgDialog.svelte:12,231), show view count on the rail (WorkFiltersBar.svelte:197,351)

## Automation forms (FormsAutomation.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Routine Time zone select | partial | zone stored (crates/fleet-core/src/service/routines/mod.rs:156,525-536) but always the device zone, shown as text, no select (src/lib/automation/RoutinesPanel.svelte:360,694-695) |
| Routine delete: runs stay, "Pause it instead", typed name | partial | Pause it instead and typed name present (RoutinesPanel.svelte:993-1001); runs still go with it, board says they stay (src/lib/routines.ts:672-681) |

Closed since 2026-10-09: 10. Schedule picker with next run (RoutinesPanel.svelte:685), Account picker apart from Profile (RoutinesPanel.svelte:713-721), dry run + Run once now (RoutinesPanel.svelte:352-396,767-771), PR review/checks/merge events (commit c4b6bb93, RoutineEventTrigger.svelte:4), event repo/author filters (RoutineEventTrigger.svelte:30-45), event rate limit (RoutineEventTrigger.svelte:5, routines.ts RATE_CHOICES), planner drafts first tasks (WorkMissions.svelte:581,1324), Import a plan from create (WorkMissions.svelte:175,1331), repo per row "Row N has no repo: pick one" (PlanImportForm.svelte:87), answer a mission card on the mission page (WorkMissions.svelte:963-988)

## Toolkit forms (FormsToolkit.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Asset editor: Discard button / explicit Lint button | partial | still Cancel + Save only (src/lib/AssetEditor.svelte:563-566); lint runs on mount/save as a report, no Lint button |
| MCP server editor: Harness as checkboxes (Claude Code / Codex / Agy) | partial | boxes for Claude Code and Codex only (src/lib/assets.ts:20-21, AssetEditor.svelte:203-208); no Agy, it has no harness renderer |
| Prompt snippet form in Toolkit › Prompts & snippets | partial | editor moved to Toolkit (src/lib/Toolkit.svelte:97, src/lib/PromptsSnippets.svelte) but as an inline row list, not the per-snippet form; Remove is a one-click x with no confirm (PromptsSnippets.svelte:69-71,143) |
| Set up the catalog: "Push after each change" | partial | setup still path + remote only (src/lib/AssetsPanel.svelte:472-475); `catalog.auto_push` stays a separate setting (src/lib/fleet_settings.ts:67) |
| New layer: "Applies by Organisation / To <org>" | missing | create still asks Catalog, Name, Axis context/role (src/lib/LayerChangeForm.svelte:37,90); cut in 9086681c (needs an org rule in layer resolution) |

Closed since 2026-10-09: 8. Command asset kind (assets.ts:7, catalog/model.rs:42), "Write it with Claude..." from New asset (NewAssetDialog.svelte:82), name help line (NewAssetDialog.svelte:66), Environment secrets help (AssetEditor.svelte:436), Import What boxes (ImportDialog.svelte:22-27,54), secret per-host Write/Skip (SecretsPanel.svelte:84-97), Write it with Claude Host picker (AuthorSessionDialog.svelte:53-65), Commit with file list, drafted message, Commit and push (CommitAssetsDialog.svelte:2-25)

---

# 2. Forms, desktop (Accounts, Org, Session)

Checked against claude-fleet main 6c0853eb.

## Accounts forms (FormsAccounts.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Add account wizard, step 1 (host, Claude subscription / API key / Bedrock-Vertex, nickname) | partial | + Add account exists (src/lib/AccountsPage.svelte:287, src/lib/forms/wizards/add_account.json). "Sign in with" offers only subscription and API key (add_account.json:17). No match for Bedrock or Vertex in add_account.ts / AddAccountDialog.svelte / service/add_account.rs. |
| Add account step 2: device-code sign-in (URL, code, Copy code, 10-min expiry, Done enabled when the host reports the login) | partial | Login pane shows the sign-in link with Copy link, a field to paste the code back, and Done once the host reports the login (src/lib/AddAccountLogin.svelte:96-158). No shown device code with Copy code, and no 10-minute expiry line (no match for expir/minute in AddAccountLogin.svelte). |
| Projects layout: base path checked on the host ("/srv/work is not writable by user dev") | partial | Validation is still local syntax only (basePathError, src/lib/SettingsDialog.svelte:674-693). No match for a writable probe on the host. |
| Start rules as an inline list under Settings › Projects with Discard/Save | partial | Still only in Automation › Rules (src/lib/StartRules.svelte). No match for StartRules in SettingsDialog.svelte; the Projects block has only "Save & rescan" (:700-702). |
| Group a project: project count per group ("3 projects") | partial | Group options carry only the name (src/lib/ProjectActionsMenu.svelte:44-46). No member count, still a menu mode, no Cancel/Save. |
| Host integrations: "Debug devices, look for Android phones on USB" per-host toggle | missing | No match for debug device / Android / USB in src/lib/HostDetail.svelte. |
| Host integrations as one form with Discard/Save | partial | The Codex select still applies on change (src/lib/HostDetail.svelte:360-364, :1035). |

Closed since 2026-10-09: 4. API key with the provider's 401 on the field and daily limit (add_account.json:30-47, AddAccountDialog.svelte:40-67, service/add_account.rs:592), New Control API token with scope/expiry/hosts (ControlApiTokens.svelte:153, forms/wizards/new_token.json:17-26), Token created sheet with Copy token / Copy as env line (ApiTokenCreated.svelte:26-33), Fleet agent install row (HostDetail.svelte:694)

## Organisation forms (FormsOrg.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Org settings switch "Hub settings follow <org>" | missing | No match for settings_follow / follow_org / "settings follow" in crates, src or src-tauri. |
| Edit a device: org and person as dropdowns inside the edit form | partial | Org and person are read-only Text fields in the device record (crates/fleet-core/src/pages/resources.rs:1101-1102). Changing them is still the separate device.bind and device.hand_over actions (:1145-1156); the person now suggests known people (:524-531). |
| Install on a debug device: "Claim the phone while installing" and a note in the same form | partial | debug_device.install still takes only path and host (resources.rs:1256-1260). Claim stays a separate action (:1254). |
| Set an organisation value for an arbitrary key, with a Settings-reference link and the hub default shown | partial | Unchanged: src/lib/pages/OrgSettingsList.svelte:27-55 lists only registered keys with Inherit / Set for this org. No free key field and no reference link. |

Closed since 2026-10-09: 7. Isolate switch at create (resources.rs:885-896), "Members see only their own sessions" (resources.rs:834-839), one rule form with Match by (resources.rs:537-571), live rule impact (resources.rs:572 org_rule_preview), add member with known people (resources.rs:524-531, :764), device modes Full / Answer only / Watch only (resources.rs:1069-1073), org project catalog (resources.rs:651-670, migration 158)

## Session forms (FormsSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Switch login: "Proposed by Jev" suggestion | partial | Switch account now shows a ProposedBy for the login with most headroom, but its source is 'rule', never Jev, by design (src/lib/SessionDetails.svelte:297-300, :1368-1371). |
| Local changes: optional question for any intent (e.g. Ask the agent to review) | partial | Question still shown and sent only for 'custom' (src/lib/LocalChangesDialog.svelte:86, :176). |
| Sync a local folder: "Leave out" patterns set before starting the sync | partial | enableLocalWorkspace accepts excludes (src/lib/local_workspaces.ts:245-254) but the start button passes none (src/lib/LocalWorkspaceCard.svelte:165). Excludes are edited only after linking (:354-375). |
| Attach to a running session from host › Attach… (outside tmux sessions with age, then "Switch to it" / "Add it to the list") | partial | Host detail lists outside-fleet panes with "running <age>", Adopt… and Ignore (src/lib/HostDetail.svelte:947-985). No host Attach… entry and no "Switch to it". |

Closed since 2026-10-09: 9. Rename and Label (RenameLabelSheet.svelte, SessionRowItem.svelte:760), login headroom per option (SessionDetails.svelte:277-295, account_usage.ts:638-662), Push after commit / Amend last (FileList.svelte:403-410), commit header "On <branch> · N ahead" (FileList.svelte:152-155), New branch form with "Use ...?" suggestion (NewBranchSheet.svelte:45-57, :115-117, FilesPanel.svelte:585), lost session Ignore (LostTargetForm.svelte:132-139), ticket in the project pick (LostTargetForm.svelte:73, :124-127), Send later in the composer (ConversationPanel.svelte:2651, :2684), Send later times and skip if archived (SendLaterSheet.svelte:45-47, :133-138; service/sessions/deferred.rs, migration 155)

---

# 3. Forms, phone

Paths: fm = /home/claude/fleet-mobile/shared/src/commonMain/kotlin/dev/claudefleet/mobile (main 1ffaba2), cf = /home/claude/claude-fleet (main 6c0853eb)

## Mobile session forms (MobileFormsSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Board rule "bottom sheets, not dialogs" with Cancel and the verb at the thumb | partial | the board's five forms are now `BottomSheet` with Cancel + verb at the foot (fm/ui/kit/BottomSheet.kt:46-80; fm/ui/SessionForms.kt:117, :206, :316); other session forms are still `AlertDialog`: Review this worktree with a prompt field (fm/ui/SessionScreen.kt:1957), model/effort picker (:1803) |

Closed since 2026-10-09: 8. One sheet for rename and tags (fm/ui/SessionForms.kt:101-160), Save above the keyboard (scrollable sheet with pinned actions, fm/ui/kit/BottomSheet.kt:51-54), Edit quick reply sheet with Send on tap switch and Remove (fm/ui/SessionForms.kt:183-241), Fork "From" turn picker (fm/ui/SessionForms.kt:326-337), Fork New worktree / Same worktree (fm/ui/SessionForms.kt:346, :352), Fork copy "Uncommitted changes are not carried." (fm/ui/SessionForms.kt:294), Background agent from inside a session (fm/ui/SessionMenu.kt:82, fm/ui/SessionScreen.kt:1634), Background agent read-only and Inbox copy (fm/ui/NewSessionScreen.kt:533-534)

## Mobile work forms (MobileFormsWork.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Link a ticket: error under the field with the fix offered ("Not a key. Did you mean FLEET-142?") | missing | no match for "Did you mean" or "Not a key" in fm |
| Link a ticket: check on Link, verb "Link" | partial | Tasks sheet has "Link “key”" (fm/ui/SessionTasksSheet.kt:197); session menu's Set work is still an `AlertDialog` with verb "Set" (fm/ui/WorkSheet.kt:201, :213) |
| Place work: ticket header (key, title, acceptance "1 of 3 done") above the group picker | missing | PlaceSheet title is only "Place in group" (fm/ui/TaskScreen.kt:265) |
| Place work: group rows with task counts ("4 tasks") and an explicit "+ New group" row | partial | filter-as-you-type and "Place in “x”" exist; rows show the name only (fm/ui/TaskScreen.kt:289, :296); no "New group" row |
| Place work: Cancel and Place buttons at the bottom | partial | Place button sits above the list, no Cancel (fm/ui/TaskScreen.kt:288-290); sheet is a raw `ModalBottomSheet`, not the kit `BottomSheet` (:262) |
| Share: "The whole org" hides the person field | partial | Org choice relabels the field "Org" and still asks which org (fm/ui/ShareSheet.kt:94-111) |
| Share: level named "Steer · send prompts" | partial | level word is still "Drive" / "can send prompts" (fm/model/Sharing.kt:27-38); no match for "Steer" in fm |

Closed since 2026-10-09: 3. One Save bar with change count and Discard (fm/ui/FleetSettingsScreen.kt:243-280), previous value under the field via changedFrom with Reset (fm/ui/FleetSettingsScreen.kt:423-433), scope badge on a setting (fm/ui/FleetSettingsScreen.kt:405-420)

## Mobile chat forms (MobileChatForms.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Full screen for long/secret forms, with who asked and why, "Chat" back | partial | header now has "‹ Chat", "Asked by" and why (fm/ui/OrbitChatForm.kt:385-393) and first Back says Chat (:123-127), but the full form is still a `ModalBottomSheet` at fillMaxHeight, not a screen (fm/ui/OrbitChatForm.kt:237-250, :377) |
| The work a form started follows the answered line (Pulse row for the new session) | missing | answered state shows outcome line, summary and View only, no follow-up row (fm/ui/OrbitChatForm.kt:185-204); no match for a started-session/Pulse row in OrbitChatForm.kt or ChatFormCard.kt |

Closed since 2026-10-09: 8. Building while Control writes the form, draft streamed with fillable fields and "reading ..." line (fm/model/FormDraft.kt:18-38, fm/ui/ChatFormDraft.kt:60-68, :138, fm/ui/SessionScreen.kt:871), "Proposed by Jev" with reason and Change (fm/model/ChatForms.kt:90-125, fm/ui/components/RichCards.kt:799, :908), per-option detail line (fm/ui/components/RichCards.kt:842-848), "Another…" free entry (fm/ui/components/RichCards.kt:855-861), Comet on the button while sending (fm/ui/OrbitChatForm.kt:225, :293-299), answered summary with View (fm/ui/OrbitChatForm.kt:197-203), form from another session in Control's chat "Asked by ..." with Answer here (fm/ui/ControlChat.kt:601-650), secret note "Stays on the hub ..." (fm/ui/components/RichCards.kt:901, :918-919)

---

# 4. Desktop: Inbox, Work, Missions, Automation, Control

Re-checked 2026-10-10 against claude-fleet main 6c0853eb. Paths are relative to src/lib unless marked otherwise.

## Fleet inbox with a session in focus (Main.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Inspector PR "15/15 checks · no reviews" | partial | SessionDetails.svelte:949-951 inspector shows the PR via prValue (:862-873): number and one CI chip, no checks count; the review decision is only in the Details tab Reviews block (:1192-1195) |

Closed since 2026-10-09: 6. Inbox "Group: state" sections (Sidebar.svelte:1592-1596, inbox.ts:165-193), "+1 proposed" row with Not waiting (Sidebar.svelte:1610-1626), mission waiting in the Inbox (Sidebar.svelte:1609 MissionWaits), stopped-on-host Restore line (Sidebar.svelte:1640-1643), "N completed today" footer (inbox.ts:113), inspector Fork / Switch account / Archive / Kill (SessionDetails.svelte:975-1003)

## Fleet work view with task filters open (Work.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Task detail tabs Overview / Sessions / Activity / Comments | partial | task_detail.ts:15-17 tabs are Overview, Sessions, Activity; no Comments tab |
| Brief block in task detail: "Draft brief from the ticket", drafted by haiku, Regenerate / Clear / Undo | partial | TaskWorkSections.svelte:53-57 Brief is read-only text; drafting only in StartPopover.svelte:15,157 (DraftField, draftBrief) |

Closed since 2026-10-09: 6. Tab counts (WorkTree.svelte:620,629,647), task row PR chip with CI (WorkTaskRow.svelte:97-103), row status sentence (work_row.ts:62 "session needs you"), Hidden by filters · Show (WorkTree.svelte:716-717), per-section spend in list headers (WorkTree.svelte:782), Delivery block with PR checks, tracker column, spend over duration, owner (WorkTaskDetail.svelte:395-416)

## Missions (Missions.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Mission list grouped by state with Filters and Group | partial | grouped by state with a Group picker (WorkMissions.svelte:1342-1347, :1364-1377); no Filters control in WorkMissions.svelte |

Closed since 2026-10-09: 6. Row reason line (missions.ts:1025-1037, WorkMissions.svelte:1392), list footer loop + ceiling + Pause all (missions.ts:1053, WorkMissions.svelte:1398-1406), detail tabs Plan / Runs N / Log / Repos N (WorkMissions.svelte:853, runs list :1238), planner runs this hour (missions.ts:1064-1066), brakes line (missions.ts:1071-1073), owner line (WorkMissions.svelte:842)

## Finished mission (Finish.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Pull request block (Merged #476, checks, approved, branch deleted, "Also merged: …") | partial | MissionFinish.svelte:155-168 lists each PR with state, branch and checks; no match for approved, review or branch deleted in MissionFinish.svelte or mission_finish.ts |

Closed since 2026-10-09: 8. Reopen / Archive N sessions (MissionFinish.svelte:118-136), mission-level Finish checks "k of n" with who and when (:141-152), Sessions to archive with clean / pushed state (:173-183), After archiving line with Completed count, spend and duration (mission_finish.ts:106-115), Waves summary (MissionFinish.svelte:190-199), Work "no tracker connected" empty state (WorkTree.svelte:744-753), Automation empty state + New routine / Use a template (automation/RoutinesPanel.svelte:636-638), Control first run card (AgentPanel.svelte:147-161)

## Fleet work board (Board.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Card agent kind (Claude Code / Codex) and mission / wave chip | partial | agent kind on the live line (WorkBoard.svelte:524-526); no mission or wave chip on the card |
| "Start new" on a card without a session; Continue ▾ and "Finishes when …" on the selected card | partial | WorkButton (Start new / Continue) only on the selected card (WorkBoard.svelte:543-545); no match for "Finishes when" or done_when in WorkBoard.svelte or WorkButton.svelte |

Closed since 2026-10-09: 6. "+" add per column (WorkBoard.svelte:410-433), x select and s start keys (shortcuts.ts:275-276), multi-select with pick bar (WorkBoard.svelte:344-358), card PR chip with CI (:515-520), card assignee + due (:507-513), "Drop to move to <column>" text (:436)

## Fleet work review (Review.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Unsure case: "Jev is unsure: … both fit · nothing pre-selected" with Pick a ticket… / No ticket | partial | confidence words incl. "unsure" (WorkReview.svelte:55 confidenceWord, preselect); no match for "both fit", "Pick a ticket" or "No ticket" in WorkReview.svelte |
| "Done today: 4 confirmed · 1 rejected · Undo last" tally | partial | WorkReview.svelte:382 summary of the last decision and Undo (:507); no day tally |

Closed since 2026-10-09: 1. Merge of a duplicate proposal moves sessions and subtasks (TaskWorkSections.svelte:141, work.ts:660-663)

## Task detail (TaskDetail.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Brief drafted by haiku with Regenerate / Clear / Undo in the task | partial | TaskWorkSections.svelte:53-57 read-only Brief; drafting only in StartPopover.svelte:15,157 |
| Subtask progress "2 / 4" with ✓ ◐ ○ states | partial | header is a total count only (TaskWorkSections.svelte:62); each row has a status dot (:82), no done-of-total |
| Placement: Account, Model · effort and host fallback from a rule | missing | no match for model, effort, profile, account or fallback in WorkRuleEditor.svelte, StartRules.svelte or start_rules.ts; start rule line is project + host (WorkTaskDetail.svelte:422-424) |
| "Start with last settings  s" in the Continue menu | partial | only in QuickSwitcher.svelte:1013 (⌘↵); no match in WorkButton.svelte |

Closed since 2026-10-09: 2. Inline status dropdown in the header (WorkTaskDetail.svelte:286-298, local tasks), tabs Overview / Sessions / Activity (task_detail.ts:15-17, WorkTaskDetail.svelte:338-343)

## Control › Today (Today.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Shipped: provenance and releases installed ("Release 0.5.3 · apps installed …") | partial | "from <routine>" provenance at TodayView.svelte:310; releases installed cut (comment :13-14, fleet records no install per device) |

Closed since 2026-10-09: 5. KPI tiles (TodayView.svelte:265), date and fleet subheader (:238, :149-155), mission in Needs you (:282-285, service/work/today.rs:52-55), inline Switch account / Wait and Open per row (TodayView.svelte:211-218), "N more ›" in In progress (:298-299)

## Tidy up (Tidy.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Reopened as a group in the same sheet with Resume / Dismiss | partial | still a separate "n reopened" pill and sheet (TidyReview.svelte:448-487), with Resume and Dismiss there |
| Choice wording: Clean up / Archive / Keep for 7 days / Never / Expire | partial | tidy.ts:66-73 labels are Clean up, Kill, Archive, Keep for 7 days, Never; no Expire choice for a stopped session |

Closed since 2026-10-09: 4. "N selected · frees about X" footer (TidyReview.svelte:664), What each choice does legend (:652), selected-row explanation (:645-649), Restore all on the Stopped on host group (:546)

## Automation (Automation.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Nothing to do · kept out of Inbox · Proposed by Jev · Change · Send to Inbox" | partial | automation/RoutinesPanel.svelte:875 Nothing to do chip and :885 "Read by Jev"; no Change or Send to Inbox |

Closed since 2026-10-09: 7. Fleet-wide daily budget in the footer (AutomationView.svelte:185-191), per-run time cap (automation/RoutinesPanel.svelte:939), host fallback (:736, routines.ts:308-309), retry once (:744), autonomy stat (:860, :942), failed run Details with error code and named fix (:892-910), outcome routing row (:980, routines.ts:332)

## Control (MissionControl.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Chat threads by topic ("Overview ⚙", day separator) | missing | AgentPanel.svelte is still one operator conversation; no match for topic or thread in it |
| Receipt "↳ Sent to session/mission … ↗" that hands the message on | partial | control_route.ts:17-22 hands on to a session only; a mission receipt only opens it |

Closed since 2026-10-09: 5. Suggested from your fleet cards (ControlSuggestions.svelte:25-50), composer hint "# task · @ host · / command" (AgentPanel.svelte:54,248), Views panel Welcome back line with Running / Idle / Done today folds and inline limit actions (ControlViewsPanel.svelte:224-252), Views panel search (:227), Routines in Automation in Open elsewhere (control_views.ts:62)

## Control views: PRs, Library, Today briefing (MCViews.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "from <mission>" origin on a PR | partial | WorkPrs.svelte:134 links to the opening session only; no mission origin |
| Library folders (Artifacts today, Session outputs) and repo entries per host | partial | LibraryView.svelte:2-8 one table per host; no folder tree, no claude.ai artifacts |
| "Link a repo on a host", "Add a Google Drive folder" | partial | Link a repo… at LibraryView.svelte:133; Google Drive folder cut (comment :9-11) |

Closed since 2026-10-09: 2. PR diffstat (prs.ts:87-89, WorkPrs.svelte:115), Library table Name / Modified / Size sortable with grid toggle and Type (LibraryView.svelte:118-192)

## Control with a session in the panel (MCSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Focus header "host · account · PR" | partial | ControlViewsPanel.svelte:272-274 shows status · host · PR, no account |

Closed since 2026-10-09: 0.

## Control tasks in chat (MCTasks.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Plan card by wave, with "finishes when", agent per task, drag to reorder, "unchecked: skip" | partial | waves and finishes-when at HandoffCards.svelte:337-343; agent per task and drag cut (comment :15-17) |
| Task status card in chat (finish checks, live session line, Start review run / Mark verified / Move to Done / Open in tracker) | partial | live line, Move to Done, Mark verified at HandoffCards.svelte:264-304; Start review run cut (comment :17) |
| Created-task card "Drafted from your message · Undo" with Owner / Due / Group pickers + Proposed by Jev | partial | Drafted, Owner, Due, Undo at HandoffCards.svelte:262-290; Group picker cut (:17), no ProposedBy in HandoffCards.svelte |
| Slash commands /task /plan /done /assign /start and "# link a task" | partial | /task /done /assign /start run locally (control_slash.ts:42-45), /plan goes to the agent (:9); no "#" task link handling in ConversationPanel.svelte |

Closed since 2026-10-09: 4. "May duplicate · Merge / Keep both" (HandoffCards.svelte:357-361), Edit in Work (:369), Tasks view with Mine / Claude, Group: status, Start new / Assign… / Done (ControlTasksView.svelte:130-147, control_views.ts:44), Routines view (control_views.ts:45)

---

# 5. Desktop: sessions, files, hosts, toolkit, settings

Checked on claude-fleet main 6c0853eb. Paths are relative to /home/claude/claude-fleet.

## Fleet all sessions (Sessions.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| "Done 6 today" (done group limited to today) | partial | row_groups.ts:47 Done group has no today window. "completed today" is counted only for the Inbox footer (inbox.ts:100,113) |
| Inspector PR line "15/15 checks · no reviews" | partial | SessionDetails.svelte:862-872 PR value shows a CI state chip, no check counts. Review decision moved to the Reviews block (SessionDetails.svelte:1192-1194), not the PR line |

Closed since 2026-10-09: 6. All / Mine / Shared with me tabs (session_scope.ts:16-22, Sidebar.svelte:1647-1659), Group by Organisation (filter_schema.ts:93), "Agent: any" facet (session_scope.ts:91, SidebarFilters.svelte:313), "4 more running" overflow (row_groups.ts:132-155, Sidebar.svelte:1008), shared rows with sharer and level (session_scope.ts:73-81, Sidebar.svelte:2043), Inspector Fork / Switch account / Archive (session_actions.ts:67-80)

## Fleet session agent tab (Agent.dc.html)
Closed since 2026-10-09: 1. Pop-out Send keys / Clear view / Pop back in (TerminalView.svelte:345-365, SendKeys.svelte)

## Fleet session with terminals open (Terminals.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Next terminal on Windows/Linux = Ctrl+Alt+` | partial | shortcuts.ts:174 still binds Ctrl+` |

Closed since 2026-10-09: 3. Shell tab menu with Kill terminal (TerminalStrip.svelte:186-201), "New terminal opens on" picker (TerminalStrip.svelte:219-231), what runs in each shell (terminals.ts:19, TerminalStrip.svelte:64,134)

## Fleet session details (SessionDetails.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Timeline "Linked to TASK-219 · you confirmed · Proposed by Jev · Unlink" | partial | TimelineWorkProposal.svelte:96-104 shows Linked to, you confirmed and ProposedBy. No Unlink there (Unlink is in SessionTasks.svelte:294) |
| Reviews block: run verdict ("no blocking findings · 2 nits"), Open, "Start a review run" | partial | SessionDetails.svelte:1189-1210 lists runs and Start a review run. No findings verdict, by choice (comment at :1185-1188 says the verdict is the run's state) |
| Share actions: Copy read-only link, Copy transcript | partial | Copy transcript closed (session_actions.ts:79). Read-only link left out on purpose (session_actions.ts:17-18), no backend |
| Share sheet: Private / org / people with Read · Answer · Steer wording | partial | ShareSheet.svelte:276-278 still offers watch / answer / drive. Read / Answer / Steer names exist only for recipients (shared_view.ts:14-17). No "Private · only you" state in the sheet |
| Share sheet "Peter can only read until you trust his iPhone · Trust now" | partial | share_devices.ts:17-18 warns only. No match for "Trust now" |

Closed since 2026-10-09: 4. Private visibility badge (share.ts:415-446, VisibilityBadge in SessionTabs.svelte:27), Related Link / Not related (SessionDetails.svelte:377-393,1175), Steer actions Fork / Rewind / Switch account / Change model (session_actions.ts:67-70), session Archive (session_actions.ts:80,110)

## Files tab (Files.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Diff toolbar "Ask Claude about this" | missing | no match for "Ask" in DiffView.svelte, FileViewer.svelte or FilesPanel.svelte |

Closed since 2026-10-09: 0

## Files history (FilesHistory.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Remote "origin/claude/* · 11 merged and safe to delete" with delete | partial | BranchList.svelte:33,87 counts merged remotes. Delete merged covers local branches only (:32,50-56) |

Closed since 2026-10-09: 0

## New session (NewSession.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Agent picker: Agy startable | partial | agent_picker.ts:14-15 STARTABLE_AGENTS = claude, codex ("Agy has an adapter but no launch yet") |
| "Has previous work: Resume pd-2412 · Proposed by Jev (N2) · Start fresh instead" | partial | rule-based past-work notice (NewSessionDialog.svelte:643-657). No resume_or_new module in crates/fleet-core/src/service/decide/ |

Closed since 2026-10-09: 0

## Dialogs (Dialogs.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Move to host: each host shows latency, free disk, "account at limit" | partial | TransferSheet.svelte:376 now shows hosts_table.ts:186-196 moveTargetFacts: free disk, load, account at limit. No latency |
| Start a review run: reviewer agent choice ("pr-review skill · Codex") | partial | ReviewDialog.svelte:2-5 a skill run by Claude Code only, no agent choice |
| Placement rules: rule names account and agent | partial | crates/fleet-core/src/service/start_rules.rs:49-60 StartRuleInput = pattern, project_id, host_alias, org_id. No account or agent |

Closed since 2026-10-09: 0

## Watching a shared session (Watch.dc.html)
Closed since 2026-10-09: 4. Recipient header with sharer, level and since (shared_view.ts:65-74, SessionTabs.svelte:100,191), "Ask Martin for Answer" (SharedWithYou.svelte:3-10, shared_view.ts:101), read-only answer card (AnswerPrompt.svelte:61, shared_view.ts:96), Inbox shared rows with level and via org (shared_view.ts:84-88, Sidebar.svelte:2043)

## Accounts & hosts (Accounts.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Header "usage refreshed 1m ago" with a page-wide Refresh | partial | per-account Refresh only (AccountsPage.svelte:387-391) and per-account checked time (:393-394) |
| Hosts table agent column "0.5.2 · update" as an action | partial | HostsTable.svelte:129 "· update" is a span, not an action. Install action exists in HostDetail only (HostDetail.svelte:694) |
| Paused-sessions panel listing sessions and routines with Switch account / Wait until / Show in All sessions | partial | AccountsPage.svelte:119-123,340-345 paused count with Show and Switch. No Wait until and no paused routines on that page (LimitActions is on rows, Today and Control only) |

Closed since 2026-10-09: 4. + Add account (AccountsPage.svelte:59,291, AddAccountDialog.svelte), account card paused count / Show / Switch / routines (AccountsPage.svelte:108-123,333-345), "fallback for routines" (accounts_page.ts:238), Tidy hint (hosts_table.ts:158, HostsView.svelte:572, HostDetail.svelte:825)

## Host detail (HostDetail.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Tabs Overview / Sessions 3 / Lost & found 3 / Provisioning | partial | still one scrolling page of sections (HostDetail.svelte:592,670,747,792,1021,1079) |

Closed since 2026-10-09: 4. Open a shell (HostDetail.svelte:738-740), skills drift Sync (HostDetail.svelte:696-703), foreign pane Ignore (HostDetail.svelte:406-413), Find another (HostDetail.svelte:248)

## Toolkit (Toolkit.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| "+ Add skill" in the Skills view | partial | Toolkit.svelte:121-123 has Sync all hosts only. No match for "Add skill" |

Closed since 2026-10-09: 1. Toolkit nav (was already done on main, Toolkit.svelte:29-33)

## Assets (Assets.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Layer detail tabs Assets · Changeset · Hosts · Source | partial | LayerInspector.svelte:32-33 tabs = Members, Hosts |

Closed since 2026-10-09: 0

## Settings › Appearance (Settings.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Elsewhere › "Automation limits ↗" | partial | settings_tree.ts:141-144 Elsewhere = Accounts & hosts, Guides. Automation and Limits stay under System (:127-128) |

Closed since 2026-10-09: 3. Text size (AppearanceSettings.svelte:94, text_size.ts), Agent tab name (AppearanceSettings.svelte:34), Badge counts (AppearanceSettings.svelte:13,39)

## Settings › Hub & sync (SettingsHub.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| "Sync while away" (keep local sessions on the hub while the app is closed) | missing | no match for "while away" in src/lib or crates/fleet-core/src |
| Hub status header "last sync 4 s ago · 2 more of your devices · Unpair…" | partial | SettingsDialog.svelte:462-482 shows URL, pairing name and versions. No last-sync time, device count or Unpair |

Closed since 2026-10-09: 2. Projects on the hub (HubProjectsPick.svelte:29, hub_projects.ts), Control API + Token (ControlApiTokens.svelte:9,153)

## Settings, more sections (SettingsSections.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Decisions N2 "Resume or new" use case | missing | no resume_or_new in crates/fleet-core/src/service/decide/ or crates/fleet-core/pages/settings.decisions.json |

Closed since 2026-10-09: 3. J6 main ticket / J7 tracker duplicate (service/decide/main_ticket.rs, tracker_duplicate.rs, decide.jev.main_ticket / tracker_duplicate in settings.decisions.json), Writing help toggles (crates/fleet-core/src/service/settings.rs:1308-1332), Repair now / Restore lost sessions buttons (crates/fleet-core/src/pages/actions.rs:37,44, pages/settings.automation.json:115)

---

# 6. Desktop: kit, wizards, states, motion, AI, orgs, federation

Checked against claude-fleet main 6c0853eb. Paths are relative to /home/claude/claude-fleet.

## Fleet conversation components (Components.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Shown because 'Needs a database' is on" note on a conditional step/field | missing | `when` still evaluated silently (src/lib/forms/form_model.ts:21-27, 94-118); no match for "because" in src/lib/forms |
| Inline pre-submit check warning ("Postgres needs 2 GB free, host has 1.4 GB") | partial | per-field problems only (src/lib/forms/FormWizard.svelte:526); FormSpec has no check/host-fact field (crates/fleet-core/src/pages/forms.rs:39-50) |
| Submit shortcut "Create ⌘↵" | partial | ⌘↵ / Ctrl+Enter now submits (FormWizard.svelte:149-158) but the submit button shows no shortcut hint (FormWizard.svelte:545-547) |
| Receipt "answered by Martin on the phone · 12:41" (device + time) | partial | src/lib/forms/receipt.ts:127-130 still "answered by X", no device or time |
| Live progress: elapsed time and per-step sub-line ("attempt 2 of 10") | partial | ProgressStep is title + state only (src/lib/rich_blocks.ts:73-76, 107-117); no elapsed, no step detail |
| Error card buttons "Retry" / "Re-login on mac" as actions | partial | next steps still only fill the composer (src/lib/rich/ErrorCard.svelte:36-41) |

Closed since 2026-10-09: 4. Wizard step chips (FormWizard.svelte:326-338), Save and finish later (FormWizard.svelte:536-537), review step with per-section Edit (FormWizard.svelte:344-350), secret field note (`secret_note`, FormWizard.svelte:524, forms.rs:172)

## Orbit wizards in chat (ChatWizards.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Control drafts a form from a free-text message ("Set up a new project for…") | partial | Control still opens the catalog wizard from a button (src/lib/AgentPanel.svelte:198); the agent can write a generic form with `ask { form }` (skills/claude-fleet-control/SKILL.md:245-252) but nothing maps a message to a prefilled wizard |

Closed since 2026-10-09: 3. Wizards in chat with building card (src/lib/forms/ChatWizards.svelte, WizardChatCard.svelte now on main), secret "stays on" line (secret_note, FormWizard.svelte:524), named step chips (FormWizard.svelte:326-338)

## Fleet add host wizard (Wizard.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Per-check timing ("SSH as martin@mercury · 18 ms") | partial | latency only in host_check summary (src/lib/host_check.ts:97); wizard check rows show `c.detail` with no time (src/lib/AddHostWizard.svelte:300-303) |
| "Claude Code, Codex on PATH" | partial | Codex is listed (src/lib/add_host_wizard.ts:221-225) but the step still says "Fleet runs Claude Code today; the others are shown as they arrive" (AddHostWizard.svelte:335) |

Closed since 2026-10-09: 1. fleet-agent "Install <version>" from the wizard (AddHostWizard.svelte:304-313, src/lib/add_host_wizard.ts offersAgentInstall)

## Fleet first-run tour (Tour.dc.html)
No rows to check.

## Fleet guide walkthrough (Guide.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Provenance line on an approved guide ("proposed by a session, approved by Martin on 6 Oct") | partial | still only in the review queue (src/lib/pages/GuideReview.svelte:118); no match for "approved by" in src/lib/pages |
| "Each change is listed so you can undo it" | partial | still only an "N changed" tag (src/lib/pages/PageView.svelte:169); no per-change undo list |
| Chat guide card as a summary ("4 steps · changes 2 settings · Start") | partial | src/lib/rich/GuidePageCard.svelte:38-60 still renders the whole PageView inline + Open in Settings; no summary or Start |

Closed since 2026-10-09: 0

## States (States.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Host offline: "Move sessions…" action | missing | src/lib/states/HostOffline.svelte:85-109 offers Wake (when passed) / Try again / Host detail / Show sessions; no move |
| Inbox empty names the next routine ("next routine is Morning PR sweep at 07:30") | partial | src/lib/Sidebar.svelte:1479-1488 "Nothing needs you right now." + "Not waiting · …" counts; no routine line |
| No-results: "Start new session 'receipt totals'…" prefilled from the query | partial | sidebar no-results offers plain "Start a new session" (src/lib/Sidebar.svelte:1985); query prefill only via ⌘↵ in QuickSwitcher (src/lib/QuickSwitcher.svelte:6) |
| First run copy "runs Claude Code, Codex or Agy … This Mac counts" / "Use this Mac" | partial | src/lib/Sidebar.svelte:1988-1998 "Start with one host" with Add a host / Pair with a hub; no match for "Use this Mac" |

Closed since 2026-10-09: 0

## Toasts (Toasts.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Archive toast "freed 2.1 GB" | partial | bulk/single Archive toast says "Archived N sessions" + Undo with no size (src/lib/kill_check.ts:129-139); freed size only on Tidy (src/lib/TidyReview.svelte:304) and mission finish (src/lib/mission_finish.ts:186) |
| Downloads "Pause" on a copy in flight | missing | no match for "pause" in src/lib/downloads.ts / DownloadsSheet.svelte |

Closed since 2026-10-09: 4. Limit-hit toast with Show paused sessions (src/lib/limit_toast.ts:62-81, started in src/App.svelte:12), rule-suggestion toast (src/lib/rule_offer_toast.ts:33-61, called from trackers.ts and multi_start.ts), two-button toast with second line (src/lib/toasts.ts:33-37, Toasts.svelte:58-68), notifications panel ⚙ (src/lib/NotificationList.svelte:14-42)

## Light mode (Light.dc.html)
No open rows.

Closed since 2026-10-09: 1. Needs-you "+1 proposed" count (src/lib/Sidebar.svelte:1583-1597)

## Motion frames (Motion.dc.html)
No open rows.

Closed since 2026-10-09: 2. Question card enters and focus moves (`enter` prop, src/lib/kit/QuestionCard.svelte:51-62, used in src/lib/ConfirmCards.svelte:67,92), inbox count roll with plain swap when not Full motion (src/lib/motion_catalog.ts:187-205, used in src/lib/kit/Rail.svelte:57)

## Logo motion (LogoMotion.dc.html)
No rows to check.

## Particle loaders (Loaders.dc.html)
No rows to check.

## Loaders in use (LoadersInUse.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Host offline "Wake host" | missing | HostOffline renders Wake only when `onwake` is passed (src/lib/states/HostOffline.svelte:7-9, 87-93); no caller passes `onwake=` in src/lib |

Closed since 2026-10-09: 0

## Loaders in flows (LoadersInFlows.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Link two hubs: "waiting for Peter to sign · no loader" | missing | no match for "sign" in src/lib/forms/wizards/link_peer.json or "to sign" in src/lib/pages |

Closed since 2026-10-09: 1. Long-job toast opens the job: download toast carries the ring and an "Open" action (src/lib/downloads.ts:72, src/lib/Toasts.svelte:40-50)

## Startup (Startup.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Upgrading the database · 3 of 5" Progress ring | missing | src/lib/startup.ts:5-15 still says the migration runs before the window can paint |
| Liquid orbit for rebase | partial | liquid-orbit used for fork and combine only (src/lib/ForkSheet.svelte:242, src/lib/LocalWorkspaceCard.svelte:271); no rebase op uses it |

Closed since 2026-10-09: 0

## AI patterns (AIPatterns.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "When AI changed something" Undo line on every AI-caused change | partial | shared `aiChangeLine` (src/lib/ai_proposal.ts:152-154) is used only for work links (src/lib/WorkReview.svelte:189, src/lib/TimelineWorkProposal.svelte:101); not on other AI changes |

Closed since 2026-10-09: 4. Confidence as a word (src/lib/ProposedBy.svelte:40-46), correction line (src/lib/ai_proposal.ts:145, src/lib/NewSessionDialog.svelte:1582), Edited draft with ask-before-Regenerate (src/lib/DraftField.svelte:9-10, 53, 119), use cases off/shadow/assist in Settings › Decisions (crates/fleet-core/pages/settings.decisions.json:37-136, src/lib/fleet_settings.ts:131)

## The fleet agent (AgentStates.dc.html)
No open rows.

Closed since 2026-10-09: 1. Multi-step plan with one "Confirm N · Cancel" (src/lib/ConfirmCards.svelte:8-9, 50-66)

## Org overview (OrgOverview.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Members summary "2 admins · 2 members · 1 viewer" and tab count badges | partial | Overview tiles are spend + session_count/needs_you (crates/fleet-core/pages/settings.orgs.json:33-50, crates/fleet-core/src/pages/resources.rs:597-599); no role counts, no tab badges |
| Header "owns the hub fleet.rlt.sk · you are an admin" | partial | header has the "owns the hub" badge only (resources.rs:815); no hub name, no "you are an admin" |

Closed since 2026-10-09: 2. Sharing tab (settings.orgs.json:122-146), What belongs with Projects and Accounts (settings.orgs.json:68-95, resources.rs:652, 719)

## Org members (OrgMembers.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Device "not trusted" in the member row, "Trust device" step in Add member flow | partial | member row lists device names only (src/lib/pages/OrgMembers.svelte:172); Add flow still says trust later in Settings → Devices (OrgMembers.svelte:212) |

Closed since 2026-10-09: 2. Inline role dropdown (src/lib/pages/OrgMembers.svelte:160-169), removed member row with Take back their shares (crates/fleet-core/src/pages/resources.rs:770-785, src/lib/pages/resources.ts:318-320)

## People & devices (OrgDevices.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "+ Person" | missing | person resource still `create: None` (crates/fleet-core/src/pages/resources.rs:1180) |
| Device kind/app version ("phone · fleet-mobile 0.5.4") | missing | device fields are name/mode/this_device/trusted/org/person/catalogs/last_seen/created only (resources.rs:1086-1121) |

Closed since 2026-10-09: 1. One "People & devices" table with Org/Person filters and group by person (crates/fleet-core/pages/settings.devices.json:9-23)

## Org spend (OrgSpend.dc.html)
No rows to check.

## Sharing and presence (Sharing.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "reads only until his iPhone is trusted · Trust now" on a share | missing | no match for "Trust now" in src/lib or crates/fleet-core/src |

Closed since 2026-10-09: 3. Org Sharing list with Level/Owner filters and Revoke (src/lib/pages/OrgShares.svelte:26-86, resources.rs:793-806), admin Narrow to watch from that list (resources.rs:804, OrgShares.svelte:69-76), Team panel (resources.rs:787-791, ItemLabel::TeamMember)

## Federation (Federation.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "retrying every 30 s" | partial | peer state "Retrying" + last_error only (crates/fleet-core/src/pages/resources.rs:1279, 1309); no retry interval |

Closed since 2026-10-09: 1. Hub graph (src/lib/pages/ResourceGraph.svelte now on main)

## Debug devices (DebugDevices.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Greyed "live screen and input / device on another host" placeholders | missing | no match in crates/fleet-core/pages/debug_devices.json or resources.rs |
| Tracker cards "N columns mapped · Column map" | partial | trackers page has Write-back and Sync sections (crates/fleet-core/pages/settings.trackers.json:130, 146); no match for "mapped" in resources.rs or src/lib/pages |

Closed since 2026-10-09: 1. "Scan all hosts" (crates/fleet-core/src/pages/actions.rs:51-57, debug_devices.json:7)

---

# 7. Phone

Checked against fleet-mobile main 1ffaba2 and claude-fleet main 6c0853eb. Paths are relative to fleet-mobile `shared/src/commonMain/kotlin/dev/claudefleet/mobile/` unless they name a repo.

## Mobile · Inbox and navigation (MobileNav.dc.html)
| Item | Status | Evidence |
|---|---|---|
| More › Automation live line "3 active · $4.10 today" with an inline Pause all | partial | Line is "N of M routines on · Pause all" (`ui/AutomationViewModel.kt:245-251`, `App.kt:1835`). No spend today; Pause all is text in the line, the row has one tap (`ui/MoreScreen.kt:22`) |

Closed since 2026-10-09: 5. Failed row Open log / Retry (`ui/InboxScreen.kt:410-411`), Paused row Switch account / Wait (`ui/InboxScreen.kt:405,412-415`), mission ask as Inbox row (`ui/InboxScreen.kt:287-290`), Jev-proposed row with Not waiting (`ui/InboxScreen.kt:298,319-324`), New layout is the default (`ui/PhoneLayoutPref.kt:9-14`)

## Mobile · One session (MobileSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Voice input in the composer | missing | No match for voice/speech/mic/microphone under `ui/` |
| "Thinking · reading X" Atom in the conversation | missing | `Atom()` only in chat-form drafting (`ui/OrbitChatForm.kt:278`, `ui/ChatFormDraft.kt:135`, "Writing the form · reading X" at `:61`). No match for "Thinking" in the conversation |

Closed since 2026-10-09: 3. Account row with 5h / week meter (`ui/SessionDetailsSheet.kt:103-121,245`), PR checks "15/15 checks" (`ui/SessionDetailsSheet.kt:125-127,263`), Fork and Switch account in Details (`App.kt:2865-2866`)

## Mobile · Control, notifications and chat forms (MobileControl.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Failed notification with "Open log" / "Retry" actions | partial | Buttons are only Open and Later, on purpose (`notify/NeedsYouContent.kt:31-45`) |
| Mission notification ("Review grant") | partial | Posted per wait (`notify/MissionWaiting.kt:33-44`, android `NeedsYouService.kt:289-301`). No "Review grant" action; the tap opens MainActivity, not the grant |
| "Proposed by Jev" host pick in Control's plan | missing | "Proposed by" only in forms (`model/ChatForms.kt:126`) and Inbox (`ui/InboxScreen.kt:298,319`). Nothing in `ui/ControlChat.kt` |
| Atom "Watching CI on #476" status in the chat | partial | Handoff chips only. No match for Atom or "Watching" in `ui/ControlChat.kt` |

Closed since 2026-10-09: 2. Jev proposal inside a chat-form step (`model/ChatForms.kt:48-54,90-126`), sign the autonomy grant on the phone (`ui/OrbitMissionDetail.kt:320,446`)

## Mobile · New session (MobileNewSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Jev host proposal on Step 1 | missing | No match for propos/Jev in `ui/NewSessionWizard.kt` |
| Drafted branch name ("Drafted from FLEET-151 · Regenerate · Clear") | partial | "Drafted from KEY" and Clear exist (`ui/NewSessionWizard.kt:379-381,539-544`). No Regenerate |

Closed since 2026-10-09: 4. Start from "A branch" (`ui/NewSessionWizard.kt:418`), Review Account row (`ui/NewSessionWizard.kt:375-376,699`), optional first message (`ui/NewSessionWizard.kt:723-729`), Pulse ticks real start steps from start:progress (`ui/NewSessionWizard.kt:327-334`, `model/StartProgress.kt`; ticket mode and older hubs fall back to fixed names)

## Mobile · Work (MobileWork.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Task detail: Acceptance criteria as a list | partial | Task detail renders only the description markdown (`ui/PhoneWork.kt:646`). Criteria list exists for ticket cards only (`model/TicketCard.kt:47-51`) |
| Comet while summarising | partial | Still DotWave beside "Summarizing" (`ui/PhoneWork.kt:834-836`) |

Closed since 2026-10-09: 0.

## Mobile · Search, bulk actions and Today (MobileSessionsTools.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Project hit by file content ("matches HostsScreen.kt") | missing | `ui/SearchEverywhere.kt:29-31` matches only label or owner/repo |
| Today opened from a Control answer ("what did I ship today?") | missing | `App.kt:1707` "Today is an Inbox view until Control grows its own". No match for today in `ui/ControlChat.kt` |

Closed since 2026-10-09: 0.

## Mobile · Hosts, accounts and files (MobileMore.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Host recovery "Run plan when back" (deferred until the host answers) | partial | Button and confirm exist (`ui/HostDetailSheet.kt:131-139,207`), but it runs from the phone only while the app is open and connected; the hub has no deferred restore |
| Per-host verdict chip (Resume / Recreate / Skip) with the reason | partial | Plan lists "name - action: reason" as plain text, and only for entries that do not restore (`ui/HostDetailSheet.kt:113-119`). No chip |

Closed since 2026-10-09: 1. Ping latency on host rows (`ui/MorePlacesScreen.kt:119-123`)

## Mobile · Offline, reconnecting and loading (MobileStates.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Reconnecting full panel on the New layout | partial | `ReconnectingPanel` (`ui/kit/PhoneStates.kt:211`) is still used only by Classic `ui/SessionsScreen.kt:849` |

Closed since 2026-10-09: 0.

## Mobile · Errors and recovery (MobileRecovery.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Move options state the toolchain facts ("Android SDK not checked") | missing | `moveHostFacts` gives transport and running count only (`ui/MoveSheet.kt:157-160`). No match for SDK/toolchain/JDK in `ui/MoveSheet.kt` |

Closed since 2026-10-09: 0.

## Mobile · Tidy and tickets (MobileTidyTickets.dc.html)
No open rows.

Closed since 2026-10-09: 0.

## Mobile · Missions and background agents (MobileMissions.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Background agent as a screen with an Agent choice (Claude Code / Codex) | partial | Now a sheet (`ui/NewSessionScreen.kt:466-490`), but the agent is fixed to "Claude" (`:490`); hub refuses Codex (`model/Recovery.kt:77`) |

Closed since 2026-10-09: 2. "Waits on you" first (`ui/OrbitMissionsScreen.kt:214`), autonomy and spend meter on the row (`ui/OrbitMissionsScreen.kt:140-144,272-294`)

## Mobile · A session's Files tab (MobileSessionFiles.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Commit: pushed / not pushed, ticket refs as links, Open on GitHub, Ask to push | partial | Pushed / Not pushed shown (`ui/RepoScreen.kt:661-662`). No ticket links, Open on GitHub or Ask to push (CommitPane `ui/RepoScreen.kt:655-670`) |
| File view: Share and search | partial | Only "Send to Downloads" (`ui/RepoScreen.kt:686-690`). No Share or search |
| "Ask Claude Code to commit" named after the session's agent | partial | Still `DEFAULT_AGENT_NAME` (`ui/RepoScreen.kt:249`) |

Closed since 2026-10-09: 2. Per-file +/− counts (`model/Repo.kt:19-26`, `ui/RepoScreen.kt:676`), "N behind main" (`model/Repo.kt:103,114`)

## Mobile · Terminals, send later, find and the session menu (MobileSessionExtras.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Send later opened from a clock in the composer | partial | Still reached from ⋮ "Send later…" (`ui/SessionMenu.kt:81`). No clock in the composer |

Closed since 2026-10-09: 2. Send later time choices (`ui/SessionLater.kt:152-155`), terminal key bar with ← → and Ctrl row (`ui/Terminals.kt:108-117`; Alt tracked under Full-screen)

## Mobile · Organisations and hub settings (MobileOrgsSettings.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Automation playbooks as toggles with recent activity ("Ran 3 times this week") | partial | Generic hub fields. No match for "times this week" or per-playbook run counts |
| No Save buttons; History in ⋮ | partial | History moved to ⋮ (`ui/FleetSettingsScreen.kt:438-443`). Typed values now stage to one page Save bar (`:243-282`, gap plan G1.5), not saved as changed |
| Decisions (Jev): key set date, model, "This week N proposals · kept · changed" | missing | `DecisionsHead` (`ui/OrbitOrgsScreen.kt:319-338`) shows only opted-in / not and What Jev may do |

Closed since 2026-10-09: 0.

## Mobile · Full-screen loaders (MobileFullscreenLoaders.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Hex field for Repair / Recreate with a step checklist | partial | Hex field and checklist now used (`ui/RepairWait.kt:51-62`), but nothing ever ticks: the hub answers once (`ui/RepairWait.kt:22-41`) |
| Radar: network scan and "Enter an address by hand" | partial | Reads only the hub's SSH config (`ui/AddHostScreen.kt:90-93`). No match for manual address / "by hand" |

Closed since 2026-10-09: 0.

## Mobile · Light theme (MobileLight.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Screen-by-screen light check against MobileLight | partial | Token contrast test only (`commonTest/.../ui/theme/LightCheckTest.kt`, 4.5:1 on every ground). No screen-by-screen check |

Closed since 2026-10-09: 0.

## Mobile · Updating the app (MobileUpdate.dc.html)
No open rows.

Closed since 2026-10-09: 0.

## Mobile · First launch and installing on a host (MobileInstall.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Show a one-line install command" | missing | No match for "one-line" or curl in `ui/FirstInstall.kt`. Commit cb7c30b cut it: agent token only via `fleet-hub agent-token` |

Closed since 2026-10-09: 3. Assemble splash (`App.kt:725-733`, `ui/kit/Loaders.kt:546-553`), Install agent from Add a host (`ui/AddHostScreen.kt:54,81-87`), install installs tmux (`ui/FirstInstall.kt:243-247,271`)

## Mobile · Wizards (MobileWizards.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Where step: host facts (free disk, toolchain such as JDK) | partial | Free disk, CPUs, memory shown (`ui/HostsViewModel.kt:204-208`, `ui/AddProjectWizard.kt:532-536`). No toolchain; hub probes none |
| Organisation from the repository owner, with Change | partial | Org and why shown (`ui/AddProjectWizard.kt:195-233,523-525`). No Change |
| "A folder already on the host" for any host | partial | Still hub's own machine only (`ui/AddProjectWizard.kt:122-125,456`) |
| Clone progress with real counts (38 of 59 MB · objects) | partial | Fixed step list from `addSteps()` (`ui/AddProjectWizard.kt:148`). No byte or object counts |
| A wizard started on one device resumes on the other | missing | No match for resume/draft hand-off in the wizards. Needs hub wizard_state, not on main |

Closed since 2026-10-09: 0.

## Mobile · Full-screen and landscape (MobileFullscreen.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Split terminals key bar with Alt and ←/→ | partial | Arrows and Ctrl row exist (`ui/Landscape.kt:146-149`, `ui/Terminals.kt:115-117`). No Alt: hub refuses Meta chords (`ui/Terminals.kt:112`) |

Closed since 2026-10-09: 1. Portrait ⤢ for the conversation (`ui/SessionScreen.kt:650,1384-1385,1425`, `ui/Landscape.kt:341`)

## Mobile · Tutorials (MobileTutorials.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Practice notifications marked "Practice" | partial | Practice banner exists (`ui/help/Practice.kt`). No match for practice under `notify/` |

Closed since 2026-10-09: 0.

## Mobile · Tutorial modes, lessons and guides (MobileTutorialModes.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Practice fleet "shown as its own fleet in the account switcher" | missing | No account or fleet switcher. No match for switcher |
| Guide step with real usage meters | partial | Guide steps exist. No match for meter or usage under `ui/help/` |

Closed since 2026-10-09: 1. Tips count "N seen · N left" (`ui/help/HelpScreens.kt:284`)

## Mobile · More, settings and pairing (MobileSettings.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Quiet hours set on This phone | partial | This phone only shows the hub's `notify.quiet_hours` (`ui/OrbitSettingsScreen.kt:252-265`). Set on the hub page, not per phone |

Closed since 2026-10-09: 1. Pairing Draw-on and Halo (`ui/PairScreen.kt:268,382-383`, `ui/kit/Loaders.kt:64-67`)

---

