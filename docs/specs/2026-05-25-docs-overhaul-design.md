# Documentation Overhaul — Design

**Date:** 2026-05-25
**Status:** Approved (brainstorm), pending implementation plan
**Scope:** Phase 4 of the new-user experience effort: a documentation overhaul —
README rewrite + four new docs + a light tidy of the Control API guide. This is
the final planned phase (the app-wide polish pass, Phase 3, remains separate and
out of scope here).

**Branching:** Stacked on `feat/tutorial-hints` (which is itself stacked on
`feat/onboarding-setup-flow`/PR #24). The Getting Started guide describes the
guided onboarding checklist (Phase 1) and feature hints (Phase 2), so it must sit
on top of both. Merge order: #24 → #26 → this. Rebase onto `main` once the lower
PRs land.

## Problem

`README.md` is solid but **developer-facing**: it explains building from source,
testing, and project layout, but offers no end-user journey — how to install,
add a host, and get a first session running. There is no Getting Started guide,
no troubleshooting reference, and no user-level concepts doc. New users (the
audience the Phase 1–2 onboarding work targets) have nowhere to read about what
they're seeing.

## Goal

A task-oriented documentation set with a routing README, so a new user can go
from "what is this?" to a running session, understand the mental model, and
self-serve when something breaks — without duplicated content.

## Information architecture

Each file has one job; cross-links point rather than re-explain.

| File | Responsibility |
|---|---|
| `README.md` (rewrite) | Routing + first impression: what it is, a 4-line quickstart, trimmed feature highlights, links into the guides. Dev build/test/layout moved under a "Development" heading. |
| `docs/getting-started.md` (new) | The user journey: prerequisites → launch → guided "Get started" checklist walkthrough → feature hints → everyday use. |
| `docs/concepts.md` (new) | Mental model: sessions, hosts & accounts, projects/worktrees, background vs work sessions, Control API + reverse tunnels, the custom terminal. Links to CLAUDE.md for backend internals. |
| `docs/troubleshooting.md` (new) | Symptom → cause → fix table for the common failure modes, referencing UI messages / `E_*` codes. |
| `docs/control-api.md` (tidy) | Add a cross-link to Getting Started; verify steps match the current Settings UI. No rewrite. |
| `docs/control-api-reference.md` | **Untouched** — it is source-generated (`REGEN_DOCS=1`). |
| `docs/README.md` (new) | One-screen docs index: one line per guide. |

**No-duplication rule:** setup *steps* live only in getting-started; *concepts*
only in concepts.md; *fixes* only in troubleshooting. README and cross-links
point, never re-explain.

## Content outlines

### `README.md` (rewrite)
1. Title + badges (keep).
2. One-paragraph user-facing intro.
3. **Quickstart** — install/run (`pnpm tauri dev` until packaged builds are
   published), then "the app walks you through setup" → link to getting-started.
4. **Features** — the existing list, tightened.
5. **Documentation** — links to the four guides.
6. **Development** — current Requirements + Build & run + Test + Project layout,
   relocated under this heading.
7. Known gaps, Releasing & documentation, License (keep, update layout/links if
   paths changed).

### `docs/getting-started.md` (new)
1. *Prerequisites* — local `claude` CLI + `tmux` + a `~/projects/github.com`
   layout (note `CLAUDE_FLEET_PROJECTS_BASE` override); remote hosts reachable
   via key-based SSH in `~/.ssh/config`.
2. *Install & launch* — run the app; first launch on an empty fleet shows the
   Welcome dialog.
3. *Guided setup (the "Get started" checklist)* — walk each step with its real
   label: local prerequisites, add SSH host (discover → probe → add), provision
   & tunnels (incl. the "tunnel: starts with Control API" → "tunnel: up"
   behaviour), pick projects, optional Control API enable, create first session.
4. *Feature hints* — the one-time bubbles; "Show feature hints" toggle + "Reset
   hints" in Settings.
5. *Everyday use* — attach a session, send prompts (incl. broadcast), background
   sessions, files/diffs/git, host & recency filters.

### `docs/concepts.md` (new)
Short sections: session = tmux window + Claude Code process; hosts & the account
model (probed from `~/.claude.json`, no credentials stored); projects & git
worktrees; background vs work sessions; the embedded MCP **Control API** and the
supervised **reverse SSH tunnels** that expose it to remote hosts; the
hand-rolled ANSI terminal (why not xterm.js). Each links to CLAUDE.md / the specs
for depth.

### `docs/troubleshooting.md` (new)
Symptom → cause → fix, covering at least:
- Host shows offline / probe fails (SSH reachability, key auth).
- `claude` or `tmux` missing on a host.
- Provisioning failed (surface `HostProvisionResult.detail`).
- Tunnel "down — retrying" (Control API off, or SSH `-R` blocked).
- MCP bind error (port already in use).
- No projects found (scan path / `CLAUDE_FLEET_PROJECTS_BASE`).
- Session won't attach / ghost session.
Reference the `E_*` IPC error codes where they surface in the UI.

### `docs/control-api.md` (tidy)
Add a one-line cross-link at the top ("Enable via the Getting Started flow or
Settings → Control API"); read through and correct any step that no longer
matches the current Settings UI. No structural rewrite.

### `docs/README.md` (new)
A short index linking getting-started, concepts, troubleshooting, control-api,
RELEASING, and the generated reference.

## Accuracy constraints

- Every UI label, setting name, and flow step **must match the code on this
  branch** (onboarding + hints are present here). Verify against
  `OnboardingCard.svelte`, `WelcomeDialog.svelte`, `hints.ts`, `SettingsDialog.svelte`,
  and the service layer rather than inventing copy.
- Commands and paths (`pnpm tauri dev`, `~/projects/github.com`,
  `CLAUDE_FLEET_PROJECTS_BASE`, the `E_*` codes) must be real — cross-check
  against the source.
- Internal doc links must resolve (relative paths within `docs/` and to
  `CLAUDE.md`).

## Testing / verification

Docs have no unit tests. Verification is:
- **Link check:** every relative link in the new/edited docs resolves to a real
  file/anchor.
- **Fidelity check:** each described step/label is cross-checked against the
  actual component or command (spot-check during review).
- **Markdown sanity:** files render (headings, tables, code fences well-formed).
- The repo's existing checks (`pnpm test`, `cargo` suite) are unaffected by docs;
  run `npx vitest run` once at the end only to confirm nothing inadvertently
  changed in code (it shouldn't — this phase touches only Markdown).

## Out of scope

- App-wide visual polish pass (Phase 3).
- Regenerating or restructuring `control-api-reference.md`.
- Screenshots / image assets (prose-only for now; can be added later).
- A hosted documentation site (Markdown in-repo only).
