# claude-fleet improvement report and work plan

Date: 2026-09-10. Baseline: `main` at `fdea436`. Six expert reviews (Rust
backend, security, frontend/UX, MCP agent ergonomics, DevOps/testing, product)
run read-only against this checkout, deduplicated and merged into one plan.

## 1. Executive summary

The fleet control plane is sound where it matters most: store mutex discipline
is clean, shell interpolation goes through one `shell::quote`, reconcile writes
are transactional, and the event bus needs no work. The problems concentrate in
four places:

1. **Operational fragility.** No wall-clock timeout on any SSH command, so one
   hung host wedges the reconcile tick for the app's lifetime. `list_sessions`
   triggers a full unguarded fleet reconcile on every UI focus and MCP poll.
2. **Fleet-wide trust from one token.** The same bearer token is copied to every
   host and reachable on every host's loopback, with no caller identity. One
   compromised or prompt-injected session controls the whole fleet.
3. **Invisible operator signals.** The backend classifies `oom`, `auth_menu`,
   `press_enter` stuck states and tracks `context_pct`, but the frontend never
   receives them. Two identity bugs (sessions keyed by `tmux_name` only, a
   frozen `selectedSession` snapshot) will misfire on any multi-host fleet.
4. **Orchestration without a completion signal.** Agents can send prompts but
   cannot wait for a result, read a transcript, or hand off a task object.
   Hooks are installed on the local host only, so remote status is pane-scrape.

The live fleet today (5 hosts, ~60 sessions, 16 anonymous `bg:` rows, junk
worktree names, stuck sessions sitting for hours) confirms the product is good
at creating sessions and weak at retiring them.

## 2. Verdict per lens

| Lens | Verdict |
|---|---|
| Rust backend | Decent shape. Risks are operational (timeouts, reconcile overlap), then structural (four files hold 9,383 lines; reconcile core tested only through stand-ins). |
| Security | Shell construction is fixed. Exposure is architectural: one unscoped token replicated everywhere, no caller identity, no confirmation step. Fix before provisioning more hosts. |
| Frontend/UX | Store and event architecture sound; hand-rolled terminal capable. Two identity bugs bite multi-host immediately. No triage, notification, or bulk-action layer. |
| MCP / agents | Read surface is solid and token-conscious, but the fleet is not yet a team: no completion signal, no task object, lossy status. Vocabulary drifts across four places. |
| DevOps | Quality gates that exist (fmt, clippy, 346 backend tests, 307 frontend tests) pass. The delivery pipeline is inconsistent: release-please documented but unused, manifest stuck at 0.2.0, no v0.2.4 tag, no built artifact. Logging goes only to a discarded stderr. |
| Product | Value sits in the control plane, not the terminal or git browser. Ship lifecycle hygiene (GC, stuck remediation, attention inbox) before new surface area. |

## 3. Consolidated findings

Severity: C critical, H high, M medium, L low. Effort: S under a day, M one to
three days, L a week or more. Paths are relative to the repo root; `st/` is
`src-tauri/src/`.

### Backend (BE)

| ID | Sev | Eff | Finding | Evidence |
|---|---|---|---|---|
| BE-1 | H | S | No wall-clock timeout on SSH/tmux commands; `timeout` only sets `ConnectTimeout`. A hung command blocks `JoinSet::join_next` forever and the tick's `try_lock` guard stays held. | `st/ssh.rs:110-145`, `st/tmux.rs:159-168`, `st/service/sessions.rs:321-327`, `st/lib.rs:196-203` |
| BE-2 | H | M | `list_sessions` runs a full fleet reconcile with no overlap guard; called from UI focus, Tauri command, and MCP tool. | `st/service/sessions.rs:474-479`, `st/commands/sessions.rs:24`, `st/mcp/tools.rs:755`, `src/lib/sessions.ts:55,139` |
| BE-3 | M | S | Overlapping reconciles can ghost a freshly created session (stale `keep` set). | `st/service/sessions.rs:959`, `st/store.rs:1766-1795` |
| BE-4 | M | S | Blocking `std::process::Command` git calls inside tokio tasks. | `st/service/projects.rs:45-49`, `st/projects.rs:63-69` |
| BE-5 | M | L | God files: `store.rs` 3681 lines, `service/sessions.rs` 3191, `mcp/tools.rs` 1738, `lib.rs` 773. | see split proposal in section 5, F4 |
| BE-6 | M | S | Migrations hand-unrolled 16 times; no test that each SQL file inserts its version; stale test name asserts 17; CLAUDE.md says 001-015. | `st/store.rs:193-292`, `st/store.rs:2443-2446` |
| BE-7 | M | M | Reconcile core (`reconcile_write_one_host`, `reconcile_sessions`) has no direct tests; `exec_for` is hardwired. | `st/service/sessions.rs:56,71,276,2256-2346` |
| BE-8 | L | S | About 60 `E_*` literals; three codes for the same DB failure; `.expect` on store mutex in `broadcast_prompt`; `lock_err()` copied four times. | `st/ipc_error.rs:37`, `st/service/sessions.rs:1450-1453`, `st/service/messages.rs:76` |
| BE-9 | L | S | `pty.rs` and `service/messages.rs` have zero tests; `send_message` is non-atomic across three lock windows. | `st/service/messages.rs:70-120` |
| BE-10 | L | S | `.lock().unwrap()` in tunnel supervisor. | `st/service/tunnel.rs:42,67,78,87` |
| BE-11 | L | S | Reconcile emits `SessionUpdated` for every session every tick, unconditionally. Sixty store flushes per tick in the frontend. | `st/store.rs:1698,1843-1862` |
| BE-12 | L | S | Reconcile write window holds the store lock for the whole multi-host loop. | `st/service/sessions.rs:331-349` |

