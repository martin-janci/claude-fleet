# Merge the Terminal and Conversation tabs into one Session tab

*Design — 2026-09-20*

## Problem

The right-hand view strip currently reads:

```
Terminal | Files | Conversation | Assets            Hosts ⌘I
```

Five siblings at one level, but they are not siblings. `Terminal` and
`Conversation` are two *views of the same running session* — the same agent,
seen through tmux or through its transcript. `Files` is a view of that
session's worktree. `Assets` and `Hosts` are fleet-scoped and need no session
at all.

Flattening all five hides that structure and, worse, makes `Terminal` the
de-facto privileged view: it is what "no mode is set" means in the code, so
every code path that clears a mode lands there. Conversation is increasingly
the primary way to watch a session, and the strip does not reflect that.

## Goal

Give Conversation and Terminal one shared parent in the strip, with a
second-level switch between them:

```
Session | Files | Assets          (Conversation | Terminal)     Hosts ⌘I
```

Neither sub-view is privileged. Which one you get is a remembered preference,
not a side effect of which state variable happens to be false.

## Current implementation

`src/App.svelte` holds four independent `$state` booleans — `filesMode`,
`conversationMode`, `assetsMode`, `hostsMode` — that the `show*()` helpers
clear by hand to keep them mutually exclusive. "Terminal" is the absence of
all four, which is why the strip's active-state expressions read
`class:active={conversationMode && !hostsMode && !assetsMode}`.

`Conversation` and `Files` render as opaque overlays inside `.right-body`,
stacked above a permanently mounted `TerminalView`. That is deliberate: the
PTY and its ANSI buffer survive a round trip through another view, so flipping
back is instant and never refits or reconnects. **This design keeps that
mechanism untouched.**

Rows with no tmux pane (background agents, external Claude sessions) have no
PTY. `TerminalView` is not mounted for them at all and `ConversationPanel`
renders directly in the slot. A `prevNoPane` flag drives an effect that forces
`conversationMode` on when such a row is selected and back off when leaving
it.

## Approach

Rewire the strip and the defaults; leave the four-boolean structure and the
overlay stacking alone.

Two alternatives were weighed and rejected for this change:

- **A `rightView` state machine** (`'terminal' | 'conversation' | 'files' |
  'assets' | 'hosts'`) replacing the four booleans. Genuinely cleaner — the
  `&& !hostsMode && !assetsMode` guards are a symptom of modelling mutually
  exclusive states as independent flags. But it rewrites the top half of a
  934-line file and most of `App.hosts.test.ts`, which buries a refactor
  inside a feature change. Worth doing on its own afterwards.
- **Extracting a `RightPanel.svelte`.** The right long-term answer for
  `App.svelte`'s size, and a separate piece of work.

## Design

### 1. The tab strip

`Session` replaces the `Terminal` and `Conversation` buttons and takes the
first slot. It is active whenever none of Files, Assets or Hosts is open.

The second-level switch is a segmented control rendered at the right end of
the same strip, immediately left of `Hosts`. It is **visible only while
`Session` is active** — offering a switch for a view you cannot see is noise.

Layout mechanics: `.hosts-tab` currently claims the free space with
`margin-left: auto`. That moves to a container element that is always present
and holds the segment (empty when `Session` is not active), so `Hosts` stays
pinned right and does not shift when the segment appears or disappears.
`.hosts-tab` keeps its `::before` divider rule.

**Disabled states.** In the segment:

- `Terminal` is disabled for rows with no tmux pane, titled with the existing
  `NO_PANE_TITLE` — *"Runs outside tmux — no terminal"*.
- `Conversation` is disabled until the row has a `claude_session_id`, titled
  *"No Claude session id yet"*.

The `Session` tab itself is disabled only when both sub-views are
unavailable — no selected session at all.

**Accessibility.** The outer strip keeps `role="tablist"` with `Session`,
`Files`, `Assets` and `Hosts` as its tabs. The segment is **not** a nested
tablist — two tablists in one strip make a screen reader announce two
independent tab positions for what is one place. It is a
`role="radiogroup"` with two `role="radio"` buttons, labelled
`aria-label="Session view"`, and carries `aria-keyshortcuts` for the chord.

### 2. State

A new pref in `src/lib/prefs.ts`, following the `copyOnSelect` pattern
(`readPref` with a validator for the initial value, `subscribe` →
`writePref`):

```ts
export type SessionView = 'conversation' | 'terminal';
export const sessionView = writable<SessionView>(
  readPref('ui.sessionView', 'conversation', isSessionView),
);
sessionView.subscribe((v) => writePref('ui.sessionView', v));
```

The default is `'conversation'` on a clean `localStorage`.

`conversationMode` stops being set from session selection and becomes derived:

```
conversationMode = effectiveView === 'conversation'
                   && !filesMode && !assetsMode && !hostsMode
```

