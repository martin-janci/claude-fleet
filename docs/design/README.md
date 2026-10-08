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