### Security (SEC)

| ID | Sev | Eff | Finding | Evidence |
|---|---|---|---|---|
| SEC-1 | H | M | One shared bearer token grants full fleet control from every host; the reverse tunnel exposes the MCP port on every remote loopback; auth checks only the token. | `st/service/provision.rs:356-389`, `st/service/tunnel.rs:12-24`, `st/mcp/auth.rs:74` |
| SEC-2 | M | S | Token written to remote `~/.claude.json` with no file mode (umask 022 gives 0644). | `st/service/provision.rs:333-338` |
| SEC-3 | M | S | Hook install puts the token in curl argv (`/hook?token=`), visible in `ps` and transcripts. | `st/commands/mcp.rs:217-221,326,333` |
| SEC-4 | M | M | No caller identity: `register_self`, `send_message` sender, and `inbox` are client-declared. Any agent can seize controller, spoof senders, drain inboxes. | `st/mcp/tools.rs:464,822-845,1226`, `st/store.rs:491` |
| SEC-5 | M | M | Unbounded prompt-injection blast radius: `broadcast_prompt`, `send_prompt`, deliver-mode messages, and raw `start_command` have no rate limit, confirmation, or untrusted marker. | `st/mcp/tools.rs:1066-1112`, `st/service/sessions.rs:1041,1440`, `st/tmux.rs:381-388` |
| SEC-6 | M | S | Devtools compiled into release builds; hardening review marks M2 fixed but it is not. | `src-tauri/Cargo.toml:21` |
| SEC-7 | M | S | claude CLI argv: prompt, session id, project path can become flags (no `--`, no id validation). | `st/claude_cli.rs:63,89,104`, `st/service/bg_sessions.rs:20-55` |
| SEC-8 | L | S | `/hook` mounted outside the origin/host check; accepts arbitrary worktree rows. | `st/mcp/mod.rs:158`, `st/service/hooks.rs:74-104` |
| SEC-9 | L | S | `upload_to_session` reads any local path the webview names. | `st/commands/upload.rs:34-96` |
| SEC-10 | L | S | Audit trail is stderr only. | `st/mcp/tools.rs:43-49` |
| SEC-11 | L | S | Token and account metadata plaintext in `state.db` with default perms. | `st/lib.rs:26-31`, `st/store.rs:79-87` |
| SEC-12 | L | S | `tunnel_argv` passes host without `--`. | `st/service/tunnel.rs:12-24` |

Hardening review status: CR1, CR2, H5, M1, M3 verified fixed. M2 still open.
H1-H4, H6, H7, CR3 not re-checked.

### Frontend (FE)

