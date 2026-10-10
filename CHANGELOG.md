# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Releases are cut with `scripts/release.sh` — see [docs/RELEASING.md](docs/RELEASING.md).
Entries before 0.2.4 were plain version bumps and were not recorded individually.

## [0.6.2-dev.58.desktop.g7a0e164] - 2026-10-10

### Added
- **forms:** destructive confirm with typed name and a safer way out (G1.4)
- **sessions:** All/Mine/Shared tabs, agent facet, group by organisation, running cap, shared-by rows
- **missions:** the Missions board's list and detail (gap G3.6)
- **tidy:** say how much space a tidy frees, from per-worktree sizes (M15 G1.9)
- **work:** tasks carry a due date beside their assignees (M15 G1.7)
- **sessions:** one session action registry with Fork, Rewind, Switch account, Change model, Copy transcript, Archive
- **settings:** scope pill, changed-from line and batched Save on settings rows
- **forms:** form kit behaviour from the FormsAnatomy board
- **ui:** draw a work handover as a card in the Conversation view
- **redesign:** 5.11 J8 warns in the agent tab when the rules cannot read the screen
- **redesign:** 6.8 J6/J7 in Review and bench sets for every closed-choice Jev use case
- **redesign:** 11.1/11.8/11.12 org overview tabs, Federation graph, Link two hubs wizard
- **redesign:** 10.12 wizards in chat, 9.12 trails for a long search, 10.10 transfer loaders

### Changed
- **pages:** regenerate the page registry after the main merge
- **mcp:** raise BYTES_PER_TOOL to 815 from the merged surface
- **backend:** start:progress crosses the bridge with its token; install_agent and agent_installs route under a case

### Fixed
- **ci:** main green again — the handover card's status word, a feature-gated helper
- **ui:** a blocked handover reads Needs you, not a seventh status word
- **ui:** a blocked handover reads Needs you, not a seventh status word
- bugs from a sweep of the last two days' PRs
- **mcp:** trim this branch's tool and parameter docs back under the 800-byte budget
- **redesign:** 8.1, 4.4, 4.12, 8.7, 12.3, 2.6, 11.5, 6.9 backend audit gaps
- **redesign:** 14.14 send_prompt presses the arrows, BTab and a closed list of Ctrl keys
- **redesign:** 5.13 start steps, 5.3 terminals, 0.1 registry keys, 3.6/1.4/4.5/3.2/3.17 tests, 1.5 hierarchy
- **redesign:** 3.14, 3.15, 0.6 and layout tokens from the audit
- **redesign:** 4.9 install fleet-agent from the desktop; 6.3 Blocked and $ per task; 4.1/4.2 spend per account; 6.8 J1 in the Details timeline
- **redesign:** 3.9 push and open in VS Code in the palette; 3.11 the draft's Atom
- **redesign:** 7.8, 0.9 design-manual conformance: six words, one icon set, the kit

### Documentation
- **redesign:** snapshot the 9 Forms boards and the canvas gap audit
## [0.6.1] - 2026-10-09

### Added
- **updates:** the desktop downloads from the hub's mirror; org admins set their org's policy
- **updates:** update now — an operator's install at once
- **work:** Work tasks list and filters panel as on the board
- **ui:** Overview in Control's header opens the Today briefing
- **ui:** Automation page matches the Automation board
- **ui:** Settings rows read like the Settings board
- **ui:** session list rows and header match the Sessions board
- **updates:** a dev track from every green main push
- **ui:** move the prompt-chip editor to Toolkit › Prompts & snippets
- **ui:** Automation and Toolkit get their own left column
- **ui:** Settings is a rail destination page, not a modal
- **updates:** the rest of S9 — org policy, artifact mirror, binary target
- **ui:** session header per the Main board (named buttons, context meter, meta line)
- **ui:** inspector and Details tab per the Main and SessionDetails boards
- **updates:** staged rollouts and the maintenance window (S9, part)
- **release:** sign and list the phone's amendment (S8, fleet side)
- **updates:** push update:decision, hub-e2e section U, signed amendments
- **desktop:** the desktop updates itself (S7)
- **release:** the nightly update track (S2b)
- **updater:** fleet-updater, the Docker hub's update sidecar (S6)

### Changed
- **windows:** leave fleet-updater out of the Windows clippy leg too
- **windows:** leave out fleet-updater, Unix-only like fleet-agent and fleet-hub
- **hub-e2e:** run the fake-tracker refusal check without timeout(1)

### Fixed
- **work:** key the planner slot by store too, so parallel tests stop refusing each other's planner
- **work:** stages filter uses contains (clippy manual_contains)
- **ui:** Automation page uses type and radius tokens, tab keys and status words
- **llm:** say a Claude login expired instead of "not a JSON array"
- **ui:** Control keeps a Needs-you session in its Views panel; header per the board
- **ui:** the shell follows the destination, not the 0.5 session column
- **desktop:** colour the update banner through theme tokens

### Documentation
- **updates:** status of the dev track, update now, the mirror and org admins' policy
## [0.6.0] - 2026-10-09

The Orbit Fleet layout replaces the 0.5.x layout: the rail, the left list
with filters and grouping, and the inspector. The Classic layout and the
Layout switch in Settings → Appearance are gone.

### Changed
- **redesign:** the Classic layout and the `ui.layout` pref are removed; a stored layout choice is cleared at start
- **hub:** the hub contract is revision 14 (it was 10): upgrade the hub together with the desktop, since a revision-10 hub does not serve the new tools (`wire_contract.rs`)
- **share:** a grant's level gains Answer, between Watch and Drive; a client that reads the level as a closed enum must accept `answer`
- **control:** on a hub the operator's starts and kills wait for the owner's paired device to confirm them instead of being refused

### Added
- **redesign:** the Orbit Fleet layout, now the only one: the rail, the inspector, Inbox, Get started and a first-run tour
- **control:** Control with Chat and Today tabs, the Views panel, confirmation cards, handoff receipts, tasks from the agent's plan and the Library with Upload
- **automation:** routines on a schedule, an event or Run now; one list of runs; start rules that send a task key to a project; the Automation screen
- **agents:** sessions can run the Codex CLI, offered where the host has it
- **jev:** use cases K2–K5, J1 live, J2, J5, J9, N1, N3–N6 and J10, each off by default
- **pr-shepherd:** nudge a session to fix its conflicting or red PR, a merge queue at the `merge` level, and the owner's `pr_shepherd` control-API tool
- **sessions:** shell terminals and a pop-out terminal, Files with Go to file, queued prompts for a busy session, Lost and found proposals
- **wizards:** New session, Add host, Pair a device, Add project and Link to a hub as forms, in a dialog or in the chat
- **notifications:** a per-state matrix for desktop and phone, and quiet hours
- **orgs:** spend by person; federation links managed from the desktop

