# Hardening Review — 2026-05-21

A multi-agent review of `claude-fleet` at commit `b77a952`: Rust backend,
Svelte frontend, security, and spec-vs-implementation gaps. Findings are
deduplicated and prioritised below. Severities: **CRITICAL** (RCE / data loss /
startup brick), **HIGH** (correctness / leaks under normal use), **MEDIUM**,
**LOW**.

The Rust build could not be compiled in the review environment (missing GUI
system libs) — the backend review is static.

## Resolution status (updated 2026-05-21)

**Fixed and verified** (`cargo test` + `cargo clippy -D warnings` + `vitest`
all green):

- **CRITICAL:** CR1, CR2, CR3.
- **HIGH:** H1–H7.
- **MEDIUM:** M1, M2, M3, M4, M7, M8, M9, M10, M11, M12. M6 (`with_transaction`
  in reconcile) was resolved independently by the iter-4b `apply_host_reconcile`
  rework.
- **LOW:** L1, L8. L4 (double bootstrap) and L6 (ANSI scroll region) were
  resolved by other work.
- The pre-existing `localStorage`-undefined vitest failure is also fixed
  (in-memory polyfill in `vitest.setup.ts`).

**Deliberately deferred:**

- **M5** (`ensure_master` keeps a stale `OnceCell` after `ControlPersist`
  expiry) — the only fixes are a round-trip-per-call `ssh -O check` or an
  invasive rewrite to `ControlMaster=auto`; the impact is a lost-multiplexing
  perf degradation after 10 min idle, not a correctness bug.
- **L2, L3, L5, L7** and the remaining LOW items — micro-opts / naming /
  feature gaps; tracked here but low value.
- The spec-vs-code gaps (Handoff, Freeze) — need a product decision.

The remainder of this document is the original review, left intact as the
backlog.

---

## CRITICAL

### CR1 — SSH host-alias option injection → local RCE
`ssh.rs:85-201`, `pty.rs:82-117`, `commands/hosts.rs:43-72`

`add_host` / `probe_ssh_alias` accept `alias` and `ssh_alias` from IPC with **no
validation**, and `pty_open` takes `host_alias` the same way. These strings are
passed as the bare host argument to `ssh`. `ssh` interprets a leading `-` as an
option, so an alias like `-oProxyCommand=sh -c "<cmd>"` runs an arbitrary local
command at probe time. `ssh_config.rs::is_real_alias` does not reject `-`-leading
aliases, so a crafted `~/.ssh/config` block reaches `ssh` unchecked; the IPC
commands are also directly callable from the webview.

**Fix:** validate every host/ssh alias before use — non-empty, allowlist
`[A-Za-z0-9._-]`, must not start with `-`. Pass `--` before the host argument
in every `ssh` invocation. Centralise validation in the Rust command layer
(never trust the frontend).

### CR2 — `send_prompt` / remote commands run through `bash -c` guarded only by a hand-rolled quoter
`commands/sessions.rs:649-716`, `tmux.rs:167-179`, `pty.rs:315-327`

Every remote tmux/git command and the local `send_prompt` branch builds a shell
string (`bash -c` / `bash -lc <script>`) interpolating user-controlled values
(session name, prompt, owner, repo, branch, worktree). The defence rests
entirely on a single-quote escaper that is **copy-pasted into four divergent
copies** (`shell_quote`, `shq`, `shell_quote_str`, `shell_escape`). The escaper
logic is currently correct, but one missed `shq()` at any call site is direct
remote command injection, and there is no test asserting every substitution is
quoted.

**Fix:** consolidate to one audited `shell_quote` in a shared module; delete the
three copies. Where possible, spawn `tmux`/`git` with discrete argv elements
instead of building a shell string (e.g. local `send_prompt` →
`Command::new("tmux").args([...])`, no shell). Add per-call-site tests that
metacharacters are neutralised.

### CR3 — SQLite migrations are not atomic → a partial migration bricks startup
`store.rs:97-120`, `migrations/003`, `migrations/004`

Migrations `002`–`004` run without a transaction. If `003` is interrupted after
`ALTER TABLE hosts ADD COLUMN account_uuid` but before the `schema_version`
bump, the next launch re-runs `003`, the `ALTER TABLE` fails with "duplicate
column", `migrate()` returns `Err`, and `expect("open store")` panics — the app
will not start, with no user-facing message.

**Fix:** wrap each migration plus its version bump in a single transaction.

---

## HIGH

### H1 — `Store` mutex held across event emission (and runtime-blocking contention)
`commands/sessions.rs:72-132`, `events.rs`

`reconcile_sessions` holds `store.lock()` for the whole write loop, and every
`upsert_session`/`update_host_probe`/`delete_*` inside it calls `bus.emit`,
which serialises a payload and dispatches into the webview. Emitting while
holding the mutex blocks every other thread that needs `store.lock()`. Because
`Store` is behind a `std::sync::Mutex` (not async-aware), a blocked `lock()`
parks an entire tokio worker thread; under JoinSet fan-out plus concurrent IPC
this can stall the runtime.

**Fix:** collect rows to emit into a `Vec` inside the lock, drop the guard, then
emit. Longer term, move `Store` access onto a dedicated blocking thread / actor.

