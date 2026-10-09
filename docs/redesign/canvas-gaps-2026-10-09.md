# Orbit Fleet canvas vs the current apps (2026-10-09)

What the live [design canvas](https://claude.ai/artifact/B2sVtJEZodahNG4cvRu7Pu) shows that the apps don't have yet. All 90 boards were checked against claude-fleet `main` (800af4b) plus open PR #779, and fleet-mobile `main` (d170c75, which already includes #187). Each row was checked in code, not by name; evidence is file:line or "no match". Rows list only gaps; each board ends with a count of items that already match.

Totals: **204 missing, 177 partial, 12 present only in PR #779** (not yet on main).

Only 9 boards are new since the repo copy in `docs/ux/2026-10-08-orbit-fleet-redesign/canvas/`: the seven desktop Forms boards and the two phone Forms boards. The other 81 are byte-identical to the repo copy.

## Biggest gaps

**Forms (the 9 new boards, no plan step covers them yet)**
- The form spec (`fleet.form/1`, `forms.rs`) can't express a disabled-with-reason field, a drafted field, a "Proposed by Jev" choice, a per-option detail line or an "Another…" entry. This blocks the same items on desktop and phone.
- No shared form behaviour: no check on leaving a field, no ⌘↵ submit, no "Discard changes?", no Undo toast, no "type the name" delete confirm, no settings Save bar with change count and "was …" values.
- Add an account (subscription, API key, Bedrock/Vertex, device-code sign-in) and New Control API token (scopes, expiry, shown once) don't exist.
- Routines: only three trigger events, no time zone or next run, no dry run. Missions: no planner-drafted first tasks. Toolkit: no Command asset kind, commit still the plain prompt dialog.
- Desktop session forms: no Send later in the composer (and no timed sends in the backend), no Push after commit / Amend, no session labels, no Attach for outside tmux sessions.
- Phone: session forms are still dialogs rather than bottom sheets, and the phone ignores the form drafts PR #779 adds.

**Desktop**
- Missions that wait on you never reach Inbox or Today; Inbox has no state grouping or separate Jev row.
- Task detail has no tabs or Delivery block; tasks have no due date; Board has no per-column add, multi-select or PR chip.
- Missions list is flat (no Active/Paused/Drafts groups, no brake reason, no Runs tab); a finished mission can't be reopened or archive its sessions.
- Control: no suggestion cards, topic threads, /task /plan /assign, or tasks view; Views panel lacks folds, search, diffstat and Library table.
- Shared sessions: nothing reacts to the viewer's access level; Sessions has no All / Mine / Shared tabs.
- Session actions (Fork, Rewind, Switch account, Change model, Archive, Copy link, Copy transcript) are missing from Details.
- Orgs: no Sharing tab, no team panel, no per-member role dropdown; AI "ProposedBy" shows a percentage where the manual says a word.

**Phone**
- Inbox rows have no inline Retry / Switch account / Wait; mission asks and Jev proposals don't reach it; New layout is still not the default.
- New session has no start-from-branch, account row or first message; Details has no usage meter or Fork / Switch account.
- Files tab has no per-file +/− or behind count; Send later has no timed choices; the key bar has no Ctrl or Alt.

**Already known and parked** (PR #779 "Not done"): migration progress ring, Wake host, rebase loader, waiting-for-the-other-side-to-sign, agent install on a standalone desktop, Jev use cases off/shadow.

## Contents

1. Forms, desktop (Anatomy, Work, Automation, Toolkit)
2. Forms, desktop (Accounts, Org, Session)
3. Forms, phone
4. Desktop: Inbox, Work, Missions, Automation, Control
5. Desktop: sessions, files, hosts, toolkit, settings
6. Desktop: kit, wizards, states, motion, AI, orgs, federation
7. Phone

---

# 1. Forms, desktop (Anatomy, Work, Automation, Toolkit)

## Form anatomy (FormsAnatomy.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Field state "Disabled, with the reason" (per-field reason line) | missing | fleet.form/1 `FormField` has no disabled/reason key (crates/fleet-core/src/pages/forms.rs:89-125); FormWizard only disables all fields via `off` (src/lib/forms/FormWizard.svelte:110) |
| Field state "Drafted" inside fleet.form/ChatForm | missing | DraftedLabel.svelte / DraftField.svelte exist but no match for "draft" in forms.rs or FormWizard.svelte; spec cannot mark a field as AI-drafted |
| Field state "Invalid" checked on blur (client side) | partial | FormWizard shows only server problems (FormWizard.svelte:67, problemOf); local problems only disable Next (`ready`, :66); no match for "blur" in FormWizard.svelte / DialogSheet.svelte |
| Disabled verb says why UNDER it | partial | DialogSheet puts the reason in a tooltip only (`confirmTitle` -> title=, src/lib/DialogSheet.svelte:48,79); FormWizard submit has no reason at all (FormWizard.svelte:306,310) |
| Server error banner at TOP of the body | partial | DialogSheet error renders above the footer, below fields (DialogSheet.svelte:68-70); WizardDialog the same (forms/WizardDialog.svelte:62) |
| Enter submits one-field form, Cmd/Ctrl+Enter submits any form | missing | no match for metaKey/ctrlKey in Modal.svelte, DialogSheet.svelte, forms/*.svelte; only Escape handled (Modal.svelte:75) |
| "Refused by the hub" banner with "Your changes are kept. Ask an admin" action | partial | error string only (DialogSheet.svelte:68); no match for "Ask an admin" in src/ |
| "Saved" -> dialog closes and toast offers Undo (generic) | partial | Undo toasts exist only for a few actions (host_actions.ts:79, card_actions.ts:73, Sidebar.svelte:545); dialogs such as EditTaskDialog/WorkRuleEditor/AssetEditor have no Undo (no match for "Undo") |
| Close a changed form asks once "Discard changes?" inline in footer | missing | no match for "Discard changes" in src/; Modal closes on Escape/backdrop without a dirty check |
| Settings field row: scope pill (hub / this device / org overrides the hub) | missing | FieldRow head shows label, experimental/restart tags, History, Reset only (src/lib/pages/FieldRow.svelte:206-232); no scope pill |
| Settings field row: "changed from 4 h · Reset" | partial | Reset exists (FieldRow.svelte:223-231) with the default in a tooltip; no inline "changed from <old>" text |
| Settings page batched save "2 changes · Discard · Save" | missing | FieldRow writes each change immediately (`write(next)`, FieldRow.svelte:250,270); no match for "Discard" in pages/PageView.svelte |
| Destructive confirm: typed name when loss is large | missing | no typed-confirm in ConfirmDialog.svelte or kit/; RoutinesPanel delete is a one-click Delete/Keep (automation/RoutinesPanel.svelte:457-470) |
| Destructive confirm: safer way out on the left ("Pause it instead") | missing | RoutinesPanel delete confirm offers only Delete/Keep (RoutinesPanel.svelte:457-470) |
Done: 14 items match

## Work forms (FormsWork.dc.html)
| Item | Status | Evidence |
|---|---|---|
| New task as a dialog from Cmd+N in Work | missing | only the inline "+ New task" input + ▾ panel (src/lib/TaskList.svelte:177-212); no Cmd+N/new-task row in shortcuts.ts |
| New task: Tracker field ("Fleet only" / create in a tracker) | missing | TaskList add panel has Project + Notes only (TaskList.svelte:197-210); no match for a tracker issue-create in crates/fleet-core/src/service |
| New task: "Start a session for it now" checkbox | missing | no match in TaskList.svelte |
| Edit task for a TRACKER ticket (changes go back to Jira) | missing | EditTaskDialog edits local items only, tracker ticket shows "edit it there" (src/lib/EditTaskDialog.svelte:21-22,84,141-143) |
| Edit task: tracker conflict line ("Jira changed the title 2 min ago … see theirs") | missing | follows from the above; WorkConflictNotice only handles hub E_CONFLICT with Reload (src/lib/WorkConflictNotice.svelte:1-15) |
| Edit task: "Open in Jira" quiet footer link | missing | no match in EditTaskDialog.svelte |
| Name this work: Drafted title | missing | plain input, no draft (src/lib/NameWorkDialog.svelte:140-151); no match for "draft" |
| Name this work: "Also name the N other sessions on this branch" | partial | session checkboxes exist only when the caller passes several (NameWorkDialog.svelte:167-181); row entry passes one session (SessionRowItem.svelte:452), no same-branch lookup |
| Place work: combobox with existing groups and task counts, "+ New group" row | partial | text input + datalist + chips, no counts (src/lib/WorkPlaceDialog.svelte:123-143) |
| Placement rule: Host and Account (sessions start here) | missing | WorkRuleEditor is navigation-only (group); no match for host/account in WorkRuleEditor.svelte; host lives in separate StartRules.svelte (no account) |
| Placement rule: live "Matches 6 open tasks now: …" | partial | explicit Preview button required (WorkRuleEditor.svelte:181-214), not live |
| Placement rule: "Delete rule" inside the editor | partial | delete is in the list, not the editor (src/lib/WorkRules.svelte:118-170) |
| Move to another org: "X loses access" and "$ spend moves to budget" lines | partial | impact lists sessions, hosts, clients, journal (src/lib/WorkOrgDialog.svelte:164-189); no people-access or spend line |
| Move to another org: one-step "Move to Papaya" verb (impact list is the confirm) | partial | needs a separate "Review impact" press first (WorkOrgDialog.svelte:198-200) |
| Save filters as view: "Show its count on the rail" | missing | save view exists (src/lib/WorkFiltersBar.svelte:83,157-237); no match for a rail-count option |
Done: 9 items match

## Automation forms (FormsAutomation.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Routine schedule as a friendly picker with "next run Mon 12 Oct, 08:30" | partial | raw cron text input with words hint (src/lib/automation/RoutinesPanel.svelte:311-313); `next_run_at` typed (src/lib/routines.ts:37) but never shown (no match in *.svelte) |
| Routine Time zone select | missing | only the device UTC offset (crates/fleet-core/src/service/routines/mod.rs:84-86,300); no zone field |
| Routine Account picker separate from Profile | partial | one free-text "Account (login profile)" field (RoutinesPanel.svelte:335-336) |
| Routine dry-run summary + "Run once now" inside the editor | missing | no match for "dry"/"Run once" in RoutinesPanel.svelte; Run now only on a saved routine (:373) |
| Starts on an event: "A pull request gets a review comment" and other repo events | missing | events limited to turn_done/stuck/lost (crates/fleet-core/src/service/routines/mod.rs:50-53; RoutinesPanel.svelte:319) |
| Event filters "Only when repo / author" | missing | no match in RoutinesPanel.svelte or routines/mod.rs |
| Event rate limit "Not more often than once per PR per hour" | missing | no match |
| Routine delete: runs stay, "Pause it instead", typed name | partial | delete says "Its runs go with it", Delete/Keep only (RoutinesPanel.svelte:457-470) |
| New mission: "Let the planner draft the first tasks" checkbox | missing | createMission({ name, goal }) only (src/lib/WorkMissions.svelte:473) |
| New mission: "Import a plan…" from the create form | partial | import exists only on an open mission (WorkMissions.svelte:152-172, 852-867) |
| Import a plan: Repo column per row, "Row N has no repo: pick one" | partial | table parsed as step/needs/lane, counts shown (WorkMissions.svelte:852-867); no per-row repo or pick |
| Answer a mission card (options, own words, Skip) on the mission page | partial | no question/QuestionCard in WorkMissions.svelte; answerable only in the session view (src/lib/kit/QuestionCard.svelte:90) |
Done: 12 items match

## Toolkit forms (FormsToolkit.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Asset kind "Command" | missing | KIND_ORDER = skill, agent, hook, mcp_server, plugin_ref (src/lib/assets.ts:7); Kind enum has no Command (crates/fleet-core/src/service/catalog/model.rs:35-47) |
| New asset: "Write it with Claude…" from the create dialog | missing | AuthorSessionDialog opened only from AssetDetail (src/lib/AssetDetail.svelte:317); no match in NewAssetDialog.svelte |
| New asset: name help "Lower case and dashes; it becomes the folder name" | partial | regex error text only "must match [a-z0-9][a-z0-9-]*" (src/lib/NewAssetDialog.svelte:23) |
| Asset editor: Discard button / explicit Lint button | partial | Cancel + Save only (src/lib/AssetEditor.svelte:491-492); lint shown as a report (:477), no Lint button |
| MCP server editor: Harness as checkboxes (Claude Code / Codex / Agy) | partial | free-text input (AssetEditor.svelte:432-435) |
| MCP server editor: help "Use ${secret:name} …" under Environment | partial | env field present (AssetEditor.svelte:425); substitution exists in sync (crates/fleet-core/src/service/catalog/sync/apply.rs:1114) but no help text |
| Import from a host: "What" kind checkboxes | partial | `only` is a prop, shown read-only (src/lib/ImportDialog.svelte:10,43); no checkboxes |
| Add a secret: per-host Write/Skip for several hosts in one form | partial | one host-or-global select per secret row (src/lib/SecretsPanel.svelte:96-115) |
| Prompt snippet form in Toolkit › Prompts & snippets | partial | label/text/send-on-click editor lives in Settings (src/lib/SettingsDialog.svelte:885-917), not Toolkit; no Remove-with-confirm form |
| Write it with Claude: Host picker | missing | only an Instructions textarea (src/lib/AuthorSessionDialog.svelte:77-78) |
| Set up the catalog: "Push after each change" | partial | setup has path + remote (src/lib/AssetsPanel.svelte:466-467); `catalog.auto_push` is a separate setting (src/lib/fleet_settings.ts:66) |
| Commit asset changes: file list, drafted message, "Commit and push" | missing | still the bare PromptDialog with a fixed message (src/lib/AssetsPanel.svelte:529-536) |
| New layer: "Applies by Organisation / To <org>" | missing | create asks Catalog, Name, Axis context/role (src/lib/LayerChangeForm.svelte:66-95) |
Done: 9 items match

---

# 2. Forms, desktop (Accounts, Org, Session)

## Accounts forms (FormsAccounts.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Add account wizard, step 1 (host, Claude subscription / API key / Bedrock-Vertex, nickname) | missing | No create action on the Accounts page. Accounts are only derived from host logins (src/lib/accounts_page.ts:3-4). The only account write is the nickname (src/lib/accounts.ts:58). No match for add_account / start_login. |
| Add account step 2: device-code sign-in (URL, code, Copy code, 10-min expiry, Done enabled when the host reports the login) | missing | No match for oauth/device or setup-token. Login expiry only tells you to "Run claude /login there" (src/lib/account_usage.ts:510). |
| Add account via API key, with the provider's 401 error shown on the field and a daily spend limit | missing | No API-key or Bedrock account path. Spend budgets exist only per org (crates/fleet-core/src/pages/resources.rs ORG budget_daily_usd). |
| New Control API token (name, Read/Act/Admin scope, expiry, host restriction) | missing | src/lib/ControlApiTokens.svelte only lists, rotates (:70, :112) and revokes tokens. No create action and no expiry or scope field (no token expires_at in store). |
| "Token created" shown-once sheet with Copy token / Copy as env line | missing | Same. Device tokens are shown once only at pairing (ControlApiTokens.svelte:126). |
| Projects layout: base path checked on the host ("/srv/work is not writable by user dev") | partial | Layout and per-host base paths with Save exist (src/lib/SettingsDialog.svelte:369-420). Validation is local syntax only (basePathError). There is no writable probe on the host. |
| Start rules as an inline list under Settings › Projects with Discard/Save | partial | Rules (pattern → project + host, add/edit/delete) live in Automation › Rules (src/lib/StartRules.svelte, AutomationView.svelte:162) and are edited one rule at a time with "Save rule" (:120). There is no batch Discard/Save grid in Settings › Projects. |
| Group a project: project count per group ("3 projects") | partial | The group picker, "New group", and "Back to automatic" (= Remove from group) exist (src/lib/ProjectActionsMenu.svelte:44-46). The options show no member count, and it is a menu mode, not a dialog with Cancel/Save. |
| Host integrations: Fleet agent install row | in PR #779 | src/lib/HostDetail.svelte:601 AgentInstallAction (file only in cf-779). |
| Host integrations: "Debug devices — look for Android phones on USB" per-host toggle | missing | No debug-device setting in HostDetail.svelte. Scanning is fleet-wide plus a per-device "Rescan host" (resources.rs DEBUG_DEVICE actions). |
| Host integrations as one form with Discard/Save | partial | The Codex select applies immediately on change (HostDetail.svelte:298, :857-862). |
Done: 3 items match (tracker sign-in with email/token/stored-on-hub, private CA (flows.rs:484 extra_ca), Replace credential and Atlassian link; Codex assets Auto select; projects layout + per-host base path)

## Organisation forms (FormsOrg.dc.html)
| Item | Status | Evidence |
|---|---|---|
| New organisation: "Its sessions are visible only to its members" switch at create | partial | org.add takes only name and colour (resources.rs ORG create). isolate_sessions can only be set afterwards, as an edit. |
| Org settings switch "Members see only their own sessions" | missing | No org-level field. Per-person privacy is fixed behaviour (ORG members help text), not a switch. No match for "own sessions only". |
| Org settings switch "Hub settings follow <org>" | missing | No match for settings_follow / follow org. Only owns_hub plus per-key org overrides (resources.rs ORG fields). |
| Add a rule: one form with a "Match by" segment (Repository / Path / Host / Owner) | partial | There are three separate add actions: org.add_owner_rule (owner + optional repo), org.add_path_rule, and org.add_host_rule (resources.rs ORG_RULE_ADDS). There is no single segmented form and no separate Repository mode. |
| Add a rule: live impact ("Matches 14 sessions now; 3 of them are in Personal and would move") | missing | No org-rule preview. rule_preview and org_impact exist only for work/task rules (crates/fleet-core/src/mcp/tools/orchestration.rs:1056, :1095). |
| Add a member: pick an existing person or type a new one | partial | The person field is free text (resources.rs org.set_member param "person" text(64)). OptionSource has no People source (resources.rs:50-61). The role choice and the pairing code after adding are done (src/lib/pages/OrgMembers.svelte:94-117). |
| Edit a device: modes Full / Answer only / Watch only | partial | Only full and readonly exist (resources.rs DEVICE_MODES). There is no "Answer only" mode. |
| Edit a device: org and person as dropdowns inside the edit form | partial | These are separate confirm actions, device.bind and device.hand_over (resources.rs DEVICE_RESOURCE actions). The person is free text. |
| Install on a debug device: "Claim the phone while installing" and a note in the same form | partial | debug_device.install takes only path and host (resources.rs DEBUG_DEVICE actions). Claiming is a separate action with its own note. |
| Project catalog entry (org-wide project: name, remote, path on hosts, org, hosts allowed) | missing | The only catalog is the asset catalog (resources.rs CATALOG: name/checkout/remote/org/admitted hosts). No match for project_catalog / org projects. |
| Set an organisation value for an arbitrary key, with a Settings-reference link and the hub default shown | partial | OrgSettingsList.svelte:27-55 sets or inherits only the listed registered keys and shows the fleet's value. There is no free key field and no reference link. |
Done: 9 items match (org name/colour create, bound devices see unassigned, admins see unclaimed, owns hub, auto-tidy inherit, Jev decide, Jev replies, Discard/Save record editor, remove member with revoke/read-only/keep choice (OrgMembers.svelte:59-92), device trusted + revoke)

## Session forms (FormsSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Rename and label: a separate "Label" field (a short grouping word, e.g. "release") | partial | Rename edits friendly_name and tmux name (src/lib/SessionDetails.svelte:289-320). Session tags exist in the backend (store/rows.rs:252, sessions.ts:145), but the desktop has no UI to set or show them (only share.ts:118 mentions set_session_tags). |
| Switch login: each option shows its headroom/limit reason ("5h 75% · week 85%", "weekly limit until Fri 11:00") | partial | The login select shows only name and email (SessionDetails.svelte:891-892). Headroom appears only on the Accounts page "Switch to" (AccountsPage.svelte:126). |
| Switch login: "Proposed by Jev" suggestion | missing | ProposedBy in SessionDetails is used only for related_session (:982). |
| Commit: "Push after commit" and "Amend last" checkboxes | missing | The FileList commit footer has only the message and the commit button (src/lib/FileList.svelte:304-345). The backend supports amend (crates/fleet-core/src/service/repo_mutate.rs:241, :257), but the UI does not expose it. |
| Commit: header "On <branch> · N ahead of origin" in the commit form | partial | The ahead count lives in BranchPushBar.svelte:25 and in a group title (FileList.svelte:166), not in the commit form. |
| New branch: dedicated form replacing PromptDialog | partial | It is still PromptDialog (src/lib/FilesPanel.svelte:568-585). Validation (branch-slug.ts:43) and "Check it out now" are done. There is no corrected-name suggestion ("Use fix/...-2?"), and "From <branch> at <sha>" is shown only when a start point is given. |
| Local changes: optional question for any intent (e.g. Ask the agent to review) | partial | The question field is shown only for the 'custom' intent (src/lib/LocalChangesDialog.svelte:86, :176). |
| Sync a local folder: "Leave out" patterns set before starting the sync | partial | Excludes can be edited only after the link exists ("Excludes…", src/lib/LocalWorkspaceCard.svelte:354-375). |
| Attach to a running session from host › Attach… (tmux sessions fleet did not start, with age, then "Switch to it" / "Add it to the list") | partial | AttachPicker.svelte is task→session linking (:2-8). Host-level adoption exists only as Lost & found adopt (HostDetail.svelte:344-353). There is no host Attach picker. |
| Adopt a lost session: "Ignore" button | missing | No match for ignore/dismiss in LostTargetForm.svelte / LostFoldRow.svelte. |
| Adopt a lost session: ticket in the project pick ("papaya-pos · PD-2988") | in PR #779 | LostTargetForm.svelte:123 ticketProposal (diff vs cf-main). |
| Send later on desktop (composer › Send later form) | missing | queuePrompt is called only by BulkPromptDialog.svelte:71. There is no composer entry. The phone has it (fm-main ui/SessionLater.kt). |
| Send later: "In 1 hour / Tomorrow 09:00 / At…" times and "Skip it if the session is archived first" | missing | The backend queue is idle-only (crates/fleet-core/src/service/sessions/deferred.rs:74). No match for send_at / not_before. |
Done: 5 items match (rename name, switch-and-restart with confirm, commit file list + drafted message, local changes dialog with file pick + intent select, local folder sync start with folder pick)

---

# 3. Forms, phone


Paths: fm = fleet-mobile main (d170c75) shared/src/commonMain/kotlin/dev/claudefleet/mobile, cf = claude-fleet with PR #779

## Mobile session forms (MobileFormsSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Board rule "bottom sheets, not dialogs" with Cancel and the verb at the thumb | missing | Rename, Tags, Edit quick reply, Fork and Background agent are all `AlertDialog`: fm/ui/SessionScreen.kt:1941, :1963, :3232, :2230; fm/ui/NewSessionScreen.kt:455 |
| One sheet for both rename and tags (Name field plus tag chips with ✕ and + Tag, one Save) | missing | still two dialogs, RenameDialog and TagsDialog, opened separately: fm/ui/SessionScreen.kt:1865-1877 |
| Save stays above the keyboard (sheet with IME padding) | missing | dialog only; no imePadding in RenameDialog/TagsDialog (fm/ui/SessionScreen.kt:1941-2000) |
| Edit quick reply: label, prompt, Send on tap, Remove, Save | partial | all fields and Remove exist in an AlertDialog with a checkbox, not a sheet with a switch: fm/ui/SessionScreen.kt:3232-3294 |
| Fork: "From Turn 14 ▾" turn picker inside the sheet | missing | the anchor is fixed by which reply's menu was used (fm/ui/SessionScreen.kt:2178, :2202); ForkDialog has no turn field (:2230) |
| Fork: New worktree / Same worktree as two choices, Name | partial | a "In a new worktree" checkbox plus name field: fm/ui/SessionScreen.kt:2240-2258 |
| Fork copy "Uncommitted changes are not carried." | missing | no match for "Uncommitted" in fm/ui/SessionScreen.kt |
| Background agent started from inside a session (task, Read-only, Stop after time/$, Start) | partial | BackgroundAgentDialog exists with read-only and stop-after time/spend (fm/ui/NewSessionScreen.kt:455-512) but is reachable only from New session on a host, as a dialog; no entry from the session screen (no match for BackgroundAgentDialog in SessionScreen.kt) |
| Background agent read-only copy "It may read and run tests, never edit or push" and "result lands in Inbox" | partial | copy is "No edits, commits or pushes" and "shows in the list once the hub has matched it": fm/ui/NewSessionScreen.kt:477, :491 |
Done: 3 items match

## Mobile work forms (MobileFormsWork.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Link a ticket: error under the field with the fix offered ("Not a key. Did you mean FLEET-142?") | missing | no match for "Did you mean" in fm; errors go to a banner/state.error (fm/ui/SessionTasksViewModel.kt:339-360) |
| Link a ticket: check on Link, verb "Link" | partial | Tasks sheet has search + list + "Link “key”" (fm/ui/SessionTasksSheet.kt:182-199); the session menu's Set work… is still a dialog with verb "Set" (fm/ui/WorkSheet.kt:199-216) |
| Place work: ticket header (key, title, acceptance "1 of 3 done") above the group picker | missing | PlaceSheet shows only "Place in group" (fm/ui/TaskScreen.kt:259-266) |
| Place work: group rows with task counts ("4 tasks") and an explicit "+ New group" row | partial | filter-as-you-type and "Place in “x”" exist, rows show the name only: fm/ui/TaskScreen.kt:262, :291-300 |
| Place work: Cancel and Place buttons at the bottom | partial | Place button above the list, no Cancel (sheet dismiss only): fm/ui/TaskScreen.kt:287 |
| Share: "The whole org" hides the person field | partial | Org choice renames the field to "Org" and still asks which org: fm/ui/ShareSheet.kt:94-111 |
| Share: level named "Steer · send prompts" | partial | level word is "Drive" (fm/model/Sharing.kt:23-36); naming differs from board |
| Settings value: one Save bar for several changes with "1 change" count and Discard | missing | each field has its own Save (fm/ui/FleetSettingsScreen.kt:350-393); no match for "Discard" in FleetSettingsScreen.kt |
| Settings value: previous value shown under the field ("was 36") | missing | no match for "was " in fm/ui/FleetSettingsScreen.kt; only a History button (:146 of the field block) |
| Settings value: scope badge ("hub") on a setting | missing | no match for scope in fm/ui/FleetSettingsScreen.kt |
Done: 5 items match

## Mobile chat forms (MobileChatForms.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Building while Control writes the form (draft streamed, first field fillable while the rest arrives, "reading the Jira epic") | partial | phone shows Building only while `ask { get }` loads (fm/ui/OrbitChatForm.kt:230-260); no match for form_draft in fm; draft streaming exists on desktop only in PR #779 (cf/crates/fleet-core/migrations/151_form_drafts.sql, cf/docs/forms.md:26-40) |
| "Proposed by Jev" on one choice, with a reason and a Change button | missing | FormField options are (value, label) only (cf/crates/fleet-core/src/pages/forms.rs:123); no match for propos/Jev in fm/ui/OrbitChatForm.kt or cf/src/lib/forms/ChatForm.svelte |
| Per-option detail line ("2 idle", "next free on main") | missing | options carry no detail: cf/crates/fleet-core/src/pages/forms.rs:123 |
| "Another…" free-entry choice | missing | FieldType has no other/free option: cf/crates/fleet-core/src/pages/forms.rs:61-70 |
| Comet on the button while sending | partial | button text "Sending…" only, Comet exists unused here: fm/ui/OrbitChatForm.kt:194; fm/ui/kit/Loaders.kt:475 |
| Full screen for long/secret forms, with who asked and why, "Chat" back | partial | Full is a fillMaxHeight bottom sheet (fm/ui/OrbitChatForm.kt:209-221, :332); pager header has title and "Step x of y" only, no why/asked-by (:339-349); first Back says "Close" (:360) |
| Answered line: answer summary ("mercury · 2 tickets linked · token saved") and View | missing | formOutcomeLine is title + state + by only (fm/ui/OrbitChatForm.kt:104-115); no match for "View" in OrbitChatForm.kt |
| The work a form started follows the answered line (Pulse row for the new session) | missing | no follow-up row after the outcome line: fm/ui/OrbitChatForm.kt:166-175 |
| Form from another session shown in Control's chat ("Asked by PD-3012 …") | missing | the card renders only the open session's own pending_form (fm/ui/SessionScreen.kt:859; fm/model/SessionRow.kt:41) |
| Secret field note "stays on the hub" | partial | secret is a password field with the field's help only (fm/ui/components/RichCards.kt:846-868); no fixed where-it-goes note |
Done: 9 items match

---

# 4. Desktop: Inbox, Work, Missions, Automation, Control


Checked against claude-fleet with PR #779 (main up to #785 + PR #779). cf-main has moved past that base (#786-#794: the Automation page, the Control header and the session rows), so the Automation and Control rows were also checked against claude-fleet main (800af4b). Paths are relative to src/lib unless marked otherwise.

## Fleet inbox with a session in focus (Main.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Inbox "Group: state" with section headers ("Needs you 4") | missing | Sidebar.svelte:1476-1506 draws the Inbox as one flat `inboxList`; `sidebarGroupBy` only applies to Sessions |
| "+1 proposed" Jev row ("last turn ended with a question · Not waiting"), kept apart from Needs you | partial | attention.ts:273 `jevSays(s,'asked')` puts the row straight into Needs you (with a label), so there is no separate proposed row the person can accept or dismiss |
| Mission waiting for you in the Inbox ("Hub federation v2 · sign the autonomy grant", Mission badge) | missing | inbox.ts:26 lists sessions only (plus failing routines); no match for mission in Sidebar.svelte Inbox or service/work/today.rs |
| "12 stopped on claude-fleet-trn · Restore" line inside the Inbox | partial | LostFoldRow renders only in the Sessions branch (Sidebar.svelte:1893), not in the Inbox |
| Inbox footer "6 completed today" | partial | inbox.ts:56 `notWaitingText` says "N done" / "N paused", not completed today |
| Inspector actions Fork, Switch account, Archive, Force kill… | partial | SessionDetails.svelte:806-835 inspector has Send prompt, Move to host, Review, Restart, Share, Kill session; Fork, Switch account and Archive are not in the inspector |
| Inspector PR "15/15 checks · no reviews" | partial | SessionDetails.svelte:705-716 shows the PR number and one CI chip; `review_decision` exists in store/pull_requests.rs:33 but is not shown |
Done: 22 items match

## Fleet work view with task filters open (Work.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Tab counts "Tasks 5", "Pull requests 3" | missing | WorkTree.svelte:600-625 tabs have no counts; only Review has a count (work-review-count) |
| Task row PR chip with CI ("PR #476 ✓", "#478 ✕ 1") | missing | WorkTree.svelte:755-772 row shows status, active/past counts, spend; no pr/ci in the task row |
| Task row status sentence ("In progress · session needs you", "mission run 3 of 5") | partial | WorkTree.svelte:758 shows a ● needs-you dot plus status; no reason sentence |
| "Hidden by filters 31 · Show" | missing | WorkTree.svelte:846-851 counts only archived tasks that are hidden; no match for a filtered-out count |
| Per-org and per-group spend in the list headers | in PR #779 | WorkTree.svelte:723,736 `work-org-spend` / `work-group-spend` (0 hits in cf-main) |
| Task detail tabs Overview / Sessions / Activity / Comments | missing | no tab markup in WorkTaskDetail.svelte; no match for Activity or Comments |
| Brief block in task detail: "Draft brief from the ticket", drafted by haiku, Regenerate / Clear / Undo | partial | drafting exists only in StartPopover.svelte:153-162 (start_preview.ts:103); TaskWorkSections.svelte:55 Brief is read-only |
| Delivery block (PR + checks, tracker column, spend across sessions + duration, assignee) | partial | assignees (WorkTaskDetail.svelte:313) and spend (TaskBlockedSpend) exist; no PR/checks line, tracker column or duration in the task detail |
Done: 14 items match

## Missions (Missions.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Mission list grouped by state (Active / Paused / Drafts / Completed ›) with Filters and Group | missing | WorkMissions.svelte:1174-1184 is a flat `<ul>` of name + state badge + "x/y done" |
| Row reason line ("Needs you: 2 cards · wave 3", "Brake: no progress in 2 h", "Planner proposed 6 tasks · not accepted") | missing | missions.ts:378 `progressLabel` returns only "x/y done" |
| List footer "Orchestrator on · ceiling L1 · Pause all" | partial | Pause all is at WorkMissions.svelte:1156; ceiling and orchestrator state are shown only in the detail header (:239) |
| Detail tabs Plan / Runs 14 / Log / Repos 2 | partial | they are sections (h4 Tasks, Repos :1079, Log :1121); there is no Runs list for a mission |
| "Planner 2 of 4 runs this hour" | missing | `max_planner_runs_per_hour` is only in the policy type and error text (missions.ts:50,790); it is never displayed |
| Brakes line ("Pause on spent budget or 2 h without progress") in the detail | partial | `no_progress_secs` is in missions.ts:51 / :573 (log text) only; no match for no_progress in WorkMissions.svelte |
| Owner line ("You · 32bit") | missing | no match for an owner in WorkMissions.svelte detail |
Done: 24 items match

## Finished mission (Finish.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Finished header actions Reopen / Archive N sessions | missing | no match for reopen or archive in WorkMissions.svelte or missions.ts; the ⋯ menu (:690) only moves to final states |
| Mission-level Finish checks card ("3 of 3", CI run, review run, person check with name and time) | partial | the done-when checks are per task (mission-checks :951, mission-conds :1006); there is no mission-level summary |
| Pull request block (Merged #476, checks, approved, branch deleted, "Also merged: fleet-mobile #88") | missing | no PR block in WorkMissions.svelte |
| "Sessions to archive" list with clean / pushed state | missing | no match in WorkMissions.svelte |
| "After archiving: moves to Completed (5). Spent $31.40 of $40 · 3 d 4 h" | partial | spend and budget appear in the loop line (:727); no duration and no Completed count |
| Waves summary (✓ Wave N · k tasks · names) | partial | the graph and list show waves (MissionGraph.svelte), but there is no finished summary |
| Empty state "Work · no tracker connected" with Connect a tracker… / Create a task | missing | WorkTree.svelte:708 says "No work yet…" with no buttons |
| Empty state "Automation · no routines" with + New routine / Use a template buttons | partial | cf-main automation/RoutinesPanel.svelte:478 is text only ("Start with Morning PR sweep from + New") |
| Empty state "Control · first run" (Ask Control about your fleet; What needs me? / Plan a task / Start a session on mac) | partial | AgentPanel.svelte:150-176 has operator command chips ("Tidy up done tickets"; Add project chip in PR #779), but no first-run card |
Done: 5 items match

## Fleet work board (Board.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "+" add a task per column | missing | WorkBoard.svelte:308-311 column header has only label + count |
| Card keys x select and s start new (hint "drag, or ← → to move · x select · e edit · s start new") | missing | shortcuts.ts:267-270 work-board has only e, ←, →, Esc |
| Multi-select of cards | missing | no selection set in WorkBoard.svelte (only `selectedTaskId`) |
| Card PR chip ("waiting #476 ✓", "#478 ✕ 1", "#489 merged") | missing | WorkBoard.svelte:362-378 card has key, title, status, project, subtasks, proposals, spend, live session |
| Card assignee + due ("You · Fri") | missing | no due field anywhere (no due column in crates/fleet-core/migrations) |
| Card agent kind (Claude Code / Codex) and mission / wave chip | partial | the live session name and host only (WorkBoard.svelte:376) |
| "Start new" on a card without a session; Continue ▾ and "Finishes when …" on the selected card | missing | no start button or done-when line in the card snippet |
| "Drop to move to In review" drop-target text | partial | the column gets the `.over` class (:301); no text |
Done: 7 items match

## Fleet work review (Review.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Unsure case: "Jev is unsure: PD-2970 and PD-2974 both fit · nothing pre-selected" with Pick a ticket… / No ticket | missing | no match for unsure, both fit or "No ticket" in WorkReview.svelte |
| "Done today: 4 confirmed · 1 rejected · Undo last" tally | partial | WorkReview.svelte:115,196 keeps an Undo for the last decision only; there is no day tally |
| Merge of a duplicate proposal moves sessions and subtasks | in PR #779 | `mergeWorkProposal` in TaskWorkSections.svelte:141 (cf-main's Merge only rejects the proposal) |
Done: 10 items match

## Task detail (TaskDetail.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Inline status dropdown in the header ("In progress ▾") | partial | status is set in EditTaskDialog.svelte:11 `setWorkStatus`, behind Edit |
| Tabs Overview / Sessions / Activity | missing | see Work board; no tabs in WorkTaskDetail.svelte |
| Brief drafted by haiku with Regenerate / Clear / Undo in the task | partial | only in StartPopover.svelte:153 |
| Subtask progress "2 / 4" with ✓ ◐ ○ states | partial | TaskWorkSections.svelte:62 shows a count only |
| Placement: Account, Model · effort and host fallback ("mac, else mercury") from a rule | missing | StartRules.svelte:32-111 rules carry a project and a host; no match for model, effort or profile in WorkRuleEditor.svelte |
| "Start with last settings  s" in the Continue menu | partial | only in QuickSwitcher.svelte:997 (⌘↵); WorkButton.svelte has no such item |
Done: 14 items match

## Control › Today (Today.dc.html)
| Item | Status | Evidence |
|---|---|---|
| KPI tiles (Needs you 4, In progress 6, Shipped today 4, Stale 2) | missing | TodayView.svelte:181-240 goes from the header straight to lists |
| Subheader "Thursday 8 October · 3 hosts · 2 accounts active" | missing | TodayView.svelte:183 header is "Today" + buttons |
| Mission in Needs you ("Hub federation v2 · sign the autonomy grant") | missing | no mission in service/work/today.rs |
| Inline Switch account / Wait until on a paused-limit row, and an Open button per row | partial | TodayView.svelte:120-175 rows jump to the session; LimitActions is used only by SessionRowItem.svelte |
| Shipped: provenance ("from Morning PR sweep") and releases installed ("Release 0.5.3 · apps installed mac, phone, NAS hub") | missing | TodayView.svelte:218-232 shipped is work items / PRs only |
| "4 more ›" truncation in In progress | partial | groups render in full |
Done: 8 items match

## Tidy up (Tidy.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Footer "4 selected · frees about 2.1 GB" (worktree size) | missing | no size or bytes in service/gc/tidy.rs or TidyReview.svelte |
| "What each choice does" legend | missing | no match in TidyReview.svelte |
| Selected-row explanation ("Branch is merged, worktree clean, 0 unpushed… removes ~/.worktrees/… (380 MB)") | missing | no detail pane in TidyReview.svelte |
| "Restore all" on the "Stopped on host" group | missing | no match for Restore in TidyReview.svelte (Restore lives in the Sessions lost fold / HostDetail) |
| Reopened as a group in the same sheet with Resume / Dismiss | partial | a separate "n reopened" list (TidyReview.svelte:16-18,118) |
| Choice wording: Clean up (commits and pushes first) / Archive / Keep for 7 days / Never / Expire | partial | tidy.ts:63-70 labels are Safe kill / Kill / Archive only / Snooze 7 d / Never for this work / Keep N d |
Done: 8 items match

## Automation (Automation.dc.html)
Checked against cf-main, which redid this page after PR #779 branched (c68ee0cf, e9e96ad7).
| Item | Status | Evidence |
|---|---|---|
| Fleet-wide daily budget in the list footer ("Today $4.10 of $15.00 budget") | missing | cf-main AutomationView.svelte shows today's spend only; the budget is per routine (RoutinesPanel.svelte:701) |
| Per-run time cap ("$2.00 · 20 min") | missing | cf-main RoutinesPanel.svelte:716 Per run is dollars only; no minutes field in the editor (:535) |
| Host fallback ("mac, else mercury") | missing | cf-main RoutinesPanel.svelte:722 `Host: r.host_alias` |
| On failure "retry once" | missing | cf-main RoutinesPanel.svelte:724 fixed text "Inbox as Failed until you retry or pause it" |
| Routine autonomy level ("L1 · asks before push") stat | missing | no match for autonomy in cf-main RoutinesPanel.svelte |
| "Nothing to do · kept out of Inbox · Proposed by Jev · Change · Send to Inbox" | partial | cf-main RoutinesPanel.svelte:642-645 shows "Read by Jev" with no Change or Send to Inbox |
| Failed run Details with the error code ("E_GH_AUTH · gh: HTTP 401") and a named fix ("Re-login on mac") | partial | cf-main RoutinesPanel.svelte:665-667 Details shows the trigger and reason; Fix is generic |
| Outcome routing row ("Needs person goes to Inbox; Nothing to do stays in Runs") | missing | not among the cf-main inspector KeyValue rows (:714-725) |
Done: 22 items match

## Control (MissionControl.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Suggested from your fleet" cards (Finish CI on PR #478 · Start session; Restore 12 stopped; Review a routine) | missing | no match for suggested-from in src/lib outside work.ts row suggestions |
| Chat threads by topic ("Overview ⚙", day separator) | missing | AgentPanel.svelte is one operator conversation; no match for topic or thread |
| Receipt "↳ Sent to session/mission … ↗" that hands the message on | partial | control_route.ts:6-18 receipt is advisory ("never moves, holds or re-sends it"); agent handoffs are HandoffCards |
| Composer hint "# task @ host / command" | missing | no match for "# task" or "@ host" |
| Views panel Needs you: "Welcome back… 4 need you · 6 running · 9 idle" and Running / Idle / Done today folds with inline limit actions | missing | ControlViewsPanel.svelte:181-195 is a flat Needs you list (name, status, age) |
| Views panel search ⌕ | missing | no search in ControlViewsPanel.svelte (⤢ and the menu exist) |
| "Routines in Automation ↗" in Open elsewhere | missing | control_views.ts ELSEWHERE has tasks, missions and hosts only (cf-main and cf-779) |
Done: 9 items match

## Control views: PRs, Library, Today briefing (MCViews.dc.html)
| Item | Status | Evidence |
|---|---|---|
| PR diffstat (+18 −6) | missing | no additions or deletions in prs.ts / WorkPrs.svelte |
| "from <mission>" origin on a PR (board: every PR links to its session or mission) | partial | WorkPrs.svelte:132 links to the opening session only |
| Library table (Name ▴ / Modified / Size, sortable), grid/list toggle, Type ▾ | partial | LibraryView.svelte:29 filter chips + a flat list |
| Library folders (Artifacts today, Session outputs) and repo entries per host | partial | repos are listed per host; no folder tree; no claude.ai artifacts |
| "Link a repo on a host", "Add a Google Drive folder" | missing | no match in LibraryView.svelte |
Done: 9 items match

## Control with a session in the panel (MCSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Focus header "host · account · PR" | partial | ControlViewsPanel.svelte:207 has status · host · PR, no account |
Done: 8 items match

## Control tasks in chat (MCTasks.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Plan card by wave, with "finishes when" per task, agent per task (Claude Code / Codex / You), drag to reorder, "unchecked: skip" | partial | HandoffCards.svelte:176-230 tree card: untick + Create tasks / Create as a mission / Undo N min; no match for wave, done_when, agent or drag |
| "May duplicate TASK-236 · Merge / Keep both" in the plan card | missing | duplicate handling exists only on task proposals (TaskWorkSections.svelte:111), not in HandoffCards |
| "Edit in Work" on the plan card | missing | no match in HandoffCards.svelte |
| Task status card in chat (#TASK-219: finish checks, live session line, Start review run / Mark verified / Move to Done / Open in tracker) | partial | HandoffCards.svelte:164-175 a #TASK chip with a status chip that opens Work |
| Created-task card "Drafted from your message · Undo" with Owner / Due / Group pickers + Proposed by Jev | missing | no due field in the backend; no match for these pickers |
| Slash commands /task /plan /done /assign /start and "# link a task" | missing | no match for '/task' or '/plan' in src/lib |
| Tasks view in the Views panel (☑ Tasks 9: Filters Mine + Claude, Group: status, Needs you / In progress / Up next / Done this week, bulk Start new (2) / Assign… / Done) | missing | control_views.ts:26-30 views are needs-you, session, prs, library, today |
| ◷ (routines) view in the panel | missing | not in control_views.ts |
Done: 3 items match

---

# 5. Desktop: sessions, files, hosts, toolkit, settings


Paths: `main:` = claude-fleet main (800af4b), `pr:` = claude-fleet with PR #779 (main base + PR #779). Main has moved past the PR's base (#786-#794: Toolkit nav, session rows/footer, Settings rows), so main counts as current. Items found only in `pr:` are marked "in PR #779".

## Fleet all sessions (Sessions.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| All / Mine / Shared with me scope tabs with counts above the list | partial | "Shared with me" is a collapsible group (main: src/lib/Sidebar.svelte:1856). "Mine" is only the Work assignee chip (SidebarFilters.svelte:361). There is no session scope switch |
| View options › Group by Organisation | missing | main: src/lib/filter_schema.ts:81-87 SESSION_GROUPS = project, work, state, host, agent. Organisation grouping exists for Work only (:95) |
| "Agent: any" filter facet | missing | no agent facet in filter_schema.ts. Agent exists only as a grouping (:86) |
| Group overflow "4 more running ›" | missing | no match for "more running" or a group cap in Sidebar.svelte |
| "Done 6 today" (done group limited to today) | partial | row_groups.ts:35 has a Done group but no "today" window |
| Shared rows show who shared and the level ("shared by Petra · can watch / can steer") | missing | Sidebar.svelte:1869 renders a plain sessionRow. No match for "can steer" or a sharer label |
| Inspector PR line "15/15 checks · no reviews" | partial | SessionDetails.svelte:712 shows ci_status only. Review decision appears only in the Work PR list (prs.ts:57) |
| Inspector Actions: Fork, Switch account, Archive | missing | session_actions.ts:44-57 ROW_ACTIONS has none of them. Fork and Rewind are per turn only (ReplyActions.svelte:8). Switch account appears only on limit-paused rows (LimitActions.svelte). Archive applies to work only (work_filters.ts) |
Done: 20 items match

## Fleet session agent tab (Agent.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Popped-out agent tab controls "Send keys…", "Clear view", "Pop back in" | missing | no match for "Send keys", "Clear view" or "Pop back in". TerminalView.svelte:298 popOutTerminal opens only a second view |
Done: 10 items match

## Fleet session with terminals open (Terminals.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Shell tab ⋯ / right-click menu (Clear, Split right, Pop out, Kill terminal… "the session keeps running") | in PR #779 | pr: src/lib/TerminalStrip.svelte:150-221. Absent from main TerminalStrip.svelte |
| "New terminal opens on: This worktree · mac ▾" picker | in PR #779 | pr: TerminalStrip.svelte:202. No match on main |
| Terminal list shows what runs in each shell ("pnpm dev running") | missing | no pane_current_command or foreground field in TerminalStrip.svelte or terminals.ts |
| Next terminal on Windows/Linux = Ctrl+Alt+` | partial | main: shortcuts.ts:143 binds Ctrl+` |
Done: 10 items match

## Fleet session details (SessionDetails.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Header "Private" visibility badge next to the state | missing | no 'Private' label in SessionTabs.svelte or ConversationHeader.svelte (Share… exists, SessionTabs.svelte:97) |
| Timeline "Linked to TASK-219 · you confirmed · Proposed by Jev · Unlink" | in PR #779 | pr: src/lib/TimelineWorkProposal.svelte (file not on main) |
| Related sessions N1 proposal with Link / Not related | partial | SessionDetails.svelte:975-979 shows ProposedBy with no accept or reject. No match for "Not related" |
| Reviews block: run verdict ("no blocking findings · 2 nits"), Open, "Start a review run" | partial | SessionDetails.svelte:991 lists review sessions only. ReviewDialog is reachable only through the Review… action |
| Steer actions: Fork…, Rewind…, Switch account, Change model | missing | not in session_actions.ts:44-57. No match for "Change model". Rewind and Fork exist only per turn (ReplyActions.svelte) |
| Share actions: Copy read-only link, Copy transcript | missing | no match in src/lib |
| Archive (session) | missing | no session archive command. Only archive_session_work exists (share.ts:188) |
| Share sheet: Private / org / people with Read · Answer · Steer wording | partial | ShareSheet.svelte:259-261 uses watch / answer / drive. Org grant exists (:87). No "Private · only you" state |
| Share sheet "Peter can only read until you trust his iPhone · Trust now" | partial | share_devices.ts:16 warns only. No inline trust action |
Done: 12 items match

## Files tab (Files.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Diff toolbar "Ask Claude about this" | missing | no match in DiffView.svelte, FileViewer.svelte or FilesPanel.svelte |
Done: 12 items match

## Files tree (FilesTree.dc.html)
Done: 9 items match

## Files history (FilesHistory.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Remote "origin/claude/* · 11 merged and safe to delete" with delete | partial | BranchList.svelte:33,87 counts merged remotes, but delete-merged covers local branches only (:32 mergedLocals) |
Done: 10 items match

## Command palette (Palette.dc.html)
Done: 20 items match

## New session (NewSession.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Agent picker: Agy startable | partial | agent_picker.ts:14-15 STARTABLE_AGENTS = claude, codex. Agy is listed but has no launch |
| "Has previous work: Resume pd-2412 · Proposed by Jev (N2) · Start fresh instead" | partial | rule-based notice at NewSessionDialog.svelte:1305. No N2 resume-or-new use case (no module in crates/fleet-core/src/service/decide/) |
Done: 18 items match

## Dialogs (Dialogs.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Move to host: each host shows latency, free disk, "account at limit" | partial | TransferSheet.svelte:374 option shows the alias only |
| Start a review run: reviewer agent choice ("pr-review skill · Codex") | partial | ReviewDialog.svelte:2-3. A skill run by Claude Code only, no agent choice |
| Placement rules: rule names account and agent (PD-* → … · tech.silvester, FM-* → … · Codex) | partial | crates/fleet-core/src/service/start_rules.rs:49-59 StartRuleInput = pattern, project_id, host_alias, org_id |
Done: 5 items match

## Watching a shared session (Watch.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Header for a recipient: "Waiting for Martin · Shared with you · Read", "Shared by … · level · since" | missing | accessOf is used only in TerminalView, SessionTabs, Sidebar, FileViewer and ShareSheet. ConversationHeader has no level |
| "Ask Martin for Answer" (request a higher level) | missing | no match for a level-request action |
| Answer card read-only at Read level ("You can read this session… with Answer you could reply here") | missing | AnswerPrompt.svelte has no access check. No match for the copy |
| Inbox "Shared with me" rows "Martin · Read · waiting for Martin" / "via 32bit" | missing | same as Sessions: Sidebar.svelte:1869 plain rows |
Done: 4 items match

## Accounts & hosts (Accounts.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| "+ Add account" | missing | no match in AccountsPage.svelte or HostsView.svelte. Accounts are derived from host logins only |
| Header "usage refreshed 1m ago" with a page-wide Refresh | partial | per-account Refresh only (AccountsPage.svelte:370) |
| Account card: paused-by-limit count, Show, "Switch to admin@…", routines count | in PR #779 | pr: AccountsPage.svelte:104-157. No "paused" on main AccountsPage |
| "fallback for routines" account role | missing | no match |
| Hosts table agent column "0.5.2 · update" as an action | partial | hosts_table.ts:93-102 flags an old Claude version, with no action. Install action AgentInstallAction.svelte is in PR #779 |
| Tidy hint "12 stopped sessions … hold 9 GB of worktrees · Review in Tidy" | missing | no match in HostsView, HostsTable or AccountsPage |
| Paused-sessions panel listing sessions and routines with Switch account / Wait until / Show in All sessions | partial | LimitActions.svelte appears on session rows only. No panel and no paused routines |
Done: 8 items match

## Host detail (HostDetail.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| "Open a shell" on the host | missing | no match in HostDetail.svelte |
| Tabs Overview / Sessions 3 / Lost & found 3 / Provisioning | partial | one scrolling page with sections (HostDetail.svelte:579,683,849,907) |
| Skills drift row with Sync | partial | host_check.ts:212-235 reports drift. No Sync button on the row |
| Foreign tmux pane "Ignore" | missing | no match. Adopt only (HostDetail.svelte:800) |
| Found transcript "Find another…" | missing | no match. Restore into… only |
Done: 12 items match

## Add project (AddProject.dc.html)
Done: 10 items match

## Toolkit (Toolkit.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| "+ Add skill" in the Skills view | partial | main: src/lib/Toolkit.svelte has Sync all hosts and Edit only. New skills go through Assets › New asset |
| Toolkit nav (MCP servers, Hooks, Prompts & snippets, Downloads, "Agents are in Automation") | done on main, not in PR branch | main: Toolkit.svelte:29-33,81-88 (commit 84a6d36a); pr: Toolkit.svelte:42-43 has Skills and Assets only |
Done: 9 items match

## Assets (Assets.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Layer detail tabs Assets · Changeset · Hosts · Source | partial | LayerInspector.svelte:32-33 tabs = Members, Hosts |
Done: 14 items match

## Settings › Appearance (Settings.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Text size (scale from 13 px) | missing | no text-size, zoom or font-scale pref in src/lib |
| Agent tab name (Per agent) | missing | behaviour is fixed. No setting in AppearanceSettings.svelte |
| Badge counts (Needs you only) | missing | behaviour is fixed in attention. No setting |
| Elsewhere › "Automation limits ↗" | partial | main: settings_tree.ts:127 has Automation under System, not as an Elsewhere link |
Done: 6 items match (Layout Classic/New was removed on purpose in step 13.1)

## Settings › Hub & sync (SettingsHub.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| "Sync while away" (keep local sessions on the hub while the app is closed) | missing | no match in settings.rs or src/lib |
| "Projects on the hub: 6 of 9 ▾" | missing | no hub project-scope setting found |
| Control API "+ Token" (named tokens such as "Grafana dashboard · readonly") | missing | ControlApiTokens.svelte lists host and device tokens with Rotate / Revoke only (:93-117). Nothing creates a token |
| Hub status header "last sync 4 s ago · 2 more of your devices · Unpair…" | partial | SettingsDialog.svelte:463 shows the URL and the pairing name. No last-sync time or device count |
Done: 5 items match

## Settings, other sections (SettingsMore.dc.html)
Done: 14 items match

## Settings, more sections (SettingsSections.dc.html)
| Item | Status | Evidence |
| --- | --- | --- |
| Decisions N2 "Resume or new" use case | missing | no resume_or_new in service/decide/ or settings.rs (every other J/K/N row has a decide.jev.* field, pages/settings.decisions.json) |
| Decisions J6 main ticket / J7 tracker duplicate | in PR #779 | pr: service/decide/main_ticket.rs, tracker_duplicate.rs |
| Writing help toggles: Draft commit messages, Draft agent briefs, Draft release notes, Catch-up summaries | missing | settings.rs has only "Summary model" and "Classification nudge" (:1215). Drafts are always on |
| Repair workspace "Repair now" / Restore lost sessions "Restore 3 lost sessions" buttons in Settings | partial | settings_tree.ts:74-125 point to settings-only sections. The actions live in HostDetail |
Done: 30 items match

---

# 6. Desktop: kit, wizards, states, motion, AI, orgs, federation


Current = claude-fleet with PR #779 (main + PR #779). "in PR #779" = present in cf-779, absent from cf-main.

## Fleet conversation components (Components.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Wizard step chips ("✓ Basics · Database · Domain · Review") above the step | missing | src/lib/forms/FormWizard.svelte:183 shows only "Step N of M" text; no step list |
| "Shown because 'Needs a database' is on" note on a conditional step/field | missing | `when` evaluated silently in src/lib/forms/form_model.ts:88-92; no match for "Shown because" |
| "Save and finish later" on a form wizard | missing | no match for "finish later"/form draft in src/lib/forms/; FormWizard row has only Cancel/Back/Next/Submit (FormWizard.svelte:298-310) |
| Review step with per-section "Edit" links back to a step | missing | no review step type in crates/fleet-core/src/pages/forms.rs (FormStep: title/intro/when/fields only) |
| Inline pre-submit check warning ("Postgres needs 2 GB free, host has 1.4 GB") | partial | only per-field validation problems (FormWizard.svelte:293 `problemOf`); no host-fact check |
| Secret field note "🔒 Written to a file on <host>, never shown to the agent" | missing | secret is a bare password input, FormWizard.svelte:281-289; only generic `help` text |
| Submit shortcut "Create ⌘↵" | missing | no Enter/Meta handler on submit in FormWizard.svelte (only 1–9 option keys, :164) |
| Receipt "answered by Martin on the phone · 12:41" (device + time) | partial | src/lib/forms/receipt.ts:94 says "answered by X", no device or time |
| Live progress: elapsed time ("3 of 5 · 1m 12s") and per-step sub-line ("attempt 2 of 10") | partial | ProgressBlock has done/total/steps(title,state)/note only, src/lib/rich_blocks.ts:103-113; no elapsed, no step detail |
| Error card buttons "Retry" / "Re-login on mac" as actions | partial | src/lib/rich/ErrorCard.svelte:27-38: next steps only fill the composer, they do not run |
Done: 11 items match

## Orbit wizards in chat (ChatWizards.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Wizards opened in chat, building card with what it reads, collapse + Pulse "Starting…" | in PR #779 | src/lib/forms/ChatWizards.svelte, WizardChatCard.svelte, chat_wizards.ts absent from cf-main |
| Control drafts a form from a free-text message ("Set up a new project for…") | partial | Control only opens fixed catalog wizards via a button, src/lib/AgentPanel.svelte:153-160 (`openChatWizard(..., 'add_project')`); no NL-to-wizard path |
| Secret "🔒 stays on mercury" line under the field | missing | see Components; FormWizard.svelte:281-289 |
| Step chips "Step 2 of 3 · Where" with named step list | partial | step title + count only, FormWizard.svelte:183-186 |
Done: 9 items match

## Fleet add host wizard (Wizard.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "fleet-agent not installed · Install 0.5.4" from the wizard | in PR #779 / partial | src/lib/AddHostWizard.svelte:304-313 + AgentInstallAction.svelte (cf-779 only); standalone desktop install still listed as not done (4.9) |
| Per-check timing ("SSH as martin@mercury · 18 ms") | partial | latency only in host_check summary (src/lib/host_check.ts:97), not on wizard check rows |
| "Claude Code, Codex on PATH" | partial | Agents step says "Fleet runs Claude Code today; the others are shown as they arrive" (AddHostWizard.svelte:335) |
Done: 8 items match

## Fleet first-run tour (Tour.dc.html)
Done: 9 items match

## Fleet guide walkthrough (Guide.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Provenance line on an approved guide ("proposed by a session, approved by Martin on 6 Oct") | partial | only shown in the review queue, src/lib/pages/GuideReview.svelte:118; not on the approved guide in PageView |
| "Each change is listed so you can undo it" | partial | only a "N changed" tag, src/lib/pages/PageView.svelte:161; no per-change undo list |
| Chat guide card as a summary ("4 steps · changes 2 settings · Start") | partial | src/lib/rich/GuidePageCard.svelte:43-50 renders the whole guide inline + Open in Settings; no summary/Start |
Done: 8 items match

## States (States.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Host offline: "Move sessions…" action | missing | src/lib/states/HostOffline.svelte:95-109 offers Try again / Host detail / Show sessions only |
| Inbox empty names the next routine ("next routine is Morning PR sweep at 07:30") | partial | src/lib/Sidebar.svelte:1402-1410 shows "Not waiting · …" counts only; no routine line |
| No-results: "Start new session 'receipt totals'…" prefilled from the query | partial | sidebar empty offers plain "Start a new session" (Sidebar.svelte:1818); prefill only via ⌘↵ in QuickSwitcher.svelte:947 |
| First run copy "runs Claude Code, Codex or Agy … This Mac counts" / "Use this Mac" | partial | Sidebar.svelte:1822-1827 "Start with one host" with Add / pair; no "Use this Mac" match |
Done: 12 items match

## Toasts (Toasts.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Limit-hit toast "tech.silvester hit the weekly limit · 2 sessions paused until Fri 11:00 · Show paused sessions" | missing | no match for "hit the" / limit toast in src/lib (only header pill aria, header_accounts.ts:75) |
| Rule-suggestion toast "You picked papaya-pos for PD-* 6 times · Add rule / Not now" | partial | exists only in Settings, src/lib/StartRules.svelte:141-143; not a toast |
| Two-button toast (Add rule + Not now) and secondary line under the title | missing | Toast has a single `action` and one `message`, src/lib/toasts.ts:21-37 |
| Archive toast "freed 2.1 GB" | partial | src/lib/Sidebar.svelte:541-548 "Archived N sessions" + Undo, no freed size |
| Notifications panel ⚙ (notification settings link) | missing | src/lib/NotificationList.svelte:22 has only "Mark all read" |
| Downloads "Pause" on a copy in flight | missing | no match for pause in src/lib/downloads.ts / DownloadsSheet.svelte |
Done: 10 items match

## Light mode (Light.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Needs-you header "+1 proposed" count (Jev-proposed waiting rows) | missing | no match for "proposed" count in src/lib/inbox.ts / Sidebar.svelte group header |
Done: light theme and every other item match (theme.ts:3 light/dark/auto)

## Motion frames (Motion.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Question card enters (fade up 8 px) and focus moves to Approve | missing | no animation/focus call in src/lib/kit/QuestionCard.svelte or AnswerPrompt.svelte |
| Inbox count rolls 2→3 (80 ms), plain swap when Reduced | missing | src/lib/kit/Count.svelte is a static span; no transition in AppRail.svelte |
Done: 5 items match

## Logo motion (LogoMotion.dc.html)
Done: 12 items match (all in src/lib/loader-kit.generated.ts)

## Particle loaders (Loaders.dc.html)
Done: 12 items match

## Loaders in use (LoadersInUse.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Host offline "Wake host" | missing | src/lib/states/HostOffline.svelte:7-9 "nothing in the fleet can wake one yet" |
Done: 7 items match

## Loaders in flows (LoadersInFlows.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Link two hubs: "waiting for Peter to sign · no loader" | missing | known not-done 11.12; no match for "to sign" in src/lib/pages |
| Toast long job: click ring to open the job ("Open") | partial | ring in src/lib/Toasts.svelte:50; open-the-job action not verified beyond generic `action` |
Done: 10 items match

## Startup (Startup.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Upgrading the database · 3 of 5" Progress ring | missing | src/lib/startup.ts:7-16 explains migrations run before the window can paint (3.15 not done) |
| Liquid orbit for rebase | partial | liquid-orbit used for fork/merge (ForkSheet.svelte:242, LocalWorkspaceCard.svelte:271); no rebase op |
Done: 17 items match

## AI patterns (AIPatterns.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "A confidence word, never a percentage" | partial (contradicts) | src/lib/ProposedBy.svelte:38 renders `{confidence_pct}%` |
| Corrected state line "You changed papaya-pos → papaya-api · recorded as a correction" | missing | correction recorded silently (src/lib/host_placement.ts:23); no on-screen copy |
| Edited draft: "Edited · your text now · Regenerate asks before replacing it" | partial | src/lib/DraftField.svelte:76-86 Regenerate replaces without asking; no "Edited" label |
| "When AI changed something" Undo line on every AI-caused change ("Linked to PD-2592 · Proposed by Jev · you confirmed · Undo") | partial | Undo exists for auto work links (src/lib/work.ts:302,429); not a uniform pattern |
| Use cases off/shadow in Settings › Decisions | partial | listed not done in PR #779 ("Jev use cases off/shadow") |
Done: 7 items match

## The fleet agent (AgentStates.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Multi-step plan confirmed with one "Confirm 2 / Cancel" | missing | src/lib/ConfirmCards.svelte:50-66 one Approve/Deny card per request |
Done: 7 items match (all six blocked states have a next step, src/lib/operator.ts:122-175)

## Org overview (OrgOverview.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Sharing" tab on the org | missing | tabs in crates/fleet-core/pages/settings.orgs.json: Overview, Members, Devices, Spend, Settings |
| "What belongs": Projects and Accounts rows | missing | org fields rules/hosts/trackers/catalogs only (crates/fleet-core/src/pages/resources.rs:524-700) |
| Members summary "2 admins · 2 members · 1 viewer" and tab count badges | partial | session_count/needs_you only (resources.rs:22-24) |
| Header "owns the hub fleet.rlt.sk · you are an admin" | partial | `owns_hub` is a Settings-tab field, not the header line |
Done: 9 items match

## Org members (OrgMembers.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Inline role dropdown per member ("Admin ▾") | partial | role is plain text (src/lib/pages/OrgMembers.svelte:149); changing it goes through "Add or change a member" form (resources.rs:654) |
| Removed member row ("removed 3 d ago · Grants…") | missing | no match for "removed"/"Grants…" in OrgMembers.svelte |
| Device "not trusted" shown in the member row, "Trust device" step in Add member flow | partial | Add flow says trust later in Settings → Devices (OrgMembers.svelte:192-210) |
Done: 8 items match

## People & devices (OrgDevices.dc.html)
| Item | Status | Evidence |
|---|---|---|
| One "People & devices" table (Person, Org, Mode, Trust, Last seen) with Filters (Org/Person) and Group by person | partial | separate master-detail pages settings.devices.json / settings.people.json; no filters/grouping |
| "+ Person" | missing | person resource `create: None`, crates/fleet-core/src/pages/resources.rs:1004 |
| Device kind/app version ("phone · fleet-mobile 0.5.4") | missing | device fields name/mode/this_device/trusted/org/person/catalogs/last_seen/created only (resources.rs:922-945) |
Done: 9 items match

## Org spend (OrgSpend.dc.html)
Done: 11 items match

## Sharing and presence (Sharing.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Org "Sharing" list of every share (Session · owner, Shared with, Level, Since, Watch, Revoke) with Level/Owner filters | missing | no match for an org shares list in src/lib or crates/fleet-core/src/pages |
| Admin narrow-to-Read from that list | partial | narrowShare exists per session (src/lib/sessions.ts:803), not from an org list |
| "Team · who is working on what" panel (live sessions, states, watching) | partial | only per-member session word (OrgMembers.svelte memberSessionsWord) and per-session PresenceStrip (src/lib/PresenceStrip.svelte) |
| "reads only until his iPhone is trusted · Trust now" on a share | missing | no match for "Trust now" |
Done: 4 items match

## Federation (Federation.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Hub graph (this hub + peers, green up / dashed red down) | in PR #779 | src/lib/pages/ResourceGraph.svelte absent from cf-main; graph spec in settings.federation.json |
| "retrying every 30 s" | partial | state + last_error only (resources.rs:1103,1133) |
Done: 6 items match

## Debug devices (DebugDevices.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Scan all hosts" | partial | only per-host "Rescan host" (crates/fleet-core/src/pages/resources.rs:1077) |
| Greyed "live screen and input / device on another host" placeholders | missing | no match in debug_devices.json or resources.rs |
| Tracker cards "N columns mapped · Column map" | partial | not verified as a card; trackers page has write-back/token-expired (settings.trackers.json:70,130) |
Done: 9 items match

---

# 7. Phone


Paths are relative to fm-main `shared/src/commonMain/kotlin/dev/claudefleet/mobile/` unless they name a repo. Many gaps in parity-audit-14.13.md are now closed on fm-main: agent tab name, HubBanner on every tab, ConversationLoading, Control chat, Atom, Galaxy, Radar add-host, Done notifications, fingerprint lock, iOS status bar, lesson prompts, A folder already on the host, spend ask, background-agent options, quota meters. They are not listed again below.

## Mobile · Inbox and navigation (MobileNav.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Failed Inbox row with inline "Open log" / "Retry" | missing | `ui/PhoneSessions.kt:215-252` PhoneSessionRow takes no row actions. No match for "Open log" |
| Paused Inbox row with inline "Switch account" / "Wait to Fri 11:00" | partial | The row text says when the limit resets (`ui/PhoneSessions.kt:136-148`). The two buttons are only inside the session (`ui/SessionLater.kt:93-121`) |
| Mission ask as an Inbox row ("sign the autonomy grant" · Mission chip) | missing | `App.kt:1595-1625` feeds the Inbox session rows, shared rows and proposed changes only. No mission rows |
| Jev-proposed Inbox row ("Proposed by Jev · turn ended with a question · Not waiting") | missing | No match for "Proposed by" or "Not waiting" |
| More › Automation live line "3 active · $4.10 today" with an inline Pause all | partial | `App.kt:1722` uses the fixed line "Missions, and Pause all". Pause all is one level down |
| New layout is the default | missing | `ui/PhoneLayoutPref.kt:8-10` still defaults to Classic |
Done: 22 items match

## Mobile · One session (MobileSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Voice input in the composer | missing | No match for voice/speech/mic under `ui/` |
| "Thinking · reading X" Atom in the conversation | missing | `Atom(` is used only at `ui/OrbitChatForm.kt:246`. No match for "Thinking" |
| Details: Account row with usage meter (5h % left · week %) | missing | `ui/SessionDetailsSheet.kt:171-187` facts are Host, Branch, Started, Last activity, Model, Context, PR, CI, Tags. There is no account or usage |
| Details: PR checks "15/15 checks" | partial | Only `Fact("CI", ciStatus)` (`ui/SessionDetailsSheet.kt:181`). No check count |
| Details actions: Fork, Switch account | missing | `App.kt:2694-2703` offers Move, Share, Tasks, Ticket, Review, Archive and Force kill. Fork exists only per turn (`ui/ReplyActions.kt:88`) |
Done: 18 items match

## Mobile · Control, notifications and chat forms (MobileControl.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Failed notification with a "Retry" action | partial | `notify/NeedsYouContent.kt:42-44` gives only Open/Answer and Later |
| Mission notification ("Review grant") | missing | No match for mission/grant under `notify/` |
| Jev proposal inside a chat-form step ("Proposed by Jev · same repo · Change") | missing | No proposal field in `model/ChatForms.kt` or in hub `crates/fleet-core/src/pages/forms.rs` (no match for propos/suggest) |
| "Proposed by Jev" host pick in Control's plan | missing | No match for "Proposed by" |
| Sign the autonomy grant on the phone ("Review and sign…") | partial | `ui/OrbitMissionDetail.kt:55` shows the grant as a card, but it is signed on the desktop. The phone offers only Not now |
| Atom "Watching CI on #476" status in the chat | partial | Handoff chips exist (`ui/ControlChat.kt:510-526`). There is no watch line with Atom |
Done: 14 items match

## Mobile · New session (MobileNewSession.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Jev host proposal on Step 1 | missing | No match for "Proposed by". The hub's new_session has no proposal |
| Start from "A branch" | missing | `ui/NewSessionWizard.kt:337-338` offers only "A project" and "A ticket" |
| Drafted branch name ("Drafted from FLEET-151 · Regenerate · Clear") | missing | No match for "Drafted" in `ui/NewSessionWizard.kt` |
| Review: Account row "m.janci · 75% left" | missing | ReviewRows at `ui/NewSessionWizard.kt:528-543` are Host, Project, Worktree and Ticket |
| Optional first message | missing | No match for "first message". Hub `NewSessionArgs` (`crates/fleet-core/src/mcp/tools/params.rs:69-105`) has no prompt field |
| Pulse ticks off the real start steps | partial; hub side in PR #779 | The phone uses fixed `startSteps()` (`ui/NewSessionWizard.kt:308`). The hub's `start:progress` frames exist only in cf-779 (`crates/fleet-core/src/service/sessions/start_progress.rs`; absent in cf-main). The phone does not consume them (no match for start_progress / start_token) |
Done: 12 items match

## Mobile · Work (MobileWork.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Task detail: Acceptance criteria as a list | partial | `ui/PhoneWork.kt:646` renders only the description markdown. No match for criteria/acceptance |
| Comet while summarising | partial | It uses DotWave (`ui/PhoneWork.kt:834-836`). This is the wrong loader, not a missing state |
Done: 20 items match

## Mobile · Search, bulk actions and Today (MobileSessionsTools.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Project hit by file content ("matches HostsScreen.kt") | missing | `ui/SearchEverywhere.kt:27-31` matches only the project label or owner/repo |
| Today opened from a Control answer ("what did I ship today?") | missing | `App.kt:1608` "Today is an Inbox view until Control grows its own". Today opens from Inbox and ⋮ only |
Done: 16 items match

## Mobile · Hosts, accounts and files (MobileMore.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Host recovery "Run plan when back" (deferred until the host answers) | missing | `ui/HostDetailSheet.kt:93-175` has only Check again and Restore now. No match for "when back" |
| Per-host verdict chip (Resume / Recreate / Skip) with the reason | partial | The restore plan is listed with errors after the run (`ui/HostDetailSheet.kt:103-127`). There is no per-session verdict before it runs |
| Ping latency on host rows ("ping 42 ms") | missing | `ui/MorePlacesScreen.kt:218-224` shows only the age of the last ping. No ms |
Done: 20 items match

## Mobile · Offline, reconnecting and loading (MobileStates.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Reconnecting full panel ("Back on the network" · "Show the last known list") on the New layout | partial | `ReconnectingPanel` (`ui/kit/PhoneStates.kt:211`) is used only by Classic `ui/SessionsScreen.kt:849`. New shows the HubBanner only |
Done: 9 items match

## Mobile · Errors and recovery (MobileRecovery.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Move options state the toolchain facts ("Android SDK not checked") | missing | `ui/MoveSheet.kt` shows load and the offline reason only. No match for SDK/toolchain |
Done: 12 items match

## Mobile · Tidy and tickets (MobileTidyTickets.dc.html)
Done: 16 items match

## Mobile · Missions and background agents (MobileMissions.dc.html)
| Item | Status | Evidence |
|---|---|---|
| "Waits on you" section first in the Missions list | missing | `ui/OrbitMissionsScreen.kt:181-184` has the groups Working, Paused, Drafts and Done this week. The header comment at `:50` defers "Waits on you" |
| Row shows autonomy level ("autonomy: ask before push") and spend meter | partial | The spend appears as text "$x of $y" (`ui/OrbitMissionsScreen.kt:112-121`). No meter and no autonomy |
| Background agent as a screen with an Agent choice (Claude Code / Codex) | partial | It is an AlertDialog (`ui/NewSessionScreen.kt:455-520`). The agent is fixed to Claude because the hub refuses Codex (`model/Recovery.kt:74-77`) |
Done: 13 items match

## Mobile · A session's Files tab (MobileSessionFiles.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Per-file +/− line counts on Changes and Commit | missing | `model/Repo.kt:8-14` ChangedFile has no counts. Hub `crates/fleet-core/src/service/repo_read.rs:46-55` has none either |
| "N behind main" | partial | `model/Repo.kt:86-91` gives only "ahead of" |
| Commit: pushed / not pushed, ticket refs as links, Open on GitHub, Ask to push | missing | CommitPane (`ui/RepoScreen.kt:643-653`) shows the subject, author, Copy hash and body only |
| File view: Share and search | partial | Only "Send to Downloads" (`ui/RepoScreen.kt:674`). No match for Share in RepoScreen |
| "Ask Claude Code to commit" named after the session's agent | partial | `ui/RepoScreen.kt:245` uses DEFAULT_AGENT_NAME, not agentName(row) |
Done: 12 items match

## Mobile · Terminals, send later, find and the session menu (MobileSessionExtras.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Send later time choices (In 1 hour, Tomorrow 09:00, When the weekly limit resets, Pick a time…) | missing | `ui/SessionLater.kt:148-155` offers "when idle" only. The hub holds nothing until a time |
| Send later opened from a clock in the composer | partial | It is reached from ⋮ "Send later…" (`ui/SessionMenu.kt:79`) |
| Terminal key bar: Ctrl (and ←/→, Alt in landscape) | partial | `ui/Terminals.kt:102-117` "There is no free Ctrl key". Esc, Tab, ↑↓, ⌃C, | and ~ only |
Done: 15 items match

## Mobile · Organisations and hub settings (MobileOrgsSettings.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Automation playbooks as toggles with recent activity ("Ran 3 times this week") | partial | The playbook settings render as generic hub fields. No per-playbook activity (no match for "times this week") |
| No Save buttons; History in ⋮ | partial | Typed fields still have a per-field Save (`ui/FleetSettingsScreen.kt:374-393`) and a per-field History (`:92`) |
| Decisions (Jev): key set date, model, "This week N proposals · kept · changed" | missing | `DecisionsHead` (`ui/OrbitOrgsScreen.kt:319-332`) shows only opted-in / not and What Jev may do |
Done: 12 items match

## Mobile · Full-screen loaders (MobileFullscreenLoaders.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Hex field for Repair / Recreate with a step checklist | missing | `FullscreenWait.Repair` is defined (`ui/kit/FullscreenLoader.kt:46`) but nothing uses it. The hub answers a repair in one call |
| Radar: network scan and "Enter an address by hand" | partial | `ui/AddHostScreen.kt:60-77` reads only the hub's ~/.ssh/config. No manual address |
Done: 6 items match

## Mobile · Light theme (MobileLight.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Screen-by-screen light check against MobileLight | partial | The theme switch exists (`ui/PhoneSettings.kt`). No recorded check (audit §13) |
Done: 5 items match

## Mobile · Updating the app (MobileUpdate.dc.html)
Done: 16 items match

## Mobile · First launch and installing on a host (MobileInstall.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Assemble splash on first launch | missing | `App.kt:693-697` Splash is plain "Orbit Fleet" text. No Assemble motion in `MarkMotion` (`ui/kit/Loaders.kt:47-61`) |
| Install agent from Add a host ("tmux is missing" → Install agent) | partial | The install review is reachable from a Hosts row (`App.kt:1534`). AddHostScreen offers only Add (`ui/AddHostScreen.kt:50-55`). The hub allows the owner's phone since contract 13 (cf `crates/fleet-core/src/mcp/guard.rs:312-322`) |
| Install installs tmux | missing | `ui/FirstInstall.kt:428-430` says tmux "is not part of the agent" |
| "Show a one-line install command" | missing | No match for "one-line" or curl in `ui/FirstInstall.kt` |
Done: 9 items match

## Mobile · Wizards (MobileWizards.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Where step: host facts (free disk, toolchain such as JDK) | missing | No match for GB/disk/toolchain in `ui/AddProjectWizard.kt` |
| Organisation from the repository owner, with Change | missing | No org field in `ui/AddProjectWizard.kt` |
| "A folder already on the host" for any host | partial | Only "a checkout on the hub's own machine" (`ui/AddProjectWizard.kt:84`, `:373`) |
| Clone progress with real counts (38 of 59 MB · objects) | partial | The step list comes from `addSteps()` (`ui/AddProjectWizard.kt:142-146`). The hub answers once, so there are no byte counts |
| A wizard started on one device resumes on the other | missing | No match for resume/draft hand-off in the wizards |
Done: 11 items match

## Mobile · Full-screen and landscape (MobileFullscreen.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Portrait full screen via ⤢ for the conversation (hide strip and bar, keep composer) | partial | The ⤢ exists only for the agent TUI (`ui/SessionTabs.kt:178`). The conversation folds its chrome on a double tap (`ui/SessionScreen.kt:387`, `ui/SessionChrome.kt:46`) |
| Split terminals key bar with Alt and ←/→ | partial | Landscape has arrows and Ctrl only as far as `send_prompt` keys allow (`ui/Landscape.kt:139-190`). No Alt |
Done: 8 items match

## Mobile · Tutorials (MobileTutorials.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Practice notifications marked "Practice" | partial | The practice banner exists (`ui/help/Practice.kt:70-75`). No practice notification path under `notify/` |
Done: 12 items match

## Mobile · Tutorial modes, lessons and guides (MobileTutorialModes.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Practice fleet "shown as its own fleet in the account switcher" | missing | There is no account or fleet switcher. Practice opens from Learn and Help only (`ui/help/HelpScreens.kt:288`) |
| Tips count ("7 seen · 12 left") | missing | `ui/help/HelpScreens.kt:283` has "Show tips again" without counts |
| Guide step with real usage meters | partial | The guide steps and Undo exist (`ui/help/HelpScreens.kt:601-661`). No meter (no match for meter) |
Done: 12 items match

## Mobile · More, settings and pairing (MobileSettings.dc.html)
| Item | Status | Evidence |
|---|---|---|
| Pairing "Draw-on" mark and "Halo" on success | missing | No DrawOn or Halo in `MarkMotion` (`ui/kit/Loaders.kt:47-61`) |
| Quiet hours set on This phone | partial | The phone shows the hub's `notify.quiet_hours` (`ui/OrbitSettingsScreen.kt:249-256`). It is set on the hub page, not per phone |
Done: 18 items match