| ID | Sev | Eff | Finding | Evidence |
|---|---|---|---|---|
| FE-1 | H | S | Session identity keyed by `tmux_name` alone; same name on two hosts does not reattach the PTY, rename/kill can hit the wrong host. | `src/lib/TerminalView.svelte:398,407`, `src/lib/Sidebar.svelte:72,268,312-313,372,404` |
| FE-2 | H | S | `selectedSession` is a frozen snapshot written only on click; details pane and safe-kill pills go stale. | `src/lib/selection.ts:13,37-46`, `src/lib/sessions.ts:160-179`, `src/lib/SessionDetails.svelte:71-95` |
| FE-3 | H | M | `stuck_kind` and `context_pct` never reach the UI; TS `SessionRow` omits them. | `st/store.rs:53-54`, `src/lib/sessions.ts:5-35`, `src/lib/Sidebar.svelte:617-623` |
| FE-4 | H | L | No fleet triage: no attention filter, status sort, notifications, or multi-select bulk actions. | `src/lib/Sidebar.svelte:704-766` |
| FE-5 | M | M | `ansi.ts` iterates UTF-16 units: astral emoji become garbage cells, no wide-char width; DCS/APC bodies render as text; no scrollback. | `src/lib/ansi.ts:290-367` |
| FE-6 | M | S | Terminal keyboard gaps: no Delete, Insert, F-keys, Alt-as-ESC; paste only on `metaKey`; copy-on-select overwrites clipboard. | `src/lib/TerminalView.svelte:273,646-703` |
| FE-7 | M | S | `window.prompt`/`confirm` in FilesPanel; WKWebView returns null so "New branch" never works on macOS. | `src/lib/FilesPanel.svelte:175-187` |
| FE-8 | M | M | Dialogs lack modal semantics: no focus trap, four scattered Escape listeners, Settings closes under stacked AddHostPicker. | `src/lib/Sidebar.svelte:905-950`, `src/lib/SessionDetails.svelte:106-149`, `App.svelte:189` |
| FE-9 | M | M | `Sidebar.svelte` 1478 lines, `TerminalView.svelte` 1173 lines; confirm markup duplicated six times. | see split proposal, F5 |
| FE-10 | M | S | Same as BE-11 from the consumer side: every derived recomputes sixty times per tick. | `src/lib/events.ts` |
| FE-11 | L | S | Resize sends `pty_resize` plus full repaint on every observer frame. | `src/lib/TerminalView.svelte:475-487` |
| FE-12 | L | S | Errors drop `IpcError.code`, never clear; bootstrap failures swallowed so a broken DB shows "No projects yet". | `src/lib/sessions.ts:138-141`, `App.svelte:104-109`, `TerminalView.svelte:707` |
| FE-13 | L | S | Test health: 306/307 pass; Sidebar 500-session perf tripwire took 5624 ms vs 5000 ms budget; 12 svelte-check warnings (five dialogs missing tabindex/keyboard). CLAUDE.md `localStorage` note is stale. | `src/lib/Sidebar.test.ts:632`, `vitest.setup.ts` |

### MCP and orchestration (MCP)

| ID | Sev | Eff | Finding | Evidence |
|---|---|---|---|---|
| MCP-1 | H | M | No completion signal or wait primitive; only Stop and WorktreeCreate hooks; reconcile overwrites hook idle every 20 s. | `st/mcp/hooks.rs:22-27`, `st/service/hooks.rs:59`, `st/service/sessions.rs:117-118`, `st/service/pane_intel.rs:290-321` |
| MCP-2 | H | S | Status vocabulary drifts across pane_intel, tool param docs, server instructions, and the control skill. | `st/service/pane_intel.rs:36-40,54`, `st/mcp/tools.rs:236,1645`, `skills/claude-fleet-control/SKILL.md` |
| MCP-3 | H | M | Hooks installed on local host only; `provision_one` writes no hook block, so remote status is pane-scrape. | `st/commands/mcp.rs:239-244`, `st/service/provision.rs:55-90` |
| MCP-4 | H | M | Screen scraping is the only result channel; `capture_session` JSON-escapes the pane. | `st/mcp/tools.rs:1066-1089,1140` |
| MCP-5 | H | L | Orchestration is ad hoc: one global controller, no task object, no `reply_to`, pull-only inbox. safe-kill nonce marker proves the pattern. | `st/mcp/tools.rs:824`, `st/service/safe_kill.rs:64-84,221`, `st/service/messages.rs:157` |
| MCP-6 | M | S | Split addressing: some tools take `(host_alias, tmux_name)`, others `session_id`; no `whoami`. | `st/mcp/tools.rs` |
| MCP-7 | M | S | `new_bg_session` untracked until the next reconcile; `peek_session` needs a fleet id that does not exist yet. | `st/mcp/tools.rs:1348-1369,1271`, `st/service/bg_sessions.rs:66-73` |
| MCP-8 | M | S | `docs/control-api.md` omits nine tools, miscounts provisioning steps, and still describes the `hostname -s` fallback. | `docs/control-api.md`, `st/service/provision.rs:88` |
| MCP-9 | M | S | Control skill names `E_VALIDATION` (code uses `E_VALIDATE`/`E_INVALID`); 294 lines repeating tool descriptions; managed CLAUDE.md block duplicates the friendly-name skill. | `skills/claude-fleet-control/SKILL.md`, `st/service/provision.rs:31-52` |
| MCP-10 | M | S | `repo_log` default is unbounded despite docs saying 200; `list_sessions` has no `limit`. | `st/mcp/tools.rs` RepoLogParams |
| MCP-11 | L | S | Five lifecycle verbs (kill, safe_kill, restart, recreate, dismiss_ghost) with prose-only distinctions; no tags or cost. | `st/mcp/tools.rs` |

### Product (PROD)