### H2 — Single global PTY: stale-output bleed and double-open leaks
`pty.rs:156-223`, `TerminalView.svelte:47-168`

There is one global PTY. On fast session switching:
- `drainOnce()` checks `screen`/`ptyOpen` only at entry, not after its `await`;
  an in-flight `pty_drain` can resolve after the new screen is built and write
  stale bytes into it (`TerminalView.svelte:149-168`).
- Two `$effect`s both call the async `openTerm()`, whose `currentSession` guard
  is set only after `await pty_open` — so a second open runs fully, leaking a
  `ResizeObserver` and a `setInterval` drain timer (`TerminalView.svelte:47-65`).
- `pty_open` hands the new reader thread the *same* buffer `Arc` as the old one,
  so the dying old reader and the new reader append to one `Vec` (`pty.rs:156-170`).

**Fix:** capture a generation/`screen` reference in `drainOnce` and bail if it
changed after the await; add a synchronous reentrancy guard in `openTerm`;
allocate a fresh buffer `Arc` per `pty_open`.

### H3 — Optimistic merge vs. event bus: resurrected / duplicated rows
`sessions.ts:25-92`, `App.svelte:79-89`

`killSession` optimistically `removeSession(id)`, but a `session:updated` event
already queued for that id will re-insert it (`mergeSession` pushes when
`findIndex === -1`). Same shape for hosts. `newSession`'s optimistic merge can
also overwrite a fresher event-driven row with a staler command return value.

**Fix:** keep a short-lived tombstone set of removed ids that `mergeOne` ignores;
merge with a monotonic-field guard (only overwrite if not older).

### H4 — Account FK error aborts the entire multi-host reconcile
`store.rs:588-614`, `commands/sessions.rs:78-131`

`upsert_session` uses `?`, so one host's `account_uuid` FK violation aborts the
whole reconcile and the user sees no sessions for *any* host. Account writes are
also not atomic with the host-account link.

**Fix:** isolate per-host errors (collect, continue); make account upsert + host
link a single transaction; consider `ON DELETE SET NULL` on the FK columns.

### H5 — Path traversal in remote project/worktree paths
`commands/sessions.rs:340-446`

`remote_project_path` builds `{home}/projects/github.com/{owner}/{repo}` and
worktree paths by raw `format!`. `owner`/`repo`/`wt_name`/`branch` are never
checked for `..` or `/`. A project or worktree directory named `../../…` yields
a path that escapes the intended root — `git clone` then writes to an arbitrary
remote directory. Shell-quoting does not stop `..` traversal.

**Fix:** validate `owner`/`repo`/`wt_name`/`branch` against `[A-Za-z0-9._-]`,
reject `..` and leading `-`, before building any path or git command.

### H6 — PTY reader threads leaked; children not killed on app exit
`pty.rs:186-223`, `lib.rs:171`

Reader threads are spawned detached with no `JoinHandle`. The
`WindowEvent::Destroyed` handler calls `ssh_client.shutdown_all()` but never
closes the PTY, so a `tmux attach` / `ssh -tt` child survives app quit.

**Fix:** add a PTY shutdown to the `Destroyed` handler.

### H7 — `cancel`/`run_cancellable` task and registry leaks on panic
`commands/sessions.rs:459-478`, `cancel.rs:40-58`, `ssh.rs:207-238`

If a command panics before `reg.unregister`, the `DashMap` slot leaks forever
(slow unbounded growth). `run_cancellable`'s cancel arm kills the child but
never aborts the stdout/stderr reader tasks.

**Fix:** release the registry slot via an RAII guard / `Drop`; abort the reader
tasks on the cancel arm.

---

## MEDIUM

| ID | Area | Finding | Fix |
|----|------|---------|-----|
| M1 | Security | CSP is `null` in `tauri.conf.json` — webview runs with no Content-Security-Policy. | Set a restrictive CSP (`default-src 'self'`, scoped `connect-src`). |
| M2 | Security | `devtools: true` ships in release builds — anyone can open DevTools and drive every IPC command. | Gate devtools to debug builds. |
| M3 | Security | ControlMaster socket dir under `~/.cache` created with default (umask) perms. | `chmod 0700` the dir, or move to `$XDG_RUNTIME_DIR`. |
| M4 | Backend | `pty_drain` lossy-decodes UTF-8 mid-codepoint; partial trailing bytes become U+FFFD and are consumed. | Decode only to the last valid boundary; retain the partial tail. |
| M5 | Backend | `ensure_master` `OnceCell` stays `Ok` after the master self-closes (`ControlPersist=10m`) — multiplexing silently lost until restart. | Invalidate the cell on socket-gone, or `ssh -O check`. |
| M6 | Backend | `with_transaction` exists and is tested but is never called — reconcile does N separate writes/fsyncs/emit bursts; a mid-reconcile failure leaves a partial DB visible. | Wire `with_transaction` into `reconcile_sessions` (the `TODO(iter4a-M3)` at `sessions.rs:65`). |
| M7 | Frontend | App.svelte hydrate/save effect gate is a racy boolean; rapid session switches can cross-write pane sizes between sessions. | Gate on a per-session token, not a boolean. |
| M8 | Frontend | `Resizer.svelte` `pointermove` listener leaks if `pointerup` is missed (drag into native chrome). | Use `setPointerCapture`; also handle `pointercancel`. |
| M9 | Frontend | Rename inputs share `data-testid="rename-input"`; `document.querySelector` can target the wrong row. | Use `bind:this` on the specific element. |
| M10 | Frontend | `commitRename` / `onRestart` / `doKill` have no in-flight guard; Enter+blur double-fires rename. | Add a `busy`/`committing` guard. |
| M11 | Frontend | ANSI SGR `38`/`48` with truncated sub-params falls through and the colour numbers get re-read as standalone SGR codes. | On incomplete sub-params, set `i = params.length`. |
| M12 | Backend | `migrate()`/`appdata_db_path()` panic on any DB failure — unrecoverable crash, no user message. | Surface a user-facing error path for store-open failure. |

