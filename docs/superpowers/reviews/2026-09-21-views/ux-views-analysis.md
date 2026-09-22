# UX views analysis — host links, filters, conversation scroll, markdown

Desktop repo: `claude-fleet` (`.claude/worktrees/nevies-spojenie-trust-6babca`).
Mobile repo: `fleet-mobile`.

## A. Findings per complaint, per app

### 1. Host → sessions link

**Desktop — exists, keyboard-only, incomplete.**
`HostsView.svelte:317-319` binds `s` → `onFilterSidebar(alias)`, wired at `src/App.svelte:477-480` to `hostFilter.set(alias)`. No mouse/tap equivalent: `HostsList.svelte:163-165`'s row `onclick` only calls `onselect(h.alias)`, which just changes the master-detail selection (`HostsView.svelte:171-174`). Even `s` doesn't leave Hosts: `onHostsFilterSidebar` (`App.svelte:477-480`) never calls `closeHosts()`, and the center pane stays hidden behind Hosts (`sessionTabActive = !filesMode && !assetsMode && !hostsMode`, `App.svelte:346`). Clicking a session inside `HostDetail.svelte:220-231` (`onclick={() => selectSession(s)}`) sets the global selection (`src/lib/selection.ts:26-31`) but likewise never closes Hosts. Severity: **Medium**.

**Mobile — does not exist.**
`HostsScreen.kt:78-127`'s `HostLineItem` has no `Modifier.clickable` and `HostsScreen(...)` (`HostsScreen.kt:39-44`) takes no click callback. `Navigator.kt:9,32-92` has no host-scoped screen argument. `SessionsViewModel.kt` has no host filter, only `needsAttentionOnly` (`:49`). Severity: **High** — zero implementation of the described flow.

### 2. Filters

**Desktop — enumerated (`SidebarFilters.svelte`):** host pills (`:88-104` → `hostFilter`, persisted via `readPref`/`writePref('host-filter', …)`, `src/lib/hosts.ts:45-46`); recency pills (`:123-133` → `recency`, persisted, `Sidebar.svelte:73-76`); "Needs you" toggle (`:139-149` → `needsYouOnly`, **not** persisted, `Sidebar.svelte:139`); free-text search (`:68-73` → `search`, not persisted, `Sidebar.svelte:86`); bg-agents toggle (`:184-194`). No tag filter, no project filter pill exist.

Bugs:
- **Search ignores tags.** `matchesSearch` (`Sidebar.svelte:254-265`) matches project owner/repo, `tmux_name`, `host_alias`, `friendly_name` — never `session.tags`, though tags are settable via `set_session_tags`. The operator's "tags" filter has no UI surface at all.
- **Host filter is silently reset.** `Sidebar.svelte:217-222`: whenever the selected session's host is hidden by the current filter, the effect force-resets `hostFilter` to `'all'`, unconditionally on any `$selectedSession` change — including a reconcile-driven selection, not just an explicit user click. A filter set via the Hosts-view `s` key can be undone by an unrelated background event.
- **Inconsistent persistence** across the four filters (`hostFilter`/`recency` persisted, `needsYouOnly`/`search` not) with no documented rule for the split.

