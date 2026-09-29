# Multi-harness agents (Claude Code, Codex, Antigravity, Augment, Gemini) — design & roadmap

**Date:** 2026-09-29 · **Status:** draft for review · **Scope:** umbrella spec spanning two repos
(`martin-janci/claude-fleet` = primary, `dotfiles` = personal layer). Fleet-side sub-projects get
their own spec → plan cycle inside the claude-fleet repo when they start.

## 1. Goal

Run coding-agent sessions in **claude-fleet** with any of: Claude Code (`claude`), OpenAI Codex
CLI (`codex`), Google Antigravity CLI (`agy`), Augment (`auggie`), Gemini CLI (`gemini`, API-key
only) — sharing one set of skills / subagents / MCP servers / hooks / instructions, and make it
**easy for anyone** (public, not just the owner) to set up a host: a fresh machine must "just
know" `cl`, `ag`, the installed agent CLIs and the shared content, without hand-copied aliases.

### Non-goals

- Translating session transcripts between harnesses (a Codex session is not resumable in Claude).
- Feature parity for Claude-only fleet features (rewind, fork, move-session, bg sessions,
  `claude agents` reconcile) — they are capability-gated, not ported.
- Native Windows (WSL is supported; it is Linux).
- A second content-sync engine: fleet's asset catalog is **the** engine (see §3).

## 2. Decisions (agreed 2026-09-29)

| # | Decision | Choice |
|---|---|---|
| D1 | Audience | Public / anyone → no personal hosts, IPs, tokens in shared artifacts |
| D2 | Harnesses v1 | Claude Code, Codex, Augment, Antigravity (+ Gemini CLI API-key only) |
| D3 | Platforms | macOS, Linux (fleet hosts, containers), WSL |
| D4 | Public vs personal split | Public code in claude-fleet; personal content = a private layer (dotfiles + private `agent-assets`) |
| D5 | Launcher language | Bash (3.2-compatible) + jq |
| D6 | Launcher name | `ag`; shims (`cl`, `cx`, …) generated from config, not shell aliases |
| D7 | Permission default | Safe by default; `--yolo` opt-in (fleet passes it explicitly per host policy) |
| D8 | Content distribution | Symlink + render with manifest — **reuse fleet's catalog**, don't rebuild |
| D9 | `ag` home | claude-fleet repo; fleet provisions it to every host; standalone `curl` install from same repo |
| D10 | First non-Claude harness in fleet sessions | Codex |
| D11 | Starter content for newcomers | No fixed starter set: an **AI tutor** (fleet component) interviews the user through dynamic views in chat and generates their personal set |
| D12 | Where a user's set lives | The user's **own git repo** (local, optionally pushed private via `gh`) + optional **team catalog**; fleet composes sources library → team → personal |
| D13 | Tutor runtime | Whatever harness the user has (operator pattern via `ag`); v1 ships on Claude, other harnesses once their sessions + transcripts land |
| D14 | Owner's dotfiles | The owner's dotfiles remain a private layer; the tutor + library are the generalised, public equivalent |

## 3. Current state (findings)

### claude-fleet (origin/main 7d5600e1)

- **Asset catalog already abstracts harnesses:** `crates/fleet-core/src/service/catalog/`
  — `trait Harness { id, render, scan_script, parse_scan, installed, manifest_path, merge_config }`,
  impls `claude` (full) and `codex` (experimental: skills + MCP only). Neutral IR (`model.rs`):
  kinds Skill/Agent/Hook/McpServer/PluginRef; neutral tool names, model tiers (fast/default/strong),
  hook events (session_start, prompt_submit, before_tool, after_tool, stop, subagent_stop);
  per-harness `targets.<h>` overrides. Git-backed `agent-assets/` repo, layers, `${NAME}` secrets
  in SQLite, `plan_sync`/`apply_sync` with CAS, backups, per-harness manifests, "unmanaged" never touched.
