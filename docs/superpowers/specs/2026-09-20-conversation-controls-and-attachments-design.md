# Conversation controls and attachments: one control system, one bar, one box

**Date:** 2026-09-20
**Status:** Design
**Touches:** `src/app.css`, new `src/lib/controls.css`, `ConversationHeader.svelte`,
`ConversationPanel.svelte`, `PromptComposer.svelte`, `SegmentedControl.svelte`,
`HostChips.svelte`, `CopyButton.svelte`, `TransferChip.svelte`, `attention.ts`,
`App.svelte`, plus a new attachment path in `src-tauri/` and `crates/fleet-core/`.
**Delivery:** one PR, seven commits — one per stage (the author asked for a
single PR after the review cost was put to them).

## Goal

The conversation surfaces get one control vocabulary instead of four, one
sticky bar instead of two, and a composer that can hold an attachment.

Today the Conversations tab teaches a rule in the composer — a bordered pill is
a button you can press — and breaks it forty pixels above, where the model and
status pills are `<span>`s you cannot press. That is the clearest symptom; the
measurements behind it are worse. Across `src/` there are **14 distinct
`border-radius` values**, 34 font sizes, and **11 distinct control heights on
these three surfaces alone**, of which exactly one clears the 24×24 CSS px
minimum in WCAG 2.5.8 — and it is the Send button, which a keyboard-first tool's
users almost never click, because Enter sends.

## Decisions already made

- **Attachments land under the session's worktree root**, not in
  `~/.claude-fleet/uploads/<session>/` where `upload_to_session` puts terminal
  drops today. A path outside the working directory makes Claude Code ask for
  permission before reading it; in the terminal a human is sitting there to
  approve, but a prompt sent from the composer has nobody to answer.
- **The webview never names a local path.** The `UploadAllowList` threat model
  (SEC-9) stays whole: a Rust-side file picker records its own result into the
  allow-list, so the existing "only the OS handed us this path" invariant
  survives the new entry point.
- **Hub-client mode disables the attach control with a stated reason** rather
  than carrying bytes to the hub. See *Deferred* below.
- **Send stays where Enter is the real interface.** It becomes a 28px icon
  button inside the composer shell, not a wide filled bar.

## Non-goals

- No CSS framework and no utility classes. The app is ~55k lines of hand-written
  Svelte with scoped styles and a working token layer; the problem is that the
  tokens stop at colour. Extending them to geometry is ~120 lines.
- No restyling of the transcript itself — turns, tool lines, subagent blocks and
  diffs are out of scope. This is chrome.
- No animation budget increase. See *What this must not become*.

## Constraint: the testid contract

The conversation surfaces are wrapped in ~2 960 lines of Vitest
(`ConversationPanel.test.ts` alone is 2 571). The tests bind to `data-testid`,
not to structure. Every testid that exists today **must still exist, on an
element with the same role and the same behaviour**, after the redesign —
including the ones that change parent in stage 2: `conv-header`,
`conv-find-button`, `conv-find-input`, `conv-find-count`, `conv-find-prev`,
`conv-find-next`, `conv-find-close`, `conv-turns-button`, `conv-turn-index`,
`conv-turn-index-item`, `conv-composer-send`, `conv-chip`, `conv-chip-enter`,
`conv-model`, `conv-status`, `conv-ctx`.

A stage that needs a testid to move files is a stage that edits markup and
leaves the assertions alone. If an assertion has to change, that is a signal the
behaviour changed, and the change gets justified in the commit — not absorbed.

## Stage 0 — the fixes that wait for nothing

Four independent corrections, additive, no component restructuring.

1. **The context meter's healthy state fails contrast in light mode.**
   `attention.ts:96` returns a hardcoded `'#50c86e'` for `level === 'ok'`, used
   as both the `.ctx-pct` text colour at `0.68rem` (9.5px) and the `.ctx-bar`
   fill. Against `--bg-pane` `#fafafa` that is **2.05:1** — below 4.5:1 for text
   and below 3:1 for the bar. Dark mode is fine (8.47:1), which is why it
   survived. Meanwhile `app.css:14` carefully documents that `--usage-warn` is
   4.81:1 and `--usage-crit` 5.39:1: the most-shown state is the one that
   bypassed the tokens. Add `--usage-ok` (`#2e7d32` light = 4.91:1, `#5dd17a`
   dark = 9.37:1) and return `var(--usage-ok)`.
2. **`--mono` is referenced 19 times and defined nowhere.** Eleven declarations
   read `var(--mono, ui-monospace, …)` and eight hardcode the stack. Define it
   once on `:root`.