**Mobile — enumerated:** only "Needs attention" (`SessionsScreen.kt:123-136` → `SessionsViewModel.toggleNeedsAttentionOnly()`, `:140-142`). Its own doc comment (`:124-141`) records a prior double-toggle race that was already fixed — the one filter that exists works correctly. Not persisted: `local` is a bare `MutableStateFlow(Local())` (`:86-92`), no DataStore found; the view model is `remember(repository, scope)`-scoped in `App.kt:289` so it survives tab switches but not relaunch. No host/project/tag filter, no search box exist. Severity: **Medium** (the missing filters duplicate complaint 1's gap).

### 3. Conversation scrolling

**Desktop (`ConversationPanel.svelte`):** opens at the latest message — every session switch runs `resetThread()`→`resetView()` setting `atBottom = true` (`:302-321,310`), then `load()` calls `scrollToBottom()` once pinned (`:276-279`). **Not remembered per session** — `resetView()` unconditionally resets `atBottom = true` on every switch; no session-id→offset cache exists. "Jump to bottom" exists: "↓ Latest"/"↓ N new" (`:1598`, gated by `atBottom`, `:133`). Turn navigation is browse-and-click only: `turnIndex()` (`conversation_nav.ts:62-78`) lists every turn; clicking calls `scrollToRow` (`ConversationPanel.svelte:647-650`). No prev/next-turn keybinding exists. Severity: **Low-Medium**.

**Mobile (`SessionScreen.kt`):** opens near bottom via `atBottom` (`:103-108`) + `LaunchedEffect` (`:109-111`) — the surrounding comment documents a real prior bug (yanked view, stale closure) already fixed. **No remembered scroll position**: `rememberLazyListState()` (`:85`) isn't keyed by session id or hoisted to the view model. **No manual jump-to-bottom** (no pill/button anywhere in the file, unlike desktop's "↓ Latest"). **No turn index / no prev-next stepper.** This matches what Phase 1 Task 7 of `docs/superpowers/plans/2026-09-21-pager-phase-1.md` already plans (the "new-reply pill", plan line 402, is mobile's equivalent of desktop's "↓ Latest"). Severity: **Medium**.

### 4. Markdown

**Desktop — already solved, low severity.** Assistant text renders via `ConversationPanel.svelte:1483` (`<Markdown source={g.text} />`, also `:1509` for compaction summaries) → `MarkdownView.svelte`, which parses through `parseMarkdown()` (`markdown.ts`) into a typed `Block[]` tree and maps each block to real Svelte elements, delegating inline spans to `MarkdownInline.svelte`. Zero `{@html}` anywhere in the path (grepped `MarkdownView.svelte`, `MarkdownInline.svelte`, all `src/lib/*.svelte`) — no innerHTML/XSS surface. Headings, lists, fenced code with syntax highlighting + copy button, blockquotes, tables (`markdown.ts:39`) all supported. No markdown package in `package.json` — none needed. Residual gap: no line-by-line audit here of every hub-text surface (tool arg previews, subagent descriptions) confirming each also avoids `{@html}`.

**Mobile — plain text only.** `ConvItem.Text` renders via bare `Text(text = item.text, …)` (`SessionScreen.kt:221-225`); `**bold**`, `- ` lists, and fences render as literal characters. No markdown dependency present. Phase 1 Task 7 (`pager-phase-1.md:391-404`) already specifies `com.mikepenz:multiplatform-markdown-renderer-m3` for `ConvItem.Text`, size-gated with a `MiniMarkdown.kt` fallback if the iOS framework grows >2MB. Severity: **High** — starkest gap of the four; desktop solved it safely, mobile has nothing, fix is already scoped.

## B. Task list

### Desktop

1. **Host row click → filtered sessions, and close Hosts.** Files: `HostsList.svelte` (row `onclick`), `HostDetail.svelte` (header action), `App.svelte:477-480`. Add a "View sessions" action that calls `hostFilter.set(alias)` **and** `closeHosts()`; keep `s` as a keyboard alias for the same combined action. No new persisted state (`hostFilter` already persists, `hosts.ts:45-46`). Acceptance: clicking "View sessions" on `mefistos` closes Hosts, activates the `mefistos` sidebar pill, and shows only its sessions.

2. **Fix the host filter's silent reset.** File: `Sidebar.svelte:217-222`. Gate the "reveal on selection" widening to explicit user selects (click, quick switcher) only, not every reconcile-driven `$selectedSession` change. Acceptance: a background refresh that moves `$selectedSession` to another host does not flip `hostFilter` back to "all"; only an explicit click or the new "View sessions" action does.

3. **Search matches tags.** Files: `Sidebar.svelte:254-265` (`matchesSearch`), `sessions.ts` (`SessionRow.tags`). Extend the per-session predicate with `s.tags?.some(t => t.toLowerCase().includes(needle))`. Acceptance: typing a tag surfaces sessions carrying it even when nothing else matches.