- **Gaps in catalog:** no `Instructions` kind (CLAUDE.md/AGENTS.md block is hard-coded in
  `service/provision.rs`), no Codex agent→TOML render, no hooks for Codex, no per-host harness
  set (every harness planned on every host), import only from `~/.claude`.
- **Session runtime is Claude-only:** no harness concept on `SessionRow`; pane command is
  `cl --resume ID --name N || cl --session-id ID … || cl` with an inline `CL_FALLBACK`
  (`tmux.rs:1221-1290`); status via Claude http hooks + `X-Fleet-Pane`; pane-intel parses the
  Claude TUI; transcripts/usage/rewind parse `~/.claude/projects/*.jsonl`; accounts read
  `~/.claude.json`.

### Harness facts that shape the design (checked against vendor docs 2026-09-29; "unverified" = docs silent)

| | Claude | Codex | Antigravity `agy` | Augment `auggie` | Gemini CLI |
|---|---|---|---|---|---|
| Instructions | `CLAUDE.md` (reads `AGENTS.md` only if no CLAUDE.md, unless configured) | `AGENTS.md` (32 KiB cap) | `AGENTS.md`/`GEMINI.md` | `AGENTS.md`, `CLAUDE.md`, `.augment/rules` | `GEMINI.md` (`context.fileName` → AGENTS.md) |
| Skills (global) | `~/.claude/skills` only | `~/.agents/skills` | `.agents/skills` (global path unverified) | `~/.augment`, `~/.claude`, `~/.agents` skills | `~/.gemini/skills`, `~/.agents/skills` |
| Subagents | md+YAML | **TOML** | md+YAML | md+YAML | md+YAML |
| MCP | `~/.claude.json` / `claude mcp add` | `config.toml` / `codex mcp add` | `mcp_config.json` / `agy mcp add` | `settings.json` / `auggie mcp add` | `settings.json` / `gemini mcp add` |
| Hooks | full | Claude-like names, **hash-pinned trust** | Pre/PostToolUse, Stop (partial) | Claude names, `conversation_id` | **different names** (BeforeTool…) |
| Headless | `-p` | `exec` | `-p` (needs PTY) | `-p` | `-p` |
| Continue / resume | `-c` / `-r id` | `resume --last` / `resume id` | `--continue` / `--conversation id` | `-c` / `-r id` | `--resume` / `--resume id` |
| Pre-assigned session id | **yes** (`--session-id`) | no | no | no | no |
| Yolo | `--dangerously-skip-permissions` | `--dangerously-bypass-approvals-and-sandbox` | `--dangerously-skip-permissions` | per-tool permission config | `--yolo` |

Gemini CLI dropped personal Google accounts on 2026-06-18 → personal Google use goes through `agy`.

## 4. Architecture

```
┌──────────────────────── claude-fleet (public) ────────────────────────┐
│ Catalog (content engine)        Session runtime            Provision  │
│  IR + Harness impls  ──render──▶ SessionRuntime per harness  ag + shims│
│  claude codex agy auggie gemini  launch/status/transcript    CLIs, auth│
└───────────────▲──────────────────────────┬────────────────────────────┘
                │ agent-assets repos        │ pane command: `ag <h> …`
   ┌────────────┴──────────┐               ▼
   │ public/base layer     │        host: ~/.local/bin/ag, cl, cx …
   │ team layer (optional) │              ~/.claude ~/.codex ~/.agents …
   │ personal (private)    │
   └───────────────────────┘
```

Ownership rule: **fleet owns agent config dirs; dotfiles never symlink a whole agent config dir.**
Dotfiles keep shell/git/editor config and, as a private `agent-assets` layer, personal content.

## 5. Components

### 5.1 `ag` launcher (claude-fleet repo, `tools/ag/`)

CLI contract:

```
ag [<harness>] [--print|-p PROMPT] [--continue|-c] [--resume|-r ID] [--new-id ID]
   [--name N] [--model M] [--effort E] [--yolo] [-- raw args…]
ag doctor [--json] | ag list | ag install <harness> | ag which <harness>
ag hook-shim --harness <h> --event <e>      # normalises a hook payload and POSTs it to fleet
ag shims                                    # (re)generate alias shims from config
```

- Harness resolution: positional arg → `AG_HARNESS` → `~/.config/ag/config` `default=` → first
  installed in `order=`. Subcommand names are reserved and never collide with harness ids.
- Unknown flags after `--` pass through verbatim. Flags a harness cannot honour degrade
  explicitly: `--name` ignored outside Claude; `--new-id` unsupported ⇒ exit code 3 so fleet
  falls back to "learn id from first hook".
- Drivers: `tools/ag/drivers/<harness>.sh`, each defining three functions — `drv_bin` (binary
  path; PATH then known fallbacks, e.g. `/Applications/ChatGPT.app/Contents/Resources/codex`),
  `drv_install_hint` (install command), `drv_argv` (fills `ARGV` from the normalised `AG_*`
  globals in a fixed canonical order; returns 3 for a flag the harness cannot honour). One
  `drv_argv` per driver instead of one function per flag: harnesses reshape the whole command
  (Codex: `codex [exec] [resume --last|ID] [opts] [PROMPT]`), not single flags. Project
  pre-trust (`drv_pretrust`) is added with F2; agy `-p` runs under `script` for a PTY (F6).
- Config `~/.config/ag/config` (INI, bash-3.2-parseable):
  `default`, `order`, `yolo` (default false), `[alias] cl = claude --yolo` etc.
- Shims are 3-line scripts in `~/.local/bin` (work in tmux, `ssh host cmd`, fleet panes — unlike
  zsh aliases). `ag` itself never defines `cl`; fleet provision and personal config do.
- `ag doctor`: per harness — binary+version, on PATH, auth present (existence only, never read),
  instructions wired, skills visible, fleet hooks present/trusted, stray unmanaged copies; each
  failure prints the exact fix command; `--json`, non-zero exit on failure.
- `ag hook-shim`: reads the harness's hook stdin, maps event names and id fields
  (`conversation_id`→`session_id`, `BeforeTool`→`PreToolUse`, Codex rollout path), adds
  `X-Fleet-Pane: $TMUX_PANE` + `X-Fleet-Harness`, POSTs to fleet's existing `/hook` with the bearer
  token from the 0600 headers file fleet already writes. Keeps fleet's hook endpoint nearly unchanged.
- Standalone install: `curl -fsSL …/tools/ag/install.sh | bash` → `~/.local/share/ag`,
  `~/.local/bin/ag`, PATH check, optional `--harness claude,codex` installs.
- Tests: `scripts/ag-test.sh` in fleet's existing style (fake binaries on PATH, PASS/FAIL lines, no extra deps) asserting argv per driver; shellcheck in CI.

### 5.2 Catalog extensions (fleet-core `service/catalog`)

1. **`Instructions` kind**: a neutral markdown body + optional per-harness overlays, rendered as a
   sentinel block (`<!-- BEGIN fleet:<name> -->`) into `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`,
   `~/.gemini/GEMINI.md`, `~/.augment/rules/<name>.md`. New `ConfigMerge` mode `Block`.
   Replaces the hard-coded block in `provision.rs`. Enforces Codex's 32 KiB budget (warning).
2. **Codex agent render**: IR Agent → `~/.codex/agents/<n>.toml` (`name`, `description`,
   `developer_instructions`, `model` from tier map). Hooks for Codex via `hooks.json` pointing at
   `ag hook-shim` (+ document the trust step; `ag doctor` detects untrusted).
3. **Per-host harness set**: `hosts.harnesses` (detected by scan, overridable) — sync plans only
   for enabled harnesses.