---

## LOW / fragile-but-correct

- `health_check` uses `.expect("store mutex poisoned")` — panics instead of
  returning `E_LOCK` like every other command (`health.rs:28`).
- `pty.rs` re-derives the ControlPath by hand instead of sharing
  `SshClient::control_path`; the two can disagree when `HOME` is unset.
- `delete_sessions_not_in` does a SELECT then a DELETE — a `DELETE … RETURNING`
  would remove the TOCTOU and the duplicated placeholder building.
- App.svelte and Sidebar.svelte both bootstrap all four stores on mount —
  duplicate startup IPC. Bootstrap once.
- `ipc.ts` holds only `healthCheck`; the real IPC layer lives in `result.ts` —
  misleading naming.
- ANSI emulator implements no scroll-region (`CSI r`); LF at the bottom always
  scrolls the whole buffer, including on the alt screen.
- IME / composition / paste input never reaches the PTY (`keyToBytes` handles
  single-char `keydown` only).
- `set_host_hidden` emits no event, unlike every other host mutation.
- Confirmed **safe**: all SQL is parameterised (no injection); OAuth tokens are
  never read (only the non-secret `oauthAccount` subset); `StrictHostKeyChecking`
  is never disabled (`BatchMode=yes` fails closed); terminal output and error
  strings are Svelte-escaped (no XSS, no `{@html}`).

---

## Spec-vs-implementation gaps

- **Handoff (orig. spec §8.3) and Freeze (§8.4) are unimplemented.** The
  `handoffs` table and `frozen_scrollback` column exist but are never
  read/written; dead `.status-frozen` CSS remains. No spec records the descope.
- **xterm.js → custom renderer** is a deliberate, well-reasoned drift but no
  spec was updated to record it.
- **Multiple terminal tabs** (orig. spec §7.1) are not implemented — `PtyState`
  is single-PTY by design.
- **`reconcile` tests test stand-ins, not the real functions.**
  `parallel_reconcile_does_not_serialise_on_slow_host` and
  `reconcile_one_host_does_not_touch_other_hosts` exercise inline re-implementations
  / lower-level helpers, not `reconcile_sessions`/`reconcile_one_host` — they
  pass but prove little. Add tests driving the real functions with a mock
  `TmuxExec`.
- **Missing tests:** `send_prompt` command dispatch (only the pure helper is
  tested), `run_cancellable` → `E_CANCELLED`, multi-line / `$`-containing
  prompts, per-mutation event emission for hosts/accounts/projects/worktrees,
  the `related_sessions` command + `relatedSessions()` wrapper (dead, untested).
- **`ensure_remote_project` cancel cleanup unmet:** the iter4a cancellation
  contract promised clone-into-temp + atomic rename + `Drop`-guard cleanup; the
  code clones directly into the final path and leaves corrupt repos on cancel,
  which the `[ ! -d .git ]` idempotency guard then skips re-cloning.
- **Stale in-code TODOs** that the iter4a spec claims as done:
  `sessions.rs:39` (JoinSet orphans children) and `sessions.rs:65`
  (`with_transaction` in reconcile). Also obsolete: `ssh.rs:137` note,
  `health.rs:14,26` Phase-1 TODOs.

---

## Suggested fix order

1. **CR1 + H2(pty)** — alias validation + `--` separator + per-`pty_open` buffer.
   Highest impact: closes the local RCE and the worst PTY race.
2. **CR3** — wrap migrations in transactions (prevents startup brick).
3. **CR2** — consolidate `shell_quote`; argv-spawn where possible.
4. **H1 + H4 + M6** — decouple emit from the `Store` lock; per-host error
   isolation; wire `with_transaction` into reconcile.
5. **H3** — tombstones + monotonic merge guard for the optimistic/event races.
6. **H5** — path-component validation for remote project/worktree paths.
7. **M1 + M2** — restrictive CSP, disable release devtools.
8. **H6 + H7** — PTY shutdown on exit; RAII registry guard; abort reader tasks.
9. Remaining MEDIUM/LOW and the test-coverage gaps.
10. Write an ADR formally descoping (or scheduling) Handoff and Freeze so the
    spec set stops misrepresenting the product.