4. **Remember per-session scroll position.** File: `ConversationPanel.svelte` (`resetView`/`resetThread`, `scroller`, `load()`). Before `resetThread()` on a switch, save `scroller.scrollTop` (or top visible `rowKey`) into a `Map<sessionId, anchor>`; on return, restore it instead of forcing `atBottom = true`, unless the saved anchor was already near the bottom. State: per-session offset keyed by session id, in-memory for app lifetime. Acceptance: scroll up in session A, switch to B, switch back — same row, not snapped to bottom.

5. **Turn-by-turn prev/next stepper.** Files: `ConversationPanel.svelte` (`onKeydown`), `conversation_nav.ts` (`turnIndex`). Add prev/next keys (e.g. `[`/`]`) that move to the adjacent `turnIndex(thread)` entry relative to the most-visible turn, via the existing `scrollToRow`. Acceptance: repeated next-turn key steps through every turn boundary without opening the turn list.

6. **Markdown: no new library needed.** `MarkdownView.svelte`/`MarkdownInline.svelte`/`markdown.ts` already render safely (parser → Svelte elements, zero `{@html}`). Task is an audit only: confirm `ToolLine.svelte`/`SubagentBlock.svelte` and any other hub-text surface never use `{@html}` — none currently do among the files inspected. Sanitisation rule to keep: never interpolate hub-sourced text via `{@html}`; render only through the `markdown.ts`→Svelte pipeline so the browser's own text-node escaping applies.

### Mobile

1. **Host tap → filtered Sessions, with a clear chip.** Files: `ui/HostsScreen.kt` (`HostLineItem`, add `onClick`), `ui/Navigator.kt` (extend `Screen.Sessions` with `hostAlias: String? = null`, update `tabOf`), `ui/SessionsViewModel.kt` (add `hostFilter: String?` to `Local`, apply in `groupSessions`), `ui/SessionsScreen.kt` (dismissible `AssistChip` next to "Needs attention"). State: in-memory, cleared like `needsAttentionOnly` on tab reselect (`Navigator.select`'s existing "leave state behind" rule) — no new persistence layer. Acceptance: tapping a host row switches to Sessions showing only that host's groups, with a chip that clears the filter on tap.

2. **Markdown for `ConvItem.Text`** — pull forward the markdown slice of Phase 1 Task 7 specifically (not tool-row collapsing/load-older/new-reply-pill, unless bundled deliberately). Files: `gradle/libs.versions.toml`, `shared/build.gradle.kts` (add `multiplatform-markdown-renderer-m3` to `commonMain`, run the plan's iOS size check, fall back to `MiniMarkdown.kt` if >2MB growth), `ui/SessionScreen.kt` (`ConvItem.Text` branch). Recommendation: **yes, pull forward** — highest-severity gap of the four, already fully scoped. Acceptance: a turn with `**bold**`, a `- ` list, and a fenced code block renders bold text, a bulleted list, and a monospace block on both Android and iOS.

3. **Jump-to-bottom pill** (same file, bundle with task 2 since both touch `atBottom`/`newest`). File: `ui/SessionScreen.kt` (`:103` `atBottom` already exists). Add a pill shown when `!atBottom` that calls `listState.scrollToItem(newest)` on tap — the "new-reply pill" from Task 7. Acceptance: scroll away from bottom while new turns arrive; a "↓ New reply" pill appears and tapping it jumps to newest.

## C. Needs the hub

- **Per-turn anchors for cross-device resume.** Neither app has a hub-side read-position concept today. Desktop task B4 and mobile's remembered scroll can each be done client-side, but surviving an app restart or syncing between desktop and phone on the same session needs a hub-stored anchor (e.g. `last_seen_turn`/`since_turn` beside the existing `turn_seq`). Grepped `crates/fleet-core/src/mcp/*.rs` and `service/*.rs` for `since_turn`/`anchor` — neither exists.
- **`session_conversation`'s existing `turnsWanted`/`cid` pagination** already supports "load older turns" with no hub change — both apps' pagination tasks (desktop's Load older, mobile's Task 7 "Load 20 earlier turns") are client-side against the current contract.
- **Server-side tag filtering** is not needed for desktop task B3 (tags already arrive on `SessionRow`), but a future fleet-wide tag search (beyond what's currently loaded) would need a new hub filter parameter — flagged, not required now.