4. **Shared skills dir**: skills for codex/agy/auggie/gemini render once into `~/.agents/skills`
   (one manifest owner), Claude keeps `~/.claude/skills`. Import learns `~/.agents/skills` and
   `~/.codex` so the diverged copies can be adopted (backup) instead of duplicated.
5. **New `Harness` impls** in order: `agy`, `auggie`, `gemini` (skills, agents, MCP via each CLI's
   `mcp add` where JSON editing is fragile, hooks where the event model maps).
6. **Provision on catalog**: fleet's own skills (`claude-fleet-control`, `fleet-friendly-name`),
   MCP entry and hooks become built-in catalog assets rendered per enabled harness.
7. **Catalog sources (multi-repo)**: replace the single-row `catalog_config` (`CHECK (id = 1)`)
   with `catalog_sources(id, kind library|team|personal, repo_path, remote_url, priority, writable)`.
   Resolution walks sources by priority (library < team < personal; later wins by asset name, same
   rules as layers' `overrides`/`exclude`). Exactly one writable **personal** source is the default
   target for authoring, imports and tutor proposals; team sources are read-only unless the user
   has push rights. `resolve_preview` shows provenance as `source/layer`.
8. **`Alias` kind**: launcher shims (`cl = claude --yolo`) as catalog assets, rendered by provision
   into `~/.config/ag/config` — so a user's aliases follow them to every host.

### 5.3 Session runtime (fleet-core)

- Migration: `sessions.harness TEXT NOT NULL DEFAULT 'claude'`; neutral aliases for
  `claude_session_id`/`claude_status` (rename later, keep columns now).
- `trait SessionRuntime { id; launch_argv(opts) -> Vec<String>; supports(Capability) -> bool;
  normalize_hook(payload) -> HookEvent; pane_intel(screen) -> PaneState; transcript_locator(..) }`
  with capabilities: `PreassignedId, NamedSession, Transcript, Usage, Rewind, Move, Bg, Effort`.
- Launch: pane command becomes `ag <harness> --yolo? --resume ID --name N || ag <harness> --new-id ID … || ag <harness> …`,
  with an `AG_FALLBACK` equivalent to today's `CL_FALLBACK` (so hosts without `ag` still run Claude).
  Harnesses without `PreassignedId` start fresh and fleet binds the id from the first
  `session_start` hook (`X-Fleet-Pane` pairs it).
- Pane-intel parsers per harness for idle/working/blocked/auth/trust prompts; start with Codex.
- UI/MCP: `new_session { harness }`, harness badge, feature buttons hidden when capability absent.
- Transcripts/usage for non-Claude harnesses come last (Codex `rollout-*.jsonl` first).

### 5.4 Hosts & auth

- fleet-host image: `ARG AGENT_CLIS="claude codex"`; install each via its official method
  (decoupled from `INSTALL_PAPERCLIP`); state volumes per harness (`~/.codex`, `~/.gemini`,
  `~/.augment`); env passthrough per harness (`CLAUDE_CODE_OAUTH_TOKEN`, `CODEX_ACCESS_TOKEN`/
  `OPENAI_API_KEY`, `AUGMENT_SESSION_AUTH`, `GEMINI_API_KEY`).
- Accounts: `accounts.harness` column; host probe reads identity per harness
  (`~/.claude.json`, `~/.codex/auth.json` account claims, …) — never token values into the DB.
- `claude-creds` → per-harness descriptor model (`agent-creds <harness> backup|restore|list`);
  lives with `ag` in claude-fleet.
- Hermes: its probes become a registry keyed by harness id; later feeds a "pick harness with
  headroom" hint into `dispatch_task`.

### 5.5 AI tutor (fleet component) — "AI-assisted setup", layers sub-project 4

Purpose: a newcomer ends up with a working, personalised agent set (skills, subagents, MCP,
instructions, hooks, aliases) for the harnesses they use — without knowing any config format.

**Building blocks already in fleet:** operator pattern (dedicated tmux agent session, own
instructions, scoped client token, AgentFab/ConversationPanel), declarative pages + L3 flows
(`fleet.page/1`, closed widget catalog, `flow_start/submit/back/cancel`, `Secret` fields never
stored/never shown to the model), P5 proposals + L6 `review_apply` (per-row accept/reject, audit),
catalog authoring/lint, layers + `resolve_preview`, onboarding checklist (`deriveSteps`).

**New pieces:**

1. **View tool in chat** — MCP tool `show_view { spec: fleet.page/1 | flow_step, id }`. Fleet
   stores the view as a conversation event (server-side, so rendering does not depend on parsing
   a harness transcript); `ConversationPanel` renders it inline with the existing page/flow
   renderer (same closed catalog as Settings — no arbitrary HTML). User input returns via
   `flow_submit` and is delivered to the agent as the next turn (`send_prompt` with a compact
   JSON answer; secrets replaced by `${secret:NAME}` references resolved into `catalog_secrets`).
   Because it is an MCP tool, it works for any harness that speaks MCP.
2. **Recipe library** — public catalog source shipped with fleet (`library/` in claude-fleet or a
   sibling public repo): skill templates (code-review, commit/PR, test-writing, debugging, docs),
   subagent personas, MCP recipes (GitHub, Jira, Linear, Sentry, Postgres, browser…) with the
   secrets they need, instruction snippets per stack/language, hook recipes. Each item carries
   `tags`, `stacks`, `harness support`. Library items are templates: the tutor instantiates them
   into the personal source, never edits the library.
3. **Tutor agent** — a `fleet-tutor` session using the operator pattern: own working dir with
   neutral instructions (rendered per harness), the `tutor` skill, a scoped token that can
   propose but not apply. Launched through `ag <user's harness>`.
4. **Tutor flow:**
   1. *Interview* via views: role, languages/stacks, repos, trackers & services, harnesses +
      subscriptions, permission appetite (safe vs yolo), existing config to import
      (`import_assets` from `~/.claude`, `~/.codex`, `~/.agents`).
   2. *Compose*: pick + adapt library recipes, import existing assets, generate missing skills.
   3. *Propose*: one P5 proposal = new/changed assets + `layers/<user>.yaml` in the personal
      source, shown inline as `review_apply`; the user accepts/rejects per row.
   4. *Apply*: commit to the personal source → `plan_sync` preview → `apply_sync` to chosen hosts.
      Optional: create a private GitHub repo and push (`gh`, with confirmation).
   5. *Teach*: a short guided tour using the new assets on the user's real repo ("try
      `/code-review` on your last PR"), tracked as onboarding steps.
   6. *Revisit*: re-runnable any time; proposes diffs ("you added Linear — want its MCP?").
5. **Entry point** — onboarding checklist step "Personalise your agents" (after provision), plus
   AgentFab command "Open tutor".

**Safety:** tutor never applies without review; secrets only through `Secret` fields; team
sources are never written by the tutor; generated skills pass catalog lint + a secret scan before
they can be proposed.

## 6. Roadmap

| Phase | Repo | Deliverable | Exit criterion |
|---|---|---|---|
| **F0 Hygiene** | dotfiles | F0/F4: the owner's personal-config migration, tracked outside this repo | tracked outside this repo |
| **F1 `ag` launcher** | claude-fleet `tools/ag` | CLI, drivers claude+codex, config, shims, doctor, standalone installer, shell tests | `ag`, `ag codex -p hi`, `cl` shim work on mac + a Linux fleet host |
| **F2 Fleet launches via `ag`** | claude-fleet | pane command through `ag claude` with `AG_FALLBACK`; provision installs `ag` + shims | existing Claude sessions unchanged (regression suite); fresh host knows `cl` |
| **F3 Catalog: Instructions + Codex complete + host harness set** | claude-fleet | §5.2 items 1–4, 6 | one sync gives Claude and Codex identical skills/agents/MCP/instructions on a host |
| **F4 dotfiles → layer** | dotfiles + private agent-assets | F0/F4: the owner's personal-config migration, tracked outside this repo | tracked outside this repo |
| **F5 Codex sessions in fleet** | claude-fleet | harness column, `SessionRuntime`, codex launch, `ag hook-shim`, pane-intel, UI picker | start/observe/stop a Codex session from fleet UI + MCP with live status |
| **F6 More harnesses** | claude-fleet | drivers + catalog + runtime for `agy`, then `auggie`, then `gemini` (API key) | same exit as F5 per harness |
| **F7 Hosts & accounts** | claude-fleet + dotfiles docker | image `AGENT_CLIS`, volumes, per-harness accounts, `agent-creds` | new container host with codex+claude from compose alone |
| **F8 Routing & transcripts** | claude-fleet | Hermes registry → dispatch hint; Codex transcript/usage | fleet conversation view for Codex sessions |
| **F9 Newcomer docs** | claude-fleet | getting-started: install fleet → add host → pick harnesses → tutor | a fresh user reaches a running Codex+Claude session following docs only |

**AI tutor track** (claude-fleet):

| Phase | Deliverable | Exit criterion |
|---|---|---|
| **T1 Catalog sources** | §5.2 item 7 + `Alias` kind (item 8) | library + team + personal repos resolve with provenance; owner's dotfiles-derived layer is the personal source |
| **T2 View tool in chat** | `show_view` MCP + inline render + `flow_submit` round-trip | an operator session can ask a multi-field question and receive the answer as a turn |
| **T3 Recipe library** | public library source, ~20 recipes with tags/stacks/harness support, lint + secret scan in CI | library lints clean; recipes render for claude + codex |
| **T4 Tutor v1 (Claude)** | `fleet-tutor` session, `tutor` skill, interview → propose → apply → teach, onboarding step | a fresh account on a fresh host gets a reviewed personal set synced, following only the tutor |
| **T5 Tutor on any harness** | tutor launched via `ag <harness>` | same as T4 on Codex (needs F5 + Codex conversation view from F8) |

Dependencies: F0 is independent. F1 → F2 → F5 is the critical path for "Codex in fleet". F3 can
run parallel to F1/F2; F4 needs F3. T1 needs F3; T2 is independent (builds on landed pages/flows);
T3 needs T1; T4 needs T1–T3; T5 needs T4 + F5 + F8.

## 7. Risks

- **Codex hook trust pinning**: regenerated hooks need re-trust; mitigate by stable shim path
  (hook command never changes, only `ag` does) and doctor detection.
- **No pre-assigned session ids** outside Claude: id binding via first hook can race with two
  sessions in one cwd → pair strictly by `X-Fleet-Pane`.
- **TUI scraping per harness** is brittle across CLI versions → pin tested CLI versions in the
  image; pane-intel falls back to hook-only status.
- **Codex TOML merge loses comments** (existing limitation) → prefer `codex mcp add` CLI.

## 8. Testing

- `ag`: `scripts/ag-test.sh` with stub binaries asserting exact argv per driver × flag; shellcheck; macOS stock `/bin/bash` 3.2 in CI.
- Catalog: existing render/plan/apply unit tests extended per harness (golden files for TOML/JSON/blocks).
- Runtime: pane-command golden tests (Claude output byte-identical before/after F2), hook
  normaliser table tests, pane-intel fixtures captured from real CLI screens.
- End-to-end: a disposable container host running claude+codex; scripted `new_session` for each harness.

## 9. Open questions

1. Fleet naming: keep "claude-fleet" branding while becoming multi-harness?
2. Library home: `library/` inside claude-fleet (versioned with fleet) or a sibling public repo
   (community contributions without fleet releases)?

Resolved 2026-09-29: starter content → AI tutor + recipe library (D11); personal set → own repo +
optional team catalog (D12); tutor runtime → user's harness (D13).
