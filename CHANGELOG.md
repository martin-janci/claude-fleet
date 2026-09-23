# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Releases are cut with `scripts/release.sh` — see [docs/RELEASING.md](docs/RELEASING.md).
Entries before 0.2.4 were plain version bumps and were not recorded individually.

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