where `effectiveView` comes from a pure helper:

```ts
// src/lib/session_view.ts
export function resolveSessionView(
  pref: SessionView,
  noPane: boolean,
  hasClaudeId: boolean,
): SessionView;
```

Rules, in order:

1. `noPane` → `'conversation'` (there is no terminal to show).
2. `!hasClaudeId` → `'terminal'` (there is no transcript to show).
3. Otherwise the stored `pref`.

When both constraints fire at once — a pane-less row with no
`claude_session_id` — rule 1 wins and the Conversation panel shows its own
empty state. That row has nothing else to offer.

Crucially, the forced cases **never write the pref** — and that is a rule
about *writes*, not only about reads. On a row that forces one view, that
view is already showing and its pill is already checked, so clicking that
pill is a visual no-op; it must not quietly rewrite the preference another
row is relying on. The write is therefore guarded on the row being able to
offer both views, not merely on the picked view being showable. So: you are in Terminal,
you click a background agent and see its Conversation, you click back to a
tmux row — you are in Terminal again. This replaces the `prevNoPane` flag and
its effect, which exist only to approximate that behaviour today.

Returning from Files, Assets or Hosts always lands on the remembered
sub-view.

`ConversationPanel`'s own "Open terminal" affordance routes through the same
`setSessionView('terminal')`, so it persists the preference exactly as the
segment does. It is a view switch like any other; making it a one-shot
exception would mean the same visible action sometimes sticks and sometimes
does not.

### 3. The keyboard chord

`appChord()` in `src/lib/app_views.ts` gains a third result,
`'session-view'`, bound to `⌘J` on macOS and `Ctrl+Shift+J` elsewhere —
the same shape as the existing `hosts` chord, for the same reason: `⌘` and
`Ctrl+Shift` never reach the PTY, while plain `Ctrl` chords belong to the
terminal. `Alt` continues to disqualify any chord. `J` is unused (`⌘I`
Hosts, `⌘,` Settings, `⌘K`/`⌘P` quick switcher).

The chord toggles `sessionView` between the two values — but only when the
Session tab already owns the panel. If Files, Assets or Hosts is open, the
chord just returns to the session, showing the sub-view you left and writing
nothing to the pref; a second press then flips. Leaving an overlay should put
you back where you were, not somewhere else.

A chord that lands on a disabled sub-view is a no-op: it does not write the
pref and does not change the view.

`hostsChordLabel()` gets a sibling `sessionViewChordLabel()`, used by the
segment's tooltip. Settings has no shortcut list today — only an "Open
Hosts" button — so nothing is added there.

### 4. Tests

- **`src/lib/app_views.test.ts`** — `appChord` returns `'session-view'` for
  `⌘J` on mac and `Ctrl+Shift+J` off-mac; returns `null` for `Alt+J`, plain
  `Ctrl+J` on mac, and `⌘J` when `shiftKey` is also set. Plus
  `sessionViewChordLabel` on both platforms.
- **`src/lib/session_view.test.ts`** (new) — `resolveSessionView` across the
  matrix: each pref value with a normal row, a pane-less row, a row without a
  `claude_session_id`, and a row that is both. A pure function, no Svelte.
- **`src/App.hosts.test.ts`** — the `tab-terminal` and `tab-conversation`
  test ids disappear. New ids: `tab-session`, `subtab-conversation`,
  `subtab-terminal`. Assert the segment is absent while Files/Assets/Hosts is
  open and present on the Session tab, and that `Hosts` keeps its position in
  both states.

Any other test referencing the two removed test ids is updated in the same
change.

### 5. Out of scope

No "Conversation has new turns while you were in Terminal" indicator. It is
appealing and it is a separate feature with its own state — whether a turn
counts as unseen, when it clears, how it renders. Build it later if its
absence is actually felt.

No change to how `ConversationPanel` or `TerminalView` render internally.

No migration of an existing `localStorage` value — the pref is new and its
default is the intended behaviour for everyone.

## Files touched

| File | Change |
|---|---|
| `src/App.svelte` | Strip markup, segment, derived `conversationMode`, chord handling; remove `prevNoPane` effect |
| `src/lib/prefs.ts` | `sessionView` pref + `SessionView` type |
| `src/lib/session_view.ts` | New — `resolveSessionView` |
| `src/lib/app_views.ts` | `'session-view'` chord, `sessionViewChordLabel` |
| `src/lib/session_view.test.ts` | New |
| `src/lib/app_views.test.ts` | Chord cases |
| `src/App.hosts.test.ts` | New test ids, segment visibility |
| `docs/conversation-view.md` | Renamed from `conversation-tab.md`; the Conversation is a sub-view now, not a tab |
| `docs/README.md` | The docs-index link to the renamed file |