3. **`conv-retry` has no class.** `ConversationPanel.svelte:1052` is a bare
   `<button>`, and the app has no global `button` rule, so it renders as a
   native macOS push button — the only one in the panel, ignoring dark mode,
   and it appears on the error path.
4. **A file dropped outside a drop target navigates the webview.** In a
   WKWebView, an unhandled drop sends the window to `file://…` and the app state
   is gone: no router, no recovery. Install a window-level `dragover`/`drop`
   swallow in `App.svelte`'s `onMount`. This is a bug today, before any
   attachment work exists.

## Stage 1 — tokens and `controls.css`

Geometry is theme-independent and goes on `:root` once: `--control-h: 24px`,
`--control-h-lg: 28px`, `--control-px`, `--control-px-lg`, `--control-gap`,
`--radius-sm: 4px`, `--radius-md: 6px`, `--radius-pill: 999px`,
`--control-font: 12px`, `--control-font-sm: 11px`, `--ring-w`, `--ring-offset`,
`--mono`.

Colour is per-theme and must be added to **all four** blocks the file already
duplicates (`:root`, the `prefers-color-scheme: dark` block,
`:root[data-theme='light']`, `:root[data-theme='dark']`):
`--control-bg`, `--control-bg-hover`, `--control-bg-active`, `--control-border`,
`--control-border-strong`, `--control-fg`, `--control-fg-quiet`, `--accent-fg`,
`--accent-soft`, `--ring`, `--usage-ok`.

Two rules fall out of the measurements and both go in a comment next to the
tokens:

- **`--border` cannot carry state.** `#e5e5e5` on `#ffffff` is **1.26:1**;
  `#262626` on `#0f0f0f` is **1.27:1**. Every bordered control in the app
  currently announces itself with a hairline that is, by the numbers, invisible.
  `--control-border` (1.56:1) may *separate* things; `--control-border-strong`
  (3.14:1 light / 3.55:1 dark) or `--accent` must carry anything you need to
  read.
- **The surface scale carries no information.** `--bg` to `--bg-pane` is
  **1.04:1** light and 1.06:1 dark, so `.composer` over `.textarea` is optically
  one flat field. Hover and active states use `--control-bg-hover` /
  `--control-bg-active` fills, which are perceivable and are also the macOS
  idiom, instead of a border change nobody can see.

`src/lib/controls.css`, imported from `main.ts` after `app.css`, holds four
primitives as global classes. They are global deliberately: this is chrome, and
per-component scoped copies are exactly how the app arrived at four primary
buttons.

- `.btn` — the shared geometry. `height: var(--control-h)`, `border: 1px solid
  transparent` always present so hover never shifts layout, `line-height: 1` so
  the box is set by `height` and never by the line box. One focus ring for the
  whole app: `outline: var(--ring-w) solid var(--ring)`, which clears 3:1 in
  both themes (5.17:1 light, 7.54:1 dark). Inside a clipping group the offset
  goes negative.
- `.btn--primary` — 28px, filled `--accent`, `--accent-fg` text (5.17:1 /
  7.37:1), weight 600. **One per surface.** Replaces the composer Send, the
  `PromptComposer` outlined `.primary`, `ImportDialog`'s `color: white; border: 0`
  variant, and `NewBgSessionDialog`'s three `!important` declarations.
- `.btn--quiet` — the workhorse: toolbar buttons, menu triggers, secondary
  actions. Keeps `.tb-btn`'s transparent-border trick, which is the one thing
  that rule gets right, but moves the hover signal from a 1.26:1 border to a
  fill.
- `.btn--toggle` — pill, driven by `aria-pressed`. **The only thing allowed
  `--radius-pill`.** Selected state is carried by `border-color: var(--accent)`
  (4.95:1), with `--accent-soft` as reinforcement — today `SegmentedControl`'s
  14% tint is 1.22:1 against its unselected siblings and is decorative, not
  informative.
- `.btn--icon` — square by construction, `width: var(--control-h)`, so a close
  button cannot end up 13×13px the way `.dismiss` is today. Requires
  `aria-label` **and** `title`.
- `.btn--warn` / `.btn--crit` — tone modifiers, composable, not new primitives.
- `.tag` — non-interactive information. **No border, no pill, no box.** This is
  the fix for the affordance lie: once `.tag` has no box, "bordered pill ⇒
  clickable" becomes true across the app and the other affordance questions
  answer themselves.