### Added
- **watch:** review backlog: a summary capped at the last 40 turns says so
- **redesign:** hub contract 14
- **missions:** the list carries spend and budget, the plan a run estimate
- **bg:** new_bg_session takes a project, read-only and stop-after limits
- **redesign:** check_account_headroom hub tool, desktop routes to it
- **redesign:** 6.9 Tidy › Duplicates: Jev's same-work pair suggests the idler
- **pr-shepherd:** Jev pr_triage in shadow (step 4)
- **pr-shepherd:** owner's pr_shepherd tool on the control API
- **redesign:** 8.4 Routines and Rules tabs in Automation
- **redesign:** 9.10 K3 mission triage: Jev proposes a stuck mission's outcome and next step
- **redesign:** 8.10 Jev reads what a routine run came to
- **redesign:** contract 13 also records session_summary_since
- **redesign:** contract 13 and the owner's-phone admin grant
- **redesign:** 11.11 Since 13:20 summary in Watch and Details
- **redesign:** 6.9 N1 related_session: Jev proposes another session doing the same work
- **redesign:** 11.11 session_summary_since desktop command
- **control:** Jev K2 routes each Control message (redesign 9.9)
- **redesign:** 10.10 Galaxy while Get started builds the first fleet
- **redesign:** 11.11 session_summary_since hub tool
- **redesign:** 11.11 watcher summary service and J9 summary check
- **redesign:** Codex Conversation tab, UI half: name the row's agent
- **redesign:** 10.5 Get started's first session is the New session wizard
- **redesign:** 10.5 first-run tour and Get started
- **redesign:** 6.9 K5 work_placement: Jev proposes a group for a task nobody placed
- **redesign:** 10.12 Get started's first session on the New session wizard
- **redesign:** 12.2 follow-up, Codex conversations in the Conversation tab (#669)
- **redesign:** fold Runs and Control handoffs into contract 12; fix migration 141's stamp
- **redesign:** 8.4 the Automation screen and its rail item
- **redesign:** 9.13 comet onto the "Sent to a session" chip
- **redesign:** 8.11 the start popover offers the rule; Automation's Rules list
- **redesign:** 4.12 Lost and found with proposals (#666)
- **redesign:** 10.10 a long copy's toast opens Downloads
- **redesign:** 8.6 Routines UI, Morning PR sweep, failed runs in the Inbox
- **redesign:** 6.10 Draft brief from a ticket
- **redesign:** 10.12 the New session wizard as a form
- **redesign:** 10.10 transfer loaders in Downloads and long-copy toasts
- **control:** the Library view in Control's panel (redesign 9.7)
- **redesign:** 12.4 agent picker offers Codex where the host has it
- **library:** library_items index and the library tool (redesign 9.7)
- **redesign:** 12.3 follow-up: new sessions may run agy
- **redesign:** 5.11 J2 turn outcome from the pane tail, hooks always win
- **redesign:** 9.11 LLM drafts in Control: release note and morning brief
- **redesign:** 5.3 Terminals tab and 5.14 Pulse on the new row
- **redesign:** hub contract 12, the tools routed since 11
- **redesign:** 11.7a share level Answer between watch and drive
- **redesign:** 9.13 Control chat Atom indicator
- **redesign:** 7.2 session lists as trees, rows keep their own controls
- **redesign:** 10.12 Add host and Pair a device wizards as forms
- **redesign:** 9.12 Comet trails for running missions in Control
- **redesign:** 6.11 follow-ups: clone on several hosts, Organisation and Tracker
- **redesign:** 10.12 the Add project wizard in the chat
- **redesign:** 11.9b Updates page lists every part of the fleet
- **pr-shepherd:** merge queue at the merge level
- **redesign:** 5.12 LLM drafts in the workspace
- **redesign:** 9.3 handoff receipts and 9.6 tasks in Control
- **redesign:** 12.2 Codex adapter (#632)
- **redesign:** 5.13 ⌘N shows the Pulse while the create runs
- **redesign:** mission ⋯ menu for Complete, Mark failed and Cancel (parity P19)
- **redesign:** 5.4 pop out a terminal into its own window
- **redesign:** 10.12 wizards as forms, in a dialog or in the chat
- **redesign:** 11.7b presence and 11.7c private sessions as counts
- **redesign:** 8.7 routines name their account; bg sessions warn over the line
- **redesign:** 11.9 notifications matrix and quiet hours
- **control:** open a session inside the Views panel (redesign 9.5)
- **redesign:** 4.13 host loaders in the add-host wizard and host checks
- **control:** Views panel beside Control's chat (redesign 9.4)
- **redesign:** 5.13 session loaders, on events not on time
- **redesign:** 8.11 start rules: a rule picks a task's project before history and Jev
- **redesign:** 9.12 Comet trails beside a running mission's current steps
- **redesign:** 6.11 Add project with four sources
- **redesign:** 12.3 Agy adapter, provisional until real agy screens are captured
- **redesign:** 6.9 K4 duplicate: Jev flags a proposed task that may repeat an existing one
- **redesign:** 5.6 Go to file on the Files tab, and the viewer's path actions
- **control:** confirms as transcript cards, answered on a hub (redesign 9.2)
- **redesign:** account_usage hub tool, list_account_usage routed
- **redesign:** 8.10 routine run outcome, without Jev
- **redesign:** 11.8 spend by person, all or nothing
- **redesign:** 5.3 shell terminals
- **redesign:** 3.17 the status bar ends on Shortcuts
- **redesign:** 5.6 Changed shows what is not pushed and the branch against its base
- **redesign:** 11.12 federation loaders
- **redesign:** 12.4 store which agents each host has on PATH
- **redesign:** 5.8 Share from the session header; Shared with me group
- **redesign:** 8.7 account-aware automation
- **redesign:** 4.10 account_uuid in the phone view
- **redesign:** 3.12 the New session dialog pre-ticks Jev's sibling repository
- **redesign:** 3.17 tray and menu-bar icon states
- **redesign:** 10.9 Jev quick answer: the likely option first
- **redesign:** 5.14 loaders in the session chat: running tool call and voice input
- **redesign:** 12.1 agent adapter trait, Claude Code behind it
- **redesign:** 6.2 Work filters for a person and a column; group by org, person, mission, account or repo
- **redesign:** 4.2 cost per account and per model
- **redesign:** 8.5 routines backend
- **redesign:** 3.17 the header and brand
- **redesign:** 6.8 Jev's suggested link with its reason
- **redesign:** 4.11 Jev N5 host placement in New session
- **redesign:** 5.1 Agent tab named and marked from the session's agent
- **redesign:** 7.4 motion catalog: rows slide between groups, one wash, toasts with a timer bar
- **redesign:** 5.9 one approval card, move to next with Undo, New divider, project skills in /
- **redesign:** 2.6 hub contract 11 and the three Blocked reasons on the wire
- **redesign:** 3.15 startup with loaders
- **redesign:** 11.2 members: shares since, remove dialog, pairing code
- **pr-shepherd:** nudge a session to fix its conflicting or red PR
- **redesign:** 3.12 N3 sibling_repos: Jev proposes the sibling repository a ticket start also needs
- **redesign:** 9.1 Control rail item with the agent and Today
- **redesign:** 2.8 proposals on the wire and sessions.turn_outcome
- **redesign:** 7.2 24 px targets
- **redesign:** 4.5 New session agent and account pickers
- **redesign:** 6.1 board columns from tracker status names
- **redesign:** 5.5 Open in VS Code
- **redesign:** 6.4 pull requests
- **redesign:** 4.4 Limit handling
- **redesign:** 7.8 copy pass
- **redesign:** 3.16 Toolkit rail item with Skills and Assets tabs
- **redesign:** 3.14 connection states with loaders
- **redesign:** 8.2 book claude -p runs as cost with their origin
- **redesign:** 11.5 Federation page, link and unlink a hub
- **redesign:** 3.7 one Filters section and Group control
- **redesign:** 7.2 the 11 px type floor
- **redesign:** 10.1 form receipt, options answered with 1–9, expiry shown
- **redesign:** 7.2 accessibility pass, axe in component tests
- **redesign:** 3.5 session tabs and inspector
- **redesign:** 7.3 light mode: every component colours through the theme tokens
- **redesign:** 3.12 the proposed project carries one chip wherever a session starts
- **redesign:** 3.13 loaders in the new shell
- **redesign:** 0.9 design-system kit in Svelte
- **redesign:** 2.4 Blocked reasons and Paused · limit
- **redesign:** 11.4 Control API tokens table
- **redesign:** 3.3 Inbox: what needs you, Today as its tab, All sessions
- **redesign:** 6.6 one Continue / Start new split button wherever a task starts
- **redesign:** 5.10 dialogs in one pattern, prompts that wait for idle
- **redesign:** 4.9 add-host wizard with live checks and a fleet-agent install job
- **redesign:** 2.3 last viewed and done_unread
- **redesign:** 0.8 Comet on a filled primary button takes its text colour
- **redesign:** 0.8 loader kit
- **redesign:** 3.11 ProposedBy, DraftField and the never-decides list
- **redesign:** 3.9 ⌘K palette: > # @ prefixes, session commands, settings in plain words
- **redesign:** 11.1 org overview tiles, 14-day spend and Needs an admin
- **redesign:** 3.8 keyboard: list keys, next needs you, ⌘1–9, 1–9 answers, ? sheet
- **work:** plan missions and Import plan for the task graph
- **redesign:** 4.3 Account pills
- **redesign:** 8.1 every loop reports, and Pause all
- **redesign:** 3.2 Rail
- **redesign:** 2.2 session origin
- **redesign:** 1.5 action hierarchy in session details
- **work:** task graph view for a mission
- **redesign:** 1.2 one attention line; no native white buttons
- **redesign:** 7.7 Decisions in the Settings tree
- **redesign:** 1.1 honest badge; a mass loss folds into one row
- **redesign:** 10.6 states kit
- **redesign:** 3.6 rows: density, two-line Compact row, group by state, host or agent
- **redesign:** 4.7 host detail health checklist
- **redesign:** 0.6 motion tokens and the Motion pref
- **redesign:** 6.3 blocked tasks and cost per task
- **redesign:** 10.4 guides open as a card in chat
- **redesign:** 4.1 Accounts page
- **redesign:** 5.2 PTY map
- **redesign:** 10.3 settings-change card with Apply
- **redesign:** 7.1 one Settings tree
- **redesign:** 11.6 claim, install, logs and screenshot on Debug devices
- **redesign:** 3.10 row ⋯ menu, Board as a Work view, Review as a count
- **redesign:** 1.9 Ctrl+, opens Settings; one New session dialog
- **redesign:** 0.7 controls: tabs, segmented control, badge and chips
- **redesign:** 4.6 Hosts page split from accounts
- **redesign:** 2.5 persist account usage snapshots
- **redesign:** 3.4 one selection store for sessions and tasks
- **redesign:** 1.4 remove permanent rows
- **redesign:** 0.4 one attention model, seven states
- **redesign:** 10.7 notification centre and Downloads with progress, Show in Finder, Retry
- **redesign:** 4.8 adopt a tmux pane started outside fleet
- **redesign:** 3.1 destination store replaces the overlay flags
- **redesign:** 10.2 progress, results and error chat kinds
- **redesign:** 2.1 agent kind on sessions
- **redesign:** 1.7 Kill lists uncommitted files and offers Clean up
- **redesign:** 11.3 rename a device and change its mode
- **redesign:** 5.7 blame and merged branches in Files
- **redesign:** 1.8 mission autonomy in words, grant hosts, parallel and wake edits
- **redesign:** 0.1 shortcut registry and freeze test
- **redesign:** 6.5 review confidence
- **redesign:** 0.5 light syn-num from design system v4
- **redesign:** 1.3 planner errors in words, with Retry and Details
- **redesign:** 0.5 design tokens from the Orbit Fleet manual
- **redesign:** 0.3 layout pref and Appearance settings
- **redesign:** 1.6 one task status for List and Board

### Changed
- **decide:** review backlog: build the stranger's scope through Caller::view_scope
- **catalog:** review backlog: admin calls, apply follow-through and undo run off the async workers
- review r15: the Jev mark on a row is attribution, not a status (fixes copy_lint)
- **pty:** review backlog: rustfmt the generation test
- **watch:** review backlog: the Since summary survives the same session re-rendered
- review r15: finish the deferred findings (F08, F14, F15, F17, F22)
- review r16: seven off-screen views load in their own chunks (#771)
- review r16: tracker refresh pauses while the window is hidden (#770)
- review r11: arrow keys on the main view tabs (#766)
- review r20-sweep: the sidebar's purge and background-session buttons draw kit icons
- review r20: Move to headroom reads the account a session bills
- review r04: refresh the scope guard rows for F3's owner half (#763)
- review r15: fit the merged brief_target test and the tool-surface budget
- review r16: sidebar reads row places only under Full motion (#758)
- review r20: backlog sweep r04/r13/r17/r18
- review r16: conversation panel stops deep-proxying its transcript (#753)
- review r20: backlog sweep r05/r06/r15 (backend)
- review r20-sweep: backend items from the r01 backlog
- review r20-sweep: the roll-a-name button draws the kit's dice icon
- review r04: K2 K3 live streams re-check their caller (#746)
- review r20: backlog sweep r05/r06/r15 (frontend)
- review r20: New session keeps a profile typed before the logins arrive
- **ai:** the morning brief's refusal line uses the type token
- review r20: a failed worker's summary reaches Jev only with reply consent
- review r04: F3 a reaped session's files stay the owner's (#742)
- review r18: local sync from Windows keeps the host's executable bit (#739)
- review r20-sweep: migration 147 indexes the live org grants
- **scope_guard:** name library::visible_with, the function #721 moved the org-only arm into
- review r18: AltGr text never fires a chord on Windows; wizard re-checks a new host (#732)
- review r04: F1 F2 org admins never take over an existing person
- review r20-sweep: the manual's line icons replace emoji on desktop actions
- review r20-sweep: row events, timers and the agent grip
- **review r16:** equal Sets and facts no longer re-render every row (#727)
- review r17 flakes: tool tests never reconcile the real machine, budgets only under FLEET_SCALE_BUDGETS, hub-e2e waits for its unclaimed row
- review r18: move, agent install, SSH socket, VS Code on WSL, update order (#725)
- review r10: adopt app.css tokens into tokens.json, add scrims, theme the terminal menu
- **review r16:** hub probes and queries, desktop selection and timers (#721)
- review r04: S1 S2 S3 F4 F5 T1 T2 security fixes
- review r20: docs and in-app help match main
- review r02: routines on a removed project or host, unjudged-runs index, v0.5.4 upgrade test (#714)
- review r19: the notifications matrix reaches the desktop; status of built-but-off flags
- review r08: name the r08-1 and r08-2 proofs in the parity table
- review r08: a board card opens its task in New; Settings opens with the sidebar collapsed
- **a11y:** review r11: drop the caret and scroller rows main now carries
- review r10: type, radius, font and shadow tokens across the app (R10-01..07)
- review r14: desktop copy follows the manual's voice
- review r12: kit loaders only, one per screen, Motion setting respected
- review r08: Hosts reachable from Accounts & hosts; / and e on the Hosts table
- review r10: tokens in the serial files' styles (R10-01..04), r11 transcript focus ring
- **hub-e2e:** expect answer for a key press and hub confirms that wait (#695)
- **decide:** Feature::ALL is a slice, so a merge cannot miscount it
- **ui:** delete the Classic layout and the ui.layout switch (13.1)
- drop the merge's duplicate uiLayout import
- **redesign:** 9.11 reach and isolation-matrix rows for the draft actions
- **redesign:** pin the clock in Federation's Constellation test (11.12)
- regenerate the control-API reference (list_update_targets)
- wip runs
- **pages:** spend_by_person is a read-only org list
- **redesign:** 7.5 New-layout parity tests for P3, P8, P9, P11, P15, P21, P26, H1–H3, H7, H8
- Regenerate page docs after the merge (REGEN_PAGE_DOCS)
- Resolve the conflict markers left in tick.rs: keep the PR shepherd and deferred-prompt delivery
- renumber a branch's migrations to main's next free numbers
- stop every PR rewriting the same generated count lines
- Regenerate the hub verdict table after merging main into 10.6
- **redesign:** 1.1 move the fold tests above 3.6's block so main's 3.8 tests merge cleanly
- run the macOS and Windows legs on main, not on every pull request
- **repo:** gate the by_name helper with the Unix-only tests it serves
- **repo:** run the real-git repo tests on Unix only
- **redesign:** 2.1 keep each owner comment next to its field

### Fixed
- **tests:** green main on Windows
- **devices:** review backlog: run/install report the command's own exit status past the output cap
- **local-sync:** review backlog: ~\ is home on Windows; names Windows cannot hold stay remote
- **tmux:** review backlog: transcript discovery honours CLAUDE_CONFIG_DIR and dates files portably
- **agent-install:** review backlog: install tmux without a prompt or say why; restore SSH for a job a hub stop cut off mid-connect
- **ssh:** review backlog: uploads land through a temp file and mv; Open in VS Code refuses an agent-routed host
- **move:** review backlog: marker-anchored move reads, a landed kill is not both-alive, grants dropped only once the target is confirmed
- **hosts:** review backlog: an offline host says why and when it last answered
- **accounts:** review backlog: a failed launch read fails the start, a lapsed limit re-announces, an absent probe field keeps the stored value
- **store:** review backlog: NewHandoff's default kind is one its CHECK accepts
- **presence:** review backlog: viewers of an unclaimed session see each other
- **decide:** review backlog: control_route offers sessions past 8 missions and follows only a run the caller sees
- **routines:** review backlog: leases are stamped with a fresh clock
- **missions:** review backlog: a wake during a tick survives its release; one queued prompt per idle moment
- **orchestrate:** review backlog: one account_limit per held episode, one planner call per mission, unique card ids
- **forms:** review backlog: an agent's form starts with no risky default
- **desktop:** review backlog: no desktop command locks the store on the main thread
- **pty:** review backlog: pty_drain takes the open generation pty_open returned
- **accounts:** review backlog: LIMIT only on the raw figure everywhere
- **work:** review backlog: Work views show a skeleton after 400 ms, not loading text at once
- **sessions:** review backlog: a failed worktree scan says so plainly and offers Scan again
- **hub:** review backlog: point at Settings → Hub & sync by the page's name
- **forms:** review backlog: remove the unused one-step add-host wizard run
- **frontend:** back in New, the Inbox Classic stood in for comes back (review r07 F11)
- **app:** review r13 bootstrap failure, status bar copy and Retry
- **sidebar:** review r13 empty, failed and offline list states
- **control:** AI never decides — operator, cross-org drafts, briefs, completion (r15)
- **accounts:** review r05 A8 — the host line describes the picked login's account
- **accounts:** review r05 F9 re-announce sessions when their account's block moves
- **accounts:** review r05 F8 a profile not saved at start fails the start
- **states:** review r13 a loading conversation shows a skeleton after 400 ms
- **ai:** review r15: Jev never orders a risky question; no silent or late reorder (F10-F13, F23)
- **ai:** the "Since" summary and a Summarise result carry the Drafted pill (F16)
- **review:** r06 concurrency: double starts, stale kills, pause mid-pass, event order
- **frontend:** restore sessions.ts quoting so the share sweep reads it again
- **review:** an agent's settings proposal starts unticked (F09)
- **hub:** review r01 F13, one failed reconnect after a stream ends no longer arms the breaker
- **runs,prs,health:** review r02 backlog — newer aux kinds filter, PR states in SQL, one clock in the health parity test
- **accounts:** review r05 F2 collapse the profile guard (clippy)
- **decide:** review r15: Jev asks only with each org's consent (F01-F07)
- review round 1 follow-up — card cap, Runs kinds, limit window
- **accounts:** review r05 F7 a window with no reset time lapses
- **frontend:** projects, usage and downloads re-lists lose to newer frames (review r07 F10)
- **states:** review r13 error, empty and stale states outside the serial files
- **hub:** review r03 R3-4 keep work_rev in the phone's session view
- **accounts:** review r05 F5 the over-limit hold checks the start's host
- **review:** r06 D2/D3 list-vs-event races in sessions and hosts
- **accounts:** review r05 F4 the routine header names its flat account
- **frontend:** late answers no longer undo newer state (review r07)
- **hub:** review r03 R3-1 keep the owner in the phone's session view
- review round 1 — restore names, host checks, routine reads, Runs outcomes
- **a11y:** review r11: arrow keys on every hand-built tablist
- **accounts:** review r05 F3 no_credentials is not a signed-out login
- **frontend:** opening a session leaves the Automation page too (review r07 F1)
- **accounts:** review r05 F1+F2 relaunch re-reads the session's login
- **accounts:** review r05 A1-A3, A5-A7, A9 keep the desktop's account pill, limits and waits true
- **accounts:** review r05 A4 a host change drops the last host's login
- **work:** review round 1 — the mission loop settles reviews and resumes on time
- **a11y:** review r11: focus rings and focus return
- drop the duplicate uiLayout import in SessionDetails.svelte
- **library:** name the Library's org-only arm in ORG_HALF_SITES
- **redesign:** MorningBrief tolerates an empty today_brief answer
- **notify:** the phone's default notifications include Blocked (#682)
- **redesign:** copy lint sees the Routines dialog's own store
- **scripts:** renumber-migrations keeps a closing line both sides share
- **redesign:** HandoffCards uses the declared --bg-raise token
- **redesign:** a11y and copy re-sweep of the New screens since 7.2 and 7.8
- Feature::ALL holds 7 after the merge (duplicate + host_placement + quick_answer)
- **redesign:** 8.10 close the 137 entry the renumber merge dropped
- **scripts:** renumber-migrations stages only the files it edited
- **redesign:** 4.11 five decide features after the merge; regenerated docs
- **redesign:** 5.10 keep main's copy lint after the merge
- **redesign:** OrgMembers colours through tokens only
- **redesign:** 5.10 copy lint reads DialogSheet as a dialog frame
- **redesign:** OrgMembers colours through tokens (light-mode test)
- **redesign:** 4.11 shorter settings hint, regenerated reference; main carries pause_at
- **redesign:** 11.2 members table colours through theme tokens
- **redesign:** 7.3 debt: Sidebar colours through tokens now that 7.4 holds it
- **redesign:** 4.4 mirror accounts.pause_at in fleet_settings.ts
- **redesign:** 7.2 QuickSwitcher under the 11 px floor
- **redesign:** 1.1 fold caret reads --dur-fast, not a raw duration
- **frontend:** take main's node:fs shim verbatim
- **redesign:** port #546's org-guard classification for blocked_on
- **redesign:** classify 6.3's org guard in blocked_on
- **recreate:** re-clone a missing remote checkout instead of refusing
- **redesign:** port #546's classification of 6.3's org guard
- **redesign:** classify 6.3's org guard in blocked_on
- **frontend:** type existsSync in the node:fs shim
- **frontend:** type existsSync in the node:fs shim
- **test:** declare existsSync in the node:fs type shim
- **frontend:** match #535's node:fs shim text exactly
- **frontend:** type existsSync in the node:fs shim
- **frontend:** type existsSync in the node:fs shim

### Documentation
- drop the switch-back-to-Classic note now that Classic is gone
- **gen:** review backlog: generated headers name cargo fleet-test, regenerated
- **control-api:** name check_account_headroom in the tool index
- regenerate settings and page docs after merging main
- regenerate settings, page and API docs for 5.11
- regenerate the control API reference and hub verdicts for routines
- regenerate control API reference after merging main
- regenerate the control API reference for 9.11
- remove committed conflict markers from control-api.md (#645)
- regenerate the control API reference for draft_commit_message
- **redesign:** phone parity audit for 14.13
- **redesign:** 7.5 parity gap audit; P2, P3, P13, P16, P25 read Both
- **redesign:** parity row P30 for confirms as cards (9.2)
- regenerate control-api-reference (stale on main)
- list open_session_in_editor in the control API reference
- **hub:** regenerate the verdict table after merging main
- **redesign:** 0.2 parity checklist and PR template
- **ux:** add phone tutorials step 14.22 and its two boards to the redesign snapshot (#520)
- **ux:** add five more phone boards and steps 14.18-14.21
- **ux:** add eight more phone boards and steps 14.14-14.17 to the snapshot
- **ux:** add the phone app redesign to the Orbit Fleet snapshot
- **ux:** Orbit Fleet redesign plan, design system and canvas snapshot
## [0.5.4] - 2026-10-08

### Added
- Orbit Fleet name and app icon
- **ui:** status, type and spacing tokens; one status palette with the phone
- **conversation:** draw task reports and fleet-ui blocks as cards
- **devices:** debug devices — inventory per host, a control-API tool, and a page
- **work:** the worker guard and mission wakes from the tracker and CI

### Changed
- **devices:** pin only the stub's Android device in the stub-adb scan test
- **devices:** run the stub-adb and copy-script tests on Unix only

### Fixed
- **planner:** read the command array out of prose or a fenced answer

## [0.5.3] - 2026-10-08

### Added
- **work:** edit a task's title, description, status and assignees
- **forms:** the form card in the chat, the row chip, waiting triage
- **forms:** the form wizard and the chat card
- **forms:** the frontend form model and API, on the shared cases
- **work:** the mission loop — steps, planner, grants, integration (O4–O8)
- **forms:** desktop commands routed to the hub's ask; contract 9
- **forms:** the ask tool — an agent asks, a person answers
- **work:** a run's report, its git evidence and typed done_when (O3)
- **forms:** the forms service: wait, answer with host secrets, decline, tick
- **forms:** form_requests table and pending_form on the session row
- **jev:** K1 start_project, Jev pre-selects a task's first repository
- **work:** start progress strip and Cancel start (A3, P-5/P-6)
- **work:** a task board, To do / Doing / Done, cards drag to set status
- **work:** the mission graph — dependencies, READY / BLOCKED, waves (O2)
- **forms:** the fleet.form/1 spec, its validator and answer checker
- **operator:** the fleet-brainstorm skill, born with the operator (C0)
- **local-sync:** sync symlinks as links
- **accounts:** read each host's login profiles, attribute and poll their accounts, switch login in the desktop
- **work:** missions, the orchestration container (O1)
- **local-sync:** phases 2 and 3 — review, Ask AI, handoff, conflict tools
- **sessions:** run a session under a Claude login profile

### Changed
- **work:** run the real-git script tests on Unix only
- **forms:** cover conditions, hidden fields and spec limits in the shared corpus; redact secrets from Answers Debug
- drop graft's local .claude/ edits from this branch
- **token_store:** run the macOS keychain arm against the real keychain; record the hub live acceptance
- **hub-e2e:** redeem the federation probe peer from its own address
- **release:** ignore-list the nine tags behind the release-drift issue
- **hub-e2e:** walk the guide tool's person-side half against a real hub
- **add_project:** turn off auto maintenance in the bare-repo worktree fixture

### Fixed
- **core:** cap clipboard reads, private ssh socket dir, replace settings.json whole
- **net:** bound hub-link buffers and /events query lists; stalled downloads end; peer tokens checked
- **forms:** unknown-key problems in sorted order; closed card uses the friendly name; migration number
- **forms:** per-host sweep backoff, gone hosts swept, no secret written for a withdrawn form
- **forms:** ask values never reach the MCP audit row; cancel and list tests
- **core:** catalog symlinks and clone options, failed provisioning reads, bounded MCP inputs
- **mcp:** trim abandon_start's description to stay in the tool-surface budget
- **desktop:** keep blocking commands off the main thread; catalog resources need a user pick
- **forms:** a deleted session keeps a form with secrets for the sweep; docs and e2e fixes
- **forms:** a session switch is not a closed form; strict null guards in the card
- **forms:** a server problem moves the wizard to its step; per-card input ids; reset card state per form
- **work:** read a job's item id after the task columns, and the tool budget
- **forms:** a finish never lowers secrets_on_host
- **forms:** serialise answers per form, bound the secret sweep, mark secrets before writing them
- **answer-card:** a multi-select question can be answered from the card
- **mcp:** a client_msg_id dedupes per target, not per token
- **core:** bound four client-supplied MCP inputs
- **core:** private files are 0600 before the secret lands; downloads use a part file of their own
- **core:** read git renames and remote output past login banners
- **frontend:** linear-time markdown fences, table rules and md links; safe truncation and unit display
- **core:** parse probe, worktree-add and curl output robustly; never call a failed git status clean
- **core:** harden pairing, downloads, ssh_config, metadata guard, $HOME, TLS accept
- **tmux:** refuse '#' in new session names; branch .lock components
- **hub:** safe --since parsing, escape-free console output, refuse port 0
- **agent:** bound and neutralise peer text, refuse a relative --ca-file
- **update:** refuse expired channels, malformed offsets and re-run hold-back resets
- **local-sync:** sync on a desktop paired with a hub
- **store:** take the write lock up front in read-then-write transactions
- **provision:** refuse a git work tree only when it tracks fleet's skill dirs
- **hardening:** review follow-ups on #473
- **work-run:** run beside live work, refuse a job's mirror, show role in the approval
- **hardening:** second-review follow-ups
- **move:** carry git's reason when the target refuses the carried commits
- **local-sync:** never delete on an unreadable or emptied side, never cross a link
- **task-switch:** visible-primary compare-and-set, sticky acks, task listing gate
- **work-buckets:** keep a person's removal, fence adoption by org, survive org delete

### Documentation
- **hub:** regenerate the verdict summary on main
- **hub:** regenerate the verdict summary after merging main
- **forms:** spec status, trigger, answered_by and sweep backoff; secrets cap; REGEN_FORM_DOCS
- **forms:** the chat forms guide, the control skill, status; hub e2e scenario
- **jev:** record K1-K5, the use cases accepted on 2026-10-07
- **plan:** chat forms part 1 — implementation plan; spec aligned with the code
- **spec:** chat forms — an agent asks, a person fills a form, the answers come back
## [0.5.2] - 2026-10-07

**Upgrade note:** this release carries migrations 109–111 (local workspace
sync, work-item runs, host credential overrides). Back up `state.db` before
upgrading the desktop or the hub; an older build refuses a migrated database.

### Added
- **local-sync:** sync a remote session's worktree with a local folder — a
  three-way sync over SSH that never overwrites an edit made during a pass,
  turns divergent changes into conflicts you resolve with *Keep local* /
  *Keep remote*, and pauses a link whose folder lost most of its files
  instead of deleting the other side
- **local-sync:** the **Local workspace** card in session details (enable,
  state, paths, conflicts, sync now, pause / resume, excludes, disconnect)
  and a sync dot on linked session rows
- **work:** `work_link` run — an attempt at a work item tracked as a task,
  started in its own worktree with the brief; one open attempt per item and
  role (orchestration O0)
- **hosts:** the account probe reads the login `CLAUDE_CONFIG_DIR` points at,
  and hosts flag credential variables that outrank a `/login`
  (`ANTHROPIC_API_KEY`, Bedrock / Vertex, OAuth tokens …, by name only) in
  `fleet_health` and the Hosts view

### Fixed
- **work:** an open run attempt is only described to a caller who may see its
  task; the job-item column index follows the new task columns

## [0.5.1] - 2026-10-07

**Upgrade note:** this release carries migration 108 (sprints and releases in
the work graph). Back up `state.db` before upgrading the desktop or the hub; an
older build refuses a migrated database.

### Added
- **work:** sprints and releases in the native work graph — org-scoped
  sprints (planned → active → closed) and releases (planned → released), one
  current sprint per item, any number of releases, sprint close carrying the
  unfinished items over, and buckets linked to a tracker's sprint or version
- **work:** switch a session between tasks (`work_link` switch,
  `switch_session_work` on the desktop) in one transaction, with a warning
  when the task is already live elsewhere or in another organisation
- **work:** attach a running session to a task — *Attach running session…*
  under the Work button, with Switch / Add, an Undo for ten seconds, and a
  *Work on task…* picker that finds your own tasks first

### Fixed
- **work:** the start prompt reaches Claude through a trust dialog, a lost
  Enter and a swallowed paste; review, author and dispatch sessions now seed
  their first prompt the same way and wait for a settled REPL
- **work:** the attach picker never offers a lost session or the operator
- **hub:** never dies or wedges silently — panics are logged with a
  backtrace, a panicking tick pass is caught, and `serve` exits non-zero when
  the control API ends so the restart policy recovers it
- **mcp:** `/mcp` request bodies are capped at 8 MiB; slow and surplus
  connections are bounded (30 s header timeout, at most 512 connections);
  voice sockets time out
- **pair / auth:** a hub-wide pairing budget that still takes a burst, and
  IPv6 addresses budgeted per /64 for both pairing and failed bearer tokens
- **store:** a database a newer release migrated is no longer described as
  "corrupt — delete it"; message and task bodies over 64 KiB are refused
  before they are stored; a long unread inbox can be marked read again

### Removed
- **desktop:** eight Tauri commands nothing in the UI called

## [0.5.0] - 2026-10-06

**Upgrade note:** this release carries migrations 105–107 (worktree names with
slashes, org settings and spend, org members). Back up `state.db` before
upgrading the desktop or the hub; an older build refuses a migrated database.

### Added
- **work:** start a session from a task — the **Work** button and its start
  popover, a start preview (`preview_start`, routed through the hub on a
  desktop client) that shows the plan, branch and conflicts before anything
  runs, and **parallel start** of a second session on the same task
- **orgs:** organisation administration — the org overview, Settings → Company
  with Devices and People pages (pairing by QR), and `org_admin` for the
  company's orgs, devices and people from the owner's device; desktop org
  writes route to the hub
- **orgs:** per-org settings, spend and budgets — on the overview and the org
  page, with budget Attention items and budget alerts in `fleet_health`
- **orgs:** memberships and roles — members on the org page, sharing with an
  org, an org admin administering their own org, operator commands, and who
  sees the unclaimed counts

### Fixed
- **worktrees:** a branch-shaped worktree name (`feat/x`) no longer breaks recreate
- **work:** the Work button checks the resume gate at the moment it writes
- **operator:** the operator is never a detection subject and never accepts or
  rejects a proposal
- **operator:** the agent panel's lost-state restart works after a host reboot

### Changed
- **ci:** the hub image is built on every PR, not only on a tag
- **ci:** fleet-mobile's acceptance of the hub contract is checked on the PR, not at release
- **tests:** hub-e2e covers the owner's unclaimed count with two people; the
  scope guard's session filter now lives in `counted_rows`

### Documentation
- **spec:** task → session flow, operator threads, brainstorm → plan → agents,
  with the adversarial review folded in
- **orgs:** phase D — members and roles, the owner's answers, the operator guide
- **control-api:** reference regenerated for the new desktop and phase D commands

## [0.4.12] - 2026-10-06

### Added
- **operator:** start the UX agent on another host when its home is down
- **picker:** + New session and Hosts n open the switcher; the sidebar popover is gone
- **picker:** the switcher's New session mode — tickets, pins, suggestions, groups, keys, undo
- **picker:** NewSessionDialog autostart — ⌘↵ starts with the last settings
- **picker:** the project actions menu — pin, group combobox, hide
- **picker:** PickerList — chip, kbd, dim rows, group subtitle, mouse-only row actions
- **picker:** the ranking — hidden, dormant, clusters, Suggested, search
- **picker:** local frecency of the person's own picks
- **picker:** the project picks store, optimistic, loaded at startup
- **picker:** project_picks / set_project_pick commands, routed on a hub
- **picker:** project_picks / set_project_pick tools, a person's device only
- **picker:** project_picks — pinned, visibility and group per owner/repo
- **switcher:** assets and the Rescan / Sync fleet / Propose commands in the QuickSwitcher
- **assets:** the Hosts view — org, role per catalog, catalog admissions as toggles, and per-host provenance
- **assets:** the Layers view — layers by catalog with footprints, why-is-it-here, and layer changes as cards
- **assets:** the sync plan becomes a view in the workspace; Rollout review per host; a persistent footer live region
- **assets:** DiffView on tokens; Drift panel with Take and a confirmed Restore
- **assets:** ChangesetCard — apply, dismiss, reject items, undo from the Inbox and the Inspector; i and ⌘↵ on cards
- **assets:** M6 data layer — card views and verbs, layer changes, provenance, drift diff; line diff and layer helpers
- **settings:** Settings → Catalogs — add, remove, admit hosts; grants shown with the hub command
- **assets:** desktop commands for the catalog set, per-catalog layers, host provenance and the drift diff
- **assets:** desktop commands for the card verbs, routed to the hub's changesets tool
- **assets:** catalog_admin drift_diff — the catalog's and the host's text of a drifted asset
- **assets:** layer cards — create, rename, move a member (changesets propose_layer)
- **assets:** prune withdrawn cards a week after withdrawal; New-card slug collisions need a look
- **assets:** held lines as card item outcomes, catalogs on cards, hold pre-M5 overwrites

### Changed
- **hub:** pin ProjectPickRow's wire keys in the hub contract golden
- fix three known flakes
- **hooks:** make the pre-commit pnpm audit warn instead of block
- **ci:** check migration numbers against origin/main before a push
- **assets:** M6 carries — tokens in the remaining dialogs, wider colour guard, secrets keyed by catalog, scoped keys, narrow layout; CLAUDE.md

### Fixed
- **transcript:** a SendMessage's tool row names its addressee
- **host-health:** read kern.boottime's sec, not usec, for macOS uptime
- **picker:** an older hub or a refused token says so once per session; Add project's keep write is quiet; drop the unused .pad rule
- **picker:** ⌘Z and ⌘⌫ are the picker's only on an empty query; undo resets per open and says what it undid; Hide keeps the place
- **picker:** a person's group named like an owner's automatic group joins it; person's groups keyed lower-case
- **picker:** Add project expands a collapsed sidebar; Hosts n no longer needs it; add-blocked is an alert
- **picker:** ⌘P pins in New session mode instead of closing it; a stale toast Undo is a no-op
- **deps:** bump source-map-js to 1.2.2 (GHSA-68fv-2mgg-jv7q)
- **voice:** silence shellcheck 0.11's SC2329 on the trap handler
- **scripts:** isolate check-migration-numbers-test from a hook's git env
- **picker:** autostart gives up for good, never on a failed scan or a ticket
- **picker:** actions menu — Enter never applies a stray row, arrows reach the input, exact name ranked
- **picker:** scope row-action hover to actionable rows; actions don't pick
- **deps:** source-map-js 1.2.2 (GHSA-68fv-2mgg-jv7q)
- **assets:** M6 final-review minors
- **hosts:** re-read the selected host's provenance after an admission toggle
- **cards:** a Rollout that held hosts back says so and stays in the Inbox
- **layers:** a renamed layer re-derives its open cards and stays rolled out
- **drift:** never read a file on the host whose rendered bytes hold any ${
- **switcher:** hold commands while a card verb runs, mark Propose busy, share one propose helper
- **assets:** keep Esc and focus in the plan view; a review counts what it shows; neutral live-region finish
- **assets:** card primary follows what is on screen; per-catalog grants gate card verbs; Inspector re-reads a card after Undo; selected card is aria-current
- **assets:** overlapping card-view loads cannot overwrite a newer one, and a failed fetch keeps the old view
- **assets:** layer footprint and why-chain follow extends from a context too
- **settings:** Catalogs shows its state as a badge, and a hub client sees the page read-only with the hub's refusal
- **assets:** standalone add_catalog and remove_catalog take the authoring lock (PF7)
- **assets:** a retried card item clears its previous outcome

### Documentation
- **picker:** Phase 1 spec matches what ships — empty-query ⌘Z/⌘⌫, normal ⌘K only drops hidden, groups join same-named clusters
- move Architecture and Status out of CLAUDE.md
- **picker:** migration 104 after main took 102/103; the repo's validation ladder
- **picker:** v2 after four UX reviews — one picker in ⌘K, tickets first, nothing hidden for lack of history
- **picker:** phase 1 implementation plan; spec amended to what the code allows
- **picker:** the New session project picker — context, recent, popular, groups, noise folded, Jev proposes
- Assets M6 implementation plan — cards you can act on, Layers and Hosts, DiffView, Settings → Catalogs, QuickSwitcher
## [0.4.11] - 2026-10-06

### Fixed
- **hub-image:** COPY tools/voice — v0.4.10's hub image did not compile
## [0.4.10] - 2026-10-05

### Added
- **voice:** 🎤 toggle in the terminal header
- **voice:** desktop voice_claim — in-process standalone, /voice/source when paired
- **voice:** desktop microphone capture (cpal) and PCM conversion
- **voice:** provision the recorder stand-in and prefix claude's PATH
- **voice:** host arecord stand-in for /voice
- **voice:** /voice/capture and /voice/source relay routes
- **voice:** voice.* settings (relay off by default)
- **voice:** VoiceRegistry — one microphone claim per session

### Changed
- **voice:** run the arecord stand-in's test and shellcheck

### Fixed
- **release:** release-mobile.sh refuses a phone that would refuse its own hub
- **voice:** the 🎤 stays off after a release; say why a claim ended
- **voice:** tell a source why its claim ended; ping /voice/source
- **voice:** SIGTERM ends a live recording; ask tmux for the recorder's own pane
- **voice:** follow the attached session through one rule; no retry after error; ordered claim/release
- **voice:** a release wins over queued audio and does not wait for the device to open
- **voice:** start the source off the async workers; never play a microphone start gave up on; end a capture on a stream error
- **voice:** never lose a source stop; close a superseded source with 4001
- **voice:** capture state per session; claim replace/release revokes a live capture

### Documentation
- **voice:** guide gaps; max_capture_secs 0 is "no limit"; drop a no-op test assert
- **voice:** user guide and orientation
- **voice:** fix spec inconsistencies for revisions
- **voice:** spec revisions from planning
- **voice:** F1 implementation plan
- voice relay design (Claude Code /voice with the app's microphone)
## [0.4.9] - 2026-10-05

### Fixed
- **mobile:** pairs fleet-mobile 0.4.9, which accepts hub contract 8 — fleet-mobile 0.4.8 refused the 0.4.8 hub as "this app is too old" (fleet-mobile #104). No claude-fleet code changes since 0.4.8.

## [0.4.8] - 2026-10-05

### Added
- **hub:** fleet-hub person list | rename | disable
- **assets-ui:** keyboard for the workspace
- **assets-ui:** the Assets workspace replaces the twelve-button toolbar
- **assets-ui:** AssetsWorkspace and AssetsRail — the shell around the Inbox, Library, Inspector and footer
- **assets-ui:** the footer — a chip per catalog, auto, JobChip and the last sync
- **assets-ui:** blockedOnSecrets — the producer for the Inbox's blocked predicate
- **assets-ui:** the Inspector — a tabbed pane over the detail, the editor and History
- **assets-ui:** AssetDetail takes a section — overview, hosts or source
- **assets-ui:** the Library — every asset once per catalog, badges, host dots, the token query
- **assets-ui:** QueryInput, the token filter as a keyboard combobox
- **assets-ui:** the Inbox — open cards first, then what needs you, in sync folded
- **assets-ui:** the Inbox model and the sentence header
- **assets-ui:** the token query with completion and the one keep() every view uses
- **assets-ui:** workspace stores and loaders, selection keys, scope badge, write rights
- **assets-ui:** wire types for drift side, catalog and the auto run; one host order and one odd-host parser
- **assets-ui:** Badge in the Assets lists and plan, tokens instead of literal colours
- **assets-ui:** Badge, host-dot states and the shared visual vocabulary
- **assets:** four read-only workspace commands, routed to the hub (R13)
- **catalog:** asset_history -- the commits that touched one asset, per catalog
- **assets:** list every catalog's assets, with each catalog's own host states
- **changesets:** a drift card is only for a copy someone edited
- **m1:** T13 — desktop commands, capture_session for the watcher, contract 7
- **catalog:** a drifted managed row says which side moved
- **store:** the inventory records which side moved (migration 096: drift_side)
- **m1:** T12 — the sharing tools, Access::HostToken, and the claim path
- **sync:** the manifest records file hashes, so a plan tells an edited host copy from a stale one
- **m1:** T11 — long polls re-check access on every wake and before returning

### Changed
- **m1:** the org-boundary e2e checks assert M1's rule, not the pre-M1 one
- pin the changesets fixture's harness set and the downloads dir's writers
- **m1:** matrix cells for main's send_file and list_downloads
- **m1:** T15 — two people on one hub, end to end in hub-e2e
- **mcp:** one may_list_every_catalog predicate for list_assets and list_catalogs
- **m1:** the session isolation matrix, T14; plus F3's share sheet and D1's docs
- **assets-ui:** IdentityRow, the one unmanaged-identity row, shared by the list and the Inbox
- **m1:** multi-user foundations — people, ownership, grants, the person gate

### Fixed
- **m1:** migration 097's owner triggers lost their OR IGNORE on the upsert path
- **assets-ui:** e keeps an open draft; drifted cells say which side moved
- **sync:** a moved asset's old copy is deleted automatically only when verified unchanged
- **m1:** the merge's real breakage — migration versions, the download tier's tables
- **assets:** catalog chips say where the catalog lives; Secrets waits while busy; Import guards itself; the read-only chip controls its note
- **assets:** the Inspector keeps an open editor's draft, re-reads History, and never opens an org asset as personal
- **assets:** cards ignore every token, No matches, Import label, one shared row lead
- **assets:** only update/overwrite/plugin_update plan reasons read as warnings
- **assets:** the every-catalog listing is opt-in; a broken checkout has no empty history
- **sync:** the last sync shown is the newest a person made
- **changesets:** list answers undoability in one pass; the pass prunes old withdrawn cards
- **changesets:** rollout decisions order by when they were made (migration 097)
- **changesets:** names that import as one slug need a look, not a failed card
- **sync:** an entry that records no location stays unverified against the render
- **sync:** a card names the updates it held back for their host copy, and never counts that host done
- **sync:** read the host copy against the render, so a location it adds over a person's content is an edit
- **m1:** delete the name-keyed owner intent; close the claim primitive
- **m1:** T9e/T10 — the link-less item hole, honest guard table, remaining scope sites

### Documentation
- the transcript flake's real cause is an unpinned HOME
- two more Rust flakes, both with thin margins
- Assets M5 safety comments match the code
- Assets M5 — the workspace shell
- INTEGRATION-REPORT — D5 records the KIND_FENCES classification
- **m1:** the spec is no longer an unbuilt analysis
- **m1:** correct the plan's status and the six items T8 recorded as open
- regenerate the control API reference for the four workspace commands
- Assets M5 implementation plan (workspace shell, host-copy signal)
- MERGE-REPORT records the third pass
## [0.4.7] - 2026-10-05

### Changed
- buildkite-verify.sh takes --branch / --commit
- verify on a persistent Buildkite builder on demand
- scripts/verify.sh, one verification command for agents
- give the file-based upgrade test a Windows-only time budget
- run macOS and Windows clippy in parallel with their tests

### Documentation
- **buildkite:** install the pinned toolchain as the default before cargo install
- **audit:** Appendix J — macOS memory and the test-target split
## [0.4.6] - 2026-10-04

### Added
- file downloads — send a file from a session's host to your devices
- **mcp:** changesets tool — a grant per touched catalog, never a per-host token
- **changesets:** undo the latest applied card per catalog; dismiss and reject_item hold their subjects
- **changesets:** rollout and restore cards; catalog.auto's additive sync on rolled-out layers
- **footer:** say whose version is on screen -- this app's or the hub's
- **catalog:** authoring waits for a changeset apply in flight (PF7)
- **changesets:** apply catalog cards — one commit per catalog, nothing committed on failure
- **catalog:** repo read_asset, is_clean, reset_hard, revert (Assets M4)
- **changesets:** reconcile pass after each scan tick; propose, views, APPLY_LOCK
- **settings:** catalog.auto and catalog.auto_push (Assets M4, R18)
- **changesets:** card model and rules (bootstrap, new, drift, rollout)
- **catalog_admin:** authoring per catalog; one catalog lookup per call (M-d)
- **catalog:** CatalogTarget — authoring, get_asset and import per catalog
- **conversation:** a Workflow call is a block, on the desktop and the phone
- **store:** changesets, changeset items and triage verdicts (migration 094)
- **catalog:** block a file two harnesses would both manage (F3c)
- **catalog:** never write through a symlinked skills dir; move pre-F3c Codex skills safely (F3c)
- **catalog:** Codex scan reports symlinked skill directories (F3c)
- **catalog:** render Codex skills to ~/.agents/skills (F3c)
- **hub:** catalog add, list, remove, admit, unadmit; reload and client grant take --catalog
- **mcp:** catalog_admin takes a catalog, manages catalogs and admissions, and checks a grant per touched catalog
- **catalog:** add, list, remove and admit catalogs; layers per catalog; private copies; lint borrows
- **sync:** a catalog that cannot speak for a host never plans a remove; one snapshot per plan
- **catalog:** admissions in the effective set; speaks_for and held_back
- **catalog:** load any catalog; ensure_fresh keeps a broken org catalog as a problem entry
- **catalog:** host admissions and per-catalog client grants (migration 092)
- **sync:** plan, scan and apply against the effective catalog of each host
- **catalog:** the effective catalog for a host — acceptance, scope boundary, collisions
- **catalog:** asset origin, a borrowing registry view and the catalog acceptance rule
- **catalog:** layers and inventory rows carry their catalog (migration 091)

### Changed
- cut the full suite from 2:41 to 1:50 by removing two serial tails
- skip the build and test jobs for PRs that touch only unread prose
- build the Windows bundle in its own job, beside rust-windows
- **docs:** regenerate settings docs (REGEN_SETTINGS_DOCS, REGEN_PAGE_DOCS)
- **changesets:** *_held card actions for a caller holding APPLY_LOCK
- cargo fleet-fast-check, an inner-loop check in its own directory
- **agent:** move the hub+agent end-to-end test into its own crate
- **core:** read the frontend, desktop and doc files tests check at run time
- one live run per ref, and a timeout on every job
- **store:** in-memory test stores copy a database migrated once per process
- compile the bundled SQLite without allocation statistics
- **identity:** share the live-shape fixture as a pub(crate) cfg(test) builder
- optimise the bundled SQLite C code in dev and test builds
- **desktop:** build the desktop library as an rlib only
- dev builds keep line tables only; dev-debug profile for debuggers
- one canonical validation mode for agents (cargo fleet-check/test/lint)
- pin the Rust toolchain to 1.99.0
- **mcp:** raise the tool-surface budget to 70,623
- **docs:** regenerate control API reference (REGEN_DOCS)
- **sync:** test (b) really scans the remote org-bound host over a fake ssh
- **sync:** cfg(test)-gate resolve_for_host, the M1 single-catalog API

### Fixed
- **downloads:** the sweep leaves fresh bytes with no row alone
- **downloads:** read a list reply without rows as empty
- **mcp:** import_host from outside an org catalog's org needs the personal grant
- **mcp:** a changeset approval is bound to each item's content hash
- **catalog:** load_dir records unreadable entries as problems, never drops them
- **changesets:** take_host keeps the catalog header's metadata
- **changesets:** undo warns which adopted copies the next sync would remove
- **changesets:** SB6 runs leave the scan key alone; a failing host is backed off
- **changesets:** an imported file the checkout ignores is skipped, not a failed card
- **changesets:** SB6 never picks a drifted copy; catalog.auto says what it does
- **fleet-hub:** catalog list reads only (M-e); catalog remove names withdrawn cards
- **mcp:** changesets authorizes exactly what apply runs; non-persons refused before the card
- **changesets:** undo refuses when any path its revert touches is on disk untracked
- **changesets:** document undo's known limits — no un-hide, a layer stays rolled out (P27)
- **changesets:** trimming a follow-up rollout never lifts a person's no
- **merge:** seed the Codex copy in ~/.agents/skills in the M4 restore test
- **changesets:** undo refuses when its revert would overwrite a file on disk or its commit is gone
- **changesets:** SB6 leaves out a host a person rejected; never touches a pre-M4 fleet
- **changesets:** an applied host card and its note are one transaction
- **changesets:** a restore writes its drift's harness only and fails unless it wrote
- **changesets:** a host sync never applies a plan narrowed to nothing
- **changesets:** claim a file only if absent or untouched since pre
- **changesets:** own files, not folders — per-file guard, commit and reset
- **changesets:** apply commits and undoes only its own paths
- **store:** an apply's record and a failed apply are one transaction each
- **catalog:** is_clean sees every untracked file; scoped commit and reset
- **changesets:** a look item keeps the org catalog that failed to load
- **changesets:** an unreadable store makes the tick hook say so, not go quiet
- **changesets:** Task 5 review round 1 — PF12 undoable, atomic PF14, guarded withdraw, tick panic guard
- **sync:** an asset its catalog could not read is kept, not removed
- **deps:** bump devalue to 5.9.4 for three high advisories
- **deps:** bump async-trait to 0.1.92 for clippy 1.99's double_must_use
- **deps:** bump devalue to 5.9.4 (GHSA-j22f-vq7h-c4qm, GHSA-mcm9-63f2-9j32)
- **ci:** allow clippy::double_must_use from #[async_trait] output (clippy 1.99)
- **catalog:** a file differing only in letter case is blocked, not created over (F3c)
- **fleet-hub:** catalog add hints --org only on the needs-an-org refusal
- **catalog_admin:** configure on an org catalog points at add_catalog
- **catalog_admin:** apply_sync from a catalog removed since the plan says re-plan
- **catalog_admin:** remove_catalog is master-only, like add_catalog
- **catalog:** a catalog removed between ensure_fresh's list and load is evicted
- **catalog:** keep load errors and stale admissions out of resolve_preview's held_back
- **migrations:** renumber catalog_access to 093 after main took 092
- **catalog:** guard per-entry legacy links and the delete half of the symlink rule (F3c)
- **fleet-hub:** catalog CLI leaves owner rules to the service, names re-points, says what unadmit changed, shows a personal load error in list
- **mcp:** catalog_admin fails closed on an unseen plan, keeps list_catalogs from org-bound clients, audits the touched catalog
- **catalog:** one owner check shared by add and upsert; one host check; R4 names the org; lint reads secrets outside the registry lock
- **catalog:** only a checkout failure becomes a problem entry; cheaper eviction; shared entry lookup
- **store:** finish PF11/PF14 dedup, transactional set_catalog_config, PF11 test
- **test:** from_ref instead of a cloned one-element slice (clippy 1.98)
- **provision:** keep the warning a degraded provisioning leaves, and say why
- **settings:** one search hit per setting, so an approved guide cannot break search
- **guides:** an org-bound device never writes the fleet's guides, and nobody self-approves
- **ag:** never rm -rf an AG_HOME ag does not own, and pin where fleet installs
- **work,provision:** a native subtask's org, and a degraded ag step is not "delivered"
- **test:** run the cross-catalog Blocked test on unix only, like its local-host siblings (windows CI)
- **test:** gate plugin_ref_asset to unix like its only caller (windows clippy dead_code)
- **assets:** surface refused and withheld in the preview; scope refusals name their catalog
- **migrations:** 091 copies only host_layers rows whose host exists
- **sync:** an org host keeps private assets it already has — withheld, never removed
- **sync:** keep the unlayered guard, never remove a refused asset, keep excluded; unknown catalog ids are NULL
- **catalog:** a name or install-name overlap between catalogs refuses every copy
- **catalog:** union_all is None without a personal catalog; share the composition order
- **store:** merging a host keeps its layer assignments; HostLayerRow.catalog_id defaults
- **reconcile:** a turn that ends on a question is idle, not blocked

### Documentation
- **audit:** Appendix I, persistent Linux builder simulation / Buildkite POC
- changesets and triage verdicts (M4)
- **audit:** Appendix H, cargo-nextest, partitions and build-once/run-many
- **audit:** Appendix G, the test-target split experiment, and its corrections
- **audit:** Appendix F, the fast check and the test-module move not made
- contract-test rule in CLAUDE.md, audit Appendix E (A/B results)
- **audit:** the SQLite allocation lock and the template database (Appendix D)
- **audit:** A/B results of the Phase 1 package
- **claude-md:** the measured time of a filtered fleet-test run
- **claude-md:** list the known Rust flakes and the sequential fleet-core runtime
- Rust build performance audit (baseline, no changes applied)
- Assets M4 implementation plan (changesets, triage verdicts, per-catalog authoring)
- the reverse-link gap, stranded legacy skills, Codex source (F3c)
- Codex skills in ~/.agents/skills, migration and symlink guard (F3c)
- **hub:** fleet-wide catalog actions also need the personal grant
- admissions, per-catalog grants and org catalogs (M3)
- Assets M3 implementation plan (admissions, per-catalog grants and loading)
- scope the no-org parity constraint; document the org scope upgrade
- sync across catalogs (M2)
- **assets:** the M2 plan — sync across catalogs
## [0.4.5] - 2026-09-30

### Added
- **catalog:** assets carry scope: private | shared (private by default)
- **catalog:** a catalogs table under the existing config API (migration 089)
- **catalog:** render agents as Codex subagents in ~/.codex/agents
- **ui:** Codex auto / on / off in Host detail
- **mcp:** set_host_harnesses chooses a host's Codex sync (auto / on / off)
- **catalog:** plan and inventory Codex only where the host has it
- **catalog:** detect Codex in its scan and gate harnesses per host
- **store:** hosts.harnesses, a per-host harness choice (migration 088)

### Changed
- **catalog:** a registry of loaded catalogs replaces the CATALOG global

### Fixed
- **catalog:** registry test lock, the real lock-order rule, spec milestones
- **catalog:** no store-then-registry lock nesting; one personal entry; borrow, don't clone
- **catalog:** lint and render polish for Codex subagents
- **catalog:** a harness change drops parked plans and owes a rescan
- **catalog:** detect Codex only from evidence fleet never writes
- **catalog:** substitute secrets into TOML files as TOML strings
- **catalog:** codex agent extra cannot override name, description, instructions or model

### Documentation
- the catalogs table, the registry and scope
- fix a sentence in the Codex upgrade note
- Codex upgrade note; spec notes on tier map and catalog sources
- **plan:** drop a local build path from the F3ab plan
- **assets:** the M1 plan — catalogs table, registry, scope
- per-host harness set and Codex subagents (F3a, F3b)
- **assets:** the S1b + S2 design — catalogs, scopes, changesets and the workspace shell
## [0.4.4] - 2026-09-30

### Added
- **pages:** the fleet-guides catalog skill, tested end to end with a fresh agent
- **pages:** guides — layout L9 and guides an agent proposes at runtime
- **sync:** an unlayered remote host is skipped instead of getting the whole catalog
- **catalog:** import from any host over SSH, optionally only the assets asked for
- **catalog:** a scan tick that rescans stale hosts and every host after a catalog or sync change
- **assets:** one row per asset with a host strip; fleet internals folded away
- **catalog:** list_assets groups unmanaged copies into classified identities
- **catalog:** unmanaged inventory rows keep their content hash and flags (migration 087)
- **catalog:** a content hash and fleet/secret flags for every installed asset
- **provision:** install the ag launcher and a cl command on every host
- **settings:** provision.install_ag — install the ag launcher on provisioned hosts
- **tmux:** the pane command falls back to a provisioned ag before plain claude
- **ag:** install.sh --alias adds an alias unless the user already has one

### Changed
- **catalog:** cover HostStrip states and the identity-differ contract
- **claude:** install openssh-client in web sessions

### Fixed
- **catalog:** installed_detail recognizes the legacy token hook too
- **catalog:** normalize only's host-identifier entries before matching
- **catalog:** AssetListing.identities is Option so old-hub fallback fires
- **sync:** "Plan anyway" re-plans the clicked host, not the whole fleet
- **catalog:** the remote dump script exits 0 whatever the agents dir holds
- **catalog:** gate import_assets, confine the remote dump, scrub legacy token hooks
- **catalog:** a host whose rescan failed stays owed until it succeeds
- **assets:** the orphan filter ignores case like the identity filter
- **settings:** provision.install_ag help says the cl shim is only added where no cl exists
- **provision:** surface ag warnings from content-only runs; keep the restart hint
- **hub:** ship tools/ag in the hub image build context
- **ag:** --alias never shadows a user's command or a symlinked config

### Documentation
- CLAUDE.md names host_hash and the identities response field
- CLAUDE.md notes for the assets S1a foundation
- upgrade heads-up for the ag rollout
- provisioning installs ag; panes launch through it
## [0.4.3] - 2026-09-30

### Added
- **pages:** embed pages (L8); account usage drawn through the catalog everywhere — declarative pages P4d
- **work:** the view carries origin, project, parent, proposals, jobs and agent steps
- **work:** desktop create_work_task and proposal decisions, routed
- **work:** work_link create / propose / accept / reject
- **work:** dispatched jobs appear as agent subtasks and follow the job
- **work:** starting a native item uses its project and the parent ticket's brief
- **ui:** task page — notes, subtasks, proposals, jobs and agent steps
- **ui:** Work tab List layout — To do / Doing / Done under the existing header and filters
- **ui:** remove the global Tasks popover; jobs are in the Work list
- **work:** agent proposals — capped, decided once, a rejected title stays rejected
- **work:** native tasks and subtasks (TASK-<id>), job mirrors, depth one
- **work:** capture Claude Code task steps from PostToolUse, transcript backstop for stale hosts
- **ag:** one-command installer
- **ag:** doctor with exact fixes, install hints
- **ag:** alias shims that work in tmux, ssh and fleet panes
- **ag:** Codex driver — exec, resume, yolo, model/effort mapping
- **work:** list data — wire fields, create/decide wrappers, status grouping, layout pref
- **ag:** normalised flags and Claude Code launch
- **ag:** launcher skeleton — config reader, harness registry, list/which
- **work:** agent steps — Claude Code adapter and the journal's step kind
- **work:** a job's status ('task') is final over the live lift
- **work:** migration 086 — origin, project, notes, job and proposal columns
- **conversation:** responsive header bar with one control height and a slim context gauge
- **pages:** data-page filter bar; Work graph usage as a generated page — declarative pages P4c

### Changed
- cargo fmt
- **ag:** run the launcher tests on macOS bash 3.2 + Linux; docs
- **pages:** read the host filter's options with Array.from for svelte-check

### Fixed
- **work:** sort step groups with sort_by_key (clippy::unnecessary_sort_by on rust 1.98)
- **work:** batch-B join — start briefs never carry a parent the caller cannot see; regenerate the control API reference; raise the tool-definition budget to 68,056 (measured 67,956 on 2026-09-29 after work_link create/propose/accept and parent/notes/why, +410 bytes)
- **work:** local items named after migration 086 are origin 'manual'
- **ag:** config comments, CDPATH safety, shim/harness hardening; tests
- **ag:** stricter flag parsing, resume picker, --flag=value
- **ag:** never replace a foreign ag; doctor detects shadowing
- **desktop:** put back the nulls the hub strips, so the Hosts view stops crashing
- **terminal:** the selection follows its text when the screen scrolls
- **catalog:** name the checkout when its directory cannot be made

### Documentation
- **assets:** the interactive mockups of the Assets workspace, beside the spec
- **work:** shared work context — skill, user guide, hook matcher, command count, spec status
- **assets:** the S1a implementation plan — identities, scan tick, remote import, unlayered guard
- public-safe spec/plan copies; deferred follow-ups
- add architecture diagrams (standalone, hub, mobile)
- **work:** implementation plan for shared work context (roadmap part 1); mark the list-only plan superseded
- **assets:** hosts accept catalogs explicitly, so a personal host can run an org's assets
- **assets:** the Assets workspace design — inbox over layers, scopes and org catalogs, authoring and skill tests
- **work:** shared work context — keep today's Work header, add a List | Grouped toggle; trackers are already connected on the hub
- **work:** shared work context — map every existing Work feature into the new view; owner's mockup review
- **work:** design for shared work context (AI task system roadmap part 1)
- capture AI task system vision, roadmap and countermeasures
- **work:** implementation plan for the internal task list; spec revisions after reading the code
- **work:** design for one internal task list (own + dispatched tasks)
## [0.4.2] - 2026-09-29

### Added
- **work:** page a Work view's open sections and review total in one tree read
- **evidence:** assess a PR's result and show it — Result card, Work chip, stale CI badge
- **conversation:** tool detail on the hub, per-tool detail views, reply actions footer
- **update:** X-Fleet-Client, update:changed, fleet_health.updates, why (S4b, first half)
- **evidence:** store the PR evidence and when it was last observed (migration 082)
- **work:** a working session shows local work in progress
- **update:** Git-mode check and `fleet-hub update check` (S3)
- **evidence:** the PR probe reads the head commit, review and merge state, and the worktree's own HEAD
- **pages:** the fleet's settings on a paired device — declarative pages P6
- **work:** a merged PR stamps local work done, once
- **decide:** Jev robustness, paired languages, floor sweep, question set and fleet_health.decide
- **work:** work_link { action: set_status } for local work
- **pages:** search as a command, page actions, custom cap 3 — declarative pages P5b
- **pages:** settings proposals, review_apply and the audit — declarative pages P5a
- **work:** record who decided an item's status
- **pages:** flows and the tracker resource; Trackers generated — declarative pages P4b
- **pages:** resources, master_detail and the record editor; Organisations generated — declarative pages P4a
- **pages:** Svelte renderer, settings pages in Settings — declarative pages P3
- **pages:** page DSL, catalog, data sources and validator — declarative pages P2
- **settings:** registry metadata and describe — declarative pages P1

### Changed
- **add_project:** publish the grandchild pid atomically before cancelling
- **hub-deploy:** bound the non-numeric checks without GNU timeout
- **decide:** tidy the Jev bench, status_map trigger and claude -p runs
- **work:** pin the kind-absent Rename fallback
- **mcp:** compare fleet_health through the pool without its uptime clock
- **catalog:** the unprobeable-checkout test holds on Windows too
- **work:** guard status_changed_at writes so reverting either breaks a test
- **work:** seed tidy fixtures through set_item_status, not a raw UPDATE
- **work:** name ITEM_COLUMNS' length instead of hardcoding it

### Fixed
- **catalog:** name the checkout when its clone parent cannot be created
- **hub:** hub ops, CLI and WSL review fixes
- **update:** harden the hub's update state, reports and pins
- **release:** one build ID for tarballs, hub image and manifest
- **work-view:** one-read refreshes, kind-aware re-reads and review fixes
- **work:** a stale merged signal must not stamp work named after it
- **work:** Work view backend review fixes (counts, placements, scopes)
- **write-back:** queue PR links made after the PR; sweep the outbox as a retention table
- **work:** resolve a key two orgs share to the caller's own item
- **work:** stop the chip contradicting its own status dot
- **work:** show the effective status in the desktop Today header
- **work:** keep a done local item's journal and handover history
- **work:** one condition for "a working session on this item"
- **work:** stamp the derived done where the merged PR is recorded
- **work:** cross-check the SQL/Rust precedence, fence card.rs, fix chip
- **reply-actions:** Retry through the outbox; refresh chips without clobbering edits
- **sessions:** carry stale_demoted_at on the row; demote only reconciled hosts' rows
- **rewind:** fork keeps the source's model/effort and refuses a projectless source before copying
- **events:** judge context_full on the hub stream at health.context_red_pct
- **work:** stop breaking phone Rename; add effective_status, fence it
- **hosts:** Hide keeps a host's sessions; fence fleet_health.hosts[]; merge_host moves host-keyed data
- **work:** agree on status across views, close the last hiding CASE
- **decide:** decide status counts agreement only where the rule decided
- **work:** stop hiding a local item's status on the session row
- **work:** stamp status_changed_at so done can age into done_idle
- **work:** a stamped done classifies as pr_merged_idle, not done_idle
- **catalog:** name the checkout the hub cannot read

### Documentation
- **work:** record the journal-retention rule for local work
- **work:** say that effective_status coerces, and pin it
- **review:** two-day review of d4b7a21..HEAD — simplify, gaps, speed
- **evidence:** result evidence design and plan — tie done to a commit, CI to a clock
- **pages:** P6's phone half, on fleet-mobile
- **api:** list the settings commands under commands::pages in the reference
- **work:** the derived done stamp must not widen auto-tidy
- **work:** document work_link { action: set_status } in control-api.md
- **work:** plan for phase 1, native item status
- **api:** list the page commands in the control-api reference
- **work:** a derived 'done' must be stamped, not computed
- **work:** design for sprints, releases and epics in the native work graph
- **api:** regenerate control-api reference for get_settings describe
- **spec:** declarative pages and forms — research and design
## [0.4.1] - 2026-09-29

### Added
- **update:** sign the release manifest and publish the update channels (S2)
- **diagnostics:** hub state, earlier errors and UTC times in the bundle
- **update:** the hub side of the update wire — /update routes, updater token, desired/observed (S4a)

### Changed
- **update:** trust the release key
- **trackers:** conformance scenario 12 asks the real adapter

### Fixed
- **attention:** an attach or the TTL ends the stale_working reason, not the demotion (sessions.stale_demoted_at)
- **nl:** the census and J1 bench read person_text's Cow
- **work:** a key two orgs' trackers share resolves to the caller's item
- **work:** one harness-tag list for the hook and the transcript
- **mcp:** fence_ticket counts what it shows on the tracker's own text
- **work:** the task detail says when it cut the description
- **windows:** Disconnect removes the legacy token file even if CredDeleteW fails
- **windows:** keep the app data in the Local profile, moved once
- **provision:** say when a WSL distribution's hooks cannot reach the desktop
- **ssh:** cap concurrent connections per host when there is no mux
- **ssh:** toolchain probe finds the login shell when $SHELL is unset
- **wsl:** detect again when a wsl- alias is missing; start in $HOME
- **sidebar:** migrate saved work filters field by field
- **filters:** announce clearing the last filter; keep focus when a chip goes
- **sidebar:** the "N archived hidden" count matches a search as the list does
- **work:** the tree hides archived tasks only when asked, judged over every link
- **attention:** the tick lifts a resumed or expired stale_working stamp (reconcile.stale_working_ttl_secs)
- **test:** hub-deploy-scripts-test follows upgrade.sh — compose --env-file, compose pull, the token on curl's stdin
- **work:** describe says when its own 32,000-char cap cut the text
- **work:** a tail edit past the excerpt drops the stale describe cache
- **settings:** stale_working_secs says what it watches, not pane output
- **settings:** chip editor saves against the list it read; rows keyed by chip
- **mcp:** quick_replies set is a person's; compare-and-set and no control bytes
- **sessions:** a stale-demoted idle is not a finished turn until the pane says so
- **reconcile:** a pane showing the spinner is never demoted as stale working
- **mcp:** add_project never binds an MCP caller's call_id
- **usage:** date a line by its top-level timestamp, not the last match
- **decide:** one daily token budget for live and benchmark runs
- **rewind:** pass awk values through the environment, read an escaped cwd, refuse names git would
- **deploy:** private backups, offline pre-upgrade copy, safer prune and rollback
- **conversation:** a quick-reply chip never sends into a session waiting on an answer
- **attention:** an attach acknowledges a stale_working stamp

### Documentation
- **roadmap:** record D15 narrowed for cross-org links on the phone
- stale_working ends on a hook, an attach, a resumed row or reconcile.stale_working_ttl_secs; hub-deploy-scripts-test asserts the token rides curl's stdin only
- **windows:** WSL hosts are standalone-only; Cygwin config, dev ConPTY, data dir
- session state machine — plan ticked, OOM window, status vocabulary
- **plans:** stale_working acknowledgement — an attach, a resumed row or a TTL lifts the stamp
## [0.4.0] - 2026-09-28

### Added
- **hub:** healthcheck --ready --json, fleet-hub backup, build identity (update S5)
- **sessions:** carry model and effort through recreate and restart
- **update:** fleet-update crate — manifest, signatures, decide(), UpdateChannel (S1)
- **hub:** fleet-hub decide enable|disable|mode|unassigned|set
- **hub:** manage the asset catalog from a paired desktop
- **decide:** --split and --question for the status_map bench
- **ui:** Fork sheet defaults to a new worktree
- **rewind:** fork into a new worktree
- **work:** the Work tree hides archived tasks unless asked, and counts them
- **sidebar:** hide archived work by default, one recency rule, selection follows filters
- **sessions:** pick model and effort when creating a Claude session
- **quick-replies:** chip order and per-chip auto-send
- **sidebar:** one filter model for Sessions and Work, and fix filters that did nothing
- **windows:** ship Microsoft's ConPTY with the Windows build
- **work:** a confirm / reject by link id answers the link's new version
- **work:** SessionRow.work_rev, so a secondary link change reaches the UI
- **windows:** WSL distributions as hosts; one ssh program; Cygwin/MSYS configs
- **conversation:** model and effort pickers in the composer
- **work:** register work.describe_cache_secs with the frontend too
- **work:** sweep the describe cache with retention, floored at 30 days
- **work:** add the describe action, its setting and dispatch
- **store:** describe cache table, TTL read/write and sweep
- **work:** a describe capability on the tracker providers
- **work:** lookup, brief and card say when a description was cut
- **work:** fence_ticket names what it cut
- **projects:** forget_project and worktree path dedupe for a store nothing rescans
- **provision:** per-host content fingerprint, stale re-provision on hub start, skill-dir ownership
- **work:** carry a tracker description's true length
- **hosts:** merge_host folds a renamed alias in one transaction; unprobed hosts are reaped
- **release:** ship a Windows NSIS installer from 0.3.3
- **windows:** keep the hub client token in Credential Manager
- **windows:** home dir, no ssh multiplexing, no local host
- **ui:** the Add-project dialog on a hub client hides the folder source
- **contract:** revision 5 — add_project routes, GithubRepo on the wire
- **desktop:** route add_project and list_github_repos to the hub
- **ui:** host Health block, disk/agent attention marks and a sidebar disk dot
- **hosts:** sample disk, load, memory and uptime each pass; fleet_health.hosts[]; move target-space preflight
- **mcp:** add_project and list_github_repos as hub tools
- **core:** AddProjectArgs and AddProjectSource carry a served schema
- **hosts:** read claude/tmux versions in the reconcile probe and stamp claude_version_at

### Changed
- **hub-deploy:** name a missing call in the order checks; run the suite in CI
- **windows:** the fake-ssh add_project tests and the awk usage tests are Unix only
- **work:** the describe cache is a retention table like the others
- **windows:** answer ConPTY's startup cursor query in the child-exit test
- **mcp:** restore the field-name loop beside the wire-exclusion greps
- **hub:** the desktop's hub.remote_url key is fleet-core's constant
- **windows:** drain the pseudo console in the child-exit test
- **mcp:** the ungated-rewind test passes an anchor, now that a rewind needs one
- **mcp:** restore the wire-exclusion test's substance
- **contract:** the session row's pinned wire names include work_rev
- **work:** cargo fmt and a clippy single-match fix
- **mcp:** wire-exclusion proof for the describe cache; regen reference
- **work:** cover describe in the org isolation matrix
- one "turn is over" set, one fp-key reader; docs point at what landed
- review rounds 1-5 follow-ups (docs, offline gating, one debounce helper)
- **windows:** main's new usage tests run the awk reader under bash — Unix only
- **windows:** build main's new haiku and bench fakes' status portably
- **windows:** check PTY children through sysinfo; run every crate's tests
- add Windows legs for the desktop crates and the frontend
- **hosts:** one active_hosts() for every fleet-wide host loop

### Fixed
- **store:** open the backup copy for write before sync_all (Windows)
- renumber host-identity migrations to 74-76 and re-measure the tool budget
- **hub:** give `tracker test` a 120 s CLI limit instead of 10 s
- **test:** the fake docker in hub-deploy-scripts-test speaks upgrade.sh's compose calls
- **work:** leave prompts Claude Code submits itself out of first prompts, detection, census and J1
- **decide:** 30% dev boards so the test side can judge the haiku line
- **windows:** edge cases in WSL detection, ssh selection, ssh config, drops, tokens and ConPTY
- **rewind:** a fork's cleanup leaves a tree a live pane is working in
- **work:** finish the describe cache's wiring, docs and pinning test
- **mcp:** create_remote needs a person from any caller but a paired client
- **work:** the task detail says when it cut, and the describe cache dies with its tracker
- **work:** a complete Jira description reports the length it returns
- **decide:** the work_link bench reads a hand label's org from the database
- **windows:** read wsl.exe's UTF-16 without chunks_exact
- **store:** repair main's 064-068 on a database a branch-numbered usage migration skipped
- **usage:** every chunk of a first read, a rewrite and yesterday's lines book history right
- **decide:** a shadow answer nobody saw is never a person's follow-up
- **work:** report the describe cache's EFFECTIVE retention window
- **decide:** the status_map benchmark takes each row's org from the database
- **decide:** deciding a proposal reads and writes the tracker under one lock
- **settings:** work link's Jev mode is read-only; how a desktop sets its Jev key
- **decide:** a proposal a newer one superseded undecided is marked ignored
- **decide:** retention keeps a run a person confirmed, corrected or rejected
- **decide:** the offline benchmark needs no live mode and never counts as live
- **windows:** findings of a review of the Windows client
- **reply-actions:** Retry waits for two idle probes, not one quiet frame
- **rewind:** quiet-or-refused guard, anchor required to rewind, clean undo on a failed restart
- **decide:** a tracker due while a status_map run is going stays due
- **projects:** Stop waiting ends a hub client's add_project; no local destination preview there
- **ui:** reword the describe-cache settings hint
- **work:** batch the describe-cache sweep; add its dry-run status
- **work:** fence describe's body for a per-host token (CRITICAL)
- **conversation:** mark a cut or image-bearing prompt partial; Retry refuses to resend it
- **windows:** a BOM'd ssh config and a stuck token file
- **windows:** reap orphaned ssh -R tunnels on Windows too
- **mcp:** fence add_project / list_github_repos, confirm the operator's fork and create_remote
- **hub:** let a hub be pointed at its asset catalog
- **work-view:** keep loaded data on a failed refresh; page sections past 200; max-wait refresh
- **work-view:** Review's Undo and Change… name the versions they decide on
- **work-view:** show a conflict's current value with Reload; view delete is a compare-and-set
- **windows:** no console window for any child process
- **work:** shrink the brief retry's shown count, not its budget
- **work:** correct the brief retry's overhead math, unify describe_offer
- round 15 — never upgrade without a backup; tighter guards
- **release:** the Windows installer ships from 0.3.4
- **move:** carried memory files stay private when fleet runs as root
- **release:** the macOS signature check searches only target dirs that exist
- **mcp:** a repeated bad bearer is still 401; only its log line is throttled
- **rewind:** whole lines only, a fork carries its work, no shell rows
- **decide:** D34 holds through unlink; the status_map trigger keeps due trackers
- **sessions:** #343's states meet the paths written beside it
- **hub:** fence usage_report for org-bound clients; safer upgrade and backup
- migration 071 records 71; D34 holds through decide_batch and reconsider
- **store:** migration 071 records version 71, not 68
- **store:** migration 071 records version 71, not 68
- **work:** restore SessionRow.work_rev lost in the #343 merge
- **windows:** catalog paths and credential files; Unix-only tests declared
- **work:** count a Jira description's true length past the excerpt cap
- **windows:** keep the unix-only test modules visible to the print guard
- **windows:** cfg-guard the unix-only calls so the desktop compiles on Windows

### Documentation
- **claude-md:** 205 commands, not 204 — merge_host is the 205th
- **work-graph:** M14 truth pass — name the PRs, M14.5 waits on Part R only
- **spec:** application update architecture — UpdateChannel, manifest, desired state
- **decide:** the status_map bench's --split and --question
- **filters:** fleet-mobile follows the filter model (its branch claude/serene-faraday-7loasz)
- **usage:** backfill caveats, usage_daily retention, §6d left out
- **work:** warn against a lock or await inside describe's cache-hit block
- **claude-md:** reply actions in Status & known issues
- **reply-actions:** spec status, refusals and Retry gating as built; rewind_conversation in revision 5's note
- **windows:** plan status, hub and index links, CLAUDE.md orientation
- **work:** four plans from the work-graph re-examination
- **plan:** Windows desktop client plan
- **hub:** a hub client adds projects through the hub
- **projects:** implementation plan for adding a project from a hub client
- **projects:** design for adding a project from a hub client
## [0.3.3] - 2026-09-28

### Added
- **work:** D31's switch on the desktop and in fleet-hub org set
- **hub:** fleet-hub decide proposals apply|reject <run>
- **work:** Jev's section proposals in Settings → Work (assist)
- **desktop:** status_map_proposals and decide_status_map_proposal commands
- **decide:** a person applies, corrects or rejects one status_map proposal
- **work:** the Work view's desktop commands and work:changed (work graph M14.1d)
- **hub:** decide bench --provider haiku --haiku-host ALIAS
- **decide:** haiku provider in the work_link bench, J1 haiku line judged
- **decide:** haiku provider in the status_map bench, J3 haiku line judged
- **decide:** claude -p haiku baseline over SSH (D33)
- **hub:** fleet-hub decide bench status-map and work-link --shape
- **decide:** J1 bench acceptance, calibration and the choice+noul shape
- **decide:** offline status_map benchmark (J3 phase 0) and calibration
- **work:** the Work view's writes and compare-and-set (work graph M14.1c)
- **usage:** book tokens to the transcript's UTC day, keep first-read backfill apart, and label the report's two populations
- **desktop:** draw the context chip at the hub's context_red_pct instead of a local 70/90
- **hub:** fleet-hub decide bench work-link
- **decide:** offline work_link benchmark, J1 phase 0
- **store:** read-only queries for the work_link benchmark
- **hub:** fleet-hub decide proposals and tracker section-map
- **decide:** status_map adapter for Asana sections, shadow/assist only (J3)
- **asana:** keep unclassified section names and board order at probe
- **attention:** stop_failed, context_full, stale_working and ci_failing reasons; one context threshold (health.context_red_pct)
- **work:** the Work view's reads and org-bound clients (work graph M14.1b)
- **reconcile:** a stale working row ages out to idle (stale_working_at, reconcile.stale_working_secs)
- **hub:** fleet-hub decide set-key|clear-key|status|runs, org set --jev
- **orgs:** per-org Jev consent in Organisations (D31)
- **decide:** the decision envelope and Jev client, off by default (D35, D36)
- **desktop:** resume the hub event stream with Last-Event-ID, re-list only on resumed:false, and refetch projects and work after a gap
- **store:** decision_runs, decision_secrets and org Jev consent (D31, D37)
- **hub:** fleet_health.hub tick stats, /metrics reconcile and fleet gauges, tunnels_mode and peer_links_total, and WARN/INFO lines for usage failures and stuck transitions
- **hub:** local language census for the Jev evaluation (D40)
- **hub:** warn on a routable plaintext bind, throttle repeated bad bearers per address, and put the behind-proxy compose on the proxy network
- **work:** the Work view's backend (work graph M14)
- **deploy:** hub backup and upgrade scripts, the .env-pinned behind-proxy compose, and the backup/restore runbook
- **ui:** fork sheet — choose the new session's worktree
- **ui:** reply action row — copy, quote, retry, fork, rewind
- route rewind_conversation from the desktop
- **mcp:** rewind_conversation tool
- **rewind:** one engine for fork, rewind and retry
- **rewind:** host-side script that truncates a transcript into a new id
- **transcript:** carry each turn's prompt uuid as a truncation anchor

### Changed
- **hub-e2e:** space the bad-bearer checks by AUTH_FAIL_INTERVAL
- **decide:** status_map exposes question_for for the benchmark
- **reconcile:** write each host's rows as its probe completes instead of after the slowest host
- **work:** the Work view end to end on a real hub; tracker state wording
- **release:** sign macOS bundles with the Apple Development Team ID
- **work:** desktop Work view UI checkpoint (review fix pass in progress)
- **rewind:** cover the no-project and no-claude-id refusals
- **rewind:** harden the gsub-trap and shell-quoting tests

### Fixed
- **store:** migration 071 records version 71, not 68
- **decide:** collapse the local-item guard into its match arm (clippy 1.98)
- **decide:** haiku stays inside the org boundary and sends its prompt on stdin
- **work:** a person's "Clear work" holds against the unchanged branch or PR
- **timeline:** one status_change per transition from either writer, one stuck event per episode
- **work:** fence fleet_health totals for org-bound clients
- **work:** keep a suggestion a carry settles as a timeline event
- **work:** an agent's ticket start or named work is never a person's link
- **work:** auto-trust counts only confirmed branch suggestions
- **startup:** log the backend-resolve steps and bound the keychain read to 10 s
- **work:** keep withdrawn and decayed suggestions as timeline events
- **work:** record who decided a work link, person or agent
- **work:** an agent's link cannot overturn a person's rejection
- **hooks:** a StopFailure marks the turn failed, classed rate_limit/auth/other, until the next prompt
- **work:** claim the ticket key before a start spawns
- **sessions:** loss clears pane-derived state, external ghosts get a one-hour grace, shells carry no claude_status
- **work:** Work view review findings on the desktop
- **pane-intel:** oom needs a dead process (heap block, or a kill verdict with the shell back), never a word
- **work:** fence fleet_health for org-bound clients
- **playbooks:** oom recreate refuses a live turn, keeps a budget, and does not restart the episode on a resume re-render
- **ui:** surface rewind refusals, wait for the REPL, gate Retry and the viewed conversation
- **rewind:** refuse an empty prefix, label a fork `fork`, locate the transcript as everyone else does
- **rewind:** check the pane pre-conditions before the rebind commits
- **mcp:** fence rewind_conversation to the caller's host
- **ui,docs:** fork sheet defaults to same worktree; regenerate stale MCP reference
- **transcript:** cover the tenth ConvTurn site and regen the hub contract golden

### Documentation
- **claude-md:** 185 commands carry a hub verdict
- **jev:** deciding status_map proposals in the desktop and the CLI
- regenerate the control API reference for the Work view's commands (M14.1d)
- **plans:** preserve the 2026-09-15 terminal hardening plan
- **claude-md:** phase 0, D33 and D34 in the Jev status paragraph
- **jev:** status of phase 0, D33 and D34 in CLAUDE.md and the specs
- **jev:** the haiku baseline's org boundary and prompt on stdin
- **jev:** the claude -p haiku baseline (D33) and its CLI
- **jev:** benchmarking status_map, J1 acceptance, phase-0 checklist
- **jev:** D32 and D34 as decided and built
- **jev:** benchmarking work_link; J1 components built
- **jev:** status_map (J3) guide, hub CLI lines, test map card
- **work:** the Work view spec's M14.1b revisions
- **jev:** record D35-D37 as built; hub guide section on decisions
- **jev:** the test map — cards, datasets, metrics, gates, demotion
- **jev:** record D47, keep the language models in fleet-hub
- **work-graph:** M14.0, the Work view spec on main and the M13 truth pass
- **work-graph:** M14 plan, the owner's answers to D31-D36
- **work-graph:** plan M14, landing the Work view
- **plan:** Task 7 must place the forked transcript under the NEW cwd
- **plan:** the MCP budget is 56,660, not 71,658
- **plan:** rewind_conversation takes a CancellationRegistry
- **plan:** Task 3 uses run_shell and maps the script's sentinels
- **plan:** fix five defects the pre-flight scan found
- **plan:** reply actions, one plan per repo
- **spec:** one anchor per turn, not two
- **spec:** reply actions — Copy, Quote, Retry, Fork here, Rewind here
## [0.3.2] - 2026-09-27

The work graph's M13 (live use): what real use showed, and the decided-against
ideas the owner said yes to, each in its smallest safe form. Migrations
061–064 run on first start; back up `state.db` first (see
[docs/RELEASING.md](docs/RELEASING.md#upgrading-into-the-work-graph)), since an
older build refuses a database a newer one has migrated.

### Added
- **work:** summaries of dead sessions, on demand (decision D10).
  - **Summarise** on a past-work row asks the session's own host for a
    summary of its last conversation.
  - The summary is a fork of that conversation with no tools, no MCP servers
    and fleet's hooks off, on `work.summary_model`.
  - It lands in the work journal and in the next resume brief, fenced as
    untrusted.
  - The operator's request waits for your approval, and a hub refuses it.
- **trackers:** a session's pull request as a remote link on its Jira
  ticket (decision D3). It is off by default: turn on *PR remote link* per
  tracker in Settings → Work.
  - Only for work a person linked or started, and only to the tracker of
    that work's own organisation.
  - The link is idempotent, so the same PR is never linked twice.
  - It is sent through a retrying outbox (migration 061), and old rows are
    cleaned by retention.
  - Nothing else is ever written to a tracker.
- **work:** a usage summary: how the work graph is used, as counts only.
  It is master-only, and nothing leaves the machine. Find it in Settings →
  Work → Usage, `fleet-hub work usage` or `work_admin { usage }`.
- **trackers:** a tracker that keeps skipping items now reads `degraded`,
  then `failing` after three such passes, with its own Attention item
  "Sync skipping items" (not "Reconnect").

### Changed
- **migrations:** 062 made a table for inbound webhook secrets, a feature
  (D13) removed again before any release. 064 drops that table, and 063
  bumps a session's row version only when something a client shows
  changed.

### Added
- **trackers:** webhook nudges on a public hub (work graph M13.4f)
- **trackers:** write a session's PR to its Jira ticket, opt-in (work graph M13.4e)
- **work:** M13.2 usage summary, work_admin { usage } (D24)
- **ui:** Settings → Work → Usage, a read-only table with Copy as text
- **work:** add work_admin { usage } — how the work graph is used, as counts
- **trackers:** keep running pass totals in the sync metrics
- **ui:** Settings → Work → Usage, a read-only table with Copy as text
- **work:** add work_admin { usage } — how the work graph is used, as counts
- **work-graph:** M13.1 partial sync failures are visible
- **trackers:** keep running pass totals in the sync metrics
- **sessions:** label external rows with the name Claude gives them
- **ui:** say "Sync skipping items" instead of Reconnect for a tracker stuck on items
- **trackers:** make partial sync failures visible in metrics and fleet health
- **work:** summaries of dead sessions, on demand (work graph M13.1)
- **conversation:** show a sent message at once, with its delivery state
- **sessions:** carry prompt_submit_seq on the session row

### Changed
- **mcp:** the wire-level notice repeats until the caller lists
- Revert "chore(release): v0.2.43"
- **store:** row_version bumps only on a client-visible change
- **release:** v0.2.43
- **trackers:** remove the webhook nudges (M13.4f); D13 stays no
- **work:** cover the PR remote link's drain, retries and sync wiring
- **work:** pin the summary's confirm gate and that no tool is granted back
- **e2e:** usage counts in hub W (work graph M13.2)
- Revert M13.2 (work_admin { usage }) out of the M13.1 PR
- Revert "feat(work-graph): M13.1 partial sync failures are visible"

### Fixed
- **mcp:** keep telling a stale caller the list changed until it re-lists
- **work:** M13.4e review fixes: conformance and isolation rows, outbox retention
- **work:** a summary fork really runs without fleet's hooks (M13.4c)
- **trackers:** build a GitHub Enterprise issue URL on its own instance
- **ui:** say which session a lost start race left unlinked
- **work:** re-check live work at a resume's link and name its orphan
- **events:** refuse a resume across events the ring never recorded
- **release-drift:** decide draft visibility from the listing, not a user-only probe
- **sessions:** stop dead external rows flooding the Outside fleet group

### Documentation
- **work-graph:** M13.5 close-out; the work graph is operating (D26)
- live-instance analysis (six lenses) and four fix plans
- **work-graph:** record the owner's M13.4 decisions (D3, D10, D15, D20 yes)
- **work:** the phone names and renames local work (M13.4a, D20)
- **work:** record M10.5 finished in fleet-mobile (#48)
- **work-graph:** document the usage summary and add it to the acceptance run
- **work-graph:** document the usage summary and add it to the acceptance run
- **work-graph:** document sync skipping items in fleet health and troubleshooting
- **work-graph:** M13.0 roadmap truth pass
- **work-graph:** M13 plan — live use
- **work:** pin M13.1's no-tools flags from the installed claude --help
- **work:** plan M13, the decided-against items the user said yes to
- **work-graph:** M12 plan — ship and operate
- **work-graph:** M11 plan — the long tail
## [0.3.1] - 2026-09-26

A tracker sync fix, and a release pipeline that keeps releases on their tag.
No migrations, so upgrading from v0.3.0 is a plain image or app update.

### Fixed
- **trackers:** before this release, one tracker item that failed to store
  (bad data, a constraint) rolled back its whole batch and aborted the
  tracker's sync pass. The view's watermark then never moved, so the same item
  stalled the tracker on every pass. It could be a single changed item in an
  incremental listing. Now:
  - The item is skipped with a warning and the rest of the batch is stored.
  - The item's view keeps its watermark, so the item is read again on the next
    pass and nothing is lost.
  - Every other view, the linked items and the binding still run.
  - The tracker reports an error only when a whole pass stores nothing.
  - A transaction SQLite itself rolls back (I/O error, full disk) still aborts
    the batch.
- **release:** a published release is kept on its `vX.Y.Z` tag. v0.3.0 went
  out under an `untagged-…` placeholder until it was re-bound by hand.
- **ci:** the hub end-to-end check of `fleet_health`'s version now reads the
  version from `Cargo.toml` rather than assuming `0.2.x`.

### Changed
- **ci:** the work-graph end-to-end leg (hub W) runs in `hub-headless` on
  every PR.

### Documentation
- **work:** the M10.3 manual acceptance run is written down.

## [0.3.0] - 2026-09-26

A faster hub. On a NAS with spinning disks every store write held the one
database lock for ~200 ms, so even read-only MCP calls queued behind it: 0.4–2 s
for `whoami` / `list_hosts`, and ~15 s spikes for `list_sessions` /
`list_worktrees`. Reads now come off the writer, and writes are batched into
single transactions. The WAL change in v0.2.40 fixed the fsync cost; this
release fixes the queueing.

**Upgrading:** this release adds migrations 059 and 060. Back up `state.db`,
together with any `state.db-wal` / `state.db-shm` next to it, before
upgrading. See `docs/RELEASING.md`.

### Added
- **hub:** a read-only connection pool on the WAL database serves the listing
  tools (`list_hosts`, `list_projects`, `list_worktrees`, `whoami`,
  `fleet_health`, and the final read of `list_sessions`) without waiting on
  the writer.
- **hub:** `authorize` checks tokens against an in-memory cache. Migration 060
  adds triggers that bump an auth epoch on every token change except the
  `last_seen_at` stamp, and the epoch is re-read on every request. A revoked,
  rotated or re-scoped client or host token is refused on the very next
  request, even when another process changed it (the `fleet-hub` CLI via
  `docker exec`).
- **release:** releases publish themselves once every asset is verified. The
  hub image is pinned to the release in `deploy/hub/docker-compose.yml`, and
  drift is watched (#311).

### Changed
- **sessions:** `list_sessions` no longer probes every host inline when its
  cache is stale. It answers from the stored rows and refreshes in the
  background; only a cold start or `force` waits for the probe.
- **reconcile:**
  - `claude agents --json` runs at most once a minute per host, as intended;
    before, it ran on every pass.
  - Each host's writes commit in one transaction and unchanged host identity
    is not rewritten.
  - The GC sweep no longer runs inside the tick.
- **hooks:** each hook's writes commit in one transaction, and updates that
  change nothing are skipped.
- **audit:** read-only tools no longer write an audit row on every call.
- **usage / trackers:**
  - Usage collection commits once per host and skips sessions with no new
    usage.
  - Tracker sync commits once per batch and stamps `fetched_at` in one
    statement.
- **store:**
  - Migration 059 indexes live sessions by worktree.
  - `list_worktrees` no longer queries once per worktree.
  - `whoami` looks the session up by its name instead of loading every
    session.

### Fixed
- **store:** a transaction that SQLite aborts midway (I/O error, full disk, out
  of memory) no longer half-commits. The writes after the failure stop
  instead of committing on their own without their events, and a failed
  savepoint release can no longer leave the connection inside an open
  transaction.
- **store:** transactions begin `IMMEDIATE`, so a CLI writer next to the hub
  makes the transaction wait instead of failing at once.
- **usage:** a pass with no new usage no longer clears the usage cursor's last
  message id.
- **mcp:** `fleet_health` still reports a failed database lock when it reads
  through the pool.

## [0.2.42] - 2026-09-26

### Added
- **mcp:** tell paired clients about tool-list changes too
- **mcp:** advertise tools.listChanged and send it on the next call
- **assets:** read-only catalog overview on a hub client
- **work:** filter by the tracker's own status name (QA Review)
- **agent:** resize and maximize the agent sheet
- **ui:** reconnect Attention item for failing trackers (work graph M12.4, D22)
- **health:** tracker roll-up and detection backlog in fleet_health (work graph M12.4)
- **settings:** work retention windows, status and dry run in Settings (M12.3)
- **work_admin:** status and sweep_now for retention (M12.3)
- **work:** retention sweep for journal, done tickets and work events (M12.3)
- **store:** refuse a database a newer build has migrated
- **composer:** the quick-reply chips are the fleet's, not each device's
- **mcp:** get_settings / set_setting, so a hub's settings can be changed
- **mcp:** get_settings / set_setting, so a hub's settings can be changed
- **work-graph:** M11.3 tidy reason idle_unlinked and per-session keep

### Changed
- Revert "docs(work-graph): start the work graph user guide (M12.5, in progress)"
- **e2e:** use a live key in the set_setting scenario
- **mcp:** set BUDGET_BYTES to the M12.3 measurement plus 100
- **routing:** record the two retention refusals in the local-only fixture
- **store:** name the upgrade test's fingerprint type
- **release:** release fleet-mobile under the same version
- **work:** satisfy clippy's is_multiple_of in the scale fixture
- **ui:** use a real ClaudeStatus in the scale fixture
- **store:** prove the upgrade into the work graph on a generated database (M12.1)
- **ui:** group-by-work, rowMatches and the Today view at 2,000 rows
- **work:** seeded scale fixture and budget tests for the work graph
- **store:** index ended work links and the handover guard; key lookup by index

### Fixed
- **composer:** More keeps the chip row open; Send stays right; center the loader
- **hub:** self-heal leftovers of a disabled local host
- **safe-kill:** never remove the main checkout; read a wrapped FAILED echo
- **store:** read the tidy keep without scanning session_events
- **work:** index-backed tidy_kept lookup
- **lint:** collapse the nested if clippy 1.95 now reads as a guard

### Documentation
- **net:** describe the tracker SSRF fence as it is since M6
- **net:** stop promising an acli transport in https.rs
- **work:** restate D15 as multi-start on the phone only
- **work:** revisit the decided-against list (work graph M12.6)
- **hub:** the Assets tab is a read-only overview on a hub client
- **control-api:** name fleet_health's trackers roll-up (work graph M12.4)
- **work-graph:** start the work graph user guide (M12.5, in progress)
- **work:** add the work graph user guide (M12.5)
- **plan:** add the M12 plan with the M12.3 revision
- upgrading into the work graph, and the M12 plan with M12.1's revision
- **plan:** add the M12 plan with the M12.2 scale numbers
## [0.2.41] - 2026-09-26

### Added
- **ui:** Settings → Work connects GitHub Enterprise and shows sync metrics
- **desktop:** tracker_sync_metrics command, LocalOnly like the tracker admin
- **trackers:** GitHub Enterprise Server and per-tracker sync metrics (M11.4)
- **ui:** "Name this work…" dialog, row menu and group header
- **desktop:** Routed commands for local work items
- **work:** name local work items (work graph M11.1)
- **work:** resume planning probes that the transcript is still on the host
- **move:** refuse a cross-org move unless force_cross_org
- **trackers:** a test-only loopback override for the e2e fake tracker
- **work:** show the latest handover outcome and offer Start anyway on a cross-org multi-start

### Changed
- **transcript:** a one-turn window stops costing a megabyte of SSH
- **work:** ignore tracker_sync_metrics in the Data Center re-connect command list
- **e2e:** name local work on hub W (work graph M11.1)
- **mcp:** pay back the tool-description budget (M11.5, M0.6)
- **sidebar:** the past-work header's Resume continues the last conversation, or opens the dialog
- **work:** startWorkMulti takes the started rows into the store, refreshTidy keeps the last report on an error
- **orgs:** the colour picker, the host / tracker assign selects and createFromSuggestion's tracker branch
- **session-row:** Pick another… goes to the key box, Escape closes the work menu
- **work:** TicketCard reload on a key change, TodayView's Refresh / Close / links, ResumeDialog's last mode
- **work:** the hub-blocked state of both review sheets, the tidy refresh tick and the Reopened pill
- **sheets:** navigation from a focused control and a refused focus never claiming ownership
- **org-settings:** the add form, rules, unassign, remove and a failure
- **work-settings:** Test and Remove on a listed tracker
- **resume:** the host override reaches the plan, the brief and the start
- **tidy-review:** reopened toast, partial failure and hub refusal
- **link-review:** a decided row leaves the list and the sheet closes itself
- **quick-switcher:** Ctrl+Enter on a ticket opens what it started
- **new-session:** the E_EXISTS jump asserts the session it lands on
- **http_client:** pin the transport-side half of the moved trust-store assertion and the TLS-handshake prefix
- **settings:** the lifecycle test reads off-default thresholds and reasons
- **peer:** read B's pending outbox before the restarted dialer can drain it
- **trackers:** rustfmt and clippy over the tests of the review fixes
- **e2e:** detection on a session with no work yet, after M4.6
- **gc:** feed the tidy planner offline rows, not only a remote host
- **trackers:** fail the extra_ca TLS test loudly without openssl
- **trackers:** reach the per-host start fence on another host
- **peer:** pin the supervisor's own stop of a revoked link
- **trackers:** prove the 429 wait expires on the same sync
- **isolation:** check row redaction on rows that carry work
- **orgs:** announce org moves in one transaction
- **tidy:** bundle apply_one's caller context
- **e2e:** the work graph end to end in scripts/hub-e2e.sh (M10.2)
- **isolation:** compare the link-id oracle against a live link
- **work:** measure and guard replay-ring pressure under tracker sync
- **mcp:** assert per-caller outcomes for handover and multi-start in the isolation matrix

### Fixed
- **clippy:** collapse the org-scope guard into its match arm
- **review:** six follow-ups from the M5/M6 second-round review
- **safe-kill:** read the marker below the prompt's echo, not the echo
- **orgs:** run the org admin commands off the main thread
- **peer:** drop a useless format! in the marker-defusing test
- **trackers:** a lost start race names the winner only when the caller may see it
- **trackers:** the row's 429 deadline alone decides whether the sync waits
- **trackers:** a Jira key's shape bounds the prefix at 50 chars, not Cloud's 10
- **trackers:** a key over KEY_MAX_CHARS is dropped, not cut to another item's key
- **store:** a removed tracker's rows come last in work_item_by_key and never make a key known
- **via_host:** the curl script also unsets tr, break and continue
- **sheets:** navigation keys still work from a focused control; focus ownership only when set
- **hooks:** a prompt fleet typed is nobody's touch on the hook path either
- **peer:** a re-pair merge sheds the old watermark instead of keeping it
- **sessions:** only a person's prompt touches a session, never fleet's own
- **resume:** key the in-flight resume registry per store
- **mcp:** keep the inbox description inside the served-definition budget
- **net:** unlink the curl header file before curl runs and sweep stale dirs
- **hub:** open state.db read-only for peer list and say a re-pair needs a new code
- **trackers:** a Retry-After from the tracker is capped at an hour
- **github:** the issue URL is built from the validated repository and number
- **trackers:** a tracker key is kept only in a key's shape
- **jira_dc:** a URL is this site's only with the exact host and context path
- **github:** a RATE_LIMITED GraphQL error waits until the reset
- **trackers:** the key in a brief and a start prompt is tracker text
- **net:** refuse a truncated body in parse_response instead of returning it
- **store:** migration 057 re-issues the read_cursors session-delete trigger
- **trackers:** a lookup honours the sync's Retry-After back-off
- **trackers:** bound every short tracker field before it is stored
- **trackers:** a start re-checks for a live session before it links
- **net:** let the curl transport report an over-cap body as TooLarge
- **peer:** defuse every fleet marker in a peer body, not only the listed prefixes
- **trackers:** refuse a via_host / via_cli transport on a fleet-agent host
- **trackers:** clamp a 429's wait and add it without overflow
- **net:** anchor the via-host parsers on a start marker, not byte 0
- **trackers:** a lookup by URL reads the cache of that URL's tracker
- **mcp:** flag peer-originated inbox summaries as untrusted
- **jira:** a key inside another site's URL is not recognised as ours
- **asana:** a failed batch action is a batch error, never a missing task
- **jira_dc:** a moved key is found next to other references in a fetch
- **jira:** a moved key is found next to other references in a bulkfetch
- **store:** a removed tracker's rows no longer shadow the re-added one
- **trackers:** write a view's sync mark only after its items are stored
- **trackers:** a successful test re-enables the views a 403 disabled
- **trackers:** a bare link from another org is not live work on a ticket
- **mcp:** bind a fresh_for reader to the caller's host before touching its cursor
- **ssh:** run a stdin-fed command on `local` locally, never `ssh local`
- **hooks:** stamp a handover delivered on SessionStart only for the sync curl form
- **linear:** a missing issue in a batched fetch no longer fails the chunk
- **orgs:** remove_org unassigns the org's past links, in one transaction
- **sessions:** never dismiss a restorable lost session on a rename
- **move:** warn when a carried work link crosses the org boundary
- **tidy:** a tree a live review uses is only ever plain-killed
- **resume:** refuse a concurrent resume of the same key
- **tidy:** honour a snooze or never on any live confirmed link
- **sessions:** link a new session to the worktree it was started in
- **work:** percent-decode ticket URLs all or nothing, like the TS twin
- **sessions:** stamp last_prompt before the tmux send, roll back on failure
- **work:** match a carried or merged link by item under either spelling
- **work:** snapshot the branch, not a closing ref, when R7 ends a link
- **work:** key detection candidates by the item's current key
- **work:** a promoted suggestion takes the promoting signal's source
- **work:** keep the session's primary when a cross-org candidate is skipped
- **store:** create state.db owner-only before the open so the WAL sidecars inherit 0600
- **net:** refuse a curl answer whose exit is non-zero even with a status
- **resume:** a blank edited brief is no brief, and a brief never fails the resume after the spawn
- **work:** refresh session rows that show a tracker item as a suggestion or rejection
- **trackers:** first-sync needs a known tracker; list loads never overwrite newer frames
- **session-focus:** never focus a session the store does not have
- **work:** check the primary link's visibility for snooze, never and archive
- **tidy:** never write gc_failed to a session outside the caller's scope
- **sheets:** Escape closes the tidy and link-review sheets from any focused control
- **today:** a hub without work_today is the plain empty state, not an error
- **work-settings:** forget the token and email on Cancel and failure
- **new-session:** multi-repo start carries the edited brief and name
- **sidebar:** a focused session is never hidden under a work group's Done
- **work-settings:** re-connecting a site keeps its transport and settings
- **trackers:** retro-link reveal skips GitHub and Asana keys before taking a prefix
- **link-review:** sheet chords ignore keydowns from focused buttons
- **quick-switcher:** place a ticket by its key family, not the text before the first dash
- **tidy:** sheet chords ignore keydowns from focused controls
- **work:** send session:updated for a tracker item only when the row shows the change
- **work:** bind a handover request to its turn and keep the markers out of the timeline
- **work:** settle the multi-repo start's leftovers (M10.1)
- **pane_intel:** OOM text above a live REPL is scrollback, not a crash

### Documentation
- M11 plan with the M11.4 revision line; regenerate the control API reference
- **plan:** add the M11 plan with the M11.5 revision
- **plan:** add the M11 plan with the M11.1 revision
- **review:** record the second-round review, the test audits and the workspace-wide validation
- **claude-md:** name migration 057 next to the classify-nudge note
- **review:** record the fix status, the deliberate non-changes and the final validation
- **hub:** say why hosts are assigned before a second tracker
- **review:** deep code review of the 48 hours ending 2026-09-25
- **work:** record the M10.6 replay-ring pressure numbers and decision
- **store:** put list_session_events' doc comment back on it
## [0.2.40] - 2026-09-25

### Added
- **ui:** a spiral loader for loading and working states
- **phone:** org_id in the phone session view; plan M8.6
- **sidebar:** chips for the work filters (tracker, status, mine, session, archived)
- **tidy:** Today's Stale opens the Tidy-up sheet narrowed to those sessions
- **work:** opt-in classification nudge and agent_inferred suggestions (M4.6)
- **work-graph:** M9 follow-ups on M7 — Today opens Tidy up; the operator's Tidy up done tickets

### Changed
- **ui:** the spiral loader animates per frame and holds still when paused
- **sidebar:** re-read the mine view only when the mine chip toggles
- **scripts:** measure-session-start.sh, the remote SessionStart procedure
- **peer:** wait for the parked call's drop before B sends
- **tidy:** drop the duplicate svelte/store import the main merge left in TidyReview.test.ts
- **store:** open the file store in WAL with synchronous=NORMAL

### Fixed
- **work:** link suggestions no longer look stuck after a decision
- **tidy:** Enter on the sheet's Show all widens instead of applying
- **new-session:** forget ticked sibling repos when the ticket changes
- **mcp:** bind every argument into operator confirmations and gate every start
- **work:** ask for a handover only from an idle Claude REPL

### Documentation
- **work:** M8.6 on the phone — hub.md, roadmap, plan revisions
- **work-graph:** M10.4 status, D14-D16, M4.6 decided against
- **work-graph:** M10 plan — settle, prove, and reach the phone
## [0.2.39] - 2026-09-25

### Added
- **sidebar:** clicking a suggestion shows only its session
## [0.2.38] - 2026-09-25

### Added
- **work-graph:** M9.6 multi-repo start
- **work-graph:** M9.3 agent-written handover, on demand
- **work-graph:** M9.7 the operator's starts and kills always need approval
- **work-graph:** M9.2 ticket context card in Details
- **work-graph:** M9.1 Today view and Copy standup
- **ui:** connect any tracker, provider badges, the Asana section map; docs (M6.6)
- **trackers:** Jira Data Center, with an admin-fenced site (M6.5)
- **trackers:** Linear (M6.4)
- **trackers:** via_host transport — curl on a host, the credential on stdin (M6.3)
- **ui:** org scope selector, needs-you across scopes, Organisations settings; one rowMatches (M5.4, M5.5)
- **trackers:** GitHub Issues through gh and Asana (M6.1, M6.2)
- **work:** the org boundary for per-host tokens (M5.3)
- **work:** Tidy up and Reopened in the UI, archived work in Done (M7.3)
- **work:** tidy-up storage and API, archive, snooze, reopen, auto-tidy (M7.2)
- **work:** phone view carries work_suggested; work/work_link action enums (M8.0)
- **trackers:** provider conformance suite and trait refinements (M6.0)
- **work:** organisations — schema, resolution, scopes and admin (M5.1, M5.2)
- **work:** the pure tidy-up planner, plan_tidy (M7.1)
- **work:** SessionStart work context behind work.session_start_context, off (M4.5)
- **work:** explanations and one-key correction for detected links (M4.4)
- **work:** detection signals and the pure link resolver (M4.2, M4.3)
- **work:** one ticket recogniser in Rust and TypeScript over a shared fixture (M4.1)
- **ui:** tickets in ⌘K, start work from a ticket, status on chips, Settings → Work (M3.5)
- **work:** tickets, lookup and start work on a ticket (M3.4)
- **trackers:** the sync tick, retro-binding and events on real change only (M3.3)
- **work_admin:** trackers admin over MCP, the hub CLI and LocalOnly desktop commands (M3.1)
- **trackers:** the provider trait and a read-only Jira Cloud adapter (M3.2)
- **store:** trackers, their secrets and views; tracker columns on work items (M3.1)
- **sidebar:** past work under Done, past-only work groups, Resume with a brief preview (M2.5)
- **work:** a work read with neither session nor key lists recently ended links (M2.5)
- **work:** resume past work over MCP and Tauri — plan, continue, fresh with brief (M2.4)
- **hub:** peer add/list/remove, list_peer_links, health and outbox sweep
- **work:** deterministic handover brief, delivered through additionalContext ahead of the inbox (M2.3)
- **work:** links carry across resume, fork, review and task workers; purge marks lost conversations (M2.2)
- **hosts:** E_PROBE carries the classified ssh failure
- **work:** work journal — conversations, progress and compaction summaries outlive their sessions (M2.1)
- **peer:** the dialer loop, its supervisor, and the two-hub proof
- **peer:** the listener side and the peer_exchange tool
- **sidebar:** a row's work comes from its link first; set work, "Not this", clear (M1b.2)
- **work:** desktop work-link commands routed to the hub; rows name their rejected keys (M1b.2)
- **messages:** a linked foreign address queues to the hub outbox
- **work:** a session row carries its primary work; `work` and `work_link` MCP tools (M1b.2)
- **peer:** wire types, inbound checks and dialer backoff
- **messages:** a remote end reads back as its address
- **store:** work items and session↔work links that survive moves and kills (M1b.1)
- **store:** peer links, remote participants and the hub outbox
- **auth:** a peer token mode that reaches peer_exchange only
- **new-session:** name the work, warn on a duplicate, accept a pasted ticket URL (M1a)
- **sidebar:** group sessions by work key, with no tracker and no setup (M1a)
- **store:** every session row gets its participant on insert (migration 045)
- **ssh_diag:** one classifier for ssh client failures

### Changed
- **net:** lift the hub client's TLS/HTTP/1.1 stack into fleet-core (M3.0)
- **store:** migration 045 on a populated v44 database (M9)
- **e2e:** two hubs linked end to end; docs for hub links
- **ssh_diag:** rename SshFailure.host_alias to ssh_alias
- **account_usage:** cover the OpenSSH 9.x auth-denied prefix as a connect failure
- **ssh:** mux and usage-fallback checks read the shared classifier
- **http:** move the hub HTTP client into fleet-core

### Fixed
- **peer:** a re-pair racing the old loop waits instead of stranding the link
- **hub:** CLI + docs/spec review fixes (G6, G15, G21, G22, G25b, G26b/e)
- **peer:** dialer review fixes (G3, G7, G14b, G20, G26c, G26d)
- **messages:** G2b -- a reply across a link must involve the recipient itself
- **work:** a closing ref no tracker resolves is never auto-linked (R3u)
- **peer:** inbound review fixes for the listener and apply path (G9, G11, G12, G13, G8a, G25a, G6 peer items)
- **mcp:** G2 correction -- restore the sender-participation check in send_remote
- **mcp:** federation review fixes for the tool layer and send_remote (G1, G2, G4, G19, G8b, G25c, G26a, G6 parse_client)
- **gc:** warn when the peer outbox sweep fails (G24)
- **health:** peer_links_down uses the new store roll-up (G14c)
- **store:** peer_links store-layer review fixes (G5, G10, G14a/c, G16, G18, G23, G24)
- **store:** index participants.peer_link_id (G17)
- **trackers:** ticket text cannot escape the untrusted fence (M3 review)
- **peer:** bound what a peer's words and addresses can do to a recipient
- **peer:** a new link cannot take over a live one; check the peer's fleet id; cap pages by size
- **e2e:** redact tokens in failure output; strengthen the untrusted-marker check
- **conversation:** keep working indicator and composer note mounted to stop layout jumps
- **ssh_diag:** SshFailureKind::Unknown is forward-compatible on deserialize
- **peer:** a stale dialer loop cannot write over a re-paired row; a store fault is retried, not rejected
- **peer:** a peer's kind is a short [a-z0-9_-] token
- **peer:** a newer exchange releases a parked one on the same link
- **peer:** one fresh wake per recipient; a peer's refusal is one attributed line
- **peer:** a link is bound to one live token and settles only its own rows
- **store:** cover the peer_links live-fleet unique-constraint mapping
- **move:** a moved session keeps its tags
- **sessions:** a rename keeps the session's row, participant and history
- **mcp:** phone view keeps tags so a phone tag edit no longer wipes the rest

### Documentation
- **work-graph:** M9.3, M9.6, M9.7 landed — roadmap, plan revisions, CLAUDE.md
- **work-graph:** record decisions D3 and D9-D13 for M9
- **work-graph:** M9.1/M9.2 landed — roadmap status, plan revisions, control API
- **work-graph:** M9 plan — Today view, ticket card, and the decision-gated rest
- **work-graph:** M6 landed — roadmap status, plan revisions, CLAUDE.md
- **work-graph:** M8.1–M8.5 merged in fleet-mobile (#32); correct the M8 status
- **work-graph:** M8.2 landed in fleet-mobile — plan revisions and roadmap status
- **work-graph:** M8.1 landed in fleet-mobile — plan revisions and roadmap status
- **work-graph:** record the M5 ticket-fence decision (own host and own org)
- **work-graph:** M5 landed — organisations and isolation in hub.md, concepts and control API; roadmap, plan revisions (M5.6)
- **work-graph:** M7 landed — lifecycle concepts, tidy-up settings, control API, roadmap and plan revisions (M7.4)
- **work-graph:** M8 plan — the phone (tool-gated work chips, grouping, start and resume)
- **work-graph:** M4 landed — rule table and chip vocabulary, control API, roadmap and plan revisions (M4.7)
- **work-graph:** M3 landed — trackers in hub.md and concepts, roadmap and plan revisions (M3.6)
- **peer:** re-pair merges only into a stopped link; byte-capped pages, address cap, no question; as-built rulings
- **work-graph:** M7 plan — self-cleaning lifecycle
- **work-graph:** M2 landed — journal, carry, handover, resume; control API, skill and plan revisions (M2.6)
- **work-graph:** M6 plan — more providers
- **work-graph:** M5 plan — organisations and isolation
- **work-graph:** M4 plan — smarter detection and explanations
- **work-graph:** M1b.2 landed; M1b.1 verified
- **work-graph:** M3 plan — tracker foundation and Jira Cloud (read-only)
- **work-graph:** M2 brief delivery via a handover journal row (from_session_id is NOT NULL)
- **work-graph:** M2 plan — resume and work memory
- **work-graph:** M1a done, M1b.1 written, M1b.2 scoped
- **work-graph:** record M0 status; the work graph schema moves to migration 046
- **skill:** a readonly token's E_FORBIDDEN from whoami also skips the label
- **skill:** friendly-name skill finds its row with whoami, not list_sessions
- **work-graph:** specialist review, design rev 2, roadmap, takeover prompt v2
- **spec:** work graph discovery and design proposal
- **plan:** hub-to-hub federation implementation plan
- **spec:** hub-to-hub federation design
- **plan:** SSH self-diagnosis PR 1 (classifier); spec adds Handshake kind
- **spec:** SSH self-diagnosis design
## [0.2.37] - 2026-09-24

### Added
- **gc:** sweep orphan read cursors; document fresh_for
- **mcp:** repo_diff and list_sessions answer unchanged
- **hub:** the phone keeps the hub's needs_attention, and its view covers the pager
- **mcp:** session_history and inbox page only what is new
- **mcp:** session_transcript answers only what is new
- **mcp:** fresh_for on five fetch tools (inert), one budget raise
- **service:** the pure fresh-read decision
- **store:** id-ordered stream reads after a watermark, and the conversation generation
- **store:** migration 044 — remembered read cursors
- **mcp:** the phone view carries the answer, not the columns to re-derive it
- **mcp:** the phone view carries the dialog a blocked session is waiting on
- **hub:** the fleet knows which sessions need a person

### Changed
- **fakes:** route every exec'd stub through fake_exec::write_exec
- **mcp:** list_projects can answer only the projects a session names
- **mcp:** let a pager ask list_sessions for the columns it draws

### Fixed
- **mcp:** no endless more loop for an unknown reader or an unanchorable turn
- **store:** a session's read cursors die with its row
- **mcp:** row-order-stable list_sessions unchanged, honest snapshot hashing
- **mcp:** grown-tier search direction, and a spurious reset after an empty-only read
- **mcp:** four session_transcript/history/inbox correctness gaps (round 2)
- **mcp:** close three history/inbox fresh_for correctness gaps
- **mcp:** position session_transcript's fresh_for delta by anchor, not turn_seq

### Documentation
- **cursors:** control-api and spec match the build
- **mcp:** keep ok_json_compact_view's contract on ok_json_compact_view
- implementation plan for remembered read cursors
- smart caching design — remembered read cursors (cycle 2 of 3)
## [0.2.36] - 2026-09-23

### Added
- **hub:** /metrics, so what a client costs can be read rather than guessed
- **events:** a reconnect costs what it missed, and a phone can ask for the columns it draws
- **gc:** sweep retired participants and tell senders what was never read
- **messages:** wake an idle recipient, never a blocked one
- **messages:** address-addressed send, idempotency, wait_for_reply tool
- **messages:** event-driven wait_for_reply on a store notify
- **hook:** block a Stop for a question, capped at three in a row
- **hook:** answer 200 with additionalContext when a message is pending
- **hooks:** look up and stamp the delivery a hook response carries
- **store:** undelivered-message query and delivered_at stamping
- **service:** pack pending messages into a hook additionalContext
- **service:** mint a stable fleet id and report it from whoami
- **ui:** answer Claude's dialog from the app instead of the terminal
- **service:** fleet address parse and render
- **store:** participant identity with re-point and tombstone
- **store:** migration 043 — participants, delivery columns, block streak
- **release:** publish a complete, versioned, fully checksummed release
- **release:** declare every release asset and build leg in one manifest
- **release:** gate every release leg on the carrier and tag check
- **ci:** fail CI when the six version carriers disagree
- **release:** print the version carriers with release.sh --list
- **ui:** find and resume lost Claude conversations from HostDetail
- **mcp:** discover_lost_sessions
- **sessions:** parse and rank Claude transcripts for lost-session discovery
- **sessions:** new_session can resume a conversation; never reuse a lost session's name
- **ui:** restore a host's lost sessions from HostDetail
- **mcp:** restore_host_sessions tool and command
- **sessions:** restore_host_sessions service
- **settings:** restore.batch_size and restore.stagger_ms
- **sessions:** expose lost_reason on the session row

### Changed
- **conversation:** a caller that draws no timeline can say so
- **hub:** ask SQLite about one client instead of reading the table
- **events:** a probe that found nothing new says so in three fields
- **conversation:** a caller that says where it got to gets only what it missed
- regenerate reference, verdicts and contract; correct gc docs
- **sessions:** the row fixture carries askRestart, new on main
- allow ci.yml to be started manually
- **ci:** smoke-test the release asset manifest on every PR
- **catalog:** stop the identity tests reading whoever runs them
- **ci:** mirror the version-consistency job in ci-local.sh
- **ui:** strengthen the bg/external/no-id restore filter test
- **sessions:** exercise plan_cwd's worktree and local base_path branches

### Fixed
- **mcp:** cut since_turn's text to a clause, and pay for the field on purpose
- **ui:** categorise the two Stop-block timeline kinds
- **participants:** validate both ends, reply by identity, roll back a failed move
- **delivery:** pack the Stop block reason to its own budget
- **store:** read the inbox by participant and run retention unconditionally
- **store:** a kill tombstones the participant; a move keeps its inbox
- **messages:** namespace the dedupe id and never wake a stuck session
- **review:** round-20 findings across tests, transcript, contract and UI
- **delivery:** stub an oversized message so it cannot stall the queue
- **hooks:** guard the streak reset and decide a Stop in one lock window
- **hooks:** deliver only to the conversation the row actually holds
- **delivery:** count the block joiner exactly so the context budget holds
- **address:** a session or client name may contain a slash
- **health:** assert the schema version against LATEST_SCHEMA_VERSION
- **logging:** drop rmcp's client-hung-up ERROR instead of filing it as ours
- **agent:** the sheet sends through the composer that owns its live state
- **ux:** stop junk session names, guard destructive one-click actions
- **conversations:** stop printing harness XML at the reader
- **release:** make `release-assets.sh assets` keep its sorted/deduped contract
- **release:** never lose SHA256SUMS to a missing per-target sums dir
- **core:** require app_version::set, drop the 0.1.0 fallback
- **ui:** no Resume on a paired desktop; no Restore for an all-skip plan
- **restore:** skip a lost fleet controller; clarify resume/restore docs
- **hub-client:** give restore/discover hub calls a 310s deadline
- **restore:** one restore per host at a time; skip rows no longer lost
- **discover:** offer Resume only when new_session starts in the transcript's exact cwd
- **ui:** convert seconds-based now prop to ms before timeAgo in discover-list
- **sessions:** reject a resume id on shell sessions or one already held on the host

### Documentation
- record two as-built deviations in the cycle 1 design
- correct the plan's stale re-delivery statements
- **review:** code review round 20 — post-v0.2.35 wave, with resolutions
- **claude-md:** the reboot paragraph describes both halves now
- implementation plan for fleet mesh addressing and delivery
- fleet mesh addressing and delivery design (cycle 1 of 3)
- **readme:** claim only what verify-release can enforce today
- **release:** describe the asset set, the gate, and how to verify a download
- **hub:** spell out that declaring the app version is mandatory
- **plans:** release-process + version-sync audit across claude-fleet and property-management
- **plans:** host-reboot recovery (PR 2/2)
## [0.2.35] - 2026-09-22

### Added
- **hub-cli:** fleet-hub reports reads the error channel
- **ui:** report frontend crashes and error toasts to the hub error channel
- **desktop:** report_client_error queues frontend errors for the hub
- **ui:** a pending transfer shows in the sheet and the chip, with Cancel
- **ui:** the run store waits, cancels, and remembers a wait across a reopen
- **desktop:** flush error reports to the hub in hub-client mode
- **agent:** report error-level events to the hub on the heartbeat
- **hub:** age-sweep error reports and drain the hub's own ring on the tick
- **ui:** a transfer may answer "waiting", and a wait can be cancelled
- **hub:** store an agent's Report frames under its host
- **hub:** POST /report and GET /reports behind the bearer layer
- **move:** a restart closes the waits it can no longer honour
- **move:** when reaches the hub only once the hub is known to understand it
- **core:** ingest error reports with clamp, redaction, rate limit and retention settings
- **conversation:** previous and next turn with [ and ]
- **conversation:** remember the scroll position per session
- **sidebar:** search matches tags
- **core:** ReportLayer captures error events into the process ring
- **store:** error_reports table with row and age pruning
- **move:** when=idle waits for the source, when=cancel ends the wait
- **hosts:** one click from a host to its sessions, and the overlay closes
- **proto:** AgentFrame::Report carries an agent's error batch
- **proto:** error report record, batch and bounded ring
- **move:** a bounded, cancellable wait for the source to go idle, and a sweep for waits a restart lost
- **store:** find the waits no later event has closed, and a setting to bound them
- **intel:** the row carries the dialog's numbered options as pending_input, so a client can answer with a tap
- **mcp:** send_prompt refuses blocked sessions, reports queued/acked, dedupes by client_msg_id
- **mcp:** send_prompt can press Enter, Escape or C-c, so a phone can answer a dialog without typing
- **store:** row_version per session and a prompt-submit counter for delivery acks
- **ui:** the Transfer sheet shows what would travel before you press it
- **ui:** the newest preview per session and host, debounced, never overwritten by a late one
- **ui:** a preview and a move each arrive as exactly what they are
- **move:** dry_run reaches the engine from the desktop, the tool and the hub
- **move:** a read-only preview built from the move's own checks
- **move:** read-only probes for what the target already holds

### Changed
- **events:** stop announcing what has not changed, and reads nobody asked about
- **provision:** wait on a spawner signal, not a fixed sleep, in the reestablish_tunnels tests
- **ssh:** resolve each host's login PATH once; tmux calls run under sh -c and panes inherit it
- **transcript:** make ConvItem's second owner impossible to forget
- **mcp:** the budget holds main's send_prompt keys and 3b's dry_run together
- **desktop:** find the queued frontend report by tag, not by position
- **search:** the session fixture carries pending_input from the merged hub contract
- **hub:** /mcp/json, so a phone's answers can be compressed
- **events:** the stream stops sending the word "null" to every client
- **reconcile:** one delimited probe script per host instead of 5 + N ssh calls
- **conversation:** the html gate imports the parser and scans every hub-text component
- **conversation:** a gate that hub text never reaches {@html}
- **prompt:** one tmux dispatch path for text and keys
- **ui:** pin that only a moved outcome's target reaches the sessions store
- **move:** hold gather()'s result alive so a leaked claim would fail the seam test
- **move:** the opening checks become gather(), shared with the preview

### Fixed
- **sessions:** clone into a temp dir and move on success; clamp capture scrollback
- **ssh:** the terminal attach gets its own ControlMaster and keepalive; the tunnel bounds its connect
- **ssh:** check the master before resetting it, and require ssh's own broken-pipe wording
- **ssh:** reset the ControlMaster and retry once when it dies under a command
- **hub:** mark a report truncated when redaction loses its context
- **hub:** rate-limit an empty report batch like a one-report one
- **agent:** batch report frames by bytes so the hub never refuses one
- **transfer-sheet:** true wait-end copy, and a way out of a stale wait
- **moves:** a waiting run never sticks, and survives a busy-again attempt
- **transfer:** plain words for an unconfirmed hub contract and an existing wait
- **reconcile:** escape the sessions section and make the hook-rebind race test bite
- **hub-client:** the unconfirmed-contract refusal names the move hazard, not a preview
- **hub-client:** leaving Connected withdraws the confirmed hub contract
- **hub-cli:** percent-encode the origin filter and read a full reports page
- **move:** when: idle waits when a stale idle source is found busy, and the public branch is tested
- **move:** a waiter mid-move is no longer cancellable, and a dropped waiter records its end
- **move:** a wait's deadline is wall-clock, checked between bounded poll slices
- **ui:** write the dedupe separator as an escape and mark seen only on send
- **ui:** a wait's refusal reaches the sheet, not just its bare reason
- **reconcile:** a skipped agents pass no longer lets the pane overwrite the stored status
- **desktop:** discard deterministically refused report batches and cap the body
- **reconcile:** an unanswerable claude-agents call never prunes; agents asked on a 60 s cadence
- **ui:** moves.ts keeps the pre-idle when: now behaviour until Task 7's real wait
- **operator:** pre-trust the operator directory at birth
- **views:** scroll memory anchors on the turn, remembers on scroll, no phantom new-count; one host→sessions path; a real {@html} gate
- **send:** refuse an empty prompt with submit:false instead of skipping the gate
- **conversation:** a restore that finds no row keeps the view pinned, stepper buttons disable at the ends
- **send:** outcome-unknown only for mutations, bare Enter past the gate, in-flight dedupe
- **sidebar:** a remounted sidebar does not replay an old reveal
- **sidebar:** every click that opens a session reveals it, and a reveal can never be replayed
- **sidebar:** only an explicit selection widens the host filter
- **hub:** pending_input clears on every turn boundary, keys work in local mode too, options bounded and capped
- **hub-client:** client timeouts follow the hub's deadlines; connect timeout, offline breaker, E_HUB_TIMEOUT
- **intel:** dialog options survive description lines between choices
- **ui:** rebuild loadSessions in list order, not store order
- **ui:** order optimistic merges by row_version and subscribe to row events before the first list
- **intel:** pending_input options stop at the dialog, and clear wherever the activity is reset
- **sessions:** new_session returns the row as of its last write
- **mcp:** don't fail send_prompt on a failed Enter retry; skip the retry once the turn has started
- **prompt:** gate the empty-body Enter on submit; clean up the buffer on a failed paste
- **prompt:** deliver through load-buffer/paste-buffer to the known pane; normalise CR and refuse control bytes
- **tmux:** send_named_key targets the exact session, like every other builder
- **hub:** refuse a move preview until this launch has confirmed the hub's contract
- **move:** the target probe never reports an enclosing repository's state as the target's
- **move:** a dry run's source inspection never fetches and takes no optional locks
- **mcp:** trim this branch's tool wording back under the merged surface budget
- **ui:** seed preflight test entries through preflight.ts, drop the raw NUL key copy
- **ui:** escape the preflight key separator instead of a raw NUL byte
- **move:** bump the wire contract for MoveOutcome/dry_run, name it in the tool, and prove Preview round-trips
- **move:** preview honours strict, wraps target-$HOME like the move, and widens the writes-nothing guard

### Documentation
- toolchain resolve, ControlMaster retry, scrollback clamp; phase 2a landed
- **ux:** audit of v0.2.33 in hub-client mode, iterations 1–10 and two consolidations
- **ssh:** fix three doc comments left describing the pre-task shared-ControlMaster PTY design
- **transfer:** the roadmap records 3b in review, 3c built, and 3c's debts
- **hub:** the standalone tick's own rows, origin `master`, and who edits the bounds
- **desktop:** say what `transport()` actually shares
- **hub:** the error channel — reports, bounds, privacy, the client contract
- **mcp:** move_session says it can answer a wait
- phase 2a plan (SSH path O(1) per tick, second chances)
- send_prompt contract for gated, acked, deduped delivery; phase 1 landed
- the views/filters/scrolling analysis and the desktop plan for it
- **plan:** hub error channel implementation plan
- **spec:** hub error channel design
- **transfer:** slice 3c implementation plan
- **transfer:** 3c spec names the cancel variant and the second sanctioned preview divergence
- **transfer:** slice 3c design — transfer when the session finishes, waiting on the hub
- plan for the pager's hub contract — send_prompt keys and pending_input on the row
- device communication analysis and phase 1 plan
- **transfer:** the preview spec records the fetch-free dry run and the backend guard
- move_session's dry_run and tagged result, and upgrading a desktop and hub together
- **transfer:** a dry run honours strict, which is read-only, and explains clean_target
- **transfer:** a failed target probe is unknown, never a refusal
- **transfer:** slice 3b implementation plan, and five spec revisions found planning it
- **transfer:** slice 3b design — a read-only dry run that cannot drift from the move
## [0.2.34] - 2026-09-21

### Fixed
- **sessions:** a system project is never cloned or repaired, on any host
## [0.2.33] - 2026-09-21

### Documentation
- **mcp:** list no_host in the operator_status description
## [0.2.32] - 2026-09-21

### Added
- **operator:** home the UX agent on a configurable fleet host

### Fixed
- **move:** keep the index mtime on the snapshot's index copies
- **tmux:** launch Claude even when the user's `cl` wrapper is not on PATH
## [0.2.31] - 2026-09-21

### Added
- **hub:** a paired client the operator trusts delivers its prompts unmarked

## [0.2.30] - 2026-09-21

### Fixed
- **hub-client:** attachments work from a paired desktop, as the terminal drop already did
- **operator:** a fleet with no local host says so, instead of offering a dead button
## [0.2.29] - 2026-09-21

### Added
- **hub:** demo-seed, so a freshly paired client has something to draw

### Changed
- **hub:** rustfmt, and move demo_seed above the test module

### Fixed
- **terminal:** an agent host is attached and only then explained
## [0.2.28] - 2026-09-21

### Added
- **ui:** the details panel offers the return trip and a partial's recovery
- **hub:** a paired desktop attaches its own terminal
- **ui:** the Transfer sheet can retry, clean up, come back, finish and undo
- **ui:** the run store can retry a failed transfer and resolve a partial
- **composer:** send a prompt with its attachments
- **ui:** a session's own timeline says where it came from and what is unresolved
- **ui:** a dirty target says whose work it is holding, and what can be done about it
- **move:** resolve_move reaches the desktop and the hub, with its generated docs
- **ui:** mount the agent FAB and panel over every view
- **composer:** attach files, with thumbnails in the box
- **ui:** the agent panel
- **composer:** the attachment list, with its limits and its wording
- **ui:** the agent FAB
- **ui:** the agent's store — open, wake, and say why not
- **ui:** ⌘E opens the agent
- **ui:** the agent's context chip, as a pure function
- **upload:** attachments land inside the worktree, excluded untracked
- **commands:** ensure_operator and operator_status, both routed
- **upload:** inline previews for attached images
- **move:** resolve_move finishes or undoes a partial transfer
- **mcp:** ensure_operator and operator_status
- **operator:** say why the agent cannot work, rather than letting it apologise
- **upload:** a file picker that authorises its own result
- **operator:** bring the UX agent's session into being, idempotently
- **composer:** the transcript holds still when the box grows
- **move:** a partial move records the facts a later finish or undo needs
- **operator:** the UX agent's identity, and the rule that it may not act on itself
- **composer:** a frozen session outranks the send button
- **composer:** chips hold one row, More opens the rest
- **bg:** a background session records the session that asked for it
- **store:** a system project flag for fleet-internal working directories
- **composer:** one shell, with send inside it
- **ui:** switch between a session's background agents, tasks and sessions
- **move:** clean_target replaces an unfinished attempt's leftovers, never the target's own work
- **ui:** a detail view for one piece of background work
- **ui:** derive a session's background work from its turns and fleet rows
- **ui:** a task notification reads as an event row, not as XML
- **ui:** a control token layer and four primitives
- **transcript:** a background agent's block shows its report, not its launch ack
- **transcript:** parse task notifications instead of printing their XML
- **move:** adopt a target that already holds exactly the work being carried
- **move:** a content-exact verifier for a dirty target, and a scoped rollback
- **ui:** the details panel and the app open the one Transfer sheet
- **ui:** Transfer chip on the terminal header's host name, live while moving
- **ui:** Transfer sheet — setup, live steps, a readable result and failure
- **ui:** move eligibility in one place, and move errors in words
- **ui:** moves store — one run per moving session, fed by events and the result
- **move:** report the nine steps of a move as move:progress
- **events:** move:progress — the nine steps of a move on the event bus

### Changed
- ignore the session worktree tree and local MCP wiring
- bump to 0.2.27
- **ui:** hold the palette to app.css and make weak assertions fail
- **agent:** the offline check bounds the budget, not the scheduler
- ignore the per-worktree cargo target dir
- **move:** the final source step becomes finalise_source, shared with recovery
- **ui:** every shared control onto the primitives
- **ui:** one Send, and a blocking reason you can reach
- **conversation:** one inset for the bar, the turns and the box
- **ui:** bordered pill means clickable, everywhere
- **conversation:** one sticky bar, and turns survive find
- **theme:** assert the contrast floors app.css claims in comments
- **hub-client:** the moved call timeout is a real bound, not just a table

### Fixed
- **hub-client:** the automatic workspace check is skipped, not refused
- **ui:** the run store and the sheet stop losing, coercing and offering the wrong things
- **ui:** the switcher colours what is running, and the notification row is written once
- **ui:** a running background agent says so in the thread, not only the switcher
- **move:** refuse a target already running this conversation, and stop the engine overclaiming
- **ui:** a background detail with an empty report still says so
- **conv:** an unreadable status is not a failed background task
- **hub:** a project row from a hub older than 038 still parses
- **move:** the git step names the dirty files it carries, not just commits
- **mcp:** clean_target reaches the engine from the tool and the hub
- **mcp:** new_bg_session gates its requester, and says the field exists
- **hooks:** make .githooks the hooks path without losing the local guard
- **hooks:** key CARGO_TARGET_DIR to the worktree the commit is in
- **ui:** a Finish/Undo refusal falls back to a toast once its sheet is gone
- **ui:** carry a Finish/Undo refusal on the run, not a toast; name the true cleanup total
- **composer:** drop the attachment tray when the session changes
- **attachments:** let a spent tile be re-attached, and close the remote quoting gap
- **agent:** a joiner opens the panel too — closing mid-birth wedged the button
- **attachments:** keep un-uploadable tiles, flag spent ones, and bound quoted prompt size
- **agent:** the panel can be closed — toggle, close button, Escape
- **agent:** read the operator's LIVE row, not the snapshot the panel opened with
- **projects:** actually hide the system project from the pickers
- **ui:** close the retry race, guard resolveMoveRun to a partial
- **attachments:** gate the drop on visibility and bound uploads in Rust
- **operator:** serialise ensure_operator, commit the token after the host has it
- **operator:** guard rename_session and recreate_session; argue restart's exemption
- **transcript:** a background call's newest report decides whether it failed
- **upload:** dropped attachments obey the same size limits as picked ones
- **ui:** anchor the agent-fab hint to the actual button
- **ui:** keep the draft on a failed send; the dropped chip is per-context
- **composer:** dropped paths come from Tauri's drag-drop event
- **ui:** one composer, not two, over the operator session
- **upload:** resolve the exclude file through git, not a linked worktree's .git path
- **ui:** ⌘E must not fall through to Settings
- **upload:** tighten PICKED_ALLOW_TTL to 4 hours
- **move:** resolve_move refuses an identity mismatch, keeps undo's kill best-effort
- **upload:** give picked attachments their own allow-list TTL
- **operator:** check the control API before the operator reference
- **ui:** the background switcher heads its two groups and sorts each of them
- **operator:** hand the agent's token to .mcp.json, and refuse before rotating it
- **broadcast:** never fan a prompt into the UX agent's own session
- **conv:** a background entry is keyed by the call that launched it
- **composer:** split .btn--chip from .btn--toggle; re-measure overflow on preset change
- **ui:** pin last-non-null-wins semantics for a resumed task's output file
- **conversation:** disable find with no thread; ring only on focus-visible
- **mcp:** a confirmed tool's deadline outlasts its confirmation window
- **transcript:** give each coalesced notification its own join timestamp
- **ui:** a stray file drop no longer navigates the webview away
- **move:** prep-site TARGET_DIRTY can never adopt; close replay only after verify
- **ui:** define --mono, and stop Retry rendering as a native button
- **a11y:** the healthy context meter uses a token, not a 2:1 hex
- **ui:** the conversation switcher menu drops over the toolbar, not under it
- **ui:** a refused Transfer follows the real move, and a lost hub says what it said
- **ui:** the Transfer sheet and chip tell the truth about a move that stopped
- **ui:** the moves store stops rewriting its own steps, and checks its events
- **move:** a move's progress survives a lost caller, and names the right step
- **hub-client:** toast a creation that fails after the dialog closes
- **hub-client:** cover health_check and TasksPanel in the contract skew
- **hub-client:** stop offering a cancel that does not cancel
- **hub-client:** show an honest empty state under a contract skew

### Documentation
- **transfer:** 3d has landed; its follow-ups join the roadmap's debts
- **transfer:** reconcile the spec with the budget raise and where clean_target is documented
- **attachments:** agent-only hosts CAN receive attachments
- **review:** findings from the post-merge review, and the plan that closes them
- **control-api:** register attachment_describe in the reference
- **plan:** add Task 6b so the size limits apply to dropped files too
- **attachments:** dropped paths come from Tauri's event, not the DOM
- **conversations:** the spec said to key a background entry by task_id
- **conversation:** a pill means chip, not toggle
- **transfer:** name the two residual risks the clean_target review surfaced
- **conversation:** the inset task is a cleanup, not a misalignment fix
- **plan:** the find input's ring is :focus-visible, per the global constraint
- **ux-agent:** the implementation plan, 14 tasks
- **plan:** App.svelte tears down via onDestroy, not a returned cleanup
- **transfer:** the prep-site dirty refusal is not classified — the verifier cannot answer there
- **ux-agent:** correct three claims the plan disproved
- **ux-agent:** one button, one operator, the whole fleet
- **plan:** correct the --mono count, and close the .retry-btn gap
- **conversation:** implementation plans, and three spec corrections
- **conversations:** implementation plan for background work in the Conversations tab
- **conversation:** one control system, one bar, one box
- **conversations:** design for background work in the Conversations tab
- **move:** recover_body/recover_script must not overclaim rollback safety
- **transfer:** pre-flight rulings on the 3d plan (transcript path, task order, test consts)
- **transfer:** slice 3d implementation plan, and the details panel's real event source
- **transfer:** a routed command needs a hub tool — resolve_move gets a slim one
- **transfer:** slice 3d design — retry, cleanup, the return trip and partial recovery
- **transfer:** where the Transfer work stands and what comes next
- **specs:** transfer sheet — what the whole-branch review changed
- **plans:** transfer sheet implementation plan (slice 3a)
- **specs:** transfer sheet — bridge test instead of the golden, moveProgress.ts, settle behaviour
- **specs:** transfer sheet — one button, live progress, a readable result (slice 3a)
## [0.2.26] - 2026-09-20

### Added
- **hub-client:** let the New session dialog list a remote host's worktrees
- **hub-client:** route list_host_worktrees to the hub
- **mcp:** a read-only tool to list a host's worktrees

### Changed
- replace personal email addresses with GitHub no-reply forms
- **hub-e2e:** guard the worktree check on the fixture project id
- **hub-e2e:** a paired client may list a host's worktrees

### Fixed
- **worktrees:** an unknown project is not-found on the local host too
- **hub-client:** recognise the refusal an old hub really sends
## [0.2.25] - 2026-09-20

### Added
- **ui:** merge Terminal and Conversation into one Session tab
- **ui:** add the session-view chord to appChord
- **ui:** sessionView pref and the rule that resolves it
- **move:** the session directory and the project memory travel with a move
- **move:** find, list and merge the project's Claude memory
- **chat-ui:** tell the empty state what to do next
- **chat-ui:** size the chat to its pane, not to the window
- **chat-ui:** grow the composer with its draft
- **move:** list, pack and merge the per-session Claude directory
- **hub-client:** generate the verdict table into docs/hub.md
- **move:** selection and merge policies for the Claude-side state
- **move:** report fields and the cap for the Claude-side state

### Changed
- drop a no-op reset and rename a stale test title
- **hub-client:** pin what a contract-refused read shows
- **sessions:** pin the forget-the-kill call at every tmux create site
- cover terminal remount and enablement after a pane-less row
- **move:** carry_e2e covers the Claude-side session state and memory
- drop plan-step references from comments in the desktop and frontend
- **hub-client:** sort the refusal rows by key, as newer clippy asks
- **mcp:** scope, slim and cap the control-API surface
- **hub-client:** drop plan-internal wording this branch introduced
- **hub-client:** comment-proof the route scanner, check tools against the hub
- **chat-ui:** lift the find highlighting out of the panel
- **chat-ui:** drop the empty composer foot and the doubled status rule
- **chat-ui:** one reading column, and put the toolbar on it
- **hub-client:** send the argument struct instead of re-spelling it
- **hub-client:** route a command by name, from the verdict table
- **hub-client:** pin the argument shapes a struct derive could disturb
- **hub-client:** hold ROUTED_ACTIONS and REASONS to the generated verdicts
- **hub-client:** tie health_check's row to the tool it really sends
- **hub-client:** check the table's tool, and where a body ends
- **hub-client:** refuse by name, with the sentence from the table
- **hub-client:** one verdict table for every Tauri command
- **hub-client:** pin every E_LOCAL_ONLY message in a fixture

### Fixed
- **ui:** distinguish an empty transcript from a missing session id
- **conversation:** no Retry for a tool detail refused on contract skew
- **hub-client:** make the wire-contract refusal outlive the socket
- **ui:** honest Conversation tooltip on a pane-less, id-less row
- **ui:** keep the Session tab clickable with no session selected
- **ui:** don't overwrite the session-view pref when a row forces it
- **ui:** leaving an overlay with the chord returns to the view you left
- **hub-client:** refuse hub calls while the wire contract is skewed
- **sessions:** let a re-created tmux name be inserted in the killing second
- **store:** revive a lost background session only from a newer probe
- **store:** do not re-insert a killed session from an older probe
- **agent:** sanitise peer-controlled request id and hub_version before logging
- **proto:** keep sanitize_for_log within its cap and close Malformed construction
- **move:** the flow reconciles the merge and re-checks the announced sizes
- **move:** the scripts enforce the name rules at the point of effect
- **hub-client:** drop the plan-step parenthetical from mcp_status's refusal sentence
- **ci:** match tool results whose JSON is compact in hub-e2e
- **move:** the index append refuses a delimiter line and finds the fresh line itself
- **hub-client:** narrow HubBackend::call/call_text to pub(super)
- **chat-ui:** honour reduced motion everywhere, and contain the scroll
- **chat-ui:** let a keyboard reach and scroll the transcript
- **chat-ui:** clamp from the constants that decide what is long
- **chat-ui:** state warn and error through the theme tokens
- **move:** a memory name matches its exact target entry first, then any case variant
- **chat-ui:** Escape closes find from anywhere in the panel
- **chat-ui:** walk the turn index with the arrow keys and Home/End
- **chat-ui:** walk the conversation switcher with the arrow keys
- **chat-ui:** wire the slash menu to the composer as a real listbox
- **chat-ui:** give the composer an accessible name and announce the Enter shortcut
- **move:** the session merge keeps what it cannot read, cleans its staging, replaces atomically
- **hub-client:** gate verdict_gen test-only, narrow docs/hub.md to refusals
- **move:** memory names compare case-insensitively; a skipped session dir reports every file

### Documentation
- **specs:** Open terminal persists the preference, like the segment
- **hub-client:** say what the test guards in its own words
- **specs:** guard the pref write, and list the user-facing docs
- update the Conversation doc for the Session-tab merge
- **specs:** the session-view chord returns to the view you left
- **move:** the merge script's own comment states the append-only approximation
- **move:** say what the merge really does and what a mixed fleet needs
- **move:** describe what move_session now carries
- **hub-client:** point CLAUDE.md and the skill at verdicts.rs
- **plans:** implementation plan for the Session tab merge
- **specs:** merge Terminal and Conversation into one Session tab
- **hub-client:** drop plan-internal task references from assert messages
- **hub-client:** section the verdict table, rewrap the hub.rs header
- **plan:** move carry slice 2 — the Claude-side state
- **spec:** move carry slice 2 — session directory and project memory travel too
## [0.2.24] - 2026-09-20

### Changed
- **desktop:** name hyper and reqwest precisely in remote.rs's comment
- **hub:** keep pair::exchange strict; probe keeps its own tolerance
- **desktop:** share HTTP/1.1 head parsing and de-chunking via http1
- **hub:** share pair's HTTP exchange with the healthcheck probe
- **desktop:** one hub-URL parser and one backoff curve, both from fleet-proto
- **hub:** the bind check asks fleet-proto whether the address is loopback
- **core:** share the loopback rule and the tunnel restart curve
- **agent:** take the loopback rule, the URL parser and the backoff from fleet-proto
- **proto:** one loopback rule, one hub-URL parser and one backoff curve

### Fixed
- **tunnel:** stop the reverse-tunnel restart loop and surface why it fails
- **provision:** write through a bind-mounted target when the rename fails
- **desktop:** plaintext_risk reads the host with the parser the socket is opened from
- **proto:** keep the agent's Host header byte-identical, and tighten is_loopback and port parsing
- **desktop:** separate the client's plaintext opt-in from the daemon's, and share the loopback rule
## [0.2.23] - 2026-09-20

### Added
- **agent:** refuse an incompatible hub and back off at the maximum on a version mismatch
- **hub:** judge an agent's protocol version before registering it, and welcome it with the hub's own
- **proto:** add protocol version negotiation and lenient unknown-kind decoding
- **hub:** disable remaining routed-mutation controls while offline
- **hub:** add wire-contract revision and skew check to the event bridge
- **hosts:** surface host transport (ssh vs agent) in the frontend
- **hub:** route new_session and explicit repair_session to the hub
- **move:** strict opt-out on the MCP tool and frontend, ADR 0002
- **move:** carry uncommitted, unpushed and small ignored work to the target
- **move:** probe operations in progress and split the strict verdict from the carry verdict
- **move:** list, pack and extract small git-ignored files
- **move:** snapshot, bundle, fetch and apply scripts with a real-git round trip
- **move:** carry report types and the ignored-file selection policy
- **move:** carry error codes and size settings
- **catalog:** surface install_as in listings; log suppressed host identifiers; codex dotted-key test
- **reconcile:** log session lifecycle transitions at INFO
- **reconcile:** a vanished tmux server or a reboot marks sessions lost, not deleted
- **store:** keep resumable mass-loss sessions through the reap until a TTL
- **store:** record why a session is lost; mark a host's sessions lost in one pass
- **reconcile:** carry the host identity on each probe
- **tmux:** read a host's boot identity alongside its sessions
- **store:** migration 034 and host boot-identity accessors
- **ui:** install_as in the asset editor and detail
- **catalog:** importer keeps the host identifier as install_as
- **sync:** update a pinned plugin when the catalog pin changes
- **catalog:** render and inventory by install name
- **catalog:** install_as header field
- **conversation-ui:** find in conversation, copy buttons and turn index
- **conversation-ui:** compact tool lines with lazy detail, subagent blocks, doing-now indicator
- **transcript:** session_tool_detail — lazy input/result for one tool call
- **transcript:** structured tool items with id/target/timing and subagent items
- **desktop:** show a banner while the hub's event stream is down
- **timeline:** live timeline via session:event push; docs for phase 2
- **conversation-ui:** header, earlier-conversation view, /clear follow, inline events and new item kinds
- **conversation-ui:** ConversationHeader with switcher, context meter, model, status and last event
- **conversation-ui:** live event fan-out and header/thread helpers
- **transcript:** compaction, slash-command and interrupt items; conversation events in session_conversation
- **ui:** point the desktop at a hub from Settings
- **api:** session_conversations, read earlier conversations; tasks survive /clear
- **hooks-install:** pane header, SessionStart command hook, compaction and clear/resume events
- **reconcile:** record tmux pane ids, pane context as fallback only, rebind conversations on id change
- **context:** compute context size from the transcript's last usage on Stop and on read
- **hooks:** resolve by tmux pane, rebind on SessionStart/UserPromptSubmit, track compaction and conversation end
- **store:** conversations — rebind, close, list, context setters; push timeline events
- **store:** migration 034 — conversations table and session context columns
- **desktop:** the hub's event stream drives the same frontend events
- **desktop:** every command honours the resolved backend
- **desktop:** reach an https hub through rustls and the platform trust store
- **desktop:** a hub-backed implementation of the read commands
- **desktop:** resolve a local or remote backend at startup

### Changed
- **hub:** guard the fixture commit against gpgsign, balance the skip path's tally
- **move:** a cross-host harness for the carry, run against a real target
- **mcp:** make the never-handshakes listener test deterministic
- **hub:** make the e2e's project discovery hermetic so it passes on a CI runner
- **mcp:** consolidate tool-policy exhaustiveness tests, pin both admin refusal wordings
- **mcp:** derive tool-policy predicates from one TOOL_POLICIES table
- drop review-history narrative from comments, ignore .reticle/
- give hub-image's meta job an empty permissions grant
- upload the hub-e2e logs when the step fails
- **hub:** assert the agent handshake in the e2e, and fail fast on a bad binary path
- **hub-image:** publish the fleet-hub image for linux/arm64 too
- **release:** ship fleet-agent and fleet-hub as Linux release artifacts
- **hub:** kill the /events SSE subscriber on interrupt, guard $ROOT in cleanup
- run scripts/hub-e2e.sh in the hub-headless job
- ignore .reticle
- **hub:** build hub_disabled.test.ts SessionRow fixtures via the shared factory
- **hub:** cover NewSessionDialog's usage-refresh hub-client gate
- **hub:** cover the handler-level gates and the exhaustive local/connected sweeps
- **mcp:** make the handshake-timeout test independent of scheduler timing
- **agent:** cover the hub-side timeout in PendingGuard's Cancel-on-drop
- **move:** move_session.rs becomes a module directory
- **tmux:** hide tmux with an isolated PATH, not /usr/bin:/bin
- **sync:** rustfmt apply.rs
- **events:** cover session:event / session:conversations batch wiring
- **provision:** expect the headers file before settings.json

### Fixed
- **hub:** pin ConversationRow on the wire and default its optional fields
- **proto:** neutralise a rejection's own detail before any receiver logs it
- **release:** refuse to package when the tag and the crate versions disagree
- **agent:** neutralise a hub-controlled close reason before it reaches the log
- **proto:** classify an unknown kind by probing the enum, not serde's error text
- **hub-image:** warn instead of misdescribing an amd64-only publish as making the run red
- **release:** bare checksum filenames, explicit duplicate detection, and a loud non-atomic re-upload
- **agent:** mention systemd in fleet-agent install --help
- **hub-image:** let arm64 fail without blocking or breaking amd64's publish
- **release:** merge per-target checksums, upload by release id, drop the fabricated LICENSE
- **agent:** require a compatible welcome before acting on anything else
- **hub:** make welcome unconditionally first, bound unknown-kind tracking, and guard frame_id
- **proto:** drift-proof unknown-kind classification, and bound/sanitise the tracker
- **agent:** refuse `install` cleanly on a host with no systemd
- **store:** revive a lost session only from a newer observation
- **release:** scope [package] field reads to the [package] table
- **release:** sync Cargo.lock for every crate release.sh bumps
- **hub:** make the New session dialog honest about a hub client's remote worktrees
- **contract:** pin ConversationRow and fix unreadable-contract handling
- **hub:** use Object.hasOwn instead of `in` when checking a refused action
- **hub:** gate the nickname-edit shortcut and its save, not just the button
- **hub:** gate Enter-to-submit and the remote worktree scan for hub clients
- **reconcile:** never let a cwd-inferred agent overwrite a session's claude_session_id
- **hub:** gate the handlers, not just the buttons, behind refused/offline actions
- **mcp:** session_conversations is a client tool
- **move:** the hub route forwards strict and reads the carry report back
- **hub:** disable refused controls and stop background calls that fail there
- **hub:** decide before writing when regenerating the contract golden
- **hub:** drop every frame ahead of a connection's ready frame
- **move:** harden the carry seams — status config, haves bound, upstream, rollback
- **mcp:** distinguish an unclassified tool name from a real admin tool in enforce_admin's message
- **hub:** cancel ticks before shutdown, widen the SIGTERM grace, refuse --tls auto in pair, flush after write
- **move:** carry parse failures are E_MOVE_CARRY; clean the target from the first write; pin the chunked download
- **agent:** send a best-effort Cancel when a caller drops a pending request
- **hub:** let an in-flight reconcile/usage pass finish before SIGTERM tears down SSH masters
- **move:** carry scripts survive login-shell banners; guard ids, chunk reads and a failed apply
- **mcp:** make the master-only tool gate classification mandatory
- **store:** add hosts.transport to databases from the pre-merge conversation branch
- **hub:** let pair/client CLI reach a --tls cert hub
- **settings:** mirror the carry setting bounds instead of hardcoding them
- **hub:** skip reverse ssh tunnel for agent-transport hosts
- **reconcile:** a verdict after a failed first post-loss pass still records the loss
- **reconcile:** spare live agents from a reboot verdict, read identity before the list
- **sessions:** fleet's own kill of a host's last session is not a resumable mass loss
- **desktop:** a token stranded by a half-finished pairing is clearable
- **reconcile:** correct the lifecycle_kind doc comment on duplicate lost lines
- **reconcile:** guard the mass-loss verdict against a stale probe and side-effect failures
- **store:** coalesce NULL lost_reason, cover keep+cutoff ordering, fix docs/copy
- **store:** clear lost_reason on revival paths, cover idempotency
- **tmux:** stop assuming CI has tmux installed in the identity tests
- **tmux:** classify tmux failures in Rust instead of guessing no-server in shell
- **tmux:** stop reading a timezone-rendered boot id on macOS
- **sync:** re-pointed install names refresh the manifest and remove the old paths
- **desktop:** store a new pairing's token last, beside its own hub
- **catalog:** layer overrides cannot change install_as
- **catalog:** refuse duplicate install names within a kind
- **desktop:** a configured hub that cannot be used owns nothing
- **catalog:** skip installed identifiers that collide with a catalog name
- **conversation-ui:** stable tool lines, conversation-scoped detail, live clocks while blocked, scoped find
- **conversation-ui:** find shortcut per platform, live pending calls while blocked, turn index a11y
- **conversation-ui:** final review minors
- **transcript:** only a leading command tag makes a command; MCP conversation caps events at 50
- **conversation-ui:** guard a malformed conversation list, share the view reset, hold the switch notice for a fresh list
- **desktop:** de-chunk a whole response before decoding it
- **desktop:** a silent hub stream reconnects, and only a working one resets the backoff
- **desktop:** routed commands answer what the local path would, and refusals tell the truth
- **reconcile:** keep context_at for an unchanged pane footer value so a no-op pass emits nothing
- **hooks:** only SessionStart(clear) rebinds a busy pane row; a nested --resume/-c is foreign
- **provision:** write the hook headers file before settings.json; docs: rebind eligibility and source derivation in the spec
- **hooks:** a nested claude in the pane never rebinds its parent; late SessionStart keeps the turn; safe-kill check by row
- **tasks:** tolerate only clear/resume/compact switches and re-stamp the task; read earlier conversations by direct lookup
- **reconcile:** never undo a hook rebind from an in-flight pass; one id guard; no stale mark on first sighting
- **store:** hold bus events inside atomically until commit; stale context only for the current conversation
- **desktop:** only a stream that delivered resets the reconnect backoff
- **desktop:** make the double-brain guard observable, and close two token leaks
- **desktop:** keep the hub token off argv and out of the logs

### Documentation
- **mcp:** session_conversation describes subagent items, tool fields and events_limit
- fix stale pane_intel.rs path, healthcheck comment, and keychain doc comment
- **hub:** fix set_host_token_mode, dedupe the terminal limitation, and update the keychain claim
- **hub:** replace the protocol upgrade-order bullet with the engineer's corrected text
- **skill:** drop stale CI-billing-block workflow, fix migration mechanism, and add two guard rules
- **claude-md:** recount LOC, fix version-file count, and catch up Status & known issues
- **readme:** fix stale build/test commands, migration paths, and add a hub/agent pointer
- describe the merged SHA256SUMS, conditional LICENSE, and best-effort arm64
- **hub:** document how to get the fleet-agent and fleet-hub binaries
- **hub:** document the protocol version handshake and its upgrade order
- **store:** spell out why the revive guard cannot strand a live row
- **changelog:** backfill 0.2.22 with the #136 host-agent entries
- **hub:** correct the contract-skew, terminal and confirm-tools sentences
- **move:** the dialog and the docs say what a move does now, not what it refused
- **move:** error-code docs say what strict and carry do now
- **hub:** correct the pair/client TLS trust story, the tunnel scope, and the admin-tools list
- **plan:** move carry engine implementation plan; spec corrections found while planning
- **spec:** move carry engine — transfer a session with the work as it is
- **migrations:** list 'killed' among 034's lost_reason values
- **plan:** host-reboot safety net (PR 1 of 2)
- **spec:** re-verify host-reboot findings on the fleet-core tree
- **catalog:** install_as rejects only . and .., not a leading dot
- **sync:** describe when plugin actions carry a plan and when plugin_update is planned
- phase 3 status
- **specs,plans:** catalog install names and plugin updates
- phase 3 implementation plan (detail UX)
- running the desktop against a hub
- **plan:** inline the groupItems test in phase 2 task 1
- phase 2 implementation plan (Conversations UI)
- conversation tracking in control-api, CLAUDE.md status, spec correction
- **plan:** resolve ambiguous claude_session_id by pane only; never bind one id to two rows
- phase 1 implementation plan for conversation event tracking
- conversation event tracking and Conversations tab UX design
## [0.2.22] - 2026-09-19

### Fixed
- **mcp:** a per-host token can no longer act on another host's sessions
  through `recreate_session`, `dismiss_ghost_session`, `capture_session` or
  `peek_session` — they now return `E_FORBIDDEN`, like the other
  session-addressed tools.
- **deps:** resolve devalue 5.9.2 (GHSA-9rgm-9g3h-6x36).
- **release:** release notes give the working macOS install steps.

### Also in this release

_The `v0.2.22` tag is a merge of the release commit below and PR #136 (the
`fleet-agent` host agent), so the tag contains #136 even though the release
commit that generated this section predates its merge. These entries are
added by hand for that reason — see #152._

#### Added
- **agent:** the fleet-agent binary — a lightweight process for hosts the hub
  cannot reach over SSH.
- **core:** an agent transport behind the existing SshExec seam, so a host can
  run over the agent connection just like SSH.
- **core:** route each host to its own transport.
- **hub:** the /agent WebSocket endpoint.
- **hub:** hand an agent host its enrollment token out of band.
- **hub:** set a host token's mode without a desktop.

#### Fixed
- **agent:** a system install leaves its config reachable only by the user it
  runs as.
- **agent:** the config directory's mode no longer depends on the umask.
- **agent:** `--insecure` is confined to loopback.
- **agent:** stopping the agent kills the children it is running.
- **agent:** size the frame cap to the transcript, and stop `E_TIMEOUT`
  re-firing a billed request.
- **hub:** bound agent connections per host, refuse SSH hosts, and never hold
  a stuck socket.
- **hub:** cut a live agent off when its host token is rotated, narrowed or
  removed.

## [0.2.21] - 2026-09-18

### Added
- **hub:** serve TLS directly with operator-supplied certificates
- **mcp:** session_conversation returns structured turns
- **hub:** broadcast event bus and a GET /events SSE stream
- **hub:** fleet-hub pair, client list and client revoke
- **mcp:** pair_client, list_clients and revoke_client
- **hub:** pairing codes and the POST /pair exchange
- **commands:** wire layer authoring into Tauri commands
- **catalog:** author layers through the existing commit-and-reload path
- **mcp:** client tokens resolve to a non-master client caller
- **mcp:** list_layers, resolve_preview, propose_layers, set_host_layers
- **store:** client_tokens table with hashed, revocable rows
- **catalog:** propose initial layers by host-set signature
- **sync:** report dropped plugin refs instead of uninstalling them
- **sync:** resolve each host's layers before computing its plan
- **store:** host_layers table and assignment accessors
- **catalog:** pure resolve() producing the effective catalog with provenance
- **catalog:** load layers/ with extends flattening and cycle detection
- **catalog:** layer model with members, exclude and overrides
- **mcp:** unauthenticated /healthz liveness route
- **hub:** healthcheck subcommand and Docker HEALTHCHECK
- **hub:** Docker image, compose with Caddy, systemd unit, docs/hub.md
- **hub:** fleet-hub daemon — init, serve, token, ssh-key
- **reconcile:** hub.local_host opt-out — a daemon hub has no local host
- **provision:** HubBase — public base URL for hooks and MCP entries, tunnels only for a loopback hub
- **mcp:** configurable bind address and Host/Origin allowlist
- **conversation:** file paths in reply text open in the Files tab
- **conversation:** recall earlier prompts with ArrowUp in the composer
- **conversation:** context meter beside the composer, Compact suggested when high
- **conversation:** Load older turns
- **conversation:** the Latest button counts what landed while scrolled up
- **conversation:** keep an unsent draft per session and focus the composer
- **conversation:** turn duration and an open tool group on the running turn
- **conversation:** mark tool calls whose result was an error
- **conversation:** live indicator, blocked banner and on-demand pane probe
- **conversation:** quick-action chips with presets editable in Settings
- **conversation:** slash-command menu and a hint for the composer
- **conversation:** send prompts from the Conversation tab
- **sync:** keep only the three newest .fleet-bak backups per file
- **ui:** asset editor, templates, lint, commit/push and authoring sessions in the Assets tab
- **catalog:** authoring commands
- **catalog:** open an authoring session in the catalog repo
- **catalog:** templates, lint, and authoring operations with auto-commit
- **hooks:** SessionEnd → stopped, StopFailure → turn over, Notification → blocked
- **store:** session_end, stop_failure and notification hook writes
- **catalog:** path-addressed git helpers, remove_asset, resource pruning
- **hooks:** accept SessionEnd, StopFailure and Notification payload fields
- **hooks:** install SessionEnd, StopFailure and Notification http hooks
- **ui:** sync plan dialog, secrets panel, and sync actions in the Assets tab
- **catalog:** sync and secrets commands and MCP tools (plan_sync, apply_sync, set_secret)
- **catalog:** plan_sync and apply_sync orchestration with progress and run history
- **catalog:** guarded per-host applier with backups, secret uploads, config merges, plugins and manifest
- **catalog:** sync plan computation, plan registry, managed and orphan inventory states
- **catalog:** managed manifest and secret resolution for sync
- **catalog:** Codex host scan and TOML config merge
- **catalog:** harness merge_config/manifest_path, JSON merge and unmerge helpers, config hashes in the Claude scan
- **mcp:** bound every tool call with a per-class wall clock
- **store:** migration 031 — managed inventory flag, catalog secrets, sync runs, sync:progress event
- **mcp:** return tool failures as is_error results with the E_* code
- **mcp:** stateless streamable HTTP and protocol 2025-11-25
- **catalog:** Tauri IPC commands and MCP tools for the asset catalog
- **catalog:** import skills, agents, hooks, MCP servers and plugins from ~/.claude
- **catalog:** configure/load/list/get service functions and host scan runner
- **catalog:** Claude host scan and per-asset drift state computation
- **store:** catalog_config and asset_inventory tables (migration 018) with row events
- **catalog:** load and write the catalog repo, git clone/pull/head
- **catalog:** experimental Codex renderer for skills and MCP servers
- **catalog:** Claude Code renderer for all asset kinds
- **catalog:** render plan, host snapshot and Harness trait
- **catalog:** IR model for skills, agents, hooks, MCP servers and plugin refs
- **ui:** Assets tab with catalog list, host matrix, previews and import dialog
- **terminal:** text-input style selection and cursor
- **ui:** assets store and catalog/inventory row events
- **conversation:** render replies as Markdown; fold tool calls, clamp long prompts, jump to latest
- **app:** Conversation tab; no-pane rows open it by default
- **conversation:** transcript-backed Conversation panel
- **sidebar:** Outside fleet group, inactive agents, drop log peek
- **transcript:** session_conversation; replace claude logs with the transcript
- **store:** agent kind on bg upserts, dismissed_agents table
- **agents:** keep kind, job id and start time from claude agents
- **usage:** show headroom when starting a session and in the footer
- **hosts:** open the Hosts view from anywhere; slim Settings
- **hosts:** the Hosts view
- **usage:** usage bar and block components
- **usage:** the usage model and theme tokens
- **usage:** poll account usage in the background and expose it
- **usage:** fetch each account's 5-hour and weekly usage on its own host
- **accounts:** nicknames and the extra-usage flag
- **sidebar:** reach Add project from the project picker
- **projects:** the Add project dialog
- **projects:** addProject and listGithubRepos wrappers
- **commands:** add_project and list_github_repos
- **projects:** create a new project and browse GitHub repos
- **projects:** adopt an existing checkout as a project
- **projects:** add a project by cloning a GitHub repo
- **repo-url:** parse the repo identifiers Add project accepts
- **sidebar:** two-line session rows with the name on its own line
- **sidebar:** persisted details toggle for session rows
- **new-session:** list the chosen host's worktrees; scan remote hosts
- **projects:** listHostWorktrees wrapper
- **commands:** list_host_worktrees for the new-session dialog
- **worktrees:** list a host's worktrees by scanning its checkout
- **store:** prune a host's worktree rows a scan no longer reports
- **triage:** count one bucket narrower than the filter shows
- **triage:** one ranked "Needs you" queue in the sidebar (T1a, P13/P27)
- **mcp:** auto-install the local hook on enable; show per-host hook health
- **sessions:** show the session timeline; double-click edits the label
- **usage:** Settings rows for the usage.* settings
- **usage:** per-session token usage and estimated cost
- **repair:** gate automatic stale-entry removal on the parent dev:inode fingerprint
- **sessions:** move_session between hosts; descope Freeze (W5 G2)
- **repair:** guarded automatic removal of a vanished worktree's own stale entry
- **repair:** opt-in automatic workspace repair on the reconcile tick
- **orchestration:** completion signal, wait/transcript/run_prompt, task objects, tags (W3 Track E)
- **projects:** projects base path and layout as settings, one-host quickstart (W5 G3)
- **observability:** rotating log file with redaction, copy-diagnostics, runbook
- **sessions:** generated names, quick switcher, scrollable pickers
- **mcp:** whoami, session_id addressing, list_sessions force, peek by claude id, tracked new_bg_session (W2 D5/D6)
- **triage:** stuck chips, attention filter, playbooks, session GC, outcome fields (W2 Track D)
- **mcp:** one source of truth for status vocabulary + response caps
- **safe-kill:** pre-flight inspect + clean/discard paths; fix Stop hook (#29)
- **sessions:** deterministic friendly name on create + startup backfill (#28)

### Changed
- **pty:** wait for a killed child to disappear instead of demanding it at once
- **terminal:** lock in the behaviour six passing tests did not
- **sync:** exempt the two new async tests from await_holding_lock
- **sync:** assigning a layer that drops an installed asset plans Remove
- ignore graft's local graph cache
- **hub:** end-to-end coverage for client access
- **catalog:** strengthen the no-layers backward-compat test
- **mcp:** a client token is refused admin tools and honours readonly
- **sync:** cover the layered-planning integration, context-chain flatten, and unrecognised axis
- rustfmt the catalog layer module
- cosmetic sweep across the hub branch
- **hub:** end-to-end script for a real fleet-hub binary
- **catalog:** serialise the provision HOME test on CATALOG_TEST_LOCK
- **deploy:** harden the systemd unit
- bind test discriminates on the address; config precedence coverage
- pre-commit hook lints only fleet-core and fleet-hub without Tauri libs
- headless fleet-hub build job; release bumps the hub crate
- **core:** move service, store, ssh, tmux and mcp into fleet-core
- **core:** fleet-core crate with the rt::spawn runtime seam
- cargo workspace rooted at the repository root
- **release:** bump version to 0.2.20
- **account_usage:** report bash/tool paths when the DEBUG trace assert fires
- **release:** bump version to 0.2.19
- **provision:** take the shared HOME lock in expand_home_local_expands_tilde
- **mcp:** resolve the inbox target through resolve_target_row
- **mcp:** take service Args structs as tool parameters directly
- **events:** one generic emit behind the typed EventBus methods
- **validate:** shared sub-checks behind every public validator
- **ui:** one row-store core and one inline-rename flow
- **ssh:** one run_shell for the local-vs-remote `bash -lc` hop
- **store:** one ghost_and_clean shared by the tmux and pane-less pruners
- **commands:** move cancel_command out of lib.rs into commands/cancel.rs
- **store:** derive Default on the reconcile/account fixtures, add empty_probe
- **store:** tighten visibility, collapse the get_worktree twin
- **mcp:** read the control-API settings through one mcp::settings module
- **store:** collect() row iterators, now_unix() everywhere, RETURNING id upserts
- **service:** move fleet hook installation into service/hooks_install.rs
- **store:** write→re-fetch→emit helpers for sessions and hosts
- **service:** move mutating git commands into service/repo_mutate.rs
- **store:** `in_clause(n)` + `params_then` for the IN-list builders
- **service:** move repo read views from commands/ into service/repo_read.rs
- **store:** `.optional()` for every single-row lookup
- **store:** shared column consts + row mappers for hosts, projects, accounts, cursors, messages
- **service:** move git repo plumbing from commands/ into service/repo.rs
- **ui:** fold duplicated frontend helpers into their one home
- route every `IpcError::new("E_*")` through `codes::`
- one `lock()` idiom for app mutexes, drop redundant error maps
- remove dead code confirmed by the dead_code lint
- **terminal:** read pty.rs through the bundler, not node:fs
- **terminal:** stop the idle header rewrite and cap the resize rate
- **catalog:** catalog_template takes no store
- **release:** bump version to 0.2.18
- **terminal:** tmux 3.6a attach fixture generator and recording
- **mcp:** classify plan_sync, apply_sync and set_secret for the per-call wall clock
- **mcp:** count the asset-catalog router block in the tool-count guard
- **catalog:** migration 030 and the shared ipc_error codes module
- **provision:** restore HOME after expand_home_local_expands_tilde
- **release:** bump version to 0.2.17
- **release:** bump version to 0.2.16
- **release:** refresh Cargo.lock for 0.2.15
- **release:** bump version to 0.2.15
- **release:** refresh Cargo.lock for 0.2.14
- **release:** bump version to 0.2.14
- **projects:** a remotely added project survives a local refresh
- **deps:** tauri-plugin-dialog for the folder picker
- **projects:** make the real gh unreachable from hermetic tests
- **release:** refresh Cargo.lock for 0.2.13
- **release:** bump version to 0.2.13
- **sidebar:** split rowMeta into rowElapsed and rowPrompt
- **validate:** one canonical remote worktree path rule
- **release:** refresh Cargo.lock for 0.2.12
- **release:** bump version to 0.2.12
- **release:** bump version to 0.2.11
- **store:** split store.rs into a store/ module
- **mcp,lib:** split mcp/tools.rs and lib.rs startup helpers (F4c/F4d)
- **settings:** share dialog CSS and copyText; drain-loop and focus tests
- **sessions:** split service/sessions.rs into sessions/ (F4b, pure move)
- **terminal:** split TerminalView into mouse, drain and clipboard modules (F5b)
- **logging:** eprintln sweep part B2 — last 34 sites, empty guard, codes constants
- **sidebar:** split SessionRowItem, PeekPanel, SidebarFilters, NewBgSessionDialog and session_status out of Sidebar
- **settings:** split HostsTable and McpSettings out of SettingsDialog
- **logging:** eprintln sweep part B1 — guard hardening, 8 more sites, codes constants
- **store:** fingerprint_keys_of_project covers local rows; remote keys are the stored path
- **repair:** drop the helper-thread canonicalization in fingerprint_keys
- **usage:** log move_session usage failures through tracing
- **logging:** eprintln sweep part A — tracing in 6 files + production-eprintln guard
- **logging:** Track H2 logging and diagnostics nits
- **repair-tick:** factor the fake's fingerprint map into a type alias (clippy)
- **settings:** Settings rows for every backend setting; registry test; Refresh + repair docs (Track H2)
- **repair:** replaced-parent test keeps both inodes alive; dev differs, inode equal refuses
- **reconcile:** hook-guard phantom path, real gate entry point, call-gated writers
- **reconcile:** transitions, ghost lifecycle, bg agents, multi-host through the real reconcile path (W4 F1)
- bump health schema_version to 20; fix read_bytes_for expectation
- **ssh:** tmux roundtrip cleanup guard, LocalExec test-only, review nits
- **ssh:** SshExec trait, scripted fake, end-to-end host/provision/reconcile tests (W4 F6)
- **db:** renumber lifecycle migration to 019 (Track B takes 018)
- skill note on E_SELF_TARGET, ignore proptest regressions, tidy visibility
- **backend:** pty and messages coverage, atomic send_message, proptest for shell quoting (W4 F2/F7)
- **release:** tag-only dispatch, concurrency group, deny on linux only
- macos+ubuntu matrix and tag-gated tauri release job (W4 F8)
- **store:** migration table
- **errors:** IpcError::lock(), canonical codes module, E_SQLITE for DB failures
- **ci:** pin toolchains, add local CI mirror, cargo-deny and pnpm audit
- **deps:** clear RUSTSEC and npm audit advisories via lockfile updates
- **sidebar:** deterministic perf assertion for 500-session render
- **release:** replace release-please with scripts/release.sh
- **release:** bump version to 0.2.10
- **release:** bump version to 0.2.9
- **release:** bump version to 0.2.8
- **release:** bump version to 0.2.7
- **release:** bump version to 0.2.6
- **release:** bump version to 0.2.5
- gitignore .claude/settings.local.json

### Fixed
- **release:** write the CHANGELOG section on macOS, and fail if it cannot
- **terminal:** AltGr on punctuation, one tab stop, safer IME and paste routing
- **terminal:** keep non-Latin prose in one run, measure the cell over 20 glyphs
- **terminal:** tmux-exact colour groups, cleaner wrapped copies, stable fixture
- **terminal:** one drain loop at a time, and say so when a chunk fails
- **catalog:** warn on unknown exclude keys; test the layer load branch
- **sessions:** send prompts and pane queries to an exact tmux target
- **pty:** close leaves no stale shared state, input keeps its order, reap never parks a worker
- **catalog:** list_layers shows only active assignments
- **sync:** take the layered flag from the resolution, reject any bogus axis
- **catalog:** delete host_layers on remove_host; validate host alias on layer entry points
- **hub:** pair and client list/revoke honour --port
- **mcp:** list_clients is master-only, as the spec's own ruling says
- **hub:** hardening sweep over client access
- **hub:** TLS needs an https public URL, and the healthcheck speaks TLS
- **hub:** a revoked client's event stream ends at the next heartbeat
- **catalog:** only Noop a dropped plugin_ref orphan on a layered host
- **catalog:** detect extends cycles at load, not just at plan time
- **catalog:** lint a layer before writing it, not after
- **clippy:** remove unused import, dead LayerSet::is_empty, redundant closures
- **mcp:** validate host_alias, admin-gate set_host_layers, cap resolve_preview payload
- **mcp:** bump health's schema-version tripwire, reject colliding layer names
- **catalog:** filter propose_layers to installed states, drop hyphen-joined layer names
- **catalog:** harden resolve() against identity-changing overrides and a vacuous test guard
- **sessions:** unknown project or worktree id answers E_NOTFOUND
- **hub:** one deadline for the health probe, and flag an orphaned ssh key
- **usage:** skip local on a hub
- **hub:** persist allow_plaintext; token needs an existing database; ssh-key derives from a private key
- **hub:** refuse every explicit local target when hub.local_host is off
- **hub:** own default data dir, stricter plaintext refusal, drain on shutdown
- **usage:** skip hidden hosts
- **provision:** strict public URL parsing; strip legacy token hooks; refuse ?token= on a public hub
- **core:** embedder-supplied app version
- **mcp:** pass the hub allowlist to rmcp's own Host check
- **hub:** effective allowlist includes the public host; global CLI options; serve logs to stderr
- **reconcile:** single-host reconcile refuses local when hub.local_host is off
- **provision:** match only fleet's exact hook entry shape when stripping
- **test:** match the trace dump on a variable, not on bash 4+ syntax
- **conversation:** final review batch for prompt recall and file paths
- **conversation:** second review batch for the Conversation tab
- **conversation:** review-round fixes for the Conversation tab
- **sync:** drop unparseable manifest keys on rewrite
- **terminal:** let a held key repeat reach macOS press-and-hold
- **terminal:** the IME proxy follows the caret even with the cursor hidden
- **app:** the terminal's IME proxy is the terminal, not a text field
- **terminal:** give the terminal a real input target so IME text arrives
- **terminal:** Option-composed punctuation is text, not a Meta chord
- **terminal:** a keystroke mid-tick no longer forks a second drain
- **terminal:** pre-empting a gesture now cancels its state, not just its listeners
- **terminal:** re-run a coalesced open that targets the same session
- **catalog:** keep add_project's reason in the catalog adoption error
- **catalog:** plan_sync rejects an unknown host alias
- **catalog:** clearer error when the catalog origin is already adopted elsewhere
- **terminal:** state the grid minimum once, and lock it to pty.rs
- **terminal:** a forwarded mouse press can no longer orphan window listeners
- **terminal:** a drop's paths only ever reach the session that started it
- **terminal:** give every open a generation and stop losing session switches
- **catalog:** kebab-case kinds in authoring session names, drop unreachable blank-name branch
- **terminal:** re-attach when the backend reports dropped output
- **terminal:** trust the backend's eof flag, not `[cf]` text in the output
- **catalog:** treat blank url/command as missing in validate
- **terminal:** a failed drain tick can no longer freeze the terminal
- **pty:** keep blocking PTY work off the main thread and off the lock
- **pty:** report overflow and PTY death out of band, not as output text
- **pty:** hold back only a real partial codepoint, under one lock
- **pty:** clamp to the renderer's minimum grid (10x2), not 40x10
- **terminal:** disable ssh's `~` escape on the remote PTY attach
- **terminal:** target tmux sessions exactly so an attach can't land elsewhere
- **catalog:** cap add_resource at 1 MiB
- **ui:** confirm resource removal, refresh repo status after loads, surface git stderr
- **terminal:** pin every run to its cells so a fallback glyph can't shift a row
- **catalog:** importer reports invalid resource names as problems
- **terminal:** a repaint with erased gaps ends a stale soft wrap
- **catalog:** skip empty authoring commits, tighten url and resource checks
- **terminal:** a plain click on a wide glyph no longer selects and copies it
- **terminal:** a full-width repaint ends a stale soft wrap
- **terminal:** copying a soft-wrapped line no longer adds a newline at the wrap
- **terminal:** a selection edge on a wide glyph copies and highlights it whole
- **terminal:** join VS16, ZWJ, skin-tone and flag clusters like tmux 3.6a
- **catalog:** refuse symlinked remove_asset targets, validate resource paths in write_asset
- **catalog:** resolve git identity normally, isolate tests via GIT_CONFIG_GLOBAL/NOSYSTEM
- **terminal:** ignore DECSTBM with a negative top margin
- **terminal:** DEC Special Graphics b-e map to control pictures, not controls
- **terminal:** clamp an oversize DECSTBM bottom margin instead of ignoring it
- **terminal:** scrolled and inserted lines take the current background
- **terminal:** decode OSC 52 clipboard payloads as UTF-8
- **terminal:** SGR hidden, strikethrough, ITU colon forms and underline colour
- **terminal:** RIS resets mouse, bracketed paste, cursor and DECSC state
- **terminal:** treat every private CSI marker as private, not as the public form
- **sync:** keep a re-parked plan's original TTL deadline
- **provision:** write host secrets via tmp file + atomic rename
- **ui:** keep sync plan dialog mounted after apply, disable stale re-apply
- **catalog:** set_secret's value must never reach the persisted audit trail
- **catalog:** a plan refused for missing secrets stays in the registry
- **catalog:** never rewrite an unparseable config, route every config write through the 0600 path
- **catalog:** back up merge-only assets and unmerge superseded manifest entries
- **catalog:** a latest plugin ref matches any installed version
- **catalog:** redact secrets from Substituted's Debug output
- **catalog:** fail closed on TOML datetimes in codex merge_config
- **add-project:** adopt an existing checkout only when it is that repository
- **ui:** pause the Conversation poll under the Assets overlay; spec names migration 030
- **ui:** tolerate missing tags on asset detail, reload without pull after import
- **catalog:** harden tags serialization, scan failure handling, importer slugs and error codes
- **hosts:** re-read each remote host's Claude account every reconcile pass
- **add-project:** keep sentinel exit codes alive past ~/.bash_logout
- **catalog:** flag hook secrets, scrub token everywhere, handle multi-hook entries and missing plugin versions
- **catalog:** correct hook-merge presence check and scan zero-file hang
- **catalog:** normalise relative clone paths, skip symlinks in resources, tolerate unreadable kind dirs
- **catalog:** quote unsafe YAML scalars in Claude frontmatter
- **ui:** Terminal tab must not read active while Assets tab is open
- **conversation:** emphasis around code spans, visible tool-group chevron, tighter nested lists
- **ui:** read-only external rows, inactive agents, lighter Conversation polling
- **agents:** launch lookup, inactive kill, mtime failures, lighter conversation read
- **conversation:** decouple relative-time ticker from poll content changes
- **sidebar:** stuck-count pill ignores external rows
- **transcript:** keep the newest reply when a huge prompt overflows the budget
- **bg:** stop by job id, find launched agents by name, remove from list
- **sessions:** treat external rows as pane-less everywhere
- **reconcile:** external rows for interactive agents, retire idle bg agents
- **usage:** monotonic polling floor; stricter connect-failure detection
- **usage:** keep the token off disk and out of curlrc; request-level polling floor
- **hosts:** follow an account switch on the local host
- **hosts:** record the local host's Claude account
- **projects:** add-project dialog opens the session on the right host; honest, accessible in-flight state
- **projects:** cancel kills the whole local process group and is hedged for GitHub creation
- **cancel:** send callId so the Cancel button reaches the backend; separate anonymous ids
- **projects:** only resume a repository new created
- **projects:** push-only retry only finishes a creation fleet started
- **projects:** make the GitHub-creation retry real and safe
- **projects:** recoverable GitHub creation, a real confirmation token, no fake authors
- **projects:** adopting a bare repo's worktree registers the worktree
- **projects:** keep adopted folders across a refresh; resolve worktrees and validate names
- **projects:** bound the clone connect timeout, validate the host, keep error context
- **repo-url:** reject a .git component, all-dot names and oversized components
- **sidebar:** keep the name full-width, wrap the details line, real separators
- **new-session:** keep typed branch input across a host switch
- **new-session:** never submit another host's worktree; narrow the scan effect
- **sessions:** guard the local arm, trust the scanned worktree path
- **sessions:** refuse another host's worktree row; open the scanned path
- **worktrees:** inode-compare the scan root; split_scan_output tests
- **worktrees:** canonical root, name dedupe, ssh error mapping in the host scan
- **store:** propagate scan errors, guard empty keep list, drop fingerprints on prune
- **sessions:** mirror an existing worktree from origin instead of a naive worktree add
- **prompt:** strip the untrusted marker before recording a prompt (Q2)
- **sidebar:** widen the host filter when the selected session's host is hidden
- **settings:** apply the projects preview indent and its error colour
- **settings:** keep loadHostTokens optional; test the drain re-entrancy guard
- **status:** per-row agent parsing, waitingFor precedence, live dialog fixture
- **status:** count ghosts by status; detect dialogs; map new claude agents fields
- post-B2 review nits (guard trailing-comment close, temp-file collision, set_friendly_name not read-only)
- **worktrees:** address #76 review (symmetric spellings, 026 on an existing DB)
- **worktrees:** canonical spellings and race guard in the remote prune
- **repair:** adoption guard canonicalizes outside the store lock
- **worktrees:** prune stale remote worktree rows; per-entry migration guard
- **logging:** legacy daily log sorts strictly before that day's hour-00 file
- **store:** compute fingerprint keys before the store lock and pass them to the deletes
- **repair:** no filesystem calls under the store lock; tick checks registration; bg reap test
- **usage:** make migration 025 safe to re-run
- **usage:** close the move window and seed the 023 upgrade test from MIGRATIONS
- **settings:** never send a Limits value that silently means never (#65 review)
- **worktrees:** address #63 review (pre-existing FK rows, events, linking order)
- **worktrees:** host-scoped worktree rows, sibling linking, purge FK guard
- **repair:** reap fingerprints, timelines and inboxes with their rows; tick records fingerprints
- **repair:** keep render_git_script test-only; scope a test's store lock before await
- **sessions:** move_session follow-ups — post-kill check, event order, cap setting
- **sessions:** move_session review — idle source, post-copy recheck, in-flight guard
- **paths:** address #56 review (worktree FK, bare repos, remote cwd)
- **paths:** canonical path identity, worktree dedupe, remote worktree hooks
- **repair:** only the reconcile tick may drop a stale worktree entry automatically
- **reconcile:** phantom status events, orphaned timelines, bg status filter, garbage tmux output
- **repair:** re-check before removing a stale entry, same-filesystem guard, review nits
- **repair:** keep the context-free plan() and backoff_of() test-only
- **purge:** delete the project only after every host succeeds, strict not-found match
- **projects:** purge Claude transcripts on the right host under both path forms
- **orchestration:** host-check spawns, physical transcript paths, task liveness, marker ordering
- **orchestration:** dispatch_task defers worker naming to new_session; post-rebase test fixes
- **projects:** link remote sessions under custom roots and flat layout, correct previews
- **repair:** automatic repair only creates, destructive steps explicit, canonical paths
- **sessions:** self-repairing worktrees and tmux cwd on create, recreate, restart, attach
- **sessions-ux:** platform-correct switcher chord, reveal selected session, dialog fixes
- **mcp:** gate session_id addressing on the resolved host after the Caller rebase
- **triage:** no launch notification burst, PR probe outside host budget, GC skips no-worktree rows, attached-pane guard
- **security:** bind confirm nonce to args, refuse malformed settings.json, master-only fleet admin
- **security:** per-host tokens, http hooks on every host, caller identity, blast-radius limits (W1 Track B)
- **mcp:** correct status docs and skill params, drop unknown CLI statuses
- **terminal:** loop-safe secondary DA, bounded control-string buffer
- **terminal:** code-point rendering, wcwidth, DCS/APC swallow, full key table, ansi property tests (W4 F3/F7)
- **ssh:** reset master only when wedged; forced refresh path; register E_SSH_TIMEOUT
- **backend:** ssh wall-clock timeouts, guarded list_sessions, no-op-free upserts (W1 Track A)
- **frontend:** timer-based event batching, modal scrollbar clicks, keep selection on bootstrap failure
- **frontend:** session identity, derived selection, native dialogs, toasts, event batching (W1 Track C)
- **bg-sessions:** allow prompts that start with a dash
- **ci:** runner-agnostic cargo-deny install, single pnpm version source
- **security:** gate devtools behind a feature, harden claude CLI argv and ssh -R
- **a11y:** clear the 12 svelte-check warnings in dialogs and Sidebar
- **sessions:** make bg:<uuid> rows addressable — kill via claude stop, typed E_BG_SESSION elsewhere
- **reconcile:** prune dead bg session rows and cap session_events
- **pty:** keepalive + auto-reconnect so a wedged remote attach self-heals
- **reconcile:** bound per-host probe so a wedged SSH master cannot empty the sidebar
- **safe-kill:** install Stop hook on remote hosts via provision (#30)

### Documentation
- fix stale src-tauri paths in the asset-layers design spec
- the client-access doc and comment sweep
- pairing, clients, the event stream and the two new endpoints
- renumber host-reboot spec's migration off the taken 032
- **plans:** client access implementation plan
- **specs:** client access design — pairing, events stream, conversation tool, built-in TLS
- **plan:** correct the Claude harness reference in task 6
- **plan:** implementation plan for composable asset layers
- **spec:** composable asset layers (roles + contexts)
- **mcp:** refresh_projects says what a hub without a local host answers
- point the schema-version hint at MIGRATIONS instead of a number
- **hub:** ssh ownership and known_hosts, token regeneration on migration, bare-binary steps
- **hub:** claude-fleet client name; re-provisioning keeps user hooks
- **spec:** host reboot session survival and restore
- **plans:** hub daemon implementation plan
- **specs:** hub daemon design — headless fleet-hub, core crate split, public-URL provisioning
- **conversation:** user guide for the Conversation tab, plus final cleanups
- **pty:** describe the writer thread and shared output state
- **install:** explain the macOS "damaged" Gatekeeper dialog
- name the Open in session action in the catalog concepts
- describe catalog authoring
- **control-api:** hook contract; SessionEnd, StopFailure and Notification
- **specs,plans:** hook events design and implementation plan
- **plans:** asset authoring implementation plan
- **specs:** asset catalog sub-project 3 design (authoring)
- fix misplaced doc comment and stale sync-tool/confirm wording
- describe the sync engine and its MCP tools
- **control-api:** stateless transport, is_error tool results, wall clocks
- **plans:** asset sync engine implementation plan
- **plans:** MCP transport and tool-contract implementation plan
- **specs:** asset catalog sub-project 2 design (sync engine)
- **specs:** MCP transport and tool-contract hardening design
- **specs:** note importer slugification in the asset-catalog design
- describe the asset catalog and its MCP tools
- **plans:** asset catalog sub-project 1 implementation plan
- **specs:** asset catalog sub-project 1 design (universal model, import, inventory)
- **control:** track bg runs with session_transcript; explain external rows
- **plan:** agent rows outside tmux and the Conversation tab
- **spec:** agent rows outside tmux and the Conversation tab
- **plans:** Hosts view and per-account usage
- **specs:** Hosts view and per-account usage
- **plans:** record the confirm-token flow and honest cancel for the dialog
- **plans:** the TS parser must port every rule is_component gained
- **plans:** record the cancellation trap that would undo the clone timeout fix
- **plans:** de-duplicate projects case-insensitively, not in the parser
- **plans:** use the real confirmation code, introduce E_EXISTS and E_GH
- **plans:** add a project that is not checked out yet
- **specs:** add a project that is not checked out yet
- **specs:** real separator spans, wrapping details line, overlaid row actions
- **specs:** wt-status line above the picker; no idle state
- record E_INVALID and the scanned-path cwd in the spec and plan
- **plans:** host-scoped worktree picker and two-line session rows
- **specs:** friendly name is primary on line 1, tmux name on line 2
- **specs:** host-scoped worktree picker and two-line session rows
- **specs:** keep the session-management analysis and pre-check
- **diagnostics:** say what the generated reference lists for Tauri commands
- **mcp:** regenerate the control API reference for repair_session
- **mcp:** regenerate the control API reference for repair_session
- **mcp:** regenerate the control API reference for repair_session
- CI mirror and toolchain notes, crate license, health test literal (W0 follow-ups)
- **control-api:** fix drift, slim the control skill and managed CLAUDE.md
- correct stale orientation notes (W0.1)
- **plans:** six-lens improvement report and wave plan
- **mcp:** regenerate control-api reference for kill_session description
## [0.2.4] - 2026-05-25

### Added
- **Guided first-run onboarding**: one-time welcome dialog, a get-started
  checklist card atop the sidebar, local-prereq checks, tunnel-status surfacing,
  MCP port/token/copy with `bind_error` reporting, and a "Replay setup guide"
  entry in Settings. Backed by new `check_local_prereqs` / `tunnel_status`
  commands and onboarding service/store with pure step derivation.
- **Contextual first-use hints**: a `HintLayer` rendering viewport-clamped hint
  bubbles over tagged UI anchors, driven by a hint registry, plus a Settings
  toggle to show/reset feature hints.
- **Auto-slugify** for free-form worktree names when creating sessions.
- `TunnelSupervisor::snapshot` for surfacing tunnel status.

### Changed
- `fleet-friendly-name` skill now uses deterministic triggers.

### Fixed
- Hints: gate opens for existing users; corrected session-actions anchor; bubble
  re-measure on open; bubble z-index kept below modals.
- Onboarding: use the real `provisioned` field instead of the reachable proxy;
  welcome dialog dismisses on Escape.

### Documentation
- User-facing docs overhaul: README rewrite with a routing structure, a docs
  index, and new Getting Started, Concepts, and Troubleshooting guides; refreshed
  and cross-linked the Control API guide.

[0.6.2-dev.58.desktop.g7a0e164]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.6.2-dev.58.desktop.g7a0e164
[0.6.1]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.6.1
[0.6.0]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.6.0
[0.5.4]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.5.4
[0.5.3]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.5.3
[0.5.2]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.5.2
[0.5.1]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.5.1
[0.5.0]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.5.0
[0.4.12]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.12
[0.4.11]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.11
[0.4.10]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.10
[0.4.9]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.9
[0.4.8]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.8
[0.4.7]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.7
[0.4.6]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.6
[0.4.5]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.5
[0.4.4]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.4
[0.4.3]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.3
[0.4.2]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.2
[0.4.1]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.1
[0.4.0]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.4.0
[0.3.3]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.3.3
[0.3.2]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.3.2
[0.3.1]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.3.1
[0.3.0]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.3.0
[0.2.42]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.42
[0.2.41]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.41
[0.2.40]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.40
[0.2.39]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.39
[0.2.38]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.38
[0.2.37]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.37
[0.2.36]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.36
[0.2.35]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.35
[0.2.34]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.34
[0.2.33]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.33
[0.2.32]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.32
[0.2.31]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.31
[0.2.30]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.30
[0.2.29]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.29
[0.2.28]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.28
[0.2.26]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.26
[0.2.25]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.25
[0.2.24]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.24
[0.2.23]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.23
[0.2.22]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.22
[0.2.21]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.21
[0.2.4]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.4
