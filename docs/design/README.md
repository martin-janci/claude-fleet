# Design tokens

`tokens.json` is the snapshot of the Orbit Fleet design manual's tokens that the
code follows: colour (both themes), type, spacing, radius, shadow and durations.
The manual is the source of truth (redesign plan, ground rule 8); its git copy
lives in `docs/ux/2026-10-08-orbit-fleet-redesign/design-system/`.

`src/app.css` declares every token here under the same name, and
`src/lib/tokens.ts` holds the colours as data for the contrast suite.
`src/lib/tokens.test.ts` fails when the three disagree, or when a documented
pair falls below its WCAG floor in either theme.

To change a token: change it in the manual and this snapshot first, then in all
four theme blocks of `app.css`, then in `tokens.ts`, and add any new pair it has
to clear to `CONTRAST_PAIRS`. The phone's `FleetTheme.kt` follows the same
snapshot (step 0.10).

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