One disabled convention replaces four opacities (0.4 / 0.45 / 0.5 / 0.6) and
three cursors (`default` / `not-allowed` / `progress`, the last on a *disabled*
button in `Sidebar`). Opacity never goes below 0.55: at 0.4, `--fg-muted` on
`--bg-pane` composites to ≈1.75:1, which puts the reason for the disabling
inside a control the user cannot see is there.

Where the reason matters — `PromptComposer`'s Send carries `title={sendBlocked
?? ''}`, a tooltip no keyboard or screen-reader user ever receives — use
`aria-disabled` with an `aria-describedby` pointing at the existing
`.composer-status` line, which is already `role="status"`.

**Testing.** CSS is not unit-testable, so the token values live in a TS table
that `controls.css` mirrors, and a Vitest spec computes WCAG relative luminance
for every documented pair and asserts the floor. That is what stops the next
`#50c86e` from reaching a release: stage 0 fixes the instance, stage 1 fixes the
class of bug.

Nothing changes visually in stage 1. It is purely additive.

## Stage 2 — one bar

Today `.conv-header` and `.toolbar` stack to ≈58px of permanent chrome with two
bottom hairlines ≈24px apart, over two surfaces that differ by 1.04:1 — so what
the eye reports is two parallel grey lines very close together with no surface
reason for the split. In a pane that is often 400–600px tall that is 10–15% of
the reading area spent on a seam.

Worth stating because it changes behaviour, not just looks: **`.conv-header`'s
`position: sticky` is currently a no-op.** It is a direct flex child of
`.conversation-panel` (`ConversationPanel.svelte:956`), which is
`display: flex; flex-direction: column` and not a scroll container. Only
`.toolbar`, which lives inside `.scroller`, actually sticks. After the merge the
header sticks for the first time.

Two more defects the merge fixes:

- `.toolbar` is `justify-content: flex-end` but `.find`, which replaces it in
  the same slot, stretches its input with `flex: 1 1 auto`. Pressing ⌘F
  relocates the controls from the right edge to the left edge of a bar that can
  be 640px wide — a full-width pointer jump on a keyboard shortcut.
- `{#if findOpen} … {:else if conv}` are mutually exclusive, so **`N turns`
  disappears entirely while Find is open** — exactly when jumping to a turn is
  most useful.

Target: one 32px bar on `--bg-pane` with one bottom hairline. Left to right:
switcher · meter · model · status · last event (elastic, the only thing allowed
to truncate) · `⌕` · `N turns`. The tool cluster is pinned right and **does not
move when Find opens** — Find expands into the middle slot where the facts live,
and `N turns` stays reachable.

The header's `.chip` spans become `.tag`. The context meter keeps its pill,
because it is a meter and not a chip, but takes the full colour for its border
instead of `contextTint()`'s ≈33% wash (≈1.3:1); `contextTint` loses its only
consumer and goes.

One inset expression — `max(1.1rem, calc((100% - var(--chat-col)) / 2 + 1.1rem))`
— is shared by `.conv-header`, `.thread` and `.composer`. Today `.thread`
applies `padding: 1rem 1.1rem` *inside* its 80ch box while the composer spans
the full 80ch, so **the textarea's left edge sits 15px to the left of every
prompt bubble it produces**.

Delete `.toolbar` and `.find` from `ConversationPanel.svelte`.

## Stage 3 — the composer shell

A `.composer-shell` owns the border; the textarea is borderless inside it;
`:focus-within` on the shell shows focus. This is the stage that makes
attachments possible, because today `.composer-row` is a flex row with a
textarea and a sibling button and there is nowhere for a thumbnail to live that
is visually *inside* the input.

- **Send moves inside**, bottom-right, 28px icon. It reclaims ~90px of width in
  a pane that is often 400px wide, and it stops overstating a button that exists
  as a mouse fallback for Enter.
- **The placeholder shrinks and the hint moves out of it.** The current
  placeholder is 103 characters; at 12px in a 400px pane you see about a third
  of it, and it vanishes the moment you type. `↵ send · ⇧↵ newline · ↑ history`
  sits next to the button, hidden only under `@container chat (max-width: 26rem)`
  — the container is already declared (`container-name: chat`).
- **Preset chips stay above the shell**, in one row. They are sources for the
  box, not modifiers of it (`usePreset` fills the draft; Shift+click sends), so
  putting them inside would misstate the relationship. What does not fit
  collapses into a trailing **`More ▾`** toggle that reveals a second row —
  chosen over wrapping because a wrapping row changes the composer's height
  as the preset list changes, and the author asked for the layout to hold still.