| ID | Value | Eff | Finding | Evidence |
|---|---|---|---|---|
| PROD-1 | H | M | No session GC or TTL; 16 orphan `bg:` rows synthesized with no project and no expiry; junk worktree names accumulate. | `st/service/sessions.rs:258` |
| PROD-2 | H | S | Stuck states are classified but never remediated; reconcile tick is the natural hook. | `st/service/pane_intel.rs:16`, `st/lib.rs:146` |
| PROD-3 | H | M | `fleet_health` rolls up problems but nothing pushes them to the human. | `st/service/health.rs` |
| PROD-4 | H | S | Friendly names depend on the in-session agent obeying a skill; sidebar defaults to `tmux_name`. | commit `fdea436` |
| PROD-5 | H | M | `pr_url` and `notes` exist but nothing populates them; no elapsed, last prompt, or CI badge. | `st/store.rs` SessionRow |
| PROD-6 | H | L | Orchestration exists only as MCP tools; no task board. | `docs/specs/2026-05-21-iter4b-reviews-design.md` deferred items |
| PROD-7 | M | M | No cost/usage per session; only `context_pct`. | |
| PROD-8 | M | M | Handoff unbuilt; Freeze obsolete since `--resume` recreate plus scrollback capture. Dead `handoffs` table and `frozen_scrollback` column remain. | `docs/specs/2026-05-19-claude-fleet-design.md` 8.3/8.4 |
| PROD-9 | M | L | Getting started says "run from source" and hard-codes `~/projects/github.com/<owner>/<repo>`. | `docs/getting-started.md` |

### DevOps, release, and QA (OPS)

Verified locally: `cargo fmt --check` pass, `cargo clippy --all-targets -D warnings` pass, vitest 307/307 pass with two suites failing at import on a stale `node_modules`, `cargo deny check advisories` fails.

| ID | Sev | Eff | Finding | Evidence |
|---|---|---|---|---|
| OPS-1 | H | S | Release process contradicts itself: CLAUDE.md, README, RELEASING.md say release-please; CHANGELOG says hand bumps. Manifest says 0.2.0 while four files say 0.2.4; CHANGELOG links a v0.2.4 tag that does not exist; other branches carry 0.2.5 and 0.2.6. Actions are billing-blocked so neither release-please nor the rustdoc Pages workflow has ever run. | `.release-please-manifest.json`, `CHANGELOG.md:8`, `docs/RELEASING.md`, `.github/workflows/release-please.yml`, `docs.yml` |
| OPS-2 | H | S | pnpm version mismatch: `pnpm-workspace.yaml` uses the pnpm 10 `allowBuilds` key, README says pnpm 9+, no `packageManager` or `engines` in package.json. On pnpm 9 every command fails. | `pnpm-workspace.yaml`, `README.md:49`, `package.json` |
| OPS-3 | H | M | No log file, no diagnostics bundle: zero `tracing`/`log` usage, 41 `eprintln!` sites; a Finder-launched app discards stderr. | `st/lib.rs:176`, `st/service/sessions.rs:174,192,270`, `st/mcp/mod.rs` |
| OPS-4 | H | S | Stale SSH ControlMaster after sleep/resume wedges reconcile: no `ServerAliveInterval`, no per-command timeout, no `ssh -O exit` recovery. Same root as BE-1. | `st/ssh.rs:109-145`, `st/lib.rs:196-200` |
| OPS-5 | M | S | Stale `localStorage undefined` caveat in CLAUDE.md, troubleshooting, and the repo skill; the real failure is a missing `@tauri-apps/plugin-clipboard-manager` in stale `node_modules`. | `vitest.setup.ts:85-129` |
| OPS-6 | M | S | Runbook lacks sleep/resume, tmux server restart, reconcile tick and its interval setting, and log location. | `docs/troubleshooting.md`, `App.svelte:132-142` |
| OPS-7 | M | S | Vulnerable transitive deps (quick-xml 0.39.4: RUSTSEC-2026-0194/0195; crossbeam-epoch 0.9.18: RUSTSEC-2026-0204), six unmaintained crates, no `deny.toml`, no audit step in CI. | `src-tauri/Cargo.lock` |
| OPS-8 | M | M | No SSH-level fake: `SshClient` is concrete, so probe, provision, tunnel, and unreachable-host reconcile have no end-to-end tests. | `st/ssh.rs`, `st/service/sessions.rs:2391` |
| OPS-9 | M | S | No property tests for `shell::quote` or `ansi.ts`. | `st/shell.rs`, `src/lib/ansi.test.ts` |
| OPS-10 | L | S | Sidebar perf test is a wall-clock tripwire (same as FE-13). | `src/lib/Sidebar.test.ts:585-637` |
| OPS-11 | M | M | CI has no artifact build, no Linux leg despite README claiming Linux support, no signing, no release asset upload. | `.github/workflows/ci.yml` |
| OPS-12 | L | S | No `rust-toolchain.toml`, no pre-commit hook mirroring the six local CI steps, no eslint/prettier config, migration count drifts across CLAUDE.md and README. | `README.md`, `CLAUDE.md` |

