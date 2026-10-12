# Orbit Fleet redesign (2026-10-08)

This folder holds the redesign of the desktop app in git: the full transition plan, the
Orbit Fleet design system, and a copy of every board on the design canvas. The live
artifacts on claude.ai are still the place where the work is edited. These files are
snapshots taken from them on 2026-10-08 (refreshed the same evening with the phone app), so a PR can cite a token, a component rule or
a board without leaving the repository.

## What is here

| Path | What it is | Live source |
|---|---|---|
| [`transition-plan.md`](transition-plan.md) | The plan from 0.5.4 to the redesign, A to Z. It has 15 milestones (M0 to M14, where M14 is the phone app in fleet-mobile), about 165 PR-sized steps, migrations 121 to 142, hub contract revisions 11 to 15, a task graph with 13 lanes over 9 waves, risks and decisions | [Claude Doc](https://claude.ai/artifact/FakUr921wZQNr3MVeK7vnu) |
| [`images/`](images) | The plan's two diagrams (roadmap and task graph) as static SVG | drawn in the doc |
| [`design-system/`](design-system) | The Orbit Fleet design manual: [`README.md`](design-system/README.md) (content, colour, type, layout, icons), [`tokens.json`](design-system/tokens.json), [`motion.md`](design-system/motion.md), [`ai.md`](design-system/ai.md), [`keyboard.md`](design-system/keyboard.md), its "On the phone" section, and 20 components (plus the cover card; BottomBar, PhoneRow and BottomSheet are the phone's) with their `of-` classes in [`components/bundle.css`](design-system/components/bundle.css) | [Design System](https://claude.ai/artifact/RecYyvBJYdXVpLC1oD4bpb) |
| [`canvas/`](canvas) | All 90 boards of the redesign canvas (`*.dc.html`), including the 26 of the "Mobile app" row (`Mobile*.dc.html`) and the 9 Forms boards added on 2026-10-09 (`Forms*.dc.html`, `MobileForms*.dc.html`), and the board layout (`canvas.json`). Each file is the board's source. The boards need the canvas runtime (`support.js`, which is not copied), so open the live canvas to see them drawn | [Design canvas](https://claude.ai/artifact/B2sVtJEZodahNG4cvRu7Pu) |

Step 0.5 of the plan landed with a copy of `design-system/tokens.json` at
[`docs/design/tokens.json`](../../design/tokens.json): that copy is what `src/app.css`
and `tokens.ts` follow, and `src/lib/tokens.test.ts` fails when they drift from it.
Nothing in the build reads this folder itself.

## Other references

- [Orbit Fleet gap plan](https://claude.ai/code/artifact/c4caae1a-3fea-404d-8241-56d6e97771bb): milestone M15, 57 steps (G0.1 to G6.2) in 7 iterations and 8 lanes that close what the canvas shows and the apps lack. Its evidence is the board-by-board audit in [`docs/redesign/canvas-gaps-2026-10-09.md`](../../redesign/canvas-gaps-2026-10-09.md). The audit re-run after the plan landed is [`docs/redesign/canvas-gaps-2026-10-10.md`](../../redesign/canvas-gaps-2026-10-10.md).
- [Screen inventory and UX plan](https://claude.ai/code/artifact/2c086832-200f-4f52-a69b-fedd67921530): about 60 screens, the "nothing removed, only moved" parity table, motion rules, and the 47-step UX plan that the transition plan builds on.
- [AI map](https://claude.ai/artifact/JZsDwLPdMJdh97jPb6g92Y): every AI idea for each screen. All of them are in the transition plan's AI section, and the top 10 are steps.
- [Earlier UX review](https://claude.ai/code/artifact/11727098-ce9d-41d5-a4d8-83512feaaf1f) and the [Fleet design system for 0.5.3](https://claude.ai/artifact/KvZWoiomzJDRFetDxcb81x), which shows what shipped before the redesign.
- [Multi-account token design](https://claude.ai/code/artifact/38f5422c-3bc6-4eaa-8d3b-dd149bec390e): the direction behind the Accounts milestone (M4).

## Decisions already made

- The app is named **Orbit Fleet**, and its logo is the **Orbit** mark: a hub, one orbit and three hosts, with the amber host being the one waiting on you. The display name changes, but the bundle id and data paths stay the same.
- **Constraints**:
  - The left filter and grouping panel stays.
  - No function is lost, only moved. Each PR carries a parity checklist, and a Classic/New switch stays until parity holds.
  - Fleet is sessions-first orchestration, not a task manager.
  - Agents are interchangeable: the agent tab is named after the agent (Claude Code, then Codex and Agy). Shell terminals are separate, 0..N per session.
  - Control is a mission-control coordinator chat with switchable views.
- **Rail order**: Control, Inbox, Sessions, Work, Automation, Accounts, Toolkit, then Settings at the bottom.
- **Shortcuts**: every 0.5.3 chord keeps working (⌘K/⌘P, ⌘N, ⌘J, ⌘I, ⌘E now opens Control, ⌘,, ⌘⇧W, ⌘⇧O, ⌘⇧T). The new chords, Mac first and then Linux/Windows:

  | Action | Mac | Linux/Windows |
  |---|---|---|
  | Open in VS Code | ⌘⇧E | Ctrl+Alt+E |
  | Inspector | ⌥⌘B | Ctrl+Alt+B |
  | New terminal | ⌥⌘T | Ctrl+Alt+T |
  | Next terminal | ⌘` | ⌘` |
  | Go to file (Files tab only) | ⌥⌘P | Ctrl+Alt+P |
  | Answer a question card | 1/2/3 | 1/2/3 |
- **Loaders**: all 24 are accepted, with placements per the LoadersInUse, Startup and LoadersInFlows boards. A loader shows only after 400 ms, and there is one per screen. There is never a full-screen overlay in a wizard or a chat, never a spinner while waiting on a person, and never a loader while Jev decides.
- **Wizards in chat**: each wizard is one `fleet.form/1` spec, rendered as a dialog or as a ChatForm in the conversation. It is drawn on the canvas board "Wizards built in chat" (`canvas/ChatWizards.dc.html`) and is step 10.12 of the plan. Nothing runs until the last step's button is pressed.
- **AI**: a rule decides first, then Jev, then an LLM. Each is a proposal a person confirms, and there is never an auto mode. What AI never decides is listed in `design-system/ai.md`.

**Still open**: whether Accounts (M4) comes before the session workspace (M5). The plan assumes yes, because accounts sit on the critical path to the cutover.

## Refreshing the snapshots

Re-export the doc to Markdown and replace its two embedded diagrams with
`images/roadmap.svg` and `images/task-graph.svg`. Copy the `project/` files of the
design-system and canvas artifacts over `design-system/` and `canvas/`. Change these
files in a PR of their own, so a later diff shows what the design changed.