- **`⏎ Press Enter` leaves the preset row.** It is not a prompt template; it is
  an escape hatch that appears exactly when `liveStuck === 'press_enter'`, i.e.
  when the session is frozen. It gets its own full-width `.btn--warn` row
  directly above the shell, so it reads as an alert with an action.
- **`.chip.suggest`'s glow goes.** `box-shadow: 0 0 0 2px color-mix(…)` is
  visually a focus ring, and `SegmentedControl` uses a 2px ring to mean focus.
  A *suggestion* must not outshout a *focus indicator*. Nothing but
  `:focus-visible` gets a 2px ring anywhere in the app.
- **`preserveThread()`** compensates `scrollTop` whenever the composer's height
  changes. `.composer` is `flex: 0 0 auto` at the bottom of a column flex and
  `.thread-area` is `flex: 1 1 auto`, so the composer growing by 50px shrinks the
  scroller by 50px and slides the transcript under the reader's eye. The state
  it needs — `scroller`, `atBottom` — already exists in the panel.

## Stage 4 — the rest of the app onto the primitives

`PromptComposer`, `SegmentedControl`, `HostChips`, `CopyButton`, `TransferChip`,
and the primary buttons in `ImportDialog` and `NewBgSessionDialog`.

This is where the four contradictory primary conventions collapse to one. The
sharpest case: `PromptComposer`'s Send and `ConversationPanel`'s Send are **the
same action** — send a prompt to a session — drawn today as outlined vs filled,
radius 4 vs 6, `0.85rem/400` vs `0.8rem/600`.

`CopyButton` becomes icon-only at 24×24. It is currently a 16.7px target that is
`opacity: 0` until hovered — a target you must first find by hovering.

## Stage 5 — attachments, backend

The send path, for the record: `sendPrompt` → `send_prompt` →
`service/sessions/prompt.rs` → `build_send_commands`, which is
`tmux send-keys -t <pane> -l <text>`, `sleep 0.15`, `tmux send-keys Enter`,
joined with `&&` into one script and, for a remote host, quoted a second time
as a single `bash -lc` word.

**A prompt is capped at ~128 KiB on Linux hosts** and nothing validates it. The
whole script rides inside one argv word and `MAX_ARG_STRLEN` is 128 KiB — the
carry engine already documents this at `carry.rs:46`. Attachment *paths* are
small, so this design is safe, but stage 5 adds the bound and a typed error
instead of a raw `E_TMUX` "Argument list too long". Inlining file *contents*
into a prompt is out of scope for exactly this reason.

What gets built:

- **A Rust-side picker** using `tauri-plugin-dialog`'s Rust API, which records
  its result into `UploadAllowList` before returning handles to the webview.
  `dialog:allow-open` is already granted in `capabilities/default.json`;
  no capability change is needed. The frontend receives opaque handles plus
  display metadata, never a path it could have invented.
- **A thumbnail command.** There is no `tauri-plugin-fs`, so the webview cannot
  read bytes; Rust decodes and downsamples an allow-listed image and returns it.
  This suits the security model rather than fighting it.
- **Landing under the worktree root.** Resolve it the way every Files-tab read
  already does — `tmux display-message -p '#{pane_current_path}'` then
  `git rev-parse --show-toplevel` (`service/repo.rs:56`) — because `SessionRow`
  carries `worktree_id` and `worktree_key`, an id and a name, **not a path**.
  Files go to `<root>/.claude-fleet-attachments/` with collision-free basenames,
  and the directory is added to the repo's `.git/info/exclude` rather than its
  tracked `.gitignore`, so an attachment never shows up as a change the user has
  to explain. Basenames are validated: a filename containing a newline would
  otherwise travel into the prompt text.
- **Hub parity.** A verdict row for each new command, `refuse_local_only` **by
  command name**, `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib
  verdict_gen` (which fails once by design so the diff is read), and
  `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` — the reference is
  generated from `generate_handler!` as well as from the MCP router, so *any*
  new Tauri command triggers it.
- **A `REASONS` entry in `hub.ts`.** `upload_to_session` currently takes the
  allowlist route in `hub_verdicts.test.ts` because `TerminalView`'s whole pane
  is swapped out in hub mode. **The composer is not swapped out** — it renders
  normally — so an attach button there would be visible and broken. It needs a
  real reason and a disabled affordance, not an allowlist line.

## Stage 6 — attachments, UI

No `ondrop`, `ondragover`, `onpaste` or `DataTransfer` handler exists anywhere
in `src/` today, so this is greenfield.

