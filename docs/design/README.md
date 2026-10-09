# Design tokens

`tokens.json` is the snapshot of the Orbit Fleet design manual's tokens that the
code follows: colour (both themes), type, spacing, radius, shadow and durations.
The manual is the source of truth (redesign plan, ground rule 8); its git copy
lives in `docs/ux/2026-10-08-orbit-fleet-redesign/design-system/`.

`src/app.css` declares every token here under the same name, and
`src/lib/tokens.ts` holds the colours as data for the contrast suite.
`src/lib/tokens.test.ts` fails when the three disagree, when `app.css`
declares a token this file does not name, or when a documented pair falls
below its WCAG floor in either theme. Besides the manual's sections the file
has `size` (control padding, ring width, pane and button sizes), `type.aliases`
(`mono`) and the scrims (`scrim` behind a dialog, `scrim-strong` for the
tour), which the app had before the manual named them (review r10).

To change a token: change it in the manual and this snapshot first, then in all
four theme blocks of `app.css`, then in `tokens.ts`, and add any new pair it has
to clear to `CONTRAST_PAIRS`. The phone's `FleetTheme.kt` follows the same
snapshot (step 0.10); fleet-mobile's copy of this file has not taken the r10
additions yet, which name no token the phone draws.

## Loaders

`src/lib/Loader.svelte` is the manual's loader kit: the 24 loaders of its
Loader board by name (`<Loader name="comet" size={12} />`; the default is the
Orbit). Their markup and animations are generated from the board's
`preview.html` and `bundle.css` into `src/lib/loader-kit.generated.{ts,css}`
by `src/lib/loader-kit-extract.ts`; `src/lib/loader-kit.css` adds the frame
(stage, reduced-motion fade, pause, the determinate ring). After the manual's
Loader board changes, regenerate (the run fails on purpose; run it again):

```bash
REGEN_LOADERS=1 pnpm exec vitest run src/lib/loader-kit.test.ts
```

A loader appears only after 400 ms and keeps its box until then. Reduced and
Off motion turn every loop into one 2.4 s fade. Inside a row, a button or the
status bar only the Comet or the 16 px Orbit may be used
(`src/lib/loader-use.test.ts`).

## Component kit

`src/lib/kit/` is the manual's component kit in Svelte: Button, Kbd,
StatusChip (with StatusDot and Count), Banner, QuestionCard, SessionRow,
ListFilters, Rail, AppHeader, StatusBar, Tabs, KeyValue, Meter, OrbitMark and
Icon (the 16-unit, 1.5 px set). They render the manual's `of-` classes, whose
CSS, `src/lib/kit/of.generated.css`, is `bundle.css` verbatim (up to its AI
section, which lands with AISuggestion in 3.11). After the manual's
bundle.css changes, regenerate (the run fails on purpose; run it again):

```bash
REGEN_KIT=1 pnpm exec vitest run src/lib/kit/kit.test.ts
```

`kit.test.ts` measures every piece of text each component renders, in every
state and both themes, against 4.5:1, with no exceptions: the three pairs that
fell short (fg-muted on bg-hover, fg-muted on failed-soft, status-failed on
accent-soft) were fixed in the manual's bundle.css by having the component read
fg-2 or fg in that state, not by moving a token value. `KNOWN_SHORTFALLS` is
empty and a new shortfall fails.

The app draws icons only from the kit's set (`src/lib/kit/icons.ts`, the
manual's 16-unit, 1.5 px strokes); the old 24-unit `src/lib/Icon.svelte` is
gone. A glyph the app needs is added there, drawn on the same grid. The copy
lint (`copy_lint.test.ts`) refuses a pictograph in component markup (⚠ 🔗 🔍
⏸ …) and a seventh status word (Blocked, Stuck, Queued, Ready, …) written as
a label; Blocked reads "Needs you · blocked on …". The status bar, the rail,
the hub banners, the key/value facts of a session and a host, and the context,
disk and mission meters are the kit's StatusBar, Rail, Banner, KeyValue and
Meter. The terminal grid's own colours are the manual's `term-bg` and
`term-fg`.
