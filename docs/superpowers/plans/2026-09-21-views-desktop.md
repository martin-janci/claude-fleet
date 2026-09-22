# Views, filters, scrolling — desktop (claude-fleet) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A host leads to its sessions with one click, the sidebar filters behave predictably (no silent reset, search covers tags), and the Conversation tab remembers where you were and lets you step turn by turn.

**Architecture:** Svelte 5 runes stores (`src/lib/*.ts`) hold filter state; `Sidebar.svelte` applies it; `ConversationPanel.svelte` owns the scroller. Each task is a small, testable change in one or two files with a Vitest test beside it; no backend change.

**Tech Stack:** Svelte 5, TypeScript, Vitest (`npx vitest run`), `npx svelte-check --threshold error`. Frontend only.

**Spec:** the analysis `ux-views-analysis.md` (copied to `docs/superpowers/reviews/2026-09-21-views/`), sections A–B (desktop). Findings it cites: `HostsView.svelte:317-319` (keyboard-only `s`), `App.svelte:477-480` (Hosts overlay not closed), `Sidebar.svelte:217-222` (host filter reset on any selection change), `Sidebar.svelte:254-265` (search ignores tags), `ConversationPanel.svelte` (`resetThread` snaps to bottom on every switch), `conversation_nav.ts` (`turnIndex`).

## Global Constraints

- Never render hub-sourced text through `{@html}`; the `markdown.ts` → Svelte pipeline is the only path (audit in Task 6).
- Stores patch in place (`mergeOne`/`removeOne`); do not add re-fetches.
- `hostFilter` already persists (`hosts.ts:45-46`); do not add a second persistence mechanism.
- Every task: `npx vitest run` and `npx svelte-check --threshold error` green before commit; Conventional Commits, one line, no trailers.
- Branch `feat/views-ux` off `origin/main` in the claude-fleet worktree; never push from a task.

## File map

| File | Responsibility |
|---|---|
| `src/lib/host_actions.ts`, `src/lib/HostDetail.svelte`, `src/lib/HostsView.svelte`, `src/App.svelte` | "View sessions" action: set host filter + close Hosts (as shipped: `HostsList.svelte` is untouched and the action lives in `host_actions.ts`, not `hosts.ts`) |
| `src/lib/Sidebar.svelte` | filter reset gating; search over tags |
| `src/lib/ConversationPanel.svelte` | per-session scroll memory; prev/next turn keys |
| `src/lib/conversation_nav.ts` | pure helpers: `nearestTurn`, `adjacentTurn` |
| `src/lib/*.test.ts` | Vitest per task |

---

### Task 1: Host → sessions in one click

**Files (as shipped):** `src/lib/host_actions.ts`, `src/lib/HostDetail.svelte`, `src/App.svelte:477-480`, `src/lib/HostsView.svelte:317-319` — `HostsList.svelte` is untouched: the list row keeps opening the detail, and "View sessions" is the detail's header button plus the `s` key; test `src/lib/HostsView.test.ts` (extend or create following the existing `*.test.ts` style with `@testing-library/svelte` if used there).

**Interfaces (as shipped):** a single exported action `viewHostSessions(alias: string)` in `src/lib/host_actions.ts` that does `hostFilter.set(alias)` and `closeHosts()` (import the existing overlay close from wherever `App.svelte` defines it; if it is component-local, lift a `hostsOpen` store to `hosts.ts`).

- [ ] Step 1: failing test — calling `viewHostSessions('mefistos')` sets `hostFilter` to `mefistos` and `hostsOpen` to false.
- [ ] Step 2: run, RED. Step 3: implement `viewHostSessions`; wire a "View sessions" button in `HostDetail.svelte`'s header and make the whole `HostsList` row's primary click call it (keep the `s` key in `HostsView.svelte` calling the same action). Step 4: GREEN + `svelte-check`. Step 5: commit `feat(hosts): one click from a host to its sessions, and the overlay closes`.

### Task 2: The host filter stops resetting itself

**Files:** `src/lib/Sidebar.svelte:217-222`; test `src/lib/Sidebar.test.ts` (extend).

