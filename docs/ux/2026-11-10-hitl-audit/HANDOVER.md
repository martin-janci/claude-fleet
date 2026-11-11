# Handover: human-in-the-loop audit → implementation

For the agent that picks this up. Read `README.md` first: findings H-01…H-40 with
`file:line`, the target boards, and the wave plan (§6). This file says how to start.

## State

- Audit and target design: done. Wave 0 landed on this branch (below).
- Boards: `boards/*.dc.html` (same format as `docs/ux/2026-10-08-orbit-fleet-redesign/canvas`).
  Live canvas, private to the owner: <https://claude.ai/artifact/XJx4FE3DVFx8YeEroWDpU3>.
  Renders: `images/board-*.webp`. Screenshots of today's apps: `images/today-*.webp`.
- Open owner decisions: README §8. Waves 0 and 1.2–1.9 do not depend on them;
  1.1 (the leash) depends on decision 1.

## Progress (2026-11-11)

Done on this branch, each with tests: H-25 (diff blank lines), H-57/H-58
(a card never focuses Approve; Escape never denies), H-11/H-12 (row says
why, dot is the state; phone rows too), H-15 (row keeps its name, risky
hover actions in the ⋯ menu), H-08 review asks counted, H-07 in-window
toast, H-13 card headline from the tool call and wrapping answers, H-17
failed clean up is Failed (hub, desktop, phone), H-16 Inbox header adds
up, H-02 confirm-card wording, H-22 Escape leaves select mode.

Also done: the session header keeps its title and reads the row's state
(H-15, `SessionTabs.svelte`), the status bar names a host that is down
instead of "All systems OK" (H-16, `app_version.ts`), and the phone's
question card prints the command once with answers that wrap, plus one-line
bottom-bar labels (H-33, H-36; fleet-mobile `QuestionCard.kt`, `BottomBar.kt`).

**Wave 0 is complete.** Next is Wave 1 (README §6). 1.1, the leash (ask before
a step a person owns), waits on owner decision 1 in README §8; the audit
recommends asking. 1.2–1.9 can start now.

Memory: the fleet-core test target needs most of the 16 GB box. Stop any
Gradle daemon (`./gradlew --stop`) and run nothing beside
`cargo test -p fleet-core --lib -j 1 -- <filter>`.

## Where to start

Wave 0 (README §6), one PR per row, in this order: 0.1 (diff blank lines, H-25:
move `white-space: pre` from `.diff` to `.text` in `src/lib/DiffView.svelte`),
0.2 + 0.3 (row line two and the dot, `src/lib/SessionRowItem.svelte`), 0.4, 0.6,
then the rest. Phone rows (0.10) go to fleet-mobile in step with the desktop
(`PhoneSessions.kt` `phoneLead` / `waitsOn`).

Before a subsystem you have not touched, read `docs/architecture.md` and
`docs/status.md` as CLAUDE.md says. Rules that bind every change (README §5):
the six status words, amber only for waiting, Approve never primary or
pre-selected, no Approve on a notification, nothing decided by AI, every existing
shortcut kept.

## Seeing the UI without Tauri

```bash
pnpm install --frozen-lockfile
node scripts/federation-e2e.mjs --only own-hub     # the repo's harness, works headless
node docs/ux/2026-11-10-hitl-audit/tools/capture-inbox.mjs inbox   # 25-session world, every attention state
HITL_OUT=/tmp/x node docs/ux/2026-11-10-hitl-audit/tools/capture-inbox.mjs answer
node docs/ux/2026-11-10-hitl-audit/tools/render-boards.cjs /tmp/boards   # render the target boards
```

`tools/capture-lib.mjs` wraps `e2e/federation`: per run a world, a theme, prefs,
and an extra JS file that sets `window.__FLEET_EXTRA__ = { command: (args, fake) => answer }`
for commands the repo's fake does not know. `world-inbox.mjs` / `extra-inbox.js`
are the world used for the Inbox and conversation screenshots. Take answer shapes
from the code (the store calling the command, its tests), never guess.

Phone: the JVM preview tests (`shared/src/jvmTest/.../SessionsTabPreviewTest.kt`)
render Compose to PNG without an emulator: `./gradlew :shared:jvmTest --tests '*PreviewTest*'`.
If Maven Central answers 429, use an init script that points at
`maven-central.storage-download.googleapis.com`. Wrap screens in `Surface`, as `App.kt`
does, or dark text renders black on black.

## Checks that prove a wave landed

Each README §6 row has a check. Re-run the captures after a wave and compare with
`images/today-*.webp`.
