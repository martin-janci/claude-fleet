# Documentation Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the developer-only README with a routing README plus a task-oriented user documentation set (Getting Started, Concepts, Troubleshooting, a docs index) and a light tidy of the Control API guide.

**Architecture:** Markdown-only, in-repo. Each file has one job; cross-links point rather than duplicate. Written against `feat/docs-overhaul` (stacked on `feat/tutorial-hints` → `feat/onboarding-setup-flow`), so onboarding + hints UI exists and must be described accurately.

**Tech Stack:** Markdown. No code changes.

**Spec:** `docs/specs/2026-05-25-docs-overhaul-design.md`

**Branch:** `feat/docs-overhaul` (already created; spec committed there).

---

## Conventions for this plan

- Repo root: `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3`. All commands run from there.
- **This is documentation, not code.** A "task" produces one Markdown file (or rewrites one). Instead of TDD, each task: (1) read the cited source files to confirm labels/facts, (2) write clear prose from the provided outline + verified facts, (3) verify links resolve and labels match, (4) commit. Use the `elements-of-style` writing guidance if available: clear, concise, active voice.
- **git rules (pinned):** all git ops use `git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3`. NEVER pull/push/fetch/rebase/checkout/switch/branch/merge/reset. Only `git add` + `git commit`. Before each commit, confirm `git -C <worktree> branch --show-current` is `feat/docs-overhaul`; else STOP / report BLOCKED.
- **Do not invent UI copy.** Use the verified labels embedded below; if a label differs from what you read in the source, use the source and note it.
- **Accuracy is the acceptance criterion.** Every label, path, command, port, and `E_*` code must match the code on this branch.

## Verified facts (use these; re-confirm against the cited files)

- **Welcome dialog** (`src/lib/WelcomeDialog.svelte`): title "Welcome to claude-fleet"; buttons "Let's set up →" and "Skip for now". Shows on first launch only when the fleet is empty.
- **"Get started" card** (`src/lib/OnboardingCard.svelte`): heading "Get started"; progress "{n} of {m} done"; completion "You're all set 🎉". Steps (`src/lib/onboarding.ts`): **Local prerequisites**, **Add a host**, **Provision & tunnels**, **Pick projects**, **Enable Control API** (optional), **Create first session**.
- **Tunnel badge wording** (`src/lib/onboarding.ts` / `hints` work): "tunnel: starts with Control API" (MCP off) → "tunnel: up" / "tunnel: down — retrying" (MCP on).
- **Feature hints** (`src/lib/hints.ts`): 5 hints — texts: "Filter sessions by machine — the dot shows reachability."; "Launch a background agent: headless Claude that works without an attached terminal."; "Hover or select a session to restart, rename, recreate, or kill it."; "You're attached to this session's tmux. Right-click for copy / paste."; "Narrow the list to recent activity." One at a time; dismiss with "Got it" / ✕.
- **Settings** (`src/lib/SettingsDialog.svelte`): "Replay setup guide"; "Show feature hints" toggle; "Reset hints"; "Control API (MCP)" section with an "MCP client config" disclosure and a push-to-hosts action.
- **Control API:** default port **4180** (`src-tauri/src/mcp/mod.rs` `DEFAULT_PORT`); endpoint `http://127.0.0.1:<port>/mcp`; bearer token; localhost-only.
- **Projects scan path:** `~/projects/github.com/<owner>/<repo>`, overridable via env `CLAUDE_FLEET_PROJECTS_BASE` (`src-tauri/src/service/projects.rs`).
- **Provisioning** (`src-tauri/src/service/provision.rs`) writes: the fleet-control + fleet-friendly-name skills, a managed CLAUDE.md block, the MCP entry in `~/.claude.json`, and `set -g set-clipboard on` in `~/.tmux.conf`.
- **Relevant `E_*` codes** (surface in UI): `E_HOST_OFFLINE`, `E_CLAUDE_CLI`, `E_FLEET_PROJECTS_BASE`, `E_DIRTY`, `E_NOTFOUND`, `E_LOCK`, `E_GIT`, `E_CLIPBOARD_UNAVAILABLE`. (Full list: `grep -rho "E_[A-Z_]*" src-tauri/src | sort -u`.)