## 4. Prioritized work plan

Waves are ordered by risk. Tracks inside a wave are independent and can run
in parallel as separate fleet sessions on separate worktrees. Each track lists
the files it owns so parallel sessions do not collide.

### Wave 0: hygiene and vocabulary (1-2 days, everything parallel, all S)

| # | Task | Resolves | Owns files |
|---|---|---|---|
| W0.1 | Fix CLAUDE.md, README, troubleshooting, and the repo skill: quoting is consolidated, migrations run to 017, the `localStorage` caveat is replaced by "run `pnpm install` after pulling", hardening M2 is open. | BE-6 (docs part), FE-13, OPS-5, OPS-12, SEC-6 note | `CLAUDE.md`, `README.md`, `docs/troubleshooting.md`, `skills/claude-fleet-repo/SKILL.md`, `docs/specs/2026-05-21-hardening-review.md` |
| W0.10 | Decide the release process (see section 8) and make docs, manifest, and workflows agree. Either reset `.release-please-manifest.json` to 0.2.4 and stop hand bumps, or delete the release-please workflow and add `scripts/release.sh` that bumps all four version files, writes CHANGELOG, and tags. Create the missing `v0.2.4` tag either way. | OPS-1 | `.release-please-manifest.json`, `release-please-config.json`, `.github/workflows/release-please.yml`, `docs/RELEASING.md`, `CHANGELOG.md` |
| W0.11 | Pin toolchains: `"packageManager": "pnpm@10.x"` and `"engines": {"node": ">=20"}` in package.json, `.node-version`, `rust-toolchain.toml`; `scripts/ci-local.sh` mirroring the six CI steps, called from a pre-commit hook and the repo skill. | OPS-2, OPS-12 | `package.json`, `rust-toolchain.toml`, `scripts/ci-local.sh`, `.githooks/` |
| W0.12 | `cargo update -p quick-xml -p crossbeam-epoch`; add `deny.toml` (advisories, licenses, bans); add `cargo deny check` and `pnpm audit --audit-level=high` to `ci.yml` and the local mirror. | OPS-7 | `src-tauri/Cargo.lock`, `deny.toml`, `.github/workflows/ci.yml` |
| W0.2 | One Rust enum for `claude_status` and `stuck_kind`, serialized and quoted verbatim in tool descriptions, server `instructions`, and the control skill; test that the skill text matches the enum. | MCP-2 | `st/service/pane_intel.rs`, `st/mcp/tools.rs` (descriptions only), `skills/claude-fleet-control/SKILL.md` |
| W0.3 | Replace the hand-written tool table in `docs/control-api.md` with a link to the generated reference; doc test that every router tool name appears; fix provisioning step count and hostname fallback text. | MCP-8 | `docs/control-api.md`, `st/mcp/doc_gen.rs` |
| W0.4 | Cut control skill to about 150 lines of workflow; fix `E_VALIDATE`/`E_INVALID`; shrink managed CLAUDE.md block to three lines pointing at the skill. | MCP-9 | `skills/*`, `st/service/provision.rs:31-52` |
| W0.5 | Response caps: `repo_log` default 50, `list_sessions` `limit`, `capture_session` returns `text_content` with a scrollback cap. | MCP-10, MCP-4 (partial) | `st/mcp/tools.rs` (three handlers) |
| W0.6 | `IpcError::lock()` constructor, `ErrorCode` consts, replace the `.expect` in `broadcast_prompt`, `PoisonError::into_inner` in tunnel. | BE-8, BE-10 | `st/ipc_error.rs`, `st/service/tunnel.rs`, four `lock_err` sites |
| W0.7 | Gate devtools behind `cfg(debug_assertions)`; add `--` to `tunnel_argv`; validate claude CLI ids with `validate::claude_session_id`, reject leading `-`, insert `--` before positionals. | SEC-6, SEC-7, SEC-12 | `src-tauri/Cargo.toml`, `st/claude_cli.rs`, `st/service/bg_sessions.rs`, `st/service/tunnel.rs` |
| W0.8 | Sidebar perf test: keep the row-count assertion, replace the timer with a call-count spy on the memoised index builder, or gate the timing check behind `PERF=1`; fix the twelve svelte-check warnings. | FE-13, OPS-10 | `src/lib/Sidebar.test.ts`, five dialog components |
| W0.9 | Migration table `const MIGRATIONS: &[(i64, &str)]` with a loop; test that versions are contiguous and each file contains its `INSERT OR IGNORE`. | BE-6 | `st/store.rs:193-292` |

### Wave 1: correctness and security (1-2 weeks, three parallel tracks)

