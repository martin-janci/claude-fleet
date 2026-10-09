Orbit Fleet is a dense, keyboard-first control room for long-lived Claude Code sessions running in tmux across many machines. It is sessions-first, not a task manager. The interface stays quiet and neutral so the one thing that matters stands out: which sessions need you. Colour is spent on state, never on decoration, and the dark theme is the primary one.

These rules hold for the desktop app (Tauri + Svelte, `src/app.css`) and the phone app (fleet-mobile, `FleetTheme.kt`). Token names are the CSS custom properties; component classes carry the `of-` prefix in `components/bundle.css`.

## Content fundamentals

- Lead with what the person must do, then the evidence: "Waiting for you: approve push to main", not "Session status update".
- Name things the way people know them: session, host, account, mission, pull request, organisation. Keep internal words (lease, reconcile, tmux name, ghost) on diagnostic screens only.
- Status is one plain word: **Needs you**, **Working**, **Failed**, **Done**, **Paused**, **Idle**. No emoji or glyph prefixes; the dot carries the colour, the word carries the meaning.
- Say why something is paused and what to do: "Paused · weekly limit on tech.silvester", with **Switch account** and **Wait until Fri 11:00** right on the row.
- Sentence case for buttons, headings and tabs. Second person ("need you"); the app never says "I". A label that opens a dialog ends with "…".
- Ages are short and relative: `2m`, `40m`, `3h`, `2d`. Ids, branches and paths appear only in mono detail lines or links.
- Machine suggestions always say who made them and why: "Proposed by Jev · last turn ended with a question · Not waiting", "Drafted · from the last 3 turns · Regenerate · Clear".

## Colour

- Grounds: `bg` for the conversation and dialogs; `bg-pane` for the header, rail, left list, inspector and footer; `bg-raise` for buttons, inputs and raised cards; `bg-sunk` for wells; `bg-hover` on hover.
- Text: `fg` for content, `fg-2` for secondary content and quiet buttons, `fg-muted` for metadata. All three read at 4.5:1 or better on `bg`, `bg-pane` and `bg-raise` in both themes.
- `accent` is action and selection only: primary buttons (with `accent-fg`), links, the selected row and rail item (`accent-soft`), the focus `ring`, and the AI pill. Never a status.
- Status uses five tokens, the same on desktop and phone: `status-waiting` (amber, the attention colour; nothing else is amber), `status-working` (blue), `status-failed` (red), `status-done` (green), `status-idle` (grey). Chips and banners tint with the `*-soft`, `*-faint` and `*-line` tokens; the text stays the full status token.
- `danger` is error text and quiet destructive buttons; `danger-fill` with `on-danger` is only the confirm button of a destructive dialog.
- `org-1` to `org-4` mark organisations as 8 px swatches; never as fills.
- `brand-ink`, `brand-light` and `brand-amber` belong to the mark, the splash and the loaders, never to UI chrome.
- `syn-*` and `code` are for the code viewer and inline code.
- `term-bg` and `term-fg` are the terminal grid's own ground and text, dark in both themes; terminal chrome only.

## Type

- One family, the platform UI font (`--font-sans`); `--font-mono` for terminals, diffs, tool calls, branches and paths.
- Six steps: `text-2xs` (badges, counts, section and rail labels), `text-xs` (metadata, controls, summaries), `text-sm` (row titles, the base size), `text-md` (the conversation, 720 px wide at most), `text-lg` (pane and session titles), `text-xl` (page titles).
- `tabular-nums` for ages, counts, money, percentages and quotas.

## Spacing, size and shape

- 4 px base: `space-1` 4, `space-2` 8, `space-3` 12, `space-4` 16, `space-6` 24.
- Controls are `control-h` (24 px). `control-h-sm` (22 px) only inside a row; `control-h-lg` (28 px) for dialog primaries, the composer send and the list search.
- Radii: `radius-sm` for buttons and chips, `radius-md` for rows, banners and dialogs, `radius-lg` for loader stages and toasts, `radius-pill` only for dots and meters.
- Borders before shadows: panes are separated by a `border` hairline. Only menus, toasts and the ⌘K palette take `shadow-pop`.
- Focus: a 2 px solid `ring` with a 1 px offset on every focusable element.

## Layout

- Desktop: header (`header-h` 44 px), then rail (`rail-w` 68 px), the left list (`list-w` 340 px), the main pane and the inspector (280 to 320 px), then the status bar (`status-h` 25 px).
- The rail order is Control, Inbox, Sessions, Work, Automation, Accounts, Toolkit, then Settings at the bottom. Control opens the coordinator chat with its switchable right panel.
- The left list always keeps its filters and grouping panel (`ListFilters`). Lists group by what needs the person: Needs you first, sorted by when it asked, then Working, Idle and Done.
- One object in focus: selecting a row opens it in the main pane; the inspector holds its facts and actions.
- Nothing is removed, only moved: every feature and every 0.5.3 shortcut keeps working (see Keyboard).

## Motion

UI motion runs 80 to 280 ms (`dur-fast`, `dur-base`, `dur-slow`) and stays out of the way. Loaders follow the Motion section: shown only after `loader-delay`, one per screen, chosen by what the app is really doing. Reduced motion turns every loop into a 2.4 s fade.

## Iconography

- 16-unit inline SVG strokes at 1.5 px in `currentColor`, drawn at 12 to 18 px. The rail icons in `Rail` are the reference set.
- Compact markers are text glyphs: ✓ ✗ … for CI and checks, ▾ for menus, × to remove a chip, ✦ for a Jev proposal, ✎ for an LLM draft.
- The agent tab uses the agent's own mark in its own colour (`agent-claude` for Claude Code).
- The logo is the Orbit mark in `assets/Logos`; the name is set in the UI font at weight 600 beside it.

## On the phone (fleet-mobile)

The same tokens, themes, status words and loaders, at touch size. Boards: the "Mobile app" row of the redesign canvas.

- **Navigation:** `BottomBar` with Inbox, Sessions, Control, Work, More. Inbox opens first and carries the only badge, the Needs you count.
- **Size:** every target is at least `touch-min` (48 px); screens and sheets use `phone-gutter`; rows use `PhoneRow`; dialogs are a `BottomSheet`; a `ChatForm` from Control opens as a sheet.
- **Notifications** say what the session waits on and open the question card. They never carry Approve.
- **Loaders:** Orbit for pull-to-refresh, Signal lost for an offline hub or host (static, with Retry), Gravity well while reconnecting, Progress ring with the real size for downloads, Sonar while checking a host, a skeleton with Dot wave for a loading conversation. All after `loader-delay`.
- **Pairing** is QR-first with the Draw-on mark, Halo on success, then the notification permission with a reason.