---

## Task 1: `docs/concepts.md` (mental model)

**Files:** Create `docs/concepts.md`

- [ ] **Step 1: Confirm facts.** Read `CLAUDE.md` (the "Architecture" section), `src-tauri/src/service/tunnel.rs` (reverse tunnel purpose), and `src/lib/ansi.ts` top comment (why not xterm). Note the real terms.

- [ ] **Step 2: Write the doc.** Create `docs/concepts.md` with a short intro and these `##` sections (2–5 sentences each, conceptual, no setup steps):
  1. **Sessions** — a session is a tmux window running a Claude Code process on some host; claude-fleet attaches to it. "work" vs **background** sessions (`kind: 'bg'` — headless, supervised, no attached terminal).
  2. **Hosts & accounts** — hosts come from `~/.ssh/config` plus `local`; SSH is multiplexed via per-host ControlMaster. Each host's Claude account (email/org/tier) is probed from the remote `~/.claude.json`; **no credentials are read or stored**.
  3. **Projects & worktrees** — the app scans `~/projects/github.com/<owner>/<repo>` (and git worktrees) per host; sessions group under their project.
  4. **Control API & tunnels** — an embedded, localhost-only MCP server (off by default, bearer-token) lets an AI assistant drive the fleet; supervised **reverse SSH tunnels** expose it to remote hosts so remote agents can call back. Link to `control-api.md`.
  5. **The terminal** — a hand-rolled ANSI screen-buffer renderer (not xterm.js — link the reason to `src/lib/ansi.ts`), one PTY attached at a time.
  End with a "Going deeper" line linking `../CLAUDE.md` and `specs/`.

- [ ] **Step 3: Verify.** Confirm relative links resolve from `docs/` (`control-api.md`, `../CLAUDE.md`, `specs/`). Confirm no setup steps leaked in (those belong in getting-started). Markdown renders (headings/links well-formed).

- [ ] **Step 4: Commit.**
```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add docs/concepts.md
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "docs: add concepts (mental model) guide"
```

---

## Task 2: `docs/getting-started.md` (the user journey)

**Files:** Create `docs/getting-started.md`

- [ ] **Step 1: Confirm labels.** Read `src/lib/WelcomeDialog.svelte`, `src/lib/OnboardingCard.svelte`, `src/lib/onboarding.ts` (step labels + tunnel badge logic), `src/lib/AddHostPicker.svelte` (discover→probe→add flow), and `src/lib/SettingsDialog.svelte` (Control API + "Replay setup guide"). Use the exact labels from the "Verified facts" block above; correct any drift from source.

- [ ] **Step 2: Write the doc** with these `##` sections:
  1. **Prerequisites** — local: the `claude` CLI and `tmux` installed, and a `~/projects/github.com` layout (note `CLAUDE_FLEET_PROJECTS_BASE` to override). Remote: hosts reachable over key-based SSH, listed in `~/.ssh/config`. Link `concepts.md` for what hosts/sessions are.
  2. **Install & launch** — run from source for now: `pnpm install` then `pnpm tauri dev` (note packaged builds will come later). On first launch with an empty fleet, the **"Welcome to claude-fleet"** dialog appears — "Let's set up →" reveals the sidebar **"Get started"** checklist; "Skip for now" leaves it available.
  3. **Guided setup — the "Get started" checklist** — a subsection per step, using the real labels and saying what each does + what to click:
     - *Local prerequisites* — checks `claude`/`tmux`/projects path; shows what's missing.
     - *Add a host* — opens Settings → discover hosts from `~/.ssh/config` → probe → add.
     - *Provision & tunnels* — installs the fleet skills + MCP config on the host; the tunnel badge reads **"tunnel: starts with Control API"** until you enable the Control API, then **"tunnel: up"**.
     - *Pick projects* — scans the projects path.
     - *Enable Control API* (optional) — turns on the localhost MCP server (port 4180), shows the masked token + copy.
     - *Create first session* — the finish line; opens the new-session picker.
  4. **Feature hints** — one-at-a-time bubbles that appear the first time a feature is relevant; dismiss with "Got it". Toggle "Show feature hints" / "Reset hints" in Settings.
  5. **Everyday use** — attach a session and watch it live; send a prompt (and broadcast to many); background sessions (⚡); files/diffs/commit-graph/branches; host & recency filters in the sidebar. Link `concepts.md` and `troubleshooting.md`.