**Track A: backend reliability** (owns `st/ssh.rs`, `st/tmux.rs`, `st/service/sessions.rs` reconcile section, `st/store.rs` upsert, `st/service/projects.rs`)

| # | Task | Resolves |
|---|---|---|
| A1 | Wrap `cmd.output()` in `tokio::time::timeout` and kill the child on expiry; add `-o ServerAliveInterval=15 -o ServerAliveCountMax=2` to `mux_opts`; on timeout run `ssh -O exit` for that host so the next call rebuilds the master; per-host timeout around each probe task. | BE-1, OPS-4 |
| A2 | Serve `list_sessions` from `list_all_sessions`; reconcile only when `MAX(last_reconciled_at)` is older than the interval; move the overlap guard into the service so every caller shares it. | BE-2 |
| A3 | Record probe-start time per host; skip ghosting rows whose `last_reconciled_at` is newer than that start. | BE-3 |
| A4 | `spawn_blocking` or `tokio::process::Command` for git in `refresh_projects`. | BE-4 |
| A5 | Diff before pushing `SessionUpdated` in `upsert_session_in_tx`; lock per host instead of per pass. | BE-11, BE-12, FE-10 |

**Track B: security and trust** (owns `st/mcp/auth.rs`, `st/mcp/mod.rs`, `st/commands/mcp.rs`, `st/service/provision.rs` token/hook sections, `st/service/hooks.rs`, `st/commands/upload.rs`)

| # | Task | Resolves |
|---|---|---|
| B1 | Per-host tokens in the settings table keyed by alias; map token to host on auth; master token stays local-only; per-host read-only or allow-list mode. | SEC-1 |
| B2 | `umask 077` plus `chmod 600` on every token write (remote and local); `chmod 600` on `state.db` at open. | SEC-2, SEC-11 |
| B3 | Switch hooks to the `type: http` form with an Authorization header, and install the hook block on remote hosts from `provision_one` via `write_host_file`. This is the single change that both removes the token from argv and gives remote sessions real Stop events. | SEC-3, MCP-3 |
| B4 | Derive caller identity from the per-host token (B1); reject `register_self` and `from_session_id` that do not match; scope `inbox` to the caller. | SEC-4 |
| B5 | Rate-limit `broadcast_prompt`; Settings toggle requiring GUI confirmation for broadcast, kill, delete_worktree, set_clipboard; prefix delivered text with an untrusted-content marker. | SEC-5 |
| B6 | Apply the origin/host check layer to `/hook`; validate `worktree_path` is absolute and under a known project base. | SEC-8 |
| B7 | Accept upload paths only from the Tauri drag-drop event (Rust-side allow-list). | SEC-9 |
| B8 | Persist audit rows (tool, args, source host) into `session_history`. | SEC-10 |

**Track C: frontend correctness** (owns `src/lib/*.svelte`, `src/lib/selection.ts`, `src/lib/events.ts`, `src/lib/result.ts`)

| # | Task | Resolves |
|---|---|---|
| C1 | Compare `host_alias` plus `tmux_name` (or `id`) in TerminalView and Sidebar; track `currentHost` in the open guard. | FE-1 |
| C2 | Make `selectedSession` derived from `selectedId` and the sessions store. | FE-2 |
| C3 | One `<Modal>` on native `<dialog>` with `showModal()`; one `ConfirmDialog`; replace `window.prompt`/`confirm` in FilesPanel; remove the four scattered Escape listeners. | FE-7, FE-8, part of FE-9 |
| C4 | Debounce resize about 50 ms. | FE-11 |
| C5 | Global toast store keyed by `IpcError.code` with `aria-live`; bootstrap errors to the footer banner; stop swallowing `pty_write` rejections. | FE-12 |
| C6 | Coalesce `session:updated` bursts per microtask in `events.ts` (belt and braces with A5). | FE-10 |

### Wave 2: operator triage and lifecycle (1-2 weeks, after W0.2, A5, C2)

**Track D** (owns `src/lib/sessions.ts`, `src/lib/Sidebar.svelte` filters, new `src/lib/attention.ts`, `st/service/sessions.rs` reconcile tick section, new `st/service/gc.rs`)