- **Strip of 44×44 tiles** inside the shell, above the textarea. Uniform size
  so images and non-images read as one row; non-images show their extension in
  `--mono`. Exactly 0, 50 or 94px tall — two rows, then scroll — so the
  composer's growth is quantised and `preserveThread()` corrects by a clean
  integer instead of chasing a pixel-at-a-time creep as thumbnails decode.
- **The tile is reserved before the thumbnail decodes.** Push the attachment
  with `thumb: null, state: 'reading'` first, fill `thumb` on load. Zero reflow
  from decoding. Object URLs are revoked on remove and on teardown, or a long
  session leaks every screenshot ever pasted.
- **Drop target is the shell, not the panel.** `dragleave` fires for every child
  element, so nesting is tracked with a depth counter, not a boolean.
- **Paste**: files win only when `text/plain` is empty, so pasting from a
  rich-text source does not swallow the text half. Screenshots arrive named
  `image.png`; rename to `pasted-<HH.mm.ss>.png` so ten pastes are
  distinguishable.
- **Remove is 18×18 on a scrim**, visible on `:hover` **and** `:focus-within` —
  never hover-only, or a keyboard user cannot remove an attachment at all. This
  is the one deliberate exception to the 24px floor: the 44px tile is the
  primary target and Backspace on a focused tile also removes it, which is
  WCAG 2.5.8's equivalent-control path. It is bounded to this case.
- **Errors are per-tile, never a toast**, with the reason in a status line: over
  the per-file limit, over the total, or unsupported. The failed tile *stays* in
  the strip with its remove button, because auto-removing it looks like the drop
  silently did not happen.
- **On send**, the files upload first, then their absolute remote paths are
  appended to the prompt text. An upload failure cancels the send and leaves the
  draft intact.

## What this must not become

- **Do not inflate the controls.** 24px is the floor *and* the answer. Ten
  controls at 24px in a 32px bar is dense and correct; the same ten at 36px is a
  web app, and this is a tmux orchestration tool.
- **Do not add shadows.** The app has exactly three, all on overlays that
  genuinely float (`.menu`, `.turn-index`, `.latest`). That is the correct
  budget. There is a real surface scale now — use it.
- **Do not animate.** No transition on `height`, `max-height` or `width`: the
  scroll compensation has to land on a settled layout. The existing budget —
  0.1s on `.copy-slot` opacity, 0.12s on the tools caret, with
  `prefers-reduced-motion` already honoured — is right. Extend the
  reduced-motion block to anything new.
- **Do not round everything.** `--radius-pill` means "this is a toggle" (plus
  the meter). Today it is on `.latest`, both `.chip`s, `.host-pick`, `.pill` and
  `.ctx`, which is to say it means nothing.
- **Do not keep the keyboard hints out of sight.** `↵ send · ⇧↵ newline · ↑
  history` and `aria-keyshortcuts` earn their 11px. In this app the hints are
  the documentation.

## Verification

`scripts/ci-local.sh` in CI order, and the **full** suite per stage — not a
filtered run. Filtered per-task tests have hidden a red test here before.

Component behaviour is verified with Vitest only. **Do not run a dev build of
the desktop app on this machine**: the singleton guard SIGTERMs the installed
app, and with the real `HOME` it migrates the production `state.db`
irreversibly. The attachment path therefore ships with component-level coverage
of the composer and service-level coverage of the upload, and the
drop→upload→prompt round trip is exercised against a released build, not a dev
one.

Three generated artefacts must be regenerated or CI fails: the hub verdicts
(JSON + the `docs/hub.md` table + its "Of the N commands…" sentence), the
control API reference, and — if any wire type's keys change —
`REGEN_HUB_CONTRACT=1`.

## Deferred, deliberately

**Attachments in hub-client mode.** Making them work means a new MCP tool
carrying base64 arguments, which comes out of the 56 000-byte served-definition
budget, plus a hub→agent `HubFrame::Upload` hop. The frames are JSON-only with a
200 MiB payload cap, and one frame at the cap is ~267 MiB of `String` on each
side: a 10 MB screenshot is fine, but this is not a streaming file transfer.
Until someone paired to a hub actually wants to attach a file, the honest
verdict is `LocalOnly` with a sentence that says why — the same verdict
`upload_to_session` already carries.

**Sessions on agent-only hosts** cannot receive attachments in any mode: the
desktop has no SSH route to them. Same treatment — a stated reason, not a
silently dead button.

**Chunked upload.** `move_session` chunks its reads but writes with a single
`put`; the transfer roadmap already lists this as an open debt. Attachments
inherit it and stay within the per-file limit rather than fixing it here.