- [ ] **Step 3: Verify.** Every quoted label matches source. Relative links (`concepts.md`, `troubleshooting.md`, `control-api.md`) resolve. No concept explanations duplicated from concepts.md (link instead). Commands (`pnpm install`, `pnpm tauri dev`) and the env var are correct.

- [ ] **Step 4: Commit.**
```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add docs/getting-started.md
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "docs: add Getting Started guide"
```

---

## Task 3: `docs/troubleshooting.md` (symptom → cause → fix)

**Files:** Create `docs/troubleshooting.md`

- [ ] **Step 1: Confirm error surfaces.** Run `grep -rho "E_[A-Z_]*" src-tauri/src | sort -u` to see the real codes, and skim `src-tauri/src/service/hosts.rs`, `provision.rs`, `commands/mcp.rs` for the failure messages users see. Confirm the codes referenced below exist.

- [ ] **Step 2: Write the doc** as a short intro + a Markdown table (Symptom | Likely cause | Fix), one row per item, plus a sentence of detail under the table where needed:
  - **Host shows offline / probe fails** → SSH not reachable or key auth not set up → fix `~/.ssh/config`, test `ssh <alias>` in a terminal; surfaces as `E_HOST_OFFLINE`.
  - **`claude` or `tmux` missing on a host** → not installed / not on PATH → install them on the host; relates to `E_CLAUDE_CLI`.
  - **Provisioning failed** → the per-host result shows a `detail` string → read it; common: can't write `~/.claude.json` / `~/.tmux.conf`, or skills dir not writable.
  - **Tunnel "down — retrying"** → Control API disabled, or the host blocks SSH `-R` remote forwarding → enable the Control API; check `AllowTcpForwarding`/`GatewayPorts` on the host.
  - **MCP bind error** → the configured port (default 4180) is already in use → change the port in Settings → Control API; surfaces via `McpStatus.bind_error`.
  - **No projects found** → scan path empty/missing → ensure repos live under `~/projects/github.com/<owner>/<repo>` or set `CLAUDE_FLEET_PROJECTS_BASE`; relates to `E_FLEET_PROJECTS_BASE`.
  - **Session won't attach / ghost session** → underlying tmux session gone → use Recreate, or dismiss the ghost.
  - **(dev) `localStorage is undefined` in tests** → pre-existing test-env issue (see CLAUDE.md), not your change.
  Link back to `getting-started.md` and `concepts.md`.

- [ ] **Step 3: Verify.** Every `E_*` code referenced exists in the grep output. Links resolve. Table renders.

- [ ] **Step 4: Commit.**
```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add docs/troubleshooting.md
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "docs: add Troubleshooting guide"
```

---

## Task 4: Tidy `docs/control-api.md`

**Files:** Modify `docs/control-api.md`

- [ ] **Step 1: Read** the full `docs/control-api.md` and the current `src/lib/SettingsDialog.svelte` Control API section (the `<h4>Control API (MCP)</h4>` block, the "MCP client config" disclosure, and the push-to-hosts action). Note any step in the doc that no longer matches the UI.

- [ ] **Step 2: Edit, lightly.**
  - Add a one-line note near the top: that the Control API can be enabled via the **Getting Started** flow (link `getting-started.md`) or **Settings → Control API (MCP)**.
  - Fix only steps/labels that have drifted from the current Settings UI (button names, the config disclosure, the push action). Do **not** restructure or rewrite working content. Do **not** touch `control-api-reference.md` (it is generated).

- [ ] **Step 3: Verify.** The cross-link resolves; edited steps match the current Settings UI; `control-api-reference.md` is unchanged (`git -C <worktree> status --short` shows only `control-api.md`).

- [ ] **Step 4: Commit.**
```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add docs/control-api.md
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "docs: cross-link and refresh Control API guide"
```

---

## Task 5: `docs/README.md` (docs index)

**Files:** Create `docs/README.md`

