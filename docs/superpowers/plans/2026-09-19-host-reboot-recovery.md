# Host-reboot recovery (PR 2/2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user bring back sessions lost to a host reboot: batch-restore the lost rows fleet kept (R3), and find resumable conversations on a host whose rows are already gone (R4).

**Architecture:** PR 1 (branch `feat/host-reboot-survival`, PR #135) keeps lost rows with `lost_reason` and `claude_session_id`. This PR adds `service/sessions/restore.rs`, which plans without I/O and executes each entry through the existing `recreate_session`, bounded by two settings. It also adds `service/sessions/discover.rs`, which runs one read-only host script over `~/.claude/projects/*/*.jsonl` and ranks candidates in Rust. A discovered candidate is restored through `new_session` with a new optional `resume_claude_session_id`. Both features get an MCP tool, a Tauri command, and a HostDetail UI.

**Tech Stack:** Rust (fleet-core service/store/mcp, tokio), Tauri 2 commands, Svelte 5 + Vitest.

**Spec:** `docs/superpowers/specs/2026-09-17-host-reboot-session-survival-design.md` (sections R3, R4, Testing, Delivery item 2)

## Global Constraints

- Branch `feat/host-reboot-recovery`, stacked on `feat/host-reboot-survival`. Implementers NEVER run `git pull/push/rebase/checkout/switch/stash/reset`; only `git add` + `git commit`.
- Run `cargo fmt --all` before every commit. Clippy `-D warnings` must stay clean.
- Before reporting done, each task runs the FULL affected suite (`cargo test -p fleet-core` for Rust tasks; `npx vitest run` + `npx svelte-check` for frontend tasks). A filtered run alone is not a pass.
- Every value interpolated into a shell script is quoted with `crate::shell::quote`.
- Never hold the `Store` mutex guard across `.await`.
- Every new test must be able to fail. Where a test asserts an absence (zero calls, no write), the task says how to prove it can fail.
- A new MCP tool or Tauri command requires `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- No automatic restore, and no auto-answering of first-run prompts (spec: out of scope).
- Restore is offered only for tmux-kind rows (`kind NOT IN ('bg','external')`) with a non-NULL `claude_session_id`.
- Setting defaults: `restore.batch_size` = `4`, `restore.stagger_ms` = `3000`.
- Event kinds: `session_restored` and `session_restore_failed` (detail = error message).

## Rulings (deviations from the spec, decided up front)

1. **No `RowChange::HostSessionsLost`.** Every lost row already arrives as `session:updated`. The UI derives "N lost sessions" from the session store, and a batch event would have no consumer.
2. **`013_session_events.sql` is not edited.** It is a shipped migration. The vocabulary is documented on `insert_session_event` in `store/timeline.rs` instead.
3. **Plan `cwd` without I/O.** `dry_run` does zero SSH calls, but a remote cwd is resolved over SSH. The plan therefore reports the known worktree path (worktree row `path`), falling back to the project's `base_path` for `local`, else `None` ("resolved at restore time").
4. **Discovery reads truncated lines.** A transcript's last line can be a multi-MB tool result, so the host emits, per file, the last line containing `"cwd"`, cut to 8 KiB. Rust extracts `cwd`/`gitBranch` by locating the key and decoding the JSON string literal that follows with `serde_json`. The line need not be valid JSON as a whole. Claude writes these keys before `message`, so they survive the cut.
5. **Boot time for ranking is a number, never an identity.** The script emits `bootsec=` from `/proc/uptime` (Linux: `now - uptime`) or the `sec =` field of `kern.boottime` (macOS). Both are epoch seconds, so both are timezone-independent. Unknown means `rank_hint = "unknown"`.
6. **Name reuse.** `new_session` with an explicit `name` equal to a lost row's `tmux_name` that has a `claude_session_id` is rejected with `E_EXISTS` ("…belongs to a lost session; restore or dismiss it first") instead of silently overwriting the lost conversation id. A manual `tmux new -s <same name>` outside fleet still revives the row (reconcile's `COALESCE` keeps the old id). This is documented, not changed.
7. **Restore fails fast on an unreachable host.** A non-dry-run call checks `hosts.reachable` first and returns `E_HOST_OFFLINE` for the whole call, instead of N identical per-entry failures.

## File Structure

- `crates/fleet-core/src/store/rows.rs` — `SessionRow.lost_reason`, `SESSION_COLUMNS`, `map_session_row`
- `crates/fleet-core/src/store/sessions.rs` — reclassify emits; `lost_resumable_session_for_name`
- `crates/fleet-core/src/service/settings.rs` + `src/lib/fleet_settings.ts` + `src/lib/SettingsDialog.svelte` — two settings
- `crates/fleet-core/src/service/sessions/restore.rs` (new) — plan + execute
- `crates/fleet-core/src/service/sessions/discover.rs` (new) — parse + rank + service
- `crates/fleet-core/src/tmux.rs` — `DISCOVER_TRANSCRIPTS_SCRIPT` builder
- `crates/fleet-core/src/service/sessions/lifecycle.rs` — `resume_claude_session_id`, name-reuse guard
- `crates/fleet-core/src/mcp/tools/session_ops.rs`, `mcp/tools/support.rs`, `mcp/guard.rs`, `mcp/tools/params.rs` — tools
- `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs` — commands
- `src/lib/sessions.ts`, `src/lib/HostDetail.svelte`, `src/lib/SessionRowItem.svelte` (+ tests) — UI
- `docs/control-api-reference.md` (generated), `docs/control-api.md`, `skills/claude-fleet-control/SKILL.md`

---

### Task 1: Expose `lost_reason` on the session row

**Files:**
- Modify: `crates/fleet-core/src/store/rows.rs` (struct `SessionRow` ~:97, `SESSION_COLUMNS` :167, `map_session_row` :201, `MarkedLost` doc ~:424)
- Modify: struct literals that stop compiling: `service/health.rs:164`, `service/playbooks.rs:321`, `service/gc.rs:368`, `service/sessions/tests.rs:143`, `store/reconcile.rs:563` (`bare_row`)
- Modify: `crates/fleet-core/src/store/sessions.rs` (comment ~:228 and the reclassify path of `mark_host_sessions_lost`; helper `lost_reason_of` ~:1138)
- Modify: `src/lib/sessions.ts` (type, ~:39), `src/lib/SessionRowItem.svelte` (ghost branch ~:207-216) + its test

**Interfaces:**
- Produces: `SessionRow.lost_reason: Option<String>` (serialized as `lost_reason`); TS `lost_reason?: string | null`; exported TS fn `lostReasonLabel(reason: string | null | undefined): string | null` in `src/lib/sessions.ts`.

- [ ] **Step 1: Failing Rust tests** in `store/sessions.rs` tests:
  - `lost_reason_is_on_the_row`: insert a tmux row with a claude id, call `mark_host_sessions_lost` with reason `host_reboot` (use the same call shape as the existing PR 1 tests in this file). Assert `get_session_by_id(id).unwrap().unwrap().lost_reason.as_deref() == Some("host_reboot")`, and that after `restore_session(id)` it is `None`.
  - `reclassifying_a_missing_ghost_emits_an_update`: subscribe to the store's bus the way existing emit tests in this file do, create a `missing` ghost, run `mark_host_sessions_lost` so it is reclassified, and assert a `SessionUpdated` whose row has `lost_reason == Some("host_reboot")` was emitted.
- [ ] **Step 2:** Run `cargo test -p fleet-core lost_reason_is_on_the_row reclassifying_a_missing_ghost_emits_an_update`. Expect a compile FAIL (no field).
- [ ] **Step 3: Implement.**
  - Add `pub lost_reason: Option<String>,` to `SessionRow` right after `lost_at`.
  - Append `lost_reason` as the LAST column of `SESSION_COLUMNS` (after `usage_updated_at`) and read it by the next index in `map_session_row`.
  - Add `lost_reason: None` to every struct literal the compiler reports.
  - In the reclassify branch of `mark_host_sessions_lost`, collect the reclassified ids and emit each with `self.emit_session(id)` after the tx commits (the same way marked rows are emitted). Update the ~:228 comment, which says no event is emitted because the field is not on the row.
  - Update the `MarkedLost` doc accordingly.
- [ ] **Step 4:** Rust tests pass; the full `cargo test -p fleet-core` passes.
- [ ] **Step 5: Frontend.**
  - Add `lost_reason?: string | null;` under `lost_at` in the TS `SessionRow`.
  - Add and export:
    ```ts
    export function lostReasonLabel(reason: string | null | undefined): string | null {
      switch (reason) {
        case 'host_reboot': return 'host rebooted';
        case 'tmux_server_gone': return 'tmux server stopped';
        default: return null;
      }
    }
    ```
  - In `SessionRowItem.svelte`'s ghost branch, render the label after "lost {timeAgo}" when non-null, as ` · {label}`, in a span with `data-testid="lost-reason"`.
  - Tests:
    - `src/lib/sessions.test.ts`: `lostReasonLabel` for all three kinds of input (`host_reboot`, `tmux_server_gone`, and `missing`/`null` → `null`).
    - `SessionRowItem` test: a ghost row with `lost_reason: 'host_reboot'` shows "host rebooted"; with `null`, no `lost-reason` element.
- [ ] **Step 6:** `npx vitest run`, `npx svelte-check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`.
- [ ] **Step 7:** Commit `feat(sessions): expose lost_reason on the session row`.

### Task 2: Restore settings

**Files:** `crates/fleet-core/src/service/settings.rs`, `src/lib/fleet_settings.ts`, `src/lib/SettingsDialog.svelte`

**Interfaces:**
- Produces: `pub const RESTORE_BATCH_SIZE: &str = "restore.batch_size";` (`Kind::Int { min: 1, max: 16 }`, default `"4"`) and `pub const RESTORE_STAGGER_MS: &str = "restore.stagger_ms";` (`Kind::Int { min: 0, max: 60000 }`, default `"3000"`). TS keys `restoreBatchSize`, `restoreStaggerMs`.

- [ ] **Step 1:** Add both `Spec` entries next to `SESSIONS_LOST_TTL_SECS`. Run `cargo test -p fleet-core every_spec_has_a_settings_dialog_row`. Expect FAIL (no TS mirror). This proves the sync test covers the new keys.
- [ ] **Step 2:**
  - Add the mirror lines to `SETTING_KEYS` (`  restoreBatchSize: 'restore.batch_size',`, `  restoreStaggerMs: 'restore.stagger_ms',`) and `SETTING_DEFAULTS` (`'restore.batch_size': '4',`, `'restore.stagger_ms': '3000',`).
  - Add two number-input rows to `SettingsDialog.svelte` beside the lost-TTL row, following the pattern of an existing `Kind::Int` row in the dialog (search for `settingInt`):
    - "Concurrent restores" (`data-testid="restore-batch-size"`, min 1, max 16).
    - "Delay between restores (ms)" (`data-testid="restore-stagger-ms"`, min 0, max 60000, step 500).
  - The copy for each states what it bounds: "Sessions resumed in parallel by Restore lost sessions" and "Pause between starting each resumed session".
- [ ] **Step 3:** `cargo test -p fleet-core settings` passes, then the full `cargo test -p fleet-core`, `npx vitest run` and `npx svelte-check`.
- [ ] **Step 4:** Commit `feat(settings): restore.batch_size and restore.stagger_ms`.

### Task 3: Restore service — plan and execute

**Files:**
- Create: `crates/fleet-core/src/service/sessions/restore.rs` (register in `service/sessions/mod.rs` and re-export the pub items the way sibling modules are re-exported)
- Modify: `crates/fleet-core/src/store/timeline.rs` (doc of `insert_session_event`: list the full kind vocabulary, adding `session_restored`, `session_restore_failed`, and `lost`)

**Interfaces:**
- Consumes: `recreate_session(RecreateSessionArgs{session_id, force:false}, store, ssh) -> Result<SessionRow, IpcError>` (lifecycle.rs:1102); `Store::insert_session_event(id, kind, detail)`; settings `RESTORE_BATCH_SIZE`, `RESTORE_STAGGER_MS` via `get_setting` + `settings::resolve` (pattern: `read_lost_ttl_cutoff` in reconcile.rs:57).
- Produces:
  ```rust
  #[derive(Deserialize, rmcp::schemars::JsonSchema)]
  #[schemars(crate = "rmcp::schemars", rename = "RestoreHostSessionsParams")]
  pub struct RestoreHostSessionsArgs {
      /// Host whose lost sessions to restore.
      pub host_alias: String,
      /// Return the plan only: no ssh calls, no writes. Default false.
      #[serde(default)]
      pub dry_run: bool,
      /// Restrict to these fleet session ids. Default: every restorable lost session on the host.
      #[serde(default)]
      pub session_ids: Option<Vec<i64>>,
  }
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct RestorePlanEntry {
      pub session_id: i64,
      pub tmux_name: Option<String>,   // None only for an unknown id
      pub cwd: Option<String>,
      pub claude_session_id: Option<String>,
      pub friendly_name: Option<String>,
      pub action: &'static str,        // "restore" | "skip"
      pub reason: Option<String>,      // set iff action == "skip"
  }
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct RestoreOutcome { pub session_id: i64, pub tmux_name: String, pub ok: bool, pub error: Option<String> }
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct RestoreReport { pub host_alias: String, pub dry_run: bool, pub plan: Vec<RestorePlanEntry>, pub results: Vec<RestoreOutcome> }

  pub fn plan_restore(s: &Store, args: &RestoreHostSessionsArgs) -> Result<Vec<RestorePlanEntry>, IpcError>;
  pub async fn restore_host_sessions(args: RestoreHostSessionsArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<RestoreReport, IpcError>;
  // test seam, used by restore_host_sessions:
  pub(crate) async fn restore_host_sessions_with<F, Fut>(args: RestoreHostSessionsArgs, store: &Mutex<Store>, recreate: F) -> Result<RestoreReport, IpcError>
  where F: Fn(i64) -> Fut + Send + Sync, Fut: Future<Output = Result<SessionRow, IpcError>> + Send;
  ```

Selection rules for `plan_restore`:
- It first validates `host_alias` (`crate::validate::host_alias`).
- The candidate set is the host's rows (`list_sessions_for_host`).
- Without `session_ids`: only rows with `lost_at` set, tmux kind (`kind` not `bg`/`external`) and a `claude_session_id` are entries, each `restore`. The result is ordered by `lost_at` then `id`.
- With `session_ids`: one entry per requested id, in request order, deduplicated. Skip reasons:
  - `"not found on this host"` (unknown id, or a row on another host). `tmux_name` is `None` for an unknown id.
  - `"not lost"`.
  - `"background agent: resume it with its own tooling"` (bg/external).
  - `"no claude conversation id to resume"`.
- Plan `cwd`: the worktree row's `path` when `worktree_id` is set. Otherwise, for `local`, the project's `base_path`. Otherwise `None`.

Execution rules for `restore_host_sessions_with`:
- `dry_run` → return `{plan, results: []}`. It must not call `recreate` and must not write.
- Otherwise:
  - Host row missing or not `reachable` → `Err(E_HOST_OFFLINE)` before any `recreate`.
  - Read `batch_size`/`stagger_ms` from settings, falling back to the defaults.
  - Launch `restore` entries in plan order with at most `batch_size` in flight (`tokio::sync::Semaphore`), sleeping `stagger_ms` between successive launches (`tokio::time::sleep`), and collect every result. One `Err` never aborts the others.
  - Per result:
    - Ok → `insert_session_event(id, "session_restored", None)`, then `tracing::info!(host_alias, tmux_name, claude_session_id, session_id, "[restore] session restored")`.
    - Err → `insert_session_event(id, "session_restore_failed", Some(&e.to_string()))` and `tracing::info!` with `error`.
    - Event-write failures only `warn!`.
  - Take the lock briefly per write; never across an await.
  - `results` is ordered like the plan's `restore` entries.
- `restore_host_sessions` passes `|id| recreate_session(RecreateSessionArgs { session_id: id, force: false }, store, ssh)`.

- [ ] **Step 1: Failing tests** in `restore.rs` `#[cfg(test)] mod tests`, using the in-memory store helpers the sibling `service/sessions/tests.rs` uses (host row reachable; seed rows via the same insert helpers):
  - `dry_run_plans_every_lost_resumable_session_and_touches_nothing`
    - Seed three lost tmux rows with ids, one live row, one lost bg row and one lost row without a claude id.
    - Run `dry_run: true` with a `recreate` closure that increments an `AtomicUsize`.
    - Assert: exactly the three restore entries, each field populated; the counter is 0; `list_sessions_for_host` and the `session_events` rows are identical before and after (compare full `Vec<SessionRow>` and the event count).
    - Can-fail proof: temporarily call `recreate` once in the dry-run branch and see the test fail, then revert.
  - `explicit_ids_report_skips_with_reasons`: request `[live, bg, no_claude_id, other_host_row, 999_999, lost_ok, lost_ok]`. Assert the entries in order (deduplicated) with the exact reason strings above.
  - `one_failure_does_not_stop_the_batch`: three lost rows; the closure returns `Err(IpcError::new(codes::E_REPAIR_REQUIRED, "worktree gone"))` for the middle id and `Ok(row)` otherwise. Assert results `[ok, err "worktree gone", ok]`, one `session_restored` event on each ok row and one `session_restore_failed` with detail containing "worktree gone" on the failed row.
  - `unreachable_host_fails_fast`: host `reachable=false`, `dry_run:false`. Expect `Err` code `E_HOST_OFFLINE` and 0 closure calls.
  - `concurrency_never_exceeds_batch_size` (`#[tokio::test(start_paused = true)]`): 6 lost rows, `restore.batch_size=2`, `stagger_ms=0`. The closure increments an in-flight counter, records the max, sleeps 1 s, then decrements. Assert max == 2 and 6 results.
  - `launches_are_staggered` (`start_paused = true`): 3 rows, `batch_size=4`, `stagger_ms=3000`. The closure records `tokio::time::Instant::now()`. Assert the gaps between successive start instants are ≥ 3000 ms.
- [ ] **Step 2:** Run `cargo test -p fleet-core restore::tests`. Expect FAIL (module missing).
- [ ] **Step 3:** Implement as specified. Update the `insert_session_event` doc vocabulary.
- [ ] **Step 4:** The tests pass; the full `cargo test -p fleet-core` passes; clippy is clean.
- [ ] **Step 5:** Commit `feat(sessions): restore_host_sessions service`.

### Task 4: Restore — MCP tool, Tauri command, frontend wrapper, docs

**Files:**
- `crates/fleet-core/src/mcp/tools/session_ops.rs` (new tool beside `recreate_session` ~:350)
- `crates/fleet-core/src/mcp/tools/support.rs` (`LIFECYCLE_TOOLS` :874)
- `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs` (`generate_handler!` next to `recreate_session` :240)
- `src/lib/sessions.ts` (next to `recreateSession` :490) + test
- `docs/control-api-reference.md` (regenerate), `docs/control-api.md`, `skills/claude-fleet-control/SKILL.md`

**Interfaces:**
- Consumes: Task 3's `restore_host_sessions`, `RestoreHostSessionsArgs`, `RestoreReport`.
- Produces: MCP tool `restore_host_sessions`; Tauri command `restore_host_sessions(args: RestoreHostSessionsArgs)`; TS `restoreHostSessions(hostAlias: string, opts?: { dryRun?: boolean; sessionIds?: number[] }): Promise<Result<RestoreReport>>` plus the TS types `RestorePlanEntry`, `RestoreOutcome`, `RestoreReport` mirroring the Rust ones.

- [ ] **Step 1: Failing MCP test** in `mcp/tools/tests.rs`. Follow an existing test that builds a host-bound `Caller` for a `require_host` tool such as `new_session`. A token bound to host `a` calling `restore_host_sessions { host_alias: "b", dry_run: true }` gets `E_FORBIDDEN`. The master caller with `dry_run: true` on a seeded host gets JSON with `"dry_run": true` and a `plan` array.
- [ ] **Step 2: Implement the tool.**
  ```rust
  #[tool(description = "Restore sessions a host lost to a reboot or a tmux server restart: \
      resume each lost tmux session's Claude conversation in its original worktree, under its \
      original name. Call with dry_run=true first to get the plan (no ssh, no writes). \
      Concurrency and pacing come from the restore.batch_size / restore.stagger_ms settings. \
      One failing session never fails the others; the result lists each session's outcome. \
      First-run prompts in a resumed session are not answered: they surface as stuck_kind.")]
  pub(super) async fn restore_host_sessions(
      &self,
      Extension(caller): Extension<Caller>,
      Parameters(args): Parameters<sessions::RestoreHostSessionsArgs>,
  ) -> Result<CallToolResult, McpError> {
      audit("restore_host_sessions", &format!("host={} dry_run={}", args.host_alias, args.dry_run));
      require_host(&caller, &args.host_alias, "the lost sessions")?;
      let report = sessions::restore_host_sessions(args, &self.store, &self.ssh).await.map_err(to_mcp_err)?;
      ok_json(&report)
  }
  ```
  Add `"restore_host_sessions"` to `LIFECYCLE_TOOLS`. Do NOT add it to `READONLY_TOOLS`.
- [ ] **Step 3:** Add the Tauri command (same shape as `recreate_session` at `src-tauri/src/commands/sessions.rs:147`) and register it. Add the TS wrapper; it does not patch the store itself, because row events arrive via `session:updated`. Add a vitest in `sessions.test.ts`, mocking `invokeCmd` the way the `recreateSession` tests do, that asserts the payload `{ args: { host_alias, dry_run, session_ids } }`, with `session_ids` omitted → `null`.
- [ ] **Step 4:** `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`. Add a short `restore_host_sessions` entry to `docs/control-api.md` and a line to `skills/claude-fleet-control/SKILL.md` where lifecycle tools are listed ("after a host reboot: `restore_host_sessions {dry_run:true}` then without dry_run").
- [ ] **Step 5:** Run the full suite: `cargo test --workspace`, clippy, fmt, `npx vitest run`, `npx svelte-check`.
- [ ] **Step 6:** Commit `feat(mcp): restore_host_sessions tool and command`.

### Task 5: HostDetail "Restore N lost sessions…"

**Files:** `src/lib/HostDetail.svelte` (Sessions section :183-205, dialogs :256-278), `src/lib/HostDetail.test.ts` (create if absent, following `HostsView.test.ts`'s rendering and mocking pattern)

**Interfaces:** Consumes: `restoreHostSessions`, `RestoreReport` (Task 4), `lostReasonLabel` (Task 1).

Behaviour:
- `restorable = hostSessions.filter(s => s.lost_at !== null && s.claude_session_id && s.kind !== 'bg' && s.kind !== 'external')`.
- When `restorable.length > 0`, the Sessions section header shows a button: `Restore {n} lost session{n===1?'':'s'}…` (`data-testid="restore-lost"`).
- Click → `restoreHostSessions(host.alias, { dryRun: true })`:
  - On error, show the error text inline (`data-testid="restore-error"`).
  - On success, open a `ConfirmDialog`:
    - title `Restore lost sessions on {host.alias}?`
    - `confirmLabel="Restore"`
    - `confirmTestId="confirm-restore"`
  - The dialog `children` snippet lists each plan entry: friendly_name ?? tmux_name, cwd, and for a skip the reason. It ends with the note "Each session resumes its Claude conversation. Any first-run prompt waits for you."
- Confirm → `restoreHostSessions(host.alias, { sessionIds: <restore entries' ids> })` with `busy` set. When done, close the dialog and show a summary line `Restored {ok} of {n}` plus each failure `{name}: {error}` (`data-testid="restore-summary"`).
- Extend the `confirm` state union with `'restore'`.

- [ ] **Step 1: Failing vitest tests:**
  - The button is hidden without restorable rows and shows "Restore 2 lost sessions…" with two.
  - Clicking calls the wrapper with `dryRun: true`, and the dialog lists both names.
  - Confirming calls it with `sessionIds: [ids]` and renders "Restored 1 of 2" plus the failure text for a mocked `{ok:false,error:'worktree gone'}`.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** `npx vitest run`, `npx svelte-check` (0 errors).
- [ ] **Step 4:** Commit `feat(ui): restore a host's lost sessions from HostDetail`.

### Task 6: `new_session` can resume a given conversation; name reuse guard

**Files:** `crates/fleet-core/src/service/sessions/lifecycle.rs` (`NewSessionArgs` :11, `new_session` ~:540-700), `crates/fleet-core/src/store/sessions.rs`, the MCP `NewSessionParams` in `mcp/tools/params.rs` (and its mapping into `NewSessionArgs`), `src/lib/sessions.ts` (`newSession` args type)

**Interfaces:**
- Produces:
  - `NewSessionArgs.resume_claude_session_id: Option<String>` (serde default None).
  - The same field on the MCP `NewSessionParams`, with the doc `/// Resume this Claude conversation id instead of starting a new one (from discover_lost_sessions). Must be a transcript on this host.`
  - `Store::lost_resumable_session_named(host_alias: &str, tmux_name: &str) -> rusqlite::Result<Option<SessionRow>>`: a row with `lost_at IS NOT NULL AND claude_session_id IS NOT NULL`.

Behaviour:
- Validate `resume_claude_session_id` with `crate::validate::claude_session_id` → `E_INVALID`.
- When it is set:
  - The pane command is the resume form: `tmux::pane_command_for` with that id, the same helper `recreate_pane_command` uses. Build it exactly as `recreate_pane_command(kind, Some(id))` does.
  - The value stored with `set_claude_session_id` is that id instead of a fresh uuid.
  - Read `new_session` to find where the fresh uuid is generated and threaded into both, and replace it in both places.
- Name guard: before any tmux call, if `lost_resumable_session_named(host, name)` returns a row, fail with `E_EXISTS`: `"{name} belongs to a lost session (id {id}); restore it with restore_host_sessions or dismiss it first"`.
  - This applies whether the name was explicit or filled, although `fill_session_name` already avoids ghost names.
  - Shell sessions are included: the guard runs before the kind split.

- [ ] **Step 1: Failing tests** (service tests in `service/sessions/tests.rs`, matching how existing `new_session` tests fake tmux; if `new_session` cannot be driven without ssh there, test the two pure helpers instead):
  - The guard: a lost row `dev-x` with a claude id → `new_session{name:"dev-x"}` → `E_EXISTS`. The lost row's `claude_session_id` is unchanged.
  - A lost row without a claude id does not block.
  - `resume_claude_session_id` = a valid uuid → the pane command contains `--resume '<uuid>'`, and the row's `claude_session_id` equals it.
  - An invalid id (`"x; rm -rf"`) → `E_INVALID`.
  - Store unit test for `lost_resumable_session_named`.
- [ ] **Step 2:** Implement. Add the TS field `resume_claude_session_id?: string` to the `newSession` args type.
- [ ] **Step 3:** `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, because the MCP params changed. Then the full `cargo test --workspace`, clippy, fmt, `npx svelte-check`.
- [ ] **Step 4:** Commit `feat(sessions): new_session can resume a conversation; never reuse a lost session's name`.

### Task 7: Discovery — host script, parser, ranking (pure)

**Files:** `crates/fleet-core/src/tmux.rs` (script const + tests), create `crates/fleet-core/src/service/sessions/discover.rs` (pure part + tests)

**Interfaces:**
- Produces:
  ```rust
  // tmux.rs
  pub fn discover_transcripts_script(limit: usize) -> String;
  // discover.rs
  #[derive(Debug, Clone, PartialEq)]
  pub struct TranscriptProbe { pub claude_session_id: String, pub mtime: i64, pub cwd: Option<String>, pub git_branch: Option<String> }
  pub fn parse_discover_output(stdout: &str) -> (Option<i64> /*boot epoch*/, Vec<TranscriptProbe>);
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct LostCandidate {
      pub cwd: String, pub git_branch: Option<String>, pub claude_session_id: String,
      pub transcript_mtime: i64, pub derived_tmux_name: Option<String>,
      pub project_id: Option<i64>, pub worktree_id: Option<i64>,
      pub existing_session_id: Option<i64>, pub rank_hint: String,
  }
  pub fn rank_candidates(boot: Option<i64>, probes: Vec<TranscriptProbe>) -> Vec<LostCandidate>; // derived/project/worktree/existing left None; filled by Task 8
  ```

Script (use exactly this shape; `limit` is an integer, so it needs no quoting; clamp it to 1..=500 in the builder):
```sh
now=$(date +%s)
if [ -r /proc/uptime ]; then printf 'bootsec=%s\n' "$(( now - $(cut -d. -f1 /proc/uptime) ))"; else printf 'bootraw=%s\n' "$(sysctl -n kern.boottime 2>/dev/null)"; fi
for f in "$HOME"/.claude/projects/*/*.jsonl; do [ -f "$f" ] || continue; case "$f" in */subagents/*) continue;; esac; printf '%s\t%s\n' "$(date -r "$f" +%s)" "$f"; done | sort -rn | head -n LIMIT | while IFS="$(printf '\t')" read -r m f; do
  printf '@@F\t%s\t%s\n' "$m" "$(basename "$f" .jsonl)"
  printf '@@L\t%s\n' "$(tail -c 4194304 "$f" | grep -a '"cwd"' | tail -n 1 | cut -c1-8192)"
done
```
(Build it as a Rust string with `LIMIT` replaced by the clamped number.)

Parsing rules:
- `bootsec=N` → Some(N).
- `bootraw=` → parse the integer after `sec = ` (e.g. `{ sec = 1726000000, usec = 0 } Thu Sep …`).
- Anything else → None.
- `@@F\t<mtime>\t<id>` starts a probe. The id must pass `crate::validate::claude_session_id`, else the probe is dropped. An unparseable mtime drops it too.
- The following `@@L\t<line>` fills `cwd`/`git_branch`: find `"cwd":`, skip whitespace, and decode the JSON string literal starting at the next `"` (scan to the matching unescaped quote, then run `serde_json::from_str::<String>` on that slice). The same applies to `"gitBranch":`. A malformed or absent value → None.
- Lines not starting with `@@F`/`@@L`/`boot` are ignored.

Ranking rules:
- Drop probes without `cwd`.
- Group by `cwd` and keep the max `mtime` (tie → larger id string).
- `rank_hint`:
  - `"unknown"` when boot is None.
  - `"after_boot"` when mtime ≥ boot.
  - `"before_boot"` when boot − mtime ≤ 86400.
  - `"stale"` otherwise.
- Order: `before_boot` (mtime desc), then `after_boot` (mtime desc), then `stale` (mtime desc), then `unknown` (mtime desc).

- [ ] **Step 1: Failing tests:**
  - Script tests:
    - `discover_script_runs_under_local_bash`: create a temp `$HOME` with `.claude/projects/p1/<uuid1>.jsonl` (two lines, the last with `"cwd":"/w/a","gitBranch":"main"`), `p1/<uuid2>/subagents/<uuid3>.jsonl`, and `p2/<uuid4>.jsonl` whose last cwd line is followed by a 20 KB line without `"cwd"`.
    - Run the script with `HOME` set to the temp dir and `/bin/bash -c`.
    - Parse it. Assert two probes (uuid1, uuid4), no uuid3, cwd/branch correct, and `boot.is_some()`.
    - Set the mtimes with `filetime` if it is already a dev-dependency; else via `touch -t`.
  - Parser tests:
    - A truncated line (`{"parentUuid":"x","cwd":"/a/b \"q\"","sessionId":"…","gitBranch":"feat/x","message":{"content":"abc` with no closing) → cwd `/a/b "q"`, branch `feat/x`.
    - The `bootraw` macOS format.
    - An invalid id is dropped.
    - Garbage lines are ignored.
  - Ranking tests:
    - Duplicate cwd keeps the newest.
    - The four hints at their boundaries (mtime == boot → after_boot; boot−86400 → before_boot; boot−86401 → stale).
    - The ordering across groups.
    - None boot → all `unknown`, ordered by mtime.
- [ ] **Step 2:** FAIL, then implement, then PASS; the full `cargo test -p fleet-core`; clippy; fmt.
- [ ] **Step 3:** Commit `feat(sessions): parse and rank Claude transcripts for lost-session discovery`.

### Task 8: Discovery — service, MCP tool, Tauri command, docs

**Files:** `discover.rs` (service part), `mcp/tools/session_ops.rs`, `mcp/tools/support.rs` (`LIFECYCLE_TOOLS`), `mcp/guard.rs` (`READONLY_TOOLS` :36), `src-tauri/src/commands/sessions.rs`, `src-tauri/src/lib.rs`, `src/lib/sessions.ts`, docs as in Task 4

**Interfaces:**
- Consumes: Task 7; `HostShell` (reconcile.rs:107) / `RealHostShell`; `HostPaths::for_host`, `find_project_id_for_path`, `worktree_key_for_host` (paths.rs); the name derivation in `fill_session_name` (lifecycle.rs:356).
- Produces:
  ```rust
  #[derive(Deserialize, rmcp::schemars::JsonSchema)]
  #[schemars(crate = "rmcp::schemars", rename = "DiscoverLostSessionsParams")]
  pub struct DiscoverLostSessionsArgs {
      /// Host to scan.
      pub host_alias: String,
      /// Max transcripts to read, newest first. Default 50, max 500.
      #[serde(default)]
      pub limit: Option<i64>,
  }
  pub async fn discover_lost_sessions(args: DiscoverLostSessionsArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<Vec<LostCandidate>, IpcError>;
  pub(crate) async fn discover_lost_sessions_with(args: DiscoverLostSessionsArgs, store: &Mutex<Store>, shell: &dyn HostShell) -> Result<Vec<LostCandidate>, IpcError>;
  pub(crate) fn derive_tmux_name(owner: &str, repo: &str, worktree_key: &str) -> String; // extracted from fill_session_name's deterministic branch; fill_session_name must call it (no behaviour change)
  ```
- TS: `discoverLostSessions(hostAlias: string, limit?: number): Promise<Result<LostCandidate[]>>` and the `LostCandidate` type.

Behaviour:
- Validate the host.
- Run the script via `shell.run_script` (an error propagates).
- Parse and rank.
- Under one short lock, for each candidate:
  - `existing_session_id` = the id of a row on this host (live or lost) whose `claude_session_id` equals the candidate's.
  - `project_id` = `find_project_id_for_path`.
  - `worktree_id` = the worktree row on this host whose `path` == cwd.
  - `derived_tmux_name` = `derive_tmux_name(owner, repo, worktree_key_for_host(cwd, paths))` when the project is known.
- Mutates nothing.

- [ ] **Step 1: Failing tests:**
  - A fake `HostShell` returning canned script output drives `discover_lost_sessions_with`.
    - Seed a project with owner/repo whose base path is the cwd's repo root, a worktree row, and one existing lost row whose claude id matches a candidate.
    - Assert `project_id`, `worktree_id`, `derived_tmux_name` (`dev-<owner>-<repo>--<wt>`), `existing_session_id`, and that the full session rows and event count are unchanged.
  - A shell error → `Err`.
  - Extract `derive_tmux_name` and prove `fill_session_name`'s existing tests still pass unchanged.
- [ ] **Step 2:** Implement the service.
- [ ] **Step 3: MCP tool.**
  - Its description says: read-only; it scans `~/.claude/projects` on the host for recent Claude conversations when fleet has no row for them (e.g. after a reboot before this fleet version); it ranks them relative to the host's boot; the derived tmux name may differ from the original for a second session on a worktree; restore one with `new_session { host_alias, project_id, worktree_id, name: derived_tmux_name, resume_claude_session_id }`.
  - Use `Extension(caller)` + `require_host`.
  - Add the tool to `READONLY_TOOLS` and `LIFECYCLE_TOOLS`.
  - MCP test: a host-bound token on another host → `E_FORBIDDEN`. Assert the readonly guard allows it the way existing readonly tests do.
- [ ] **Step 4:** Tauri command + registration + TS wrapper + vitest payload test. Regen docs; update `docs/control-api.md` and SKILL.md.
- [ ] **Step 5:** Full suite (Rust workspace, clippy, fmt, vitest, svelte-check).
- [ ] **Step 6:** Commit `feat(mcp): discover_lost_sessions`.

### Task 9: HostDetail "Find lost conversations…"

**Files:** `src/lib/HostDetail.svelte`, `src/lib/HostDetail.test.ts`, `src/lib/sessions.ts` (use existing `newSession`)

Behaviour:
- A secondary button `Find lost conversations…` (`data-testid="discover-lost"`) sits in the Sessions section, always visible when the host is reachable.
- Click → `discoverLostSessions(host.alias)` → an inline list (`data-testid="discover-list"`). For each candidate it shows the cwd, branch, `timeAgo(transcript_mtime)`, a rank badge (`before_boot` → "before reboot", `after_boot` → "since boot", `stale` → "older", `unknown` → none), and the derived name.
- An `existing_session_id` candidate shows "already in fleet" and no action.
- A candidate with `project_id` and `derived_tmux_name` gets a `Resume` button. It calls `newSession` with `{host_alias, project_id, worktree_id, name: derived_tmux_name, resume_claude_session_id}`; on success it marks the item "resumed", on error it shows the error inline.
- Candidates without a project show "no fleet project for this path" and no button.
- An empty result → "No Claude conversations found on {host}".

- [ ] **Step 1: Failing vitest:**
  - The list renders the three candidate shapes (resumable, existing, no project).
  - Resume calls `newSession` with the exact args, including `resume_claude_session_id`.
  - An error is shown inline.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** `npx vitest run`, `npx svelte-check`.
- [ ] **Step 4:** Commit `feat(ui): find and resume lost Claude conversations from HostDetail`.

---

## Self-review

- Spec R3 is covered: selection, dry_run, execution, settings, events, authority, UI (Tasks 2–5), plus the non-readonly classification (Task 4).
- Spec R4 is covered: the script, subagents skip, Rust parsing, cwd grouping, boot ranking, existing rows, derived name, and a separate explicit restore (Tasks 6–9).
- R5 "restored" logging is in Task 3.
- The PR 1 deferrals are covered: `lost_reason` on the row (Task 1), name reuse (Task 6), HostSessionsLost (ruling 1).
- Testing section: dry-run immutability, isolated failure, subagents/dedupe/boot ranking. Spec acceptance 3 stays manual and is recorded in the PR.