| # | Task | Resolves |
|---|---|---|
| D1 | Add `stuck_kind` and `context_pct` to the TS `SessionRow`; red stuck chip that outranks `claude_status`; context mini-bar; header "N stuck" counter that doubles as a filter. | FE-3 |
| D2 | Needs-attention filter pill; sort projects by worst child status; OS notification plus `aria-live` on stuck transitions (backend already records them); multi-select with bulk kill and send-prompt. | FE-4, PROD-3 |
| D3 | Stuck playbooks in the reconcile tick: `press_enter` sends Enter, `oom` recreates, `auth_menu` notifies. Settings toggle per playbook. | PROD-2 |
| D4 | Session GC: `idle_since` column, per-kind TTL (bg 24 h idle, shell 7 d), Settings-driven sweeper that calls `safe_kill_session` for work sessions with a dirty tree and plain kill otherwise. | PROD-1 |
| D5 | Default friendly name from the first `send_prompt` or `new_bg_session.prompt`; friendly names as the default sidebar view; `whoami { tmux_name }` tool; optional `session_id` on every name-addressed tool. | PROD-4, MCP-6 |
| D6 | Run `reconcile_one_host` after `new_bg_session` launch and return the fleet row; `peek_session` accepts `claude_session_id`. | MCP-7 |
| D7 | Populate `pr_url` from `gh pr view` on reconcile; add `last_prompt`, `started_at`, `last_turn_at`, CI status badge to the row. | PROD-5 |

**Track H: observability** (runs in parallel with D; one mechanical PR touching the 41 `eprintln!` sites, so land it when no Track A or B PR is open)

| # | Task | Resolves |
|---|---|---|
| H1 | Add `tauri-plugin-log` (or `tracing-subscriber` with a rolling file in the app data dir); route every `eprintln!` through `log::warn!`/`info!`; honour `RUST_LOG`. | OPS-3 |
| H2 | "Copy diagnostics" action in Settings: version, `schema_version`, host probe results, tunnel state, last N log lines with tokens redacted. | OPS-3 |
| H3 | Runbook sections in `docs/troubleshooting.md`: reconcile tick and `reconcile.interval_secs`, behaviour after sleep/wake, tmux server restart (all sessions become ghosts), log location. | OPS-6 |

### Wave 3: orchestration (2-3 weeks, after B3 and D6)

**Track E** (owns `st/mcp/hooks.rs`, `st/service/hooks.rs`, new `st/service/tasks.rs`, new migration, `st/mcp/tools.rs` new handlers, new `src/lib/TasksPanel.svelte`)

| # | Task | Resolves |
|---|---|---|
| E1 | `turn_seq` and `last_stop_at` on sessions; increment on Stop; set busy on `UserPromptSubmit`; `send_prompt` returns `turn_seq_before`; `wait_for_session { session_id, until: idle or turn_gt, timeout_s }` as a bounded long-poll. | MCP-1 |
| E2 | `session_transcript { session_id, since_turn?, max_chars? }` reading the Claude JSONL and returning the last assistant turn as text; `run_prompt { session_id, prompt, timeout_s }` composing send, wait, transcript. | MCP-4 |
| E3 | `tasks` table; `dispatch_task`, `wait_for_task`, `list_tasks`; `parent_session_id` on sessions; `reply_to` on messages; generalize the safe-kill nonce marker for completion detection. | MCP-5 |
| E4 | Tasks panel in the UI (prompt, assignee, state, result link). | PROD-6 |
| E5 | `tags` column with a `list_sessions` filter; one-line "use when" on each lifecycle verb. | MCP-11 |

### Wave 4: structure and tests (2-3 weeks; F4 and F5 need a quiet window because they touch everything)

| # | Task | Resolves | Notes |
|---|---|---|---|
| F1 | Inject a `TmuxExec` factory; test ghosting, transition events, and bg-agent surfacing through the real reconcile functions. | BE-7 | do first; makes A1-A3 verifiable |
| F2 | Tests for `pty.rs` and `service/messages.rs`; make `send_message` one transaction. | BE-9 | parallel with F1 |
| F3 | `ansi.ts`: iterate by code point, wcwidth table with trailing-half cells, swallow DCS/APC to ST; add Delete, Insert, F-keys, Alt-as-ESC, Ctrl+Shift+C/V on Linux, copy-on-select as a pref. | FE-5, FE-6 | parallel; only touches terminal files |
| F4 | Split `store.rs` into `store/{mod,schema,rows,sessions,hosts_accounts,projects,timeline,reconcile}.rs`; `service/sessions.rs` into `service/sessions/{reconcile,paths,lifecycle,prompt,review}.rs`; `mcp/tools.rs` into `params.rs` plus per-domain routers combined with `+`; `lib.rs` into `bootstrap/env.rs`, `singleton.rs`, tick into service. | BE-5 | sequential, one PR per file, nothing else open |
| F5 | Split `Sidebar.svelte` into `SessionRowItem`, `SidebarFilters`, `NewBgSessionDialog`, `PeekPanel`, `session_status.ts`; `TerminalView.svelte` into `terminal_keys.ts`, `terminal_mouse.ts`, drain module; `SettingsDialog` into `McpSettings` and `HostsTable`. | FE-9 | sequential, after D1/D2 land |
| F6 | Extract an `SshExec` trait (run, run_cancellable, write_file); scripted fake that records commands; opt-in `#[ignore]` integration test that spins a real local tmux server when `tmux` is on PATH. | OPS-8 | pairs with F1; same author ideally |
| F7 | `proptest` for `shell::quote` (arbitrary bytes round-trip through `bash -c "printf %s"`); `fast-check` for `ansi.ts` (random byte streams never throw, cursor stays in bounds, chunk splitting is invariant). | OPS-9 | parallel, small |
| F8 | CI matrix `[macos-latest, ubuntu-24.04]` with Tauri apt prerequisites; `tauri-apps/tauri-action` release job gated on tags; document that signing is absent until an Apple Developer ID exists. | OPS-11 | blocked on the Actions billing block; feeds G3 |