- [ ] **Step 1: Write** a one-screen index: a title ("claude-fleet documentation") and a bulleted list, one line each, linking and describing: `getting-started.md`, `concepts.md`, `troubleshooting.md`, `control-api.md`, `control-api-reference.md`, `RELEASING.md`. Add a line pointing developers to `../CLAUDE.md` and `specs/` / `plans/`.

- [ ] **Step 2: Verify.** Every link resolves to an existing file (all created in Tasks 1–4 plus the pre-existing `control-api-reference.md`, `RELEASING.md`).

- [ ] **Step 3: Commit.**
```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add docs/README.md
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "docs: add documentation index"
```

---

## Task 6: Rewrite `README.md`

**Files:** Modify `README.md`

- [ ] **Step 1: Read** the current `README.md` (preserve the badges, the good feature bullets, Known gaps, Releasing, License verbatim where reused).

- [ ] **Step 2: Rewrite** into this structure:
  1. Title + the two existing badges (keep).
  2. One-paragraph user-facing intro (keep the essence of the current intro; drop the long status blockquote — move a trimmed status line into "Known gaps" or delete).
  3. **Quickstart** — `pnpm install` + `pnpm tauri dev`, then one line: "On first launch the app walks you through setup — see the [Getting Started guide](docs/getting-started.md)."
  4. **Features** — the current bullet list, tightened (keep multi-host, project tree, account model, terminal, prompt transfer, files/history/branches, event-driven UI). 
  5. **Documentation** — bullet links to `docs/getting-started.md`, `docs/concepts.md`, `docs/troubleshooting.md`, `docs/control-api.md`, and `docs/README.md`.
  6. **Development** — move the existing **Requirements**, **Build & run**, **Test**, and **Project layout** sections here, unchanged in content (update the migrations range note only if obviously stale).
  7. **Known gaps**, **Releasing & documentation**, **License** — keep (verify the doc links still resolve).

- [ ] **Step 3: Verify.** All `docs/...` links resolve to files that now exist. Build/test commands unchanged and correct. The dev info is intact, just relocated under "Development".

- [ ] **Step 4: Commit.**
```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add README.md
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "docs: rewrite README as user-facing with a routing structure"
```

---

## Task 7: Whole-set verification

- [ ] **Step 1: Link check.** From the repo root, list every relative Markdown link in the new/edited files and confirm each target exists:
```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3
grep -rnoE "\]\(([^)]+\.md[^)]*)\)" README.md docs/README.md docs/getting-started.md docs/concepts.md docs/troubleshooting.md docs/control-api.md
```
For each `(path.md…)` result, verify the file exists (strip any `#anchor`). Fix any broken link in its source file and amend that file's commit or add a fixup commit.

- [ ] **Step 2: Confirm no code changed.** `git -C <worktree> diff --stat main...HEAD` should show only `.md` files (plus the earlier onboarding/hints commits inherited from the stacked branches). Run `npx vitest run` once and confirm it's still green (it must be unaffected — this phase is Markdown only). Report the count.

- [ ] **Step 3: Final read-through.** Re-read each new doc once for clarity and the no-duplication rule (steps only in getting-started, concepts only in concepts, fixes only in troubleshooting). Fix inline; commit if anything changed:
```bash
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 add -A
git -C /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/test3 commit -m "docs: link + clarity fixes" || echo "nothing to commit"
```

---

## Self-review notes (addressed)

- **Spec coverage:** IA + README rewrite — Task 6; getting-started — Task 2; concepts — Task 1; troubleshooting — Task 3; control-api tidy — Task 4; docs index — Task 5; accuracy/link verification — Tasks' verify steps + Task 7. `control-api-reference.md` explicitly untouched (Task 4).
- **No-duplication rule** enforced per task and re-checked in Task 7.
- **Ordering:** guides (1–4) before the index (5) and README (6) so their links resolve when written; verification last (7).
- **Placeholder scan:** none — outlines carry the actual labels/facts; prose is drafted by the implementer from them (the documented docs convention), then verified against source.
- **Accuracy:** every embedded label/port/path/`E_*` code is from source (the "Verified facts" block) and re-confirmed in each task's Step 1.
- **Out of scope (per spec):** polish pass, reference-doc regen, screenshots, hosted site.
```