- [ ] Step 1: failing test — with `hostFilter = 'mefistos'`, a programmatic `selectedSession` change to a session on `mac` (simulating reconcile) leaves `hostFilter` at `mefistos`; an explicit select (the click handler / quick switcher path) widens it.
- [ ] Step 2: RED. Step 3: introduce an `explicitSelect(id)` path (a store action `selectSessionExplicitly`) used by the click and quick-switcher handlers; only that path runs the "reveal" widening. Step 4: GREEN. Step 5: commit `fix(sidebar): only an explicit selection widens the host filter`.

### Task 3: Search covers tags

**Files:** `src/lib/Sidebar.svelte:254-265` (`matchesSearch`); test `src/lib/Sidebar.test.ts` or a pure `src/lib/search.ts` + `search.test.ts` if `matchesSearch` is lifted out (prefer lifting: one rule, one place).

- [ ] Step 1: failing test — a session tagged `review` matches the needle `rev` when no other field does; case-insensitive. Step 2: RED. Step 3: `s.tags?.some(t => t.toLowerCase().includes(needle))`. Step 4: GREEN. Step 5: commit `feat(sidebar): search matches tags`.

### Task 4: Remember where you were in each conversation

**Files:** `src/lib/ConversationPanel.svelte` (`resetView`/`resetThread`, `scroller`, `load()`); `src/lib/conversation_nav.ts` (+ `scrollMemory`); test `src/lib/conversation_nav.test.ts`.

**Interfaces (as shipped):** `export const scrollMemory = new Map<number, { turnAt: string | null; rowKey: string; atBottom: boolean }>()`; `rememberScroll(sessionId, snapshot)`, `recallScroll(sessionId)`, `forgetScroll(sessionId)`. The anchor is the CONTENT key `turnAt` (the turn's timestamp), not `rowKey`: `t<i>` is a position inside the loaded window, so it names a different turn as soon as the tail moves or "Load older" runs. `rowKey` is kept only to restore an inline event by its (window-independent) `e<id>`.

- [ ] Step 1: failing tests — `rememberScroll(1, {rowKey:'turn-7', atBottom:false}); recallScroll(1)` returns it; `recallScroll(2)` is null; an `atBottom:true` snapshot recalls as "go to bottom".
- [ ] Step 2: RED. Step 3: before `resetThread()` on a session switch, store the top-most visible `rowKey` (the existing row keys) and `atBottom`; after `load()` resolves for the returning session, if a snapshot exists and `!atBottom`, `scrollToRow(rowKey)` instead of snapping to bottom. In-memory for the app lifetime. Step 4: GREEN + a manual check in the running app (`pnpm tauri dev` is heavy — the memory says a dev build kills the installed app and migrates its DB; do NOT run it; rely on the unit test and svelte-check). Step 5: commit `feat(conversation): remember the scroll position per session`.

### Task 5: Step turn by turn

**Files:** `src/lib/conversation_nav.ts` (`nearestTurn(rows, topVisibleKey)`, `adjacentTurn(rows, current, +1|-1)`), `src/lib/ConversationPanel.svelte` (`onKeydown`: `[` previous turn, `]` next turn; two small buttons beside the existing "↓ Latest"); test `src/lib/conversation_nav.test.ts`.

- [ ] Step 1: failing tests for the two pure helpers over a `turnIndex` fixture (first/last boundaries clamp). Step 2: RED. Step 3: implement; keys call `scrollToRow(adjacent)`. Step 4: GREEN. Step 5: commit `feat(conversation): previous and next turn with [ and ]`.

### Task 6: Markdown audit (no library)

**Files:** none changed unless the audit finds `{@html}` on hub text; test `src/lib/no_html_on_hub_text.test.ts` (new): a source scan asserting no `{@html` appears in `ToolLine.svelte`, `SubagentBlock.svelte`, `MarkdownView.svelte`, `MarkdownInline.svelte`, `ConversationPanel.svelte` (extend the list to every component that renders conversation items — grep `ConvItem`/`items` usages).

- [ ] Step 1: write the scan test; Step 2: run (GREEN expected — if RED, the offending `{@html}` is a real bug: replace it with the markdown pipeline and record it). Step 3: commit `test(conversation): a gate that hub text never reaches {@html}`.

## Self-review
- Analysis coverage: D1→T1, D2→T2, D3→T3, D4→T4, D5→T5, D6→T6.
- Type consistency: `viewHostSessions`, `selectSessionExplicitly`, `scrollMemory`, `nearestTurn/adjacentTurn` named once each and used by the tasks that follow.