### Wave 5: product (after Wave 2)

| # | Task | Resolves |
|---|---|---|
| G1 | Per-session cost from `~/.claude/projects/<id>.jsonl` usage on the host during reconcile; surface in the row and `fleet_health`. | PROD-7 |
| G2 | `move_session` tool and menu item: rsync session JSONL to host B, `new_session` with `--resume`, kill source. ADR descoping Freeze; migration dropping `handoffs` and `frozen_scrollback`. | PROD-8 |
| G3 | Packaged DMG/AppImage; projects base as a Settings field; one-host quickstart in getting-started. | PROD-9 |

## 5. Parallelism and dependencies

```
Wave 0 (12 tasks, all parallel, disjoint files)
   |
   +--> Track A (backend)   --+
   +--> Track B (security)  --+--> Wave 2 Track D + Track H --> Wave 3 Track E
   +--> Track C (frontend)  --+          |
                                         +--> Wave 5 G1..G3
Wave 4 F1, F2, F3, F6, F7 can start any time after Wave 0.
Wave 4 F4, F5 need a quiet window: no other PR open against the split files.
Wave 4 F8 is blocked on the Actions billing block.
```

Hard dependencies:

- D1 needs W0.2 (enum) and C2 (derived selection).
- D2 notifications need A5 (otherwise sixty events per tick spam the toast).
- E1 needs B3 (remote hooks) or remote sessions never get a Stop.
- E3 needs D6 (tracked bg sessions) and D5 (`session_id` everywhere).
- B4 needs B1.
- G2 needs W0.9 (migration table).

Suggested fleet layout: one worktree and session per track (A, B, C, D, E,
F3), plus a short-lived session per Wave 0 task. Merge order inside Wave 1 does
not matter; the tracks own disjoint files.

## 6. Stop or remove

- **Freeze scope on the hand-rolled terminal** at "watch and type". Land F3 for
  correctness, then stop. Direct further terminal effort at an "open in native
  terminal" action.
- **Stop growing the read-only git browser** (`FilesPanel`, `CommitGraph`,
  `BranchList`, eight `repo_*` tools). Keep `repo_changes` and `repo_diff`
  maintained; mark the rest maintenance-only.
- **Remove Freeze from the spec** and drop the dead `handoffs` table and
  `frozen_scrollback` column in the G2 migration.

## 7. Coverage check

Every finding maps to at least one task:

| Findings | Tasks |
|---|---|
| BE-1..12 | A1, A2, A3, A4, F4, W0.9, F1, W0.6, F2, W0.6, A5, A5 |
| SEC-1..12 | B1, B2, B3, B4, B5, W0.7, W0.7, B6, B7, B8, B2, W0.7 |
| FE-1..13 | C1, C2, D1, D2, F3, F3, C3, C3, F5, C6, C4, C5, W0.8 |
| MCP-1..11 | E1, W0.2, B3, E2 and W0.5, E3, D5, D6, W0.3, W0.4, W0.5, E5 |
| PROD-1..9 | D4, D3, D2, D5, D7, E4, G1, G2, G3 |
| OPS-1..12 | W0.10, W0.11, H1 and H2, A1, W0.1, H3, W0.12, F6, F7, W0.8, F8, W0.11 |

## 8. Decisions needed from the owner

1. **Per-host tokens (B1)** change the provisioning contract; every host must be
   re-provisioned once. Confirm before Track B starts.
2. **Confirmation toggle for destructive MCP tools (B5)** defaults on or off? On
   is safer; off keeps unattended dispatch working.
3. **GC defaults (D4)**: bg 24 h idle and shell 7 d are proposals.
4. **God-file split window (F4)**: needs a week with nothing else open against
   `store.rs` and `service/sessions.rs`.
5. **Release process (W0.10)**: keep release-please (needs the Actions billing
   block lifted) or switch to a manual script. Recommendation: manual script
   now, since Actions cannot run; revisit when billing is resolved.
6. **Actions billing block**: F8 and the release-please path both stay blocked
   until it is lifted. Everything else in this plan works with local CI plus
   `--admin` merges.
