# Session State Machine Hardening Implementation Plan

**Status:** landed (#343). Its migration shipped as **065**, not the 061 below. Every step below is checked against `main` and ticked.

**Follow-up (2026-09-28):** Task 5's sweep read tmux `#{session_activity}` as "pane output", but it moves only on client input and attach, so one tool call longer than `reconcile.stale_working_secs` was demoted, and every agents pass that saw the spinner lifted it again (a `status_change` pair per tick). A pass whose pane shows the spinner now stamps `sessions.pane_working_at` (migration **080**), which the sweep respects. A demoted row's `idle` is a guess: `store::trusted_status` makes `run_prompt`, a move's source check and `wait_for_session { until: idle }` ask the pane (`session_activity`) before believing it. `store::turn_over` is defined through `ClaudeStatus::is_quiet`, held to the frontend's `isQuietStatus` by the shared fixture `crates/fleet-core/src/service/testdata/quiet_statuses.json`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `claude_status` / `stuck_kind` trustworthy on the live fleet: the `oom` playbook can no longer kill a working session, the `oom` detector needs a dead process, ghosts and shells stop carrying pane-derived state, a `StopFailure` reads as `failed`, a `working` row ages out, the attention model names the five things a person can act on, and the timeline records one event per transition.

**Architecture:** Everything lands in `crates/fleet-core` (store SQL, the `service/` layer the Tauri commands and MCP tools share, the pane-intel parser) plus the desktop's mirror of the settings registry and the attention buckets. The wire contract is only extended additively: two `#[serde(default)]` fields (`SessionRow.stale_working_at`, `Health.context_red_pct`) and four new `needs_attention.reason` strings; no call shape changes, so `CONTRACT_REVISION` stays at 4. Four new operator settings (`playbooks.oom_max_attempts`, `gc.external_lost_ttl_secs`, `reconcile.stale_working_secs`, `health.context_red_pct`) go through `service::settings::SPECS` and its desktop mirror.

**Tech Stack:** Rust 2021 (`rusqlite`, `regex`, `serde`, `tokio`), SQLite migrations, Svelte 5 + TypeScript (Vitest, svelte-check).

**Spec:** `docs/ux/2026-09-27-live-instance-analysis/README.md` (themes T2, T4; code table rows 1–5), evidence in `docs/ux/2026-09-27-live-instance-analysis/lifecycle.md` (F1–F4, F6, F7, F8, F10) and `docs/ux/2026-09-27-live-instance-analysis/ux.md` (F-09). All `file:line` references below are from `origin/main @ 7dad1665` (worktree `.claude/worktrees/live-instance-fix-plans`).

## Global Constraints

From `CLAUDE.md` (verbatim):

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the
  frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting has **one** canonical implementation: `crate::shell::quote`
  (alias `shq`) in `crates/fleet-core/src/shell.rs`. Every value interpolated into an
  SSH/bash command string MUST be quoted with it. The former duplicate copies
  (`shell_quote`/`shell_quote_str`/`shell_escape`) were consolidated — do not
  reintroduce them.
- SQLite access goes through `Store` behind a `std::sync::Mutex`. Never hold the
  guard across an `.await`.
- No blocking I/O under `Mutex<PtyState>` and none on a sync Tauri command (a
  sync command runs on the macOS main thread). PTY input goes to the writer
  thread through its bounded channel — `E_PTY_BUSY` when it is full,
  `E_PTY_CLOSED` when the thread is gone; kill / reap / fd teardown runs on the
  `PtyParts` taken out under the lock, after the guard is released.
- **Status vocabulary** (`claude_status`, `stuck_kind`) lives in the enums in
  `service/pane_intel.rs`; the MCP tool descriptions and the generated
  reference derive from them, so add values there, not in prose.
- `docs/control-api-reference.md` is generated from the MCP tool router. After
  editing any `#[tool(...)]` description or the `generate_handler!` list,
  regenerate it or CI fails: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
- `src/lib/hub_verdicts.generated.json` and the refusal table in `docs/hub.md`
  are generated from `src-tauri/src/backend/verdicts.rs`. After editing any row,
  regenerate them or CI fails: `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`
- SQLite in `store/` (migrations are registered in the `MIGRATIONS` table there — add a new
  `NNN_<topic>.sql` plus an entry).

Repo gotchas that apply to this plan:

- A new wire field on a struct that crosses hub↔desktop needs `#[serde(default)]` (an old hub/desktop otherwise breaks) and fails the hub contract golden test: step `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (the regen run reports FAILED once; re-run to see green). Bump `CONTRACT_REVISION` ONLY if a call shape changes — nothing in this plan does, so it stays at 4 and `MIN_HUB_CONTRACT`/`MAX_HUB_CONTRACT` in `src-tauri/src/backend/contract.rs` stay put.
- No new Tauri command or MCP tool is added and no `#[tool(...)]` description changes, so no verdict row, no `REGEN_HUB_VERDICTS`, no `REGEN_DOCS`. A test caps the served MCP tool description budget — do not add prose to tool descriptions.
- Every new setting key needs, in the same commit: the `Spec` in `service/settings.rs::SPECS`, a `SETTING_KEYS` entry **and** a `SETTING_DEFAULTS` entry in `src/lib/fleet_settings.ts`, and a row in `src/lib/SettingsDialog.svelte` that references `SETTING_KEYS.<name>` — `settings::tests::every_spec_has_a_settings_dialog_row` (`crates/fleet-core/src/service/settings.rs:740-770`) fails otherwise.
- Migrations: next free number on main is **061** (`crates/fleet-core/migrations/060_auth_epoch.sql` is the last; `MIGRATIONS` in `crates/fleet-core/src/store/schema.rs:311-587`). An `ALTER TABLE … ADD COLUMN` migration needs an `already_applied` guard (`schema.rs:96-106` pattern).
- Every `SessionRow { … }` struct literal must name every field; the seven on main are `crates/fleet-core/src/service/health.rs:490-538`, `service/playbooks.rs:321-369`, `service/gc.rs:~540-591`, `service/attention.rs:131-179`, `service/sessions/tests.rs:~170-227`, `store/reconcile.rs:~880-941` (two), `src-tauri/src/backend/tests_remote.rs:379-436`, `src-tauri/src/backend/tests_contract.rs:41-80`.
- Frontend: tests are Vitest (`npx vitest run <file>`), type-check `npx svelte-check`; `pnpm test`/`pnpm check` do not work on this Mac (use npx). Run `pnpm install --frozen-lockfile` first.
- Tests: `cargo test -p fleet-core <name>`; the full suite (`cargo test --workspace`) at the end of every task, unpiped.

---

### Task 1: `oom_recreate` playbook guard

**Files:**
- Modify: `crates/fleet-core/src/service/playbooks.rs` (config `:26-39`, action `:41-57`, `plan` `:69-120`, `run_with` `:227-285`, tests `:309-646`)
- Modify: `crates/fleet-core/src/store/sessions.rs` (after `mark_playbook_applied`, `:981-999`)
- Modify: `crates/fleet-core/src/store/reconcile.rs` (`stuck_since` CASE `:448-453`, params `:475-499`)
- Modify: `crates/fleet-core/src/service/settings.rs` (keys `:76-77`, `SPECS` `:248-257`)
- Modify: `src/lib/fleet_settings.ts` (`:15`, `:124`), `src/lib/SettingsDialog.svelte` (`:850-860`)
- Test: `playbooks.rs` tests, `store/sessions.rs` tests, `store/reconcile.rs` tests, `settings.rs::every_spec_has_a_settings_dialog_row`

**Interfaces:**
- Consumes: `SessionRow.{claude_status, last_turn_at, stuck_since, last_playbook_at}`, `Store::mark_playbook_applied`, `settings::get_string`.
- Produces: `PlaybookConfig.oom_max_attempts: u32`, `PlaybookAction::Skipped { action: &'static str, why: &'static str }`, `pub fn plan_with_attempts(rows, cfg, controller, now, oom_attempts: &HashMap<i64, u32>) -> Vec<Planned>`, `pub fn oom_recreate_refusal(row: &SessionRow, since: i64, attempts: u32, max_attempts: u32) -> Option<&'static str>`, `pub const OOM_ATTEMPT_WINDOW_SECS: i64 = 86_400`, `Store::count_oom_recreates_since(id: i64, since: i64) -> rusqlite::Result<u32>`, setting `playbooks.oom_max_attempts` (`Kind::Int { min: 0, max: 20 }`, default `"2"`), timeline detail `oom:recreate:skipped:<working|turn_after_stuck|attempts>`.

- [x] **Step 1: Write the failing planner tests** — append to `mod tests` in `crates/fleet-core/src/service/playbooks.rs` (after `oom_recreate_is_rate_limited_to_once_per_hour`, `:447`), and add `use std::collections::HashMap;` to the test module's imports:

```rust
    fn oom_row(id: i64, since: i64) -> SessionRow {
        row(id, "dev-oom", Some("oom"), Some(since), None)
    }

    #[test]
    fn oom_recreate_is_refused_while_the_row_is_working_and_says_so() {
        let mut r = oom_row(1, 100);
        r.claude_status = Some("working".into());
        let planned = plan(&[r], &ALL_ON, None, 200);
        assert_eq!(planned.len(), 1, "the refusal is planned so it is written once");
        assert_eq!(
            planned[0].action,
            PlaybookAction::Skipped { action: "recreate", why: "working" }
        );
    }

    #[test]
    fn oom_recreate_is_refused_when_a_turn_ended_after_the_episode_began() {
        let mut r = oom_row(1, 100);
        r.claude_status = Some("idle".into());
        r.last_turn_at = Some(150);
        assert_eq!(
            plan(&[r], &ALL_ON, None, 200)[0].action,
            PlaybookAction::Skipped { action: "recreate", why: "turn_after_stuck" }
        );
        // A turn that ended BEFORE the flag is no evidence of life.
        let mut r = oom_row(2, 100);
        r.last_turn_at = Some(50);
        assert_eq!(plan(&[r], &ALL_ON, None, 200)[0].action, PlaybookAction::Recreate);
    }

    #[test]
    fn oom_recreate_stops_at_the_attempt_budget() {
        let r = oom_row(1, 100);
        let mut spent = HashMap::new();
        spent.insert(1, 2);
        assert_eq!(
            plan_with_attempts(&[r.clone()], &ALL_ON, None, 200, &spent)[0].action,
            PlaybookAction::Skipped { action: "recreate", why: "attempts" }
        );
        spent.insert(1, 1);
        assert_eq!(
            plan_with_attempts(&[r], &ALL_ON, None, 200, &spent)[0].action,
            PlaybookAction::Recreate
        );
    }

    #[tokio::test]
    async fn run_with_counts_recreates_over_the_window_and_records_the_refusal() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_stuck(&store, "dev-oom", "oom");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: false,
        };
        let now = now_unix() + 10;
        // Two recreates already inside the 24 h window, the last of them
        // past the 1 h spacing — the spacing alone would let a third run.
        {
            let s = store.lock().unwrap();
            s.mark_playbook_applied(id, now - 7200, "oom:recreate").unwrap();
            s.mark_playbook_applied(id, now - 3601, "oom:recreate").unwrap();
        }
        assert_eq!(run_with(&store, &exec, &ALL_ON, now).await, 1);
        assert_eq!(exec.recreates.load(Ordering::SeqCst), 0, "the budget is spent");
        assert_eq!(run_with(&store, &exec, &ALL_ON, now + 20).await, 0, "written once per episode");
        let s = store.lock().unwrap();
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events.iter().any(|e| e.kind == "playbook_applied"
            && e.detail.as_deref() == Some("oom:recreate:skipped:attempts")));
    }
```

Change `ALL_ON` (`:372-375`) to include the budget:

```rust
    const ALL_ON: PlaybookConfig = PlaybookConfig {
        press_enter: true,
        oom_recreate: true,
        oom_max_attempts: 2,
    };
```

- [x] **Step 2: Run them, expect compile failures** — `cargo test -p fleet-core playbooks::tests` → `error[E0560]: struct PlaybookConfig has no field named oom_max_attempts`, `no variant named Skipped`, `cannot find function plan_with_attempts`.

- [x] **Step 3: Implement the planner** — in `crates/fleet-core/src/service/playbooks.rs`:

Replace the config (`:26-39`):

```rust
/// Recreates one session may get per [`OOM_ATTEMPT_WINDOW_SECS`]
/// (`playbooks.oom_max_attempts`): the F1 loop was one session recreated
/// twice in 83 minutes on a word it was reading, and a spacing is not a
/// budget.
pub const OOM_ATTEMPT_WINDOW_SECS: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaybookConfig {
    pub press_enter: bool,
    pub oom_recreate: bool,
    /// Recreates one session may get per [`OOM_ATTEMPT_WINDOW_SECS`];
    /// `0` refuses every recreate (the toggle stays on so refusals are logged).
    pub oom_max_attempts: u32,
}

impl PlaybookConfig {
    pub fn from_store(s: &Store) -> Self {
        Self {
            press_enter: settings::get_bool(s, settings::PLAYBOOK_PRESS_ENTER),
            oom_recreate: settings::get_bool(s, settings::PLAYBOOK_OOM_RECREATE),
            oom_max_attempts: settings::get_string(s, settings::PLAYBOOK_OOM_MAX_ATTEMPTS)
                .parse()
                .unwrap_or(2),
        }
    }
}
```

Replace the action enum (`:41-57`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybookAction {
    PressEnter,
    Recreate,
    /// No keystrokes: only the `playbook_applied` timeline entry + row event.
    Notify,
    /// A keystroke playbook that would have run but was refused. The reason
    /// goes on the timeline as `<kind>:<action>:skipped:<why>` and the
    /// episode is stamped, so the refusal is written once, not every tick.
    Skipped {
        action: &'static str,
        why: &'static str,
    },
}

impl PlaybookAction {
    fn as_str(self) -> &'static str {
        match self {
            PlaybookAction::PressEnter => "press_enter",
            PlaybookAction::Recreate => "recreate",
            PlaybookAction::Notify => "notify",
            PlaybookAction::Skipped { action, .. } => action,
        }
    }
}
```

Replace `plan` (`:69-120`) with the delegating pair plus the refusal rule:

```rust
/// Pure: decide which rows get which playbook this tick. See
/// [`plan_with_attempts`]; this shape (no attempt counts) is what every
/// caller that has no timeline in hand uses.
pub fn plan(
    rows: &[SessionRow],
    cfg: &PlaybookConfig,
    controller: Option<&(String, String)>,
    now: i64,
) -> Vec<Planned> {
    plan_with_attempts(rows, cfg, controller, now, &std::collections::HashMap::new())
}

/// Why an `oom` recreate must NOT run on this row right now, or `None` when
/// it may. `since` is the row's `stuck_since`; `attempts` how many recreates
/// the session already got in the window.
///
/// * `working`: the UserPromptSubmit hook says a turn is in flight — the
///   OOM text is scrollback, and a recreate destroys the turn (F1).
/// * `turn_after_stuck`: a Stop hook landed at/after the episode began, so
///   the REPL outlived whatever printed the text.
/// * `attempts`: the budget is spent.
pub fn oom_recreate_refusal(
    row: &SessionRow,
    since: i64,
    attempts: u32,
    max_attempts: u32,
) -> Option<&'static str> {
    if row.claude_status.as_deref() == Some("working") {
        return Some("working");
    }
    if row.last_turn_at.is_some_and(|t| t >= since) {
        return Some("turn_after_stuck");
    }
    if attempts >= max_attempts {
        return Some("attempts");
    }
    None
}

/// [`plan`] with each session's recreate count over
/// [`OOM_ATTEMPT_WINDOW_SECS`] (`oom_attempts`, by session id; absent = 0).
///
/// A row qualifies when it is a live tmux session (`status == running`, not a
/// pane-less `bg` / `external` sentinel) with a `stuck_kind` AND a `stuck_since` stamp that is newer
/// than its `last_playbook_at`. Keystroke actions never target the registered
/// controller session (it would be steering itself); notify still applies.
pub fn plan_with_attempts(
    rows: &[SessionRow],
    cfg: &PlaybookConfig,
    controller: Option<&(String, String)>,
    now: i64,
    oom_attempts: &std::collections::HashMap<i64, u32>,
) -> Vec<Planned> {
    let mut out = Vec::new();
    for r in rows {
        if r.status != "running" || crate::store::has_no_pane(&r.kind) {
            continue;
        }
        let (Some(kind), Some(since)) = (r.stuck_kind.as_deref(), r.stuck_since) else {
            continue;
        };
        if r.last_playbook_at.map(|lp| lp >= since).unwrap_or(false) {
            continue; // already acted on this episode
        }
        let is_controller = controller
            .map(|(h, t)| h == &r.host_alias && t == &r.tmux_name)
            .unwrap_or(false);
        let action = match kind {
            "press_enter" if cfg.press_enter && !is_controller => PlaybookAction::PressEnter,
            "oom" if cfg.oom_recreate && !is_controller => {
                let recently = r
                    .last_playbook_at
                    .map(|lp| now - lp < OOM_RECREATE_MIN_SPACING_SECS)
                    .unwrap_or(false);
                if recently {
                    continue;
                }
                let attempts = oom_attempts.get(&r.id).copied().unwrap_or(0);
                match oom_recreate_refusal(r, since, attempts, cfg.oom_max_attempts) {
                    Some(why) => PlaybookAction::Skipped {
                        action: "recreate",
                        why,
                    },
                    None => PlaybookAction::Recreate,
                }
            }
            "press_enter" | "oom" => continue, // gated off: leave the episode untouched
            "auth_menu" | "trust_prompt" | "reconnect" => PlaybookAction::Notify,
            _ => continue,
        };
        out.push(Planned {
            session_id: r.id,
            host_alias: r.host_alias.clone(),
            tmux_name: r.tmux_name.clone(),
            stuck_kind: kind.to_string(),
            action,
        });
    }
    out
}
```

In `run_with` (`:227-285`) read the counts under the same lock and record refusals:

```rust
    let (rows, controller, attempts) = {
        let Ok(s) = store.lock() else {
            return 0;
        };
        let rows = s.list_all_sessions().unwrap_or_default();
        let controller = s.get_controller().ok().flatten();
        let attempts: std::collections::HashMap<i64, u32> = rows
            .iter()
            .filter(|r| r.stuck_kind.as_deref() == Some("oom"))
            .map(|r| {
                (
                    r.id,
                    s.count_oom_recreates_since(r.id, now - OOM_ATTEMPT_WINDOW_SECS)
                        .unwrap_or(0),
                )
            })
            .collect();
        (rows, controller, attempts)
    };
    let planned = plan_with_attempts(&rows, cfg, controller.as_ref(), now, &attempts);
    let mut applied = 0;
    for p in planned {
        let result = match p.action {
            PlaybookAction::PressEnter => exec.press_enter(&p.host_alias, &p.tmux_name).await,
            PlaybookAction::Recreate => exec
                .recreate(p.session_id)
                .await
                .map(|()| PressEnterOutcome::Sent),
            PlaybookAction::Notify | PlaybookAction::Skipped { .. } => Ok(PressEnterOutcome::Sent),
        };
        let detail = match (p.action, &result) {
            (PlaybookAction::Skipped { action, why }, _) => {
                format!("{}:{action}:skipped:{why}", p.stuck_kind)
            }
            (_, Ok(PressEnterOutcome::Sent)) => format!("{}:{}", p.stuck_kind, p.action.as_str()),
            (_, Ok(PressEnterOutcome::SkippedAttached)) => {
                format!("{}:{}:skipped:attached", p.stuck_kind, p.action.as_str())
            }
            (_, Err(e)) => format!(
                "{}:{}:failed:{}",
                p.stuck_kind,
                p.action.as_str(),
                e.message
            ),
        };
```

(the rest of the loop — the `warn!` on `Err`, `mark_playbook_applied`, `applied += 1` — is unchanged). Update the module doc table row (`:7`) to `| oom | recreate (resume by claude_session_id) | playbooks.oom_recreate, ≤1/h, ≤playbooks.oom_max_attempts per 24 h, never while working |`.

- [x] **Step 4: Add the store counter** — in `crates/fleet-core/src/store/sessions.rs`, right after `mark_playbook_applied` (`:999`):

```rust
    /// How many times the `oom` playbook recreated session `id` — or tried
    /// and failed — since `since`: `playbook_applied` rows whose detail is
    /// exactly `oom:recreate` or starts with `oom:recreate:failed:`. A
    /// refusal (`…:skipped:…`) is not an attempt.
    pub fn count_oom_recreates_since(&self, id: i64, since: i64) -> Result<u32, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_events \
                 WHERE session_id = ?1 AND kind = 'playbook_applied' AND at >= ?2 \
                   AND (detail = 'oom:recreate' OR detail LIKE 'oom:recreate:failed:%')",
                rusqlite::params![id, since],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u32)
    }
```

and its test in the same file's `mod tests` (next to the `mark_playbook_applied` tests):

```rust
    #[test]
    fn count_oom_recreates_since_counts_recreates_and_failures_not_refusals() {
        let s = store();
        let id = s
            .upsert_session("a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        for detail in [
            "oom:recreate",
            "oom:recreate:failed:boom",
            "oom:recreate:skipped:working",
            "press_enter:press_enter",
        ] {
            s.mark_playbook_applied(id, 100, detail).unwrap();
        }
        let now = now_unix();
        assert_eq!(s.count_oom_recreates_since(id, now - 60).unwrap(), 2);
        assert_eq!(s.count_oom_recreates_since(id, now + 60).unwrap(), 0);
    }
```

- [x] **Step 5: Register the setting** — `crates/fleet-core/src/service/settings.rs`: after `:77` add

```rust
/// Recreates the `oom` playbook may run on one session per 24 h
/// (`service::playbooks::OOM_ATTEMPT_WINDOW_SECS`). `0` refuses every
/// recreate while keeping the refusals on the timeline.
pub const PLAYBOOK_OOM_MAX_ATTEMPTS: &str = "playbooks.oom_max_attempts";
```

and after the `PLAYBOOK_OOM_RECREATE` spec (`:257`):

```rust
    Spec {
        key: PLAYBOOK_OOM_MAX_ATTEMPTS,
        default: "2",
        kind: Kind::Int { min: 0, max: 20 },
    },
```

`src/lib/fleet_settings.ts`: after `:15` add `  playbookOomMaxAttempts: 'playbooks.oom_max_attempts',` and after `:124` add `  'playbooks.oom_max_attempts': '2',`.

`src/lib/SettingsDialog.svelte`: after the `Recreate sessions that ran out of memory` label's closing `</label>` (`:858`) insert

```svelte
      <div class="mcp-field">
        <label class="lbl" for="playbook-oom-max-attempts">oom budget</label>
        <input class="port" id="playbook-oom-max-attempts" type="number" min="0" max="20" step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.playbookOomMaxAttempts)}
          disabled={automationBusy}
          aria-describedby="playbook-oom-max-attempts-desc"
          data-testid="playbook-oom-max-attempts"
          onchange={(e) => onSecsChange(SETTING_KEYS.playbookOomMaxAttempts, e)} />
        <span class="hook-desc" id="playbook-oom-max-attempts-desc">recreates one session may get per 24 h (0 = never); a session that is working, or finished a turn after the flag, is never recreated</span>
      </div>
```

- [x] **Step 6: Run** — `cargo test -p fleet-core playbooks::tests` → all PASS (`oom_recreate_is_rate_limited_to_once_per_hour` still passes: `last_turn_at` is `None` and the map is empty). `cargo test -p fleet-core count_oom_recreates_since` → PASS. `cargo test -p fleet-core every_spec_has_a_settings_dialog_row` → PASS. `npx vitest run src/lib/fleet_settings.test.ts` → PASS. `npx svelte-check` → 0 errors.

- [x] **Step 7: Failing test for the episode pin** — in `crates/fleet-core/src/store/reconcile.rs` `mod tests`, after `reconcile_tracks_stuck_since_per_episode` (`:1926`):

```rust
    /// F1: `claude --resume` re-renders the last messages, so the `oom`
    /// text the playbook recreated the session for comes straight back.
    /// Today that is a NEW episode (`stuck_since` restarts on NULL → oom)
    /// and only the 1 h spacing stands between two recreates. A re-fire
    /// within that spacing of the playbook's last action is the same
    /// episode, anchored on that action, so the planner's "already acted on
    /// this episode" rule holds. `press_enter` keeps today's rule.
    #[test]
    fn an_oom_refire_within_the_recreate_spacing_continues_the_episode() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("oom"), None);
        let acted = now_unix() - 100;
        s.mark_playbook_applied(r.id, acted, "oom:recreate").unwrap();
        // The fresh pane read clean …
        let r = reconcile_one(&mut s, "a", Some("idle"), None, None);
        assert_eq!(r.stuck_since, None);
        // … then `--resume` painted the word again.
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("oom"), None);
        assert_eq!(r.stuck_since, Some(acted), "the episode is the recreate's, not a new one");
        assert!(r.last_playbook_at.unwrap() >= r.stuck_since.unwrap());

        let p = reconcile_one(&mut s, "b", Some("blocked"), Some("press_enter"), None);
        s.mark_playbook_applied(p.id, now_unix() - 100, "press_enter:press_enter")
            .unwrap();
        reconcile_one(&mut s, "b", Some("idle"), None, None);
        let p = reconcile_one(&mut s, "b", Some("blocked"), Some("press_enter"), None);
        assert!(
            p.stuck_since.unwrap() > now_unix() - 100,
            "a second Enter prompt is a new episode"
        );
    }
```

`cargo test -p fleet-core an_oom_refire_within_the_recreate_spacing` → FAILS: `assertion left: Some(<now>) right: Some(<now-100>)`.

- [x] **Step 8: Pin the episode in the upsert** — `crates/fleet-core/src/store/reconcile.rs:448-453`, replace the `stuck_since=` clause:

```sql
               -- stuck_since: keep the episode start while the kind is
               -- unchanged, restart it when the kind changes, clear when the
               -- flag clears. An `oom` flag re-appearing within the recreate
               -- spacing (?24) of the playbook's last action is `--resume`
               -- re-rendering the text that action was for: the episode
               -- continues, anchored on that action (F1).
               stuck_since=CASE WHEN ({new_stuck}) IS NULL THEN NULL
                                WHEN ({new_stuck}) IS stuck_kind THEN COALESCE(stuck_since, ?19)
                                WHEN ({new_stuck}) = 'oom' AND last_playbook_at IS NOT NULL
                                     AND ?19 - last_playbook_at < ?24 THEN last_playbook_at
                                ELSE ?19 END,
```

and append the bind after `reconciled_at` in the `params!` list (`:498`):

```rust
                reconciled_at,
                crate::service::playbooks::OOM_RECREATE_MIN_SPACING_SECS
```

- [x] **Step 9: Run** — `cargo test -p fleet-core an_oom_refire_within_the_recreate_spacing` → PASS; `cargo test -p fleet-core reconcile_tracks_stuck_since_per_episode` → PASS; `cargo test -p fleet-core store::reconcile` → PASS.

- [x] **Step 10: Full suite and commit** — `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, then:

```bash
git add crates/fleet-core/src/service/playbooks.rs crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/store/reconcile.rs crates/fleet-core/src/service/settings.rs src/lib/fleet_settings.ts src/lib/SettingsDialog.svelte
git commit -m "fix(playbooks): oom recreate refuses a live turn, keeps a budget, and does not restart the episode on a resume re-render"
```

---

### Task 2: The `oom` detector needs a dead process

**Files:**
- Modify: `crates/fleet-core/src/service/pane_intel.rs` (`:352-410`: `contains_word`, `is_oom_line`, `detect_stuck`; tests `:977-1030`)
- Create: `crates/fleet-core/src/service/testdata/pane_intel/oom_vocabulary_prose_idle.txt`, `oom_vocabulary_prose_working.txt`, `oom_heap_crash_to_shell.txt`, `oom_killed_to_shell.txt`
- Test: `pane_intel.rs` tests

**Interfaces:**
- Consumes: the ANSI-stripped pane tail (`analyze`, `:835`), `is_live_repl_line` (`:386-393`), `regex` crate (already a dependency, `std::sync::LazyLock<regex::Regex>` idiom as in `crates/fleet-core/src/claude_cli.rs:45`).
- Produces: `StuckKind::Oom` (value unchanged) only when, within the last `OOM_TAIL_LINES = 12` lines, the last OOM signal is Node's fatal heap block, OR a kill verdict followed by a shell prompt line — and in either case no live REPL chrome sits below the signal. Bare `oom` / `out of memory` / `cannot allocate memory` prose is never a signal.

The exact regexes (all run on the lower-cased tail):

| name | pattern | matches |
|---|---|---|
| `OOM_HEAP` | `fatal error: reached heap limit\|javascript heap out of memory\|<--- last few gcs --->\|allocation failed - javascript heap` | Node's own crash block: the process that printed it is dead |
| `OOM_KILLED` | `oomkilled\|out of memory: killed process \d+\|killed process \d+.*\b(oom\|out of memory)\b\|^\s*(zsh: )?killed\b\|\bsigkill\b` | the kernel / container / shell kill verdict |
| `SHELL_PROMPT` | `^(?:[\w.-]+@[\w.-]+[: ].*)?[$#%]\s*$` | `me@host:~/proj$ `, `$ `, `% `: the shell is back, the foreground process is gone |

- [x] **Step 1: Add the fixtures** (exact contents; each file ends with a newline):

`crates/fleet-core/src/service/testdata/pane_intel/oom_vocabulary_prose_idle.txt` — session 21480's shape, the fleet's own vocabulary in an idle REPL:

```
⏺ The tool descriptions carry the fleet's stuck vocabulary verbatim:
  stuck_kind is one of auth_menu | reconnect | trust_prompt | oom | press_enter.
  A session is flagged oom when its pane says "out of memory" — that is the false positive.

✻ Baked for 2m 10s · done 11:46 PM
───────────────────────────────────────────────────────────────
❯ 
───────────────────────────────────────────────────────────────
  Opus 5 (1M context)  [███░░░░░░░] 31% (310k/1.0M)  |  ~/projects/claude-fleet
  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents
```

`oom_vocabulary_prose_working.txt` — reading the vocabulary mid-turn:

```
⏺ Bash(grep -rn "oom" crates/fleet-core/src/service/pane_intel.rs)
  ⎿  32:    Oom,
     53:            StuckKind::Oom => "oom",
     374:fn is_oom_line(lower: &str) -> bool {
     376:        || lower.contains("out of memory")
     379:        || contains_word(lower, "oom")

✶ Cooking… (12s · esc to interrupt)
❯ 
```

`oom_heap_crash_to_shell.txt` — Claude's Node process died, the shell is back:

```
❯ 
  ⏵⏵ bypass permissions on (shift+tab to cycle)

<--- Last few GCs --->
[41233:0x6a3c2b0]  9187654 ms: Mark-Compact 4054.3 (4142.9) -> 4041.7 (4143.4) MB, 2911.09 / 0.00 ms  (average mu = 0.108, current mu = 0.041) allocation failure; scavenge might not succeed

FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory
 1: 0x104c5f2a8 node::Abort() [/opt/homebrew/bin/node]
me@mefistos:~/projects/claude-fleet$ 
```

`oom_killed_to_shell.txt` — the kernel's OOM killer took the process:

```
⏺ Bash(cargo test --workspace)
  ⎿  Running unittests src/lib.rs (target/debug/deps/fleet_core-1a2b3c)

Killed
martin@htz:~/projects/claude-fleet$ 
```

- [x] **Step 2: Write the failing tests** — in `pane_intel.rs` `mod tests`, replace `oom_matches_real_signals_not_innocent_words` (`:977-991`) and add three tests:

```rust
    #[test]
    fn oom_needs_a_kill_verdict_and_a_dead_process() {
        // A verdict followed by the shell's prompt: the process is gone.
        assert_eq!(
            analyze("Killed process 123 (OOM)\nme@host:~$ ").stuck,
            Some(StuckKind::Oom)
        );
        assert_eq!(
            analyze("container terminated reason=OOMKilled\n$ ").stuck,
            Some(StuckKind::Oom)
        );
        // The verdict alone is scrollback of unknown age.
        assert_eq!(analyze("Killed process 123 (OOM)").stuck, None);
        // Prose about memory is never a signal, whatever the words.
        assert_eq!(analyze("out of memory\ncannot allocate memory\noom\n$ ").stuck, None);
        assert_eq!(analyze("Let's zoom into the room — boom!").stuck, None);
        assert_eq!(analyze("⏺ Joining the Zoom meeting room").stuck, None);
    }

    /// F1: sessions 21480 / 21340 were flagged `oom` for reading the fleet's
    /// own stuck vocabulary, and the playbook recreated 21480 mid-turn twice.
    #[test]
    fn oom_never_fires_on_the_fleets_own_vocabulary() {
        for (fixture, status) in [
            (
                include_str!("testdata/pane_intel/oom_vocabulary_prose_idle.txt"),
                ClaudeStatus::Idle,
            ),
            (
                include_str!("testdata/pane_intel/oom_vocabulary_prose_working.txt"),
                ClaudeStatus::Working,
            ),
        ] {
            let intel = analyze(fixture);
            assert_eq!(intel.stuck, None, "{fixture}");
            assert_eq!(intel.derived_status, Some(status), "{fixture}");
        }
    }

    #[test]
    fn oom_fires_on_a_heap_block_or_a_kill_verdict_followed_by_the_shell() {
        for fixture in [
            include_str!("testdata/pane_intel/oom_heap_crash_to_shell.txt"),
            include_str!("testdata/pane_intel/oom_killed_to_shell.txt"),
        ] {
            let intel = analyze(fixture);
            assert_eq!(intel.stuck, Some(StuckKind::Oom), "{fixture}");
            assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked), "{fixture}");
        }
    }

    #[test]
    fn oom_looks_only_at_the_last_twelve_lines() {
        let mut old = String::from(
            "FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n",
        );
        for i in 0..12 {
            old.push_str(&format!("line {i} of a long build log\n"));
        }
        old.push_str("$ ");
        assert_eq!(analyze(&old).stuck, None);
    }
```

- [x] **Step 3: Run** — `cargo test -p fleet-core pane_intel::tests::oom` → `oom_needs_a_kill_verdict_and_a_dead_process` FAILS on `analyze("Killed process 123 (OOM)").stuck` (`left: Some(Oom), right: None`), `oom_never_fires_on_the_fleets_own_vocabulary` FAILS on the working fixture (no REPL chrome below `contains_word(…, "oom")` at line 5 → `Some(Oom)`), `oom_looks_only_at_the_last_twelve_lines` FAILS.

- [x] **Step 4: Implement** — in `pane_intel.rs` delete `contains_word` (`:352-369`) and `is_oom_line` (`:371-380`) and put in their place:

```rust
/// How far up the tail the OOM rule looks. The reconcile capture is 8 lines
/// (`PANE_TAIL_LINES`); `session_activity` reads more, and a crash block
/// older than a screen is history, not the state of the pane.
const OOM_TAIL_LINES: usize = 12;

/// Node's fatal heap block: the process that printed it is dead.
static OOM_HEAP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"fatal error: reached heap limit|javascript heap out of memory|<--- last few gcs --->|allocation failed - javascript heap",
    )
    .expect("OOM_HEAP is a valid regex")
});

/// A kill verdict from the kernel, a container runtime or the shell. On its
/// own it is scrollback; followed by a shell prompt it is the foreground
/// process gone.
static OOM_KILLED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"oomkilled|out of memory: killed process \d+|killed process \d+.*\b(oom|out of memory)\b|^\s*(zsh: )?killed\b|\bsigkill\b",
    )
    .expect("OOM_KILLED is a valid regex")
});

/// A shell prompt line: `me@host:~/proj$ `, `$ `, `% `. The shell is back,
/// so whatever ran in the foreground is gone.
static SHELL_PROMPT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^(?:[\w.-]+@[\w.-]+[: ].*)?[$#%]\s*$").expect("SHELL_PROMPT is a valid regex")
});

/// An OOM signal on one lower-cased line: the heap block or a kill verdict.
/// Prose (`out of memory`, `cannot allocate memory`, the bare acronym) is
/// deliberately NOT one: the fleet's own source, docs and MCP instructions
/// carry those words, and a session reading them was recreated twice (F1).
fn is_oom_signal(lower: &str) -> bool {
    OOM_HEAP.is_match(lower) || OOM_KILLED.is_match(lower)
}
```

and replace the OOM block at the top of `detect_stuck` (`:400-409`) with:

```rust
    // OOM: Claude died. Only the LAST signal within the tail window counts,
    // only when no live REPL chrome is drawn below it (an input box or
    // footer under the text means Claude outlived it), and only when the
    // signal is Node's own heap block or a kill verdict with the shell's
    // prompt back underneath — a process-level fact, not a word.
    let lines: Vec<&str> = lower.lines().collect();
    let tail = &lines[lines.len().saturating_sub(OOM_TAIL_LINES)..];
    if let Some(at) = tail.iter().rposition(|l| is_oom_signal(l)) {
        let below = &tail[at + 1..];
        let alive = below.iter().any(|l| is_live_repl_line(l));
        let heap = OOM_HEAP.is_match(tail[at]);
        let gone = below.iter().any(|l| SHELL_PROMPT.is_match(l.trim_end()));
        if !alive && (heap || gone) {
            return Some(StuckKind::Oom);
        }
    }
```

Update the doc comment on `is_live_repl_line` (`:382-385`) to say "below a crash signal", unchanged otherwise. `oom_text_above_a_live_repl_is_scrollback_not_a_crash` (`:994-1022`) still holds: the `cc1: out of memory` tool line is no longer a signal at all, and the crashed screen has the heap block.

- [x] **Step 5: Run** — `cargo test -p fleet-core pane_intel` → all PASS (including `oom_detected`, `oom_text_above_a_live_repl_is_scrollback_not_a_crash`, `stuck_kind_vocabulary…`). `cargo clippy -p fleet-core --all-targets -- -D warnings` → clean (no dead `contains_word`).

- [x] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/pane_intel.rs crates/fleet-core/src/service/testdata/pane_intel/oom_vocabulary_prose_idle.txt crates/fleet-core/src/service/testdata/pane_intel/oom_vocabulary_prose_working.txt crates/fleet-core/src/service/testdata/pane_intel/oom_heap_crash_to_shell.txt crates/fleet-core/src/service/testdata/pane_intel/oom_killed_to_shell.txt
git commit -m "fix(pane-intel): oom needs a dead process (heap block, or a kill verdict with the shell back), never a word"
```

---

### Task 3: Ghosts and shells carry no pane state; external ghosts get a short grace

**Files:**
- Modify: `crates/fleet-core/src/store/sessions.rs` (`mark_host_sessions_lost` UPDATE `:338-343`, `mark_session_killed` `:409-412`, `ghost_and_clean_bg_sessions` `:215-242` and its six test call sites `:1826, :1832, :1856, :2257, :2266, :2309`)
- Modify: `crates/fleet-core/src/store/reconcile.rs` (`ghost_and_clean` `:580-694`: signature, exempt clause `:596-635`, Phase 1 UPDATE `:645-648`; `apply_host_reconcile_in_tx` call `:788-798`; `NEW_STUCK` `:299-300`, `NEW_PENDING` `:304-305`, `NEW_STATUS` `:315-318`)
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs` (after `read_lost_ttl_cutoff` `:64-76`; the bg cleanup call `:1274-1283`)
- Modify: `crates/fleet-core/src/service/health.rs` (`summarize` `:376`, doc `:359-363`)
- Modify: `crates/fleet-core/src/service/settings.rs` (key after `:82`, spec after `:282`), `src/lib/fleet_settings.ts`, `src/lib/SettingsDialog.svelte` (GC section, after the `work` row `:888-896`)
- Test: `store/sessions.rs`, `store/reconcile.rs`, `health.rs` tests

**Interfaces:**
- Consumes: `Store::ghost_and_clean(tx, host, keep, now, kind_filter, cutoff, lost_ttl_cutoff, out)`, `KIND_PANE_LESS`, `settings::resolve`.
- Produces: loss (`mark_host_sessions_lost`, Phase 1, `mark_session_killed`) also sets `claude_status=NULL, stuck_kind=NULL, stuck_since=NULL, current_activity=NULL, pending_input=NULL`; `Store::ghost_and_clean_bg_sessions(host, keep, now, lost_ttl_cutoff, external_grace_cutoff: Option<i64>)`; `pub(super) fn read_external_grace_cutoff(raw: Option<String>, now: i64) -> Option<i64>`; setting `gc.external_lost_ttl_secs` (`Kind::Secs`, default `"3600"`, `0` = reap on the next pass); the tmux upsert never writes `claude_status` / `stuck_kind` / `pending_input` on a `kind = 'shell'` row; `health::summarize` skips `kind = shell` like `external`.

- [x] **Step 1: Failing store tests** — `crates/fleet-core/src/store/sessions.rs` `mod tests` (add `use crate::store::test_support::reconcile_one;` to the test module imports):

```rust
    /// F4: `local` ghosts said `working` a day after loss and a `mac` ghost
    /// said `blocked` — a dialog nobody can answer. Loss keeps identity
    /// (`claude_session_id`, names, project) and drops what only a live pane
    /// can vouch for.
    #[test]
    fn mark_host_sessions_lost_clears_the_fields_only_a_live_pane_can_vouch_for() {
        let mut s = store();
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        s.conn_ref()
            .execute(
                "UPDATE sessions SET current_activity = 'waiting for permission: rm -rf' WHERE id = ?1",
                [r.id],
            )
            .unwrap();
        s.mark_host_sessions_lost("local", "host_reboot", &[], 500, 0)
            .unwrap();
        let g = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(g.status, "ghost");
        assert_eq!(g.claude_status, None, "nobody can answer a dead pane's dialog");
        assert_eq!(g.stuck_kind, None);
        assert_eq!(g.stuck_since, None);
        assert_eq!(g.current_activity, None);
        assert_eq!(g.pending_input, None);
        assert_eq!(
            crate::service::attention::needs_attention(&g).map(|a| a.reason),
            Some(crate::service::attention::Reason::Lifecycle)
        );
    }

    /// F6: an `external` row (a Code-tab / terminal Claude fleet only
    /// observes) can never be resumed, so the 14 d TTL bought nothing — but a
    /// desktop merely restarting must not lose its rows either. One hour.
    #[test]
    fn a_lost_external_row_is_kept_for_the_grace_then_reaped() {
        let s = store();
        s.upsert_host("h").unwrap();
        s.upsert_bg_session("h", "bg:e1", None, "e1", Some("idle"), 1, "external", 1)
            .unwrap();
        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        // Inside the grace (lost at 500, grace cutoff 400): kept, though it
        // was already ghost before this pass.
        s.ghost_and_clean_bg_sessions("h", &[], 600, None, Some(400))
            .unwrap();
        assert!(
            s.get_session("bg:e1", "h").unwrap().is_some(),
            "a desktop restart must not reap its rows"
        );
        // Past it (cutoff 700 > lost_at 500): gone.
        s.ghost_and_clean_bg_sessions("h", &[], 4200, None, Some(700))
            .unwrap();
        assert!(s.get_session("bg:e1", "h").unwrap().is_none());
    }
```

`crates/fleet-core/src/store/reconcile.rs` `mod tests`:

```rust
    /// The routine Phase 1 ghosting clears the same fields
    /// `mark_host_sessions_lost` does (F4).
    #[test]
    fn phase_one_ghosting_clears_the_pane_derived_fields() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("blocked"), Some("press_enter"), None);
        assert_eq!(r.claude_status.as_deref(), Some("blocked"));
        s.apply_host_reconcile(HostReconcile {
            alias: "local",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 2,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let g = s.get_session("a", "local").unwrap().unwrap();
        assert_eq!(g.status, "ghost");
        assert_eq!(g.claude_status, None);
        assert_eq!(g.stuck_kind, None);
        assert_eq!(g.stuck_since, None);
        assert_eq!(g.pending_input, None);
    }

    /// F8: `noble-virgo-term` (kind `shell`) read as `idle` because the
    /// pane heuristic took a bare `❯` for the REPL's prompt. A shell has no
    /// Claude status to derive.
    #[test]
    fn a_shell_row_never_gets_a_pane_derived_claude_status() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("noble-virgo-term", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.conn
            .execute("UPDATE sessions SET kind = 'shell' WHERE id = ?1", [id])
            .unwrap();
        let r = reconcile_one(&mut s, "noble-virgo-term", Some("idle"), Some("press_enter"), None);
        assert_eq!(r.kind, "shell");
        assert_eq!(r.claude_status, None, "a bare ❯ is a shell prompt, not an idle REPL");
        assert_eq!(r.stuck_kind, None);
        assert_eq!(r.pending_input, None);
    }
```

`crates/fleet-core/src/service/health.rs` `mod tests`:

```rust
    /// F8: two `-term` shells were `by_status.unknown = 2`, a third said
    /// `idle`. A shell is not a Claude session; it leaves every roll-up.
    #[test]
    fn summarize_skips_shell_rows() {
        let mut sh = session(Some("idle"), Some(99.0), Some("press_enter"));
        sh.kind = "shell".to_string();
        let mut sh2 = session(None, None, None);
        sh2.kind = "shell".to_string();
        let s = summarize(&[sh, sh2, session(Some("working"), None, None)], &[]);
        assert_eq!(s.sessions_total, 1);
        assert_eq!(s.by_status.get("unknown"), None);
        assert_eq!(s.by_status.get("idle"), None);
        assert_eq!(s.context_red, 0);
        assert_eq!(s.stuck, 0);
    }
```

- [x] **Step 2: Run** — `cargo test -p fleet-core mark_host_sessions_lost_clears` → FAILS (`left: Some("blocked"), right: None`); `cargo test -p fleet-core a_lost_external_row_is_kept` → compile error (5 args, 4 expected); `cargo test -p fleet-core phase_one_ghosting_clears` → FAILS; `cargo test -p fleet-core a_shell_row_never_gets` → FAILS (`left: Some("idle")`); `cargo test -p fleet-core summarize_skips_shell_rows` → FAILS (`sessions_total 3`).

- [x] **Step 3: Clear on loss** — `store/sessions.rs:338-343`:

```rust
            let sql = format!(
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason=?2,
                     claude_status=NULL, stuck_kind=NULL, stuck_since=NULL,
                     current_activity=NULL, pending_input=NULL
                 WHERE host_alias=?3 AND status!='ghost' AND {kind_filter}
                   AND COALESCE(last_reconciled_at, 0) < ?4{not_in}
                 RETURNING id"
            );
```

`store/sessions.rs:409-412` (`mark_session_killed`):

```rust
            "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason='killed',
                 claude_status=NULL, stuck_kind=NULL, stuck_since=NULL,
                 current_activity=NULL, pending_input=NULL
             WHERE id=?2 AND status!='ghost'",
```

`store/reconcile.rs:645-648` (Phase 1):

```rust
            let sql = format!(
                "UPDATE sessions SET status='ghost', lost_at=?1, lost_reason='missing',
                     claude_status=NULL, stuck_kind=NULL, stuck_since=NULL,
                     current_activity=NULL, pending_input=NULL
                 WHERE host_alias=?2 AND status!='ghost' AND {kind_filter}{guard}{not_in}
                 RETURNING id"
            );
```

Update the doc comment of `mark_host_sessions_lost` (`:250-290`) and of `ghost_and_clean` (`:544-549`) with one sentence: "Loss also clears `claude_status`, `stuck_kind`/`stuck_since`, `current_activity` and `pending_input`: a ghost has no pane to vouch for them (F4)."

- [x] **Step 4: The external grace** — `store/reconcile.rs`, `ghost_and_clean` signature (`:580-589`) gains `external_grace_cutoff: Option<i64>` after `lost_ttl_cutoff`, and the Phase 2 prep (`:596-635`) becomes:

```rust
        let pre_ghost_ids: Vec<i64> = {
            // Placeholders are numbered as `head` grows, so `exempt` never
            // has to know whether its neighbour is present; the keep names'
            // bare `?`s continue from the highest number used.
            let mut head: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
            let mut exempt = String::new();
            // `COALESCE(..., 0)`: `lost_reason` is NULL on every row ghosted
            // before migration 036 introduced the column. SQL's three-valued
            // logic would otherwise make `lost_reason IN (...)` NULL, the
            // inner AND chain NULL, and `NOT NULL` NULL again — which `WHERE`
            // treats as "leave this row out of the reaped set", wrongly
            // exempting it.
            if let Some(c) = &lost_ttl_cutoff {
                head.push(c);
                exempt.push_str(&format!(
                    " AND NOT COALESCE((claude_session_id IS NOT NULL \
                                        AND kind != 'external' \
                                        AND lost_reason IN ('host_reboot','tmux_server_gone') \
                                        AND lost_at >= ?{}), 0)",
                    head.len()
                ));
            }
            // An `external` row is never resumable, so the TTL above never
            // covers it; `gc.external_lost_ttl_secs` keeps it just long
            // enough for the desktop that owns it to restart (F6).
            if let Some(g) = &external_grace_cutoff {
                head.push(g);
                exempt.push_str(&format!(
                    " AND NOT COALESCE((kind = 'external' AND lost_at >= ?{}), 0)",
                    head.len()
                ));
            }
            let sql = format!(
                "SELECT id FROM sessions
                 WHERE host_alias=?1 AND status='ghost' AND {kind_filter}{exempt}{not_in}"
            );
            let params = params_then(&head, keep_names);
            tx.prepare(&sql)?
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
```

The tmux caller in `apply_host_reconcile_in_tx` (`:788-798`) passes `None` for the new argument (a tmux row is never `external`). `store/sessions.rs:215-242`:

```rust
    pub fn ghost_and_clean_bg_sessions(
        &self,
        host_alias: &str,
        keep_names: &[String],
        now: i64,
        lost_ttl_cutoff: Option<i64>,
        external_grace_cutoff: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        let changes = self.in_savepoint("ghost_and_clean_bg_sessions", |tx| {
            let mut changes: Vec<RowChange> = Vec::new();
            Self::ghost_and_clean(
                tx,
                host_alias,
                keep_names,
                now,
                KIND_PANE_LESS,
                None,
                lost_ttl_cutoff,
                external_grace_cutoff,
                &mut changes,
            )?;
            Ok::<_, rusqlite::Error>(changes)
        })?;
```

Add `, None` as the fifth argument at every existing test call site (`store/sessions.rs:1826, :1832, :1856, :2257, :2266, :2309`).

`service/sessions/reconcile.rs`, after `read_lost_ttl_cutoff` (`:76`):

```rust
/// Resolve the external-ghost grace from the raw `gc.external_lost_ttl_secs`
/// value, like [`read_lost_ttl_cutoff`]: `<= 0` disables the grace (`None`,
/// reaped on the next pass); otherwise the cutoff is `now - grace`.
pub(super) fn read_external_grace_cutoff(raw: Option<String>, now: i64) -> Option<i64> {
    let grace = crate::service::settings::resolve(
        crate::service::settings::GC_EXTERNAL_LOST_TTL_SECS,
        raw.as_deref(),
    )
    .parse::<i64>()
    .unwrap_or(3600);
    if grace <= 0 {
        None
    } else {
        Some(now - grace)
    }
}
```

and the call at `:1274-1283`:

```rust
    let lost_ttl_raw = s
        .get_setting(crate::service::settings::SESSIONS_LOST_TTL_SECS)
        .ok()
        .flatten();
    let grace_raw = s
        .get_setting(crate::service::settings::GC_EXTERNAL_LOST_TTL_SECS)
        .ok()
        .flatten();
    s.ensure_in_tx()?;
    let lost_ttl_cutoff = read_lost_ttl_cutoff(lost_ttl_raw, now);
    let external_grace_cutoff = read_external_grace_cutoff(grace_raw, now);
    if let Err(e) = s.ghost_and_clean_bg_sessions(
        host_alias,
        &keep,
        now,
        lost_ttl_cutoff,
        external_grace_cutoff,
    ) {
```

Setting — `service/settings.rs` after `:82`:

```rust
/// How long a lost `external` row (a Claude fleet only observes, never
/// resumable) is kept before Phase 2 deletes it — long enough for the
/// desktop that owns it to restart, no longer. `0` reaps it on the next pass.
pub const GC_EXTERNAL_LOST_TTL_SECS: &str = "gc.external_lost_ttl_secs";
```

spec after the `GC_SWEEP_INTERVAL_SECS` spec (`:282`):

```rust
    Spec {
        key: GC_EXTERNAL_LOST_TTL_SECS,
        default: "3600",
        kind: Kind::Secs,
    },
```

`src/lib/fleet_settings.ts`: `  gcExternalLostTtlSecs: 'gc.external_lost_ttl_secs',` after `gcSweepIntervalSecs`, and `  'gc.external_lost_ttl_secs': '3600',` after `'gc.sweep_interval_secs': '300',`. `src/lib/SettingsDialog.svelte`, after the `work` hours row (`:896`):

```svelte
      <div class="mcp-field">
        <span class="lbl">outside fleet</span>
        <input class="port" type="number" min="0" step="0.5"
          value={secsToHours(settingSecs($fleetSettings, SETTING_KEYS.gcExternalLostTtlSecs))}
          disabled={automationBusy}
          data-testid="gc-external-lost-hours"
          onchange={(e) => onHoursChange(SETTING_KEYS.gcExternalLostTtlSecs, e)} />
        <span class="hook-desc">hours a lost session from outside fleet is kept before it is removed — it can never be resumed, this only rides out a restart (0 = next pass)</span>
      </div>
```

- [x] **Step 5: Shells never get pane state** — `store/reconcile.rs`, the three SET-clause constants:

```rust
        const NEW_STUCK: &str = "CASE WHEN kind = 'shell' THEN NULL \
                                 WHEN ?16 THEN excluded.stuck_kind \
                                 ELSE COALESCE(excluded.stuck_kind, stuck_kind) END";
        const NEW_PENDING: &str = "CASE WHEN kind = 'shell' THEN NULL \
                                   WHEN ?16 THEN excluded.pending_input \
                                   ELSE COALESCE(excluded.pending_input, pending_input) END";
        // A `shell` row (kind set by `new_shell_session`) has no Claude in
        // it: its bare `❯` is the shell's prompt, never an idle REPL (F8).
        const NEW_STATUS: &str = "CASE WHEN kind = 'shell' THEN NULL \
                                       WHEN ?20 > 0 AND last_hook_at IS NOT NULL \
                                            AND last_hook_at >= ?20 \
                                       THEN claude_status \
                                       ELSE COALESCE(excluded.claude_status, claude_status) END";
```

`service/health.rs:376`:

```rust
    for s in sessions.iter().filter(|s| s.kind != "external" && s.kind != "shell") {
```

with the doc (`:359-363`) extended: "`kind='shell'` rows (a plain shell in tmux) are left out the same way: they have no Claude status, and a pane heuristic that reads their prompt as `idle` would count them (F8)."

- [x] **Step 6: Run** — `cargo test -p fleet-core mark_host_sessions_lost` → PASS (all, including `…keeps_every_identity_field`); `cargo test -p fleet-core a_lost_external_row` → PASS; `cargo test -p fleet-core a_rebooted_external_row_is_reaped_inside_the_lost_ttl` → PASS (grace `None` keeps today's next-pass reap); `cargo test -p fleet-core phase_one_ghosting_clears` → PASS; `cargo test -p fleet-core a_shell_row_never_gets` → PASS; `cargo test -p fleet-core health::` → PASS; `cargo test -p fleet-core reconcile_tests` → PASS; `cargo test -p fleet-core every_spec_has_a_settings_dialog_row` → PASS; `npx vitest run src/lib/fleet_settings.test.ts`; `npx svelte-check`.

- [x] **Step 7: Full suite and commit**

```bash
git add crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/store/reconcile.rs crates/fleet-core/src/service/sessions/reconcile.rs crates/fleet-core/src/service/health.rs crates/fleet-core/src/service/settings.rs src/lib/fleet_settings.ts src/lib/SettingsDialog.svelte
git commit -m "fix(sessions): loss clears pane-derived state, external ghosts get a one-hour grace, shells carry no claude_status"
```

---

### Task 4: `StopFailure` is a failed turn

**Files:**
- Modify: `crates/fleet-core/src/store/sessions.rs` (`record_stop_failure_hook_for_row` `:1302-1311`)
- Modify: `crates/fleet-core/src/service/hooks.rs` (`apply_stop_failure_hook` `:1195-1234`; tests `:2615-2630`)
- Modify: `crates/fleet-core/src/store/reconcile.rs` (`NEW_STATUS`, from Task 3)
- Test: `hooks.rs` tests, `store/reconcile.rs` tests

**Interfaces:**
- Consumes: `HookPayload.{error, error_details}` (`crates/fleet-core/src/mcp/hooks.rs:42`), `END_COMPACTING`, `emit_session`.
- Produces: `claude_status = 'failed'` on a `StopFailure` (plus the Stop stamps: `turn_seq + 1`, `last_stop_at`, `last_turn_at`, `last_hook_at`, `idle_since`, `pending_input = NULL`); `pub(crate) fn stop_failure_class(error: &str) -> &'static str` (`rate_limit` | `auth` | `other`); the `stop_failure` event detail `<class> (<cli error>)[: <details>]` (unchanged `<error>[: <details>]` when the CLI's name already is the class); the tmux upsert keeps a hook-stamped `failed` until a hook moves the row or the pane shows a turn (`working`) or a dialog (`blocked`). The attention reason `stop_failed` lands in Task 6.

- [x] **Step 1: Failing hook tests** — in `hooks.rs` `mod tests`, change `stop_failure_ends_the_turn_and_records_the_error` (`:2623`) to `assert_eq!(row.claude_status.as_deref(), Some("failed"));` and add:

```rust
    /// F3: 20773's 429 was recorded as plain `idle`; the user re-prompted
    /// by hand eleven times. A failed turn reads as failed until Claude is
    /// asked again.
    #[test]
    fn a_failed_turn_stays_failed_until_the_next_prompt() {
        let store = make_store();
        let id = hooked(&store);
        let c = ctx(&Caller::master(), None);
        let mut p = make_payload("StopFailure", "uuid-1");
        p.error = Some("authentication_error".into());
        apply_hook(&store, &make_ssh(), &p, &c).unwrap();
        assert_eq!(status_of(&store, id).claude_status.as_deref(), Some("failed"));
        assert!(events(&store, id).contains(&(
            "stop_failure".to_string(),
            Some("auth (authentication_error)".to_string())
        )));
        apply_hook(&store, &make_ssh(), &make_payload("UserPromptSubmit", "uuid-1"), &c).unwrap();
        assert_eq!(status_of(&store, id).claude_status.as_deref(), Some("working"));
        apply_hook(&store, &make_ssh(), &make_payload("Stop", "uuid-1"), &c).unwrap();
        assert_eq!(status_of(&store, id).claude_status.as_deref(), Some("idle"));
    }

    #[test]
    fn stop_failure_class_sorts_the_cli_error_names() {
        assert_eq!(stop_failure_class("rate_limit"), "rate_limit");
        assert_eq!(stop_failure_class("429 Too Many Requests"), "rate_limit");
        assert_eq!(stop_failure_class("overloaded_error"), "rate_limit");
        assert_eq!(stop_failure_class("authentication_error"), "auth");
        assert_eq!(stop_failure_class("401 Unauthorized"), "auth");
        assert_eq!(stop_failure_class("server_error"), "other");
        assert_eq!(stop_failure_class(""), "other");
    }
```

- [x] **Step 2: Run** — `cargo test -p fleet-core hooks::tests::stop_failure` and `cargo test -p fleet-core a_failed_turn_stays_failed` → `stop_failure_class` compile error; after stubbing, `left: Some("idle"), right: Some("failed")`.

- [x] **Step 3: Implement** — `store/sessions.rs:1302-1311`:

```rust
    /// The StopFailure hook's write: the turn ended in an API error (F3).
    /// The Stop stamps are all made (`turn_seq`, `last_stop_at`,
    /// `last_turn_at`, `last_hook_at`, `idle_since`, `pending_input`) so
    /// waiters return and the GC clocks run, but the row reads `failed`
    /// — until the next UserPromptSubmit (`working`) or Stop (`idle`), and
    /// through reconcile's pane reads of the input box the failure left
    /// behind (`store/reconcile.rs`, `NEW_STATUS`). The handler records the
    /// `stop_failure` timeline event that says which error.
    pub fn record_stop_failure_hook_for_row(
        &self,
        row_id: i64,
    ) -> Result<Option<SessionRow>, crate::ipc_error::IpcError> {
        let now = now_unix();
        let changed = self.conn.execute(
            &format!(
                "UPDATE sessions SET claude_status = 'failed', turn_seq = turn_seq + 1, \
                     last_stop_at = ?2, last_turn_at = ?2, last_hook_at = ?2, \
                     idle_since = COALESCE(idle_since, ?2), pending_input = NULL{END_COMPACTING} \
                 WHERE id = ?1"
            ),
            rusqlite::params![row_id, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(self.emit_session(row_id)?)
    }
```

`hooks.rs`, above `apply_stop_failure_hook` (`:1195`):

```rust
/// The class of a `StopFailure`'s `error` for the timeline and the
/// attention row: `rate_limit` (429, overloaded), `auth` (401/403,
/// authentication) or `other`. The CLI's own names (`rate_limit`,
/// `authentication_error`, `server_error`, …) sort by substring so a
/// renamed variant still lands in the right class.
pub(crate) fn stop_failure_class(error: &str) -> &'static str {
    let e = error.to_ascii_lowercase();
    if e.contains("rate") || e.contains("429") || e.contains("overload") {
        "rate_limit"
    } else if e.contains("auth") || e.contains("401") || e.contains("403") {
        "auth"
    } else {
        "other"
    }
}
```

and the detail at `:1219-1224`:

```rust
            if let Some(row) = s.record_stop_failure_hook_for_row(row.id)? {
                let error = payload.error.as_deref().unwrap_or("unknown");
                let class = stop_failure_class(error);
                let head = if class == error {
                    error.to_string()
                } else {
                    format!("{class} ({error})")
                };
                let detail = match payload.error_details.as_deref() {
                    Some(d) if !d.trim().is_empty() => format!("{head}: {}", d.trim()),
                    _ => head,
                };
                best_effort_event_for(s, row.id, Some(session_id), "stop_failure", Some(&detail))?;
            }
```

- [x] **Step 4: Failing store test for the sticky failure** — `store/reconcile.rs` `mod tests`:

```rust
    /// A failed turn leaves the input box on screen, which the pane
    /// heuristic reads as `idle` every 20 s. The hook's `failed` must
    /// outlive that read (the in-flight guard only covers a pass already
    /// running when the hook landed), and give way to a real turn.
    #[test]
    fn a_hook_stamped_failure_survives_the_panes_idle_prompt_but_not_a_turn() {
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let r = reconcile_one(&mut s, "a", Some("working"), None, None);
        s.record_stop_failure_hook_for_row(r.id).unwrap();
        let pass = |s: &mut Store, status: &str| -> SessionRow {
            s.apply_host_reconcile(HostReconcile {
                alias: "local",
                reachable: true,
                claude_version: None,
                tmux_version: None,
                last_pinged_at: now_unix() + 5,
                // A pass that STARTED after the hook: the guard does not apply.
                probe_started_at: now_unix() + 5,
                sessions: &[ReconcileSession {
                    tmux_name: "a",
                    created_at: 1,
                    last_activity_at: 1,
                    claude_status: Some(status.to_string()),
                    intel_observed: true,
                    ..Default::default()
                }],
                keep: &["a".to_string()],
                lost_ttl_cutoff: None,
                skip_prune: false,
                reconciled_at: None,
            })
            .unwrap();
            s.get_session("a", "local").unwrap().unwrap()
        };
        assert_eq!(pass(&mut s, "idle").claude_status.as_deref(), Some("failed"));
        assert_eq!(pass(&mut s, "blocked").claude_status.as_deref(), Some("blocked"));
        s.record_stop_failure_hook_for_row(r.id).unwrap();
        assert_eq!(pass(&mut s, "working").claude_status.as_deref(), Some("working"));
    }
```

`cargo test -p fleet-core a_hook_stamped_failure_survives` → FAILS on the first assertion (`Some("idle")`).

- [x] **Step 5: The sticky arm** — `store/reconcile.rs` `NEW_STATUS` (as left by Task 3):

```rust
        // A StopFailure's `failed` is recognisable without a column: the
        // failure hook stamps `last_hook_at` and `last_stop_at` together, and
        // any later hook moves `last_hook_at` past `last_stop_at`. While that
        // holds, the pane's idle prompt (the input box the failure left) does
        // not overwrite it; a turn (`working`) or a dialog (`blocked`) does.
        const NEW_STATUS: &str = "CASE WHEN kind = 'shell' THEN NULL \
                                       WHEN ?20 > 0 AND last_hook_at IS NOT NULL \
                                            AND last_hook_at >= ?20 \
                                       THEN claude_status \
                                       WHEN claude_status = 'failed' AND last_hook_at IS NOT NULL \
                                            AND last_hook_at = last_stop_at \
                                            AND COALESCE(excluded.claude_status, '') NOT IN ('working','blocked') \
                                       THEN claude_status \
                                       ELSE COALESCE(excluded.claude_status, claude_status) END";
```

- [x] **Step 6: Run** — `cargo test -p fleet-core a_hook_stamped_failure_survives` → PASS; `cargo test -p fleet-core hooks::` → PASS (`stop_failure_ends_the_turn_and_records_the_error` with `failed`); `cargo test -p fleet-core store::reconcile` → PASS; `cargo test -p fleet-core reconcile_tests` → PASS.

- [x] **Step 7: Full suite and commit**

```bash
git add crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/store/reconcile.rs crates/fleet-core/src/service/hooks.rs
git commit -m "fix(hooks): a StopFailure marks the turn failed, classed rate_limit/auth/other, until the next prompt"
```

---

### Task 5: Stale `working` ages out (`stale_working_at`)

**Files:**
- Create: `crates/fleet-core/migrations/061_stale_working.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (guard after `:106`; `MIGRATIONS` after `:586`; tests after `:2960`)
- Modify: `crates/fleet-core/src/store/rows.rs` (`SessionRow` after `last_stop_at` `:208-209`; `SESSION_COLUMNS` tail — append after `prompt_submit_seq`; `map_session_row` `:460`)
- Modify: the seven `SessionRow { … }` literals (Global Constraints)
- Modify: `crates/fleet-core/src/store/sessions.rs` (new `age_out_stale_working`; the five hook recorders `:1196-1355` clear the stamp)
- Modify: `crates/fleet-core/src/store/reconcile.rs` (`ON CONFLICT` SET list, after `pending_input=` `:455`)
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs` (`write_reachable_host` `:706-737`; new `stale_working_veto` next to `status_candidate` `:1919`; new `pub fn age_out_stale_working(store)`)
- Modify: `crates/fleet-core/src/service/tick.rs` (`:98-106`)
- Modify: `crates/fleet-core/src/service/settings.rs`, `src/lib/fleet_settings.ts`, `src/lib/SettingsDialog.svelte`, `src/lib/sessions.ts` (`:81`)
- Modify: `src-tauri/src/backend/hub_contract.golden.json` (regenerated)
- Test: `store/schema.rs`, `store/sessions.rs`, `service/sessions/tests.rs`, `src-tauri` contract

**Interfaces:**
- Consumes: `sessions.{last_hook_at, last_turn_at, context_at, usage_updated_at, last_activity_at, created_at}`, `status_candidate`, `settings::get_secs`.
- Produces: column + `SessionRow.stale_working_at: Option<i64>` (`#[serde(default)]`, wire-additive); `Store::age_out_stale_working(now: i64, stale_secs: i64) -> rusqlite::Result<Vec<SessionRow>>`; `pub(super) fn stale_working_veto(stale: bool, candidate: Option<ClaudeStatus>, pane: Option<ClaudeStatus>) -> Option<ClaudeStatus>`; `pub fn age_out_stale_working(store: &Mutex<Store>) -> usize` on the tick; setting `reconcile.stale_working_secs` (`Kind::Secs`, default `"1800"`, `0` = off); timeline events `status_change idle` and `stale_working`.

- [x] **Step 1: Migration** — `crates/fleet-core/migrations/061_stale_working.sql`:

```sql
-- Live-instance analysis 2026-09-27, lifecycle F2: two `working` rows had
-- had no Stop, no transcript growth and no pane output for ~40 h, and
-- nothing could demote them. The tick now turns such a row `idle` after
-- `reconcile.stale_working_secs`, and this stamp says it did (attention
-- reason `stale_working`). Cleared by the next UserPromptSubmit / Stop /
-- StopFailure / SessionEnd / Notification hook, and by a pane that shows a
-- live turn; the cached `claude agents` status alone does not lift it.
ALTER TABLE sessions ADD COLUMN stale_working_at INTEGER;
```

`store/schema.rs` after `:106`:

```rust
/// `already_applied` guard of migration 061: `sessions` already has its
/// `stale_working_at` column, and `ALTER TABLE ... ADD COLUMN` would fail
/// again. See [`Migration`].
fn sessions_has_stale_working_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'stale_working_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

and after the version-60 entry (`:586`):

```rust
    // Lifecycle F2: `sessions.stale_working_at`, one ADD COLUMN, its own guard.
    Migration {
        version: 61,
        sql: include_str!("../../migrations/061_stale_working.sql"),
        already_applied: Some(sessions_has_stale_working_at),
    },
```

Test, after `migration_060_on_a_populated_v59_database_is_safe_to_rerun` (`:2960`):

```rust
    #[test]
    fn migration_061_adds_stale_working_at_and_is_safe_to_rerun() {
        let s = store_at_version(60);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
                 VALUES ('a', 'h', 1, 1, 'running');",
            )
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(sessions_has_stale_working_at(&s.conn).unwrap());
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE stale_working_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "an existing row starts unstamped");
        // Rolling the recorded version back re-runs 061 (the idiom of the
        // 024–026 tests, `schema.rs:1447`): the guard skips the ADD COLUMN.
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 61;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

`cargo test -p fleet-core migration_061` → PASS.

- [x] **Step 2: The row field** — `store/rows.rs` after `last_stop_at` (`:209`):

```rust
    /// When the tick demoted this row from a stale `working` to `idle`
    /// (migration 061, lifecycle F2); `None` otherwise. Cleared by the next
    /// hook or a pane that shows a live turn. `#[serde(default)]`: a hub
    /// older than the column sends none.
    #[serde(default)]
    pub stale_working_at: Option<i64>,
```

`SESSION_COLUMNS`: append `, stale_working_at` directly after `prompt_submit_seq` (the last column; `grep -n prompt_submit_seq crates/fleet-core/src/store/rows.rs` shows the one place in the column list). `map_session_row`: after `prompt_submit_seq: row.get(58)?,` add `stale_working_at: row.get(59)?,`. Add `stale_working_at: None,` to every literal listed in Global Constraints (`health.rs`, `playbooks.rs`, `gc.rs`, `attention.rs`, `sessions/tests.rs`, `store/reconcile.rs` ×2, `src-tauri/src/backend/tests_remote.rs`, `src-tauri/src/backend/tests_contract.rs`; in `tests_contract.rs::sample_session` use `stale_working_at: Some(1_790_500_000)` so the golden pins the key). `src/lib/sessions.ts` after `last_stop_at` (`:81`):

```ts
  /** When the tick demoted a stale `working` row to idle (attention `stale_working`); absent from an older hub. */
  stale_working_at?: number | null;
```

`cargo build -p fleet-core --tests` and `cargo build -p claude-fleet --tests` → clean.

- [x] **Step 3: Failing store test** — `store/sessions.rs` `mod tests`:

```rust
    /// F2: two `working` rows on trn had not moved for ~40 h.
    #[test]
    fn age_out_stale_working_demotes_a_quiet_working_row_and_leaves_the_rest() {
        let s = store();
        let quiet = s
            .upsert_session("quiet", "local", None, None, 1, 1_000, "running", None)
            .unwrap();
        let busy = s
            .upsert_session("busy", "local", None, None, 1, 9_500, "running", None)
            .unwrap();
        let sh = s
            .upsert_session("sh-term", "local", None, None, 1, 1_000, "running", None)
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET claude_status = 'working' WHERE id IN (?1, ?2, ?3)",
                rusqlite::params![quiet, busy, sh],
            )
            .unwrap();
        s.conn_ref()
            .execute("UPDATE sessions SET kind = 'shell' WHERE id = ?1", [sh])
            .unwrap();

        let demoted = s.age_out_stale_working(10_000, 1_800).unwrap();
        assert_eq!(demoted.iter().map(|r| r.id).collect::<Vec<_>>(), vec![quiet]);
        let q = s.get_session_by_id(quiet).unwrap().unwrap();
        assert_eq!(q.claude_status.as_deref(), Some("idle"));
        assert_eq!(q.stale_working_at, Some(10_000));
        assert_eq!(q.idle_since, Some(10_000));
        assert_eq!(
            s.get_session_by_id(busy).unwrap().unwrap().claude_status.as_deref(),
            Some("working"),
            "pane output 500 s ago is not stale"
        );
        assert_eq!(
            s.get_session_by_id(sh).unwrap().unwrap().stale_working_at,
            None,
            "a shell has no turns to miss"
        );
        let kinds: Vec<String> = s
            .list_session_events(quiet, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&"stale_working".to_string()));
        assert!(kinds.contains(&"status_change".to_string()));
        // Stamped rows are not judged again; `0` turns the rule off.
        assert!(s.age_out_stale_working(20_000, 1_800).unwrap().is_empty());
        assert!(s.age_out_stale_working(20_000, 0).unwrap().is_empty());
        // The next prompt clears the stamp.
        s.record_prompt_submit_hook_for_row(quiet).unwrap();
        assert_eq!(s.get_session_by_id(quiet).unwrap().unwrap().stale_working_at, None);
    }
```

`cargo test -p fleet-core age_out_stale_working_demotes` → compile error (no method).

- [x] **Step 4: Store implementation** — `store/sessions.rs`, after `record_stop_failure_hook_for_row`:

```rust
    /// The tick's stale-`working` rule (lifecycle F2): a live tmux row that
    /// says `working` but has had no hook, no turn, no transcript growth
    /// (`context_at`, `usage_updated_at`) and no pane output
    /// (`last_activity_at`) for `stale_secs` is demoted to `idle` and stamped
    /// `stale_working_at = now`, which is what the attention model reads.
    /// Pane-less and `shell` rows are never judged (no hooks or turns to
    /// miss); a stamped row is not judged twice; `stale_secs <= 0` is off.
    /// Returns the demoted rows; each gets `session_updated`, a
    /// `status_change idle` and a `stale_working` timeline entry.
    pub fn age_out_stale_working(
        &self,
        now: i64,
        stale_secs: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        if stale_secs <= 0 {
            return Ok(Vec::new());
        }
        let cutoff = now - stale_secs;
        let ids: Vec<i64> = self
            .conn
            .prepare(
                "UPDATE sessions SET claude_status = 'idle', idle_since = ?1, stale_working_at = ?1 \
                 WHERE status = 'running' AND claude_status = 'working' \
                   AND kind NOT IN ('bg','external','shell') AND stale_working_at IS NULL \
                   AND COALESCE(last_hook_at, 0) < ?2 AND COALESCE(last_turn_at, 0) < ?2 \
                   AND COALESCE(context_at, 0) < ?2 AND COALESCE(usage_updated_at, 0) < ?2 \
                   AND last_activity_at < ?2 AND created_at < ?2 \
                 RETURNING id",
            )?
            .query_map(rusqlite::params![now, cutoff], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(ids.len());
        let detail = format!("no hook, turn, transcript growth or pane output for {stale_secs}s");
        for id in ids {
            for (kind, d) in [("status_change", "idle"), ("stale_working", detail.as_str())] {
                if let Err(e) = self.insert_session_event(id, kind, Some(d)) {
                    tracing::warn!(session_id = id, kind, error = %e, "[reconcile] session_event insert failed");
                }
            }
            if let Some(row) = self.emit_session(id)? {
                out.push(row);
            }
        }
        Ok(out)
    }
```

Clear the stamp in every hook recorder: add `stale_working_at = NULL, ` to the SET list of `record_stop_hook_for_row` (`:1203`), `record_prompt_submit_hook_for_row_with` (`:1242`), `record_session_end_hook_for_row` (`:1290`), `record_stop_failure_hook_for_row` (Task 4's UPDATE) and `record_notification_hook_for_row` (`:1340`).

In the tmux upsert (`store/reconcile.rs`, the `ON CONFLICT … SET` list, after `pending_input={new_pending},`):

```sql
               -- A pane that shows a live turn lifts the stale-working
               -- demotion (F2); anything else keeps the stamp.
               stale_working_at=CASE WHEN ({new_status}) IS 'working' THEN NULL
                                     ELSE stale_working_at END,
```

`cargo test -p fleet-core age_out_stale_working_demotes` → PASS.

- [x] **Step 5: Failing veto test** — `service/sessions/tests.rs`, after `skipped_agents_pass_only_lets_the_pane_report_blocked` (`:64`):

```rust
/// F2: the cached `claude agents` status reports a session with live
/// subagents as `working` — the very signal that kept two rows `working`
/// for 40 h. Once the tick has demoted a row, only the pane's own spinner
/// may lift the demotion.
#[test]
fn a_stale_working_demotion_is_not_undone_by_the_cached_agents_status() {
    use crate::service::pane_intel::ClaudeStatus;
    let agents_working = Some(ClaudeStatus::Working);
    assert_eq!(stale_working_veto(false, agents_working, None), agents_working);
    assert_eq!(stale_working_veto(true, agents_working, None), None, "keeps the stored idle");
    assert_eq!(stale_working_veto(true, agents_working, Some(ClaudeStatus::Idle)), None);
    assert_eq!(
        stale_working_veto(true, agents_working, Some(ClaudeStatus::Working)),
        Some(ClaudeStatus::Working),
        "the pane's spinner is real"
    );
    assert_eq!(
        stale_working_veto(true, agents_working, Some(ClaudeStatus::Blocked)),
        Some(ClaudeStatus::Blocked)
    );
    assert_eq!(
        stale_working_veto(true, Some(ClaudeStatus::Idle), None),
        Some(ClaudeStatus::Idle)
    );
}
```

`cargo test -p fleet-core a_stale_working_demotion_is_not_undone` → compile error.

- [x] **Step 6: Service implementation** — `service/sessions/reconcile.rs`, after `status_candidate` (`:1933`):

```rust
/// A row the tick demoted for staleness (`stale_working_at` set) is not
/// handed back to `working` by the cached `claude agents` status alone.
/// Only the pane's own spinner (`Working`) lifts the demotion; a `Blocked`
/// pane still surfaces; anything else yields `None` so the upsert keeps
/// the stored `idle`.
pub(super) fn stale_working_veto(
    stale: bool,
    candidate: Option<crate::service::pane_intel::ClaudeStatus>,
    pane: Option<crate::service::pane_intel::ClaudeStatus>,
) -> Option<crate::service::pane_intel::ClaudeStatus> {
    use crate::service::pane_intel::ClaudeStatus;
    if !stale {
        return candidate;
    }
    match (candidate, pane) {
        (Some(ClaudeStatus::Working), Some(ClaudeStatus::Working)) => Some(ClaudeStatus::Working),
        (Some(ClaudeStatus::Working), Some(ClaudeStatus::Blocked)) => Some(ClaudeStatus::Blocked),
        (Some(ClaudeStatus::Working), _) => None,
        (other, _) => other,
    }
}

/// The tick's stale-`working` sweep: reads `reconcile.stale_working_secs`
/// and demotes every qualifying row (`Store::age_out_stale_working`).
/// Best-effort; returns how many rows were demoted.
pub fn age_out_stale_working(store: &Mutex<Store>) -> usize {
    let Ok(s) = store.lock() else {
        return 0;
    };
    let secs = crate::service::settings::get_secs(
        &s,
        crate::service::settings::RECONCILE_STALE_WORKING_SECS,
    ) as i64;
    match s.age_out_stale_working(now_unix(), secs) {
        Ok(rows) => {
            for r in &rows {
                tracing::info!(
                    host = %r.host_alias,
                    session = %r.tmux_name,
                    "[reconcile] stale working demoted to idle"
                );
            }
            rows.len()
        }
        Err(e) => {
            tracing::warn!(error = %e, "[reconcile] stale-working sweep failed");
            0
        }
    }
}
```

In `write_reachable_host`, move the prior read above the status candidate and apply the veto. Replace `:706-737` (from `let claude_status =` through the `match s.get_session(...)` block) with:

```rust
        // Transition-detection: remember the PRIOR stored values (the
        // upsert below overwrites them). A first sighting skips the
        // status/stuck detection but still opens its conversation; a
        // read failure skips the session entirely. Read here, before the
        // candidate, because the stale-working veto needs the stored stamp.
        let prior_read = s.get_session(&sess.name, &host.alias);
        if let Err(e) = &prior_read {
            tracing::warn!(
                host = %host.alias,
                session = %sess.name,
                error = %e,
                "[reconcile] prior row read failed"
            );
            s.ensure_in_tx()?;
        }
        let prior_row = prior_read.as_ref().ok().cloned().flatten();
        let stale = prior_row.as_ref().is_some_and(|p| p.stale_working_at.is_some());
        // Prefer the authoritative `claude agents` status; fall back to
        // the pane heuristic per `status_candidate` — full weight when
        // this pass actually asked, `Blocked`-only otherwise (a
        // cadence-skipped or unanswerable pass must not let a weak
        // pane guess overwrite the stored status every time). A row the
        // tick demoted for staleness keeps its `idle` unless the pane
        // itself shows a turn (`stale_working_veto`, F2).
        let claude_status = stale_working_veto(
            stale,
            status_candidate(probe.agent_rows.is_some(), agent_status_typed, pane_status),
            pane_status,
        )
        .map(|s| s.as_str().to_string());
        let stuck_kind = pane.and_then(|p| p.stuck.map(|k| k.as_str().to_string()));
        if prior_read.is_ok() {
            priors.push((
                sess.name.clone(),
                prior_row.map(|p| Prior {
                    claude_status: p.claude_status,
                    stuck_kind: p.stuck_kind,
                    claude_session_id: p.claude_session_id,
                }),
            ));
        }
```

`service/tick.rs`, after the `reconcile_now` match (`:97`) and before the playbooks:

```rust
                // Lifecycle F2: a `working` row nothing has moved for
                // `reconcile.stale_working_secs` becomes `idle` (+ the
                // `stale_working` attention reason), before the playbooks
                // read the fresh statuses.
                let stale = service::sessions::age_out_stale_working(store);
                if stale > 0 {
                    tracing::info!("reconcile tick: {stale} stale working row(s) demoted");
                }
```

Setting — `service/settings.rs` after `RECONCILE_INTERVAL_SECS` (`:61`):

```rust
/// A `working` row with no hook, no turn, no transcript growth and no pane
/// output for this long is demoted to `idle` by the tick (lifecycle F2).
/// `0` turns the rule off.
pub const RECONCILE_STALE_WORKING_SECS: &str = "reconcile.stale_working_secs";
```

spec after the `RECONCILE_INTERVAL_SECS` spec (`:232`):

```rust
    Spec {
        key: RECONCILE_STALE_WORKING_SECS,
        default: "1800",
        kind: Kind::Secs,
    },
```

`src/lib/fleet_settings.ts`: `  reconcileStaleWorkingSecs: 'reconcile.stale_working_secs',` after `reconcileIntervalSecs` and `  'reconcile.stale_working_secs': '1800',` after `'reconcile.interval_secs': '20',`. `src/lib/SettingsDialog.svelte`, after the `tick` row (`:941`):

```svelte
      <div class="mcp-field">
        <span class="lbl">stale working</span>
        <input class="port" type="number" min="0"
          value={settingSecs($fleetSettings, SETTING_KEYS.reconcileStaleWorkingSecs)}
          disabled={automationBusy}
          data-testid="reconcile-stale-working-secs"
          onchange={(e) => onSecsChange(SETTING_KEYS.reconcileStaleWorkingSecs, e)} />
        <span class="hook-desc">seconds a "working" session may go without a hook, a turn, transcript growth or pane output before it reads idle (0 = never)</span>
      </div>
```

- [x] **Step 7: Run** — `cargo test -p fleet-core a_stale_working_demotion_is_not_undone` → PASS; `cargo test -p fleet-core skipped_agents_pass_only_lets_the_pane_report_blocked` → PASS; `cargo test -p fleet-core reconcile_tests` → PASS; `cargo test -p fleet-core every_spec_has_a_settings_dialog_row` → PASS; `npx vitest run src/lib/fleet_settings.test.ts`; `npx svelte-check`.

- [x] **Step 8: Hub contract golden** — `cargo test -p claude-fleet --lib contract` → FAILS (`SessionRow: gained ["stale_working_at"]`). Then `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (reports FAILED once, writes the golden — `RegenVerdict::Write`, nothing lost), then `cargo test -p claude-fleet --lib contract` → PASS. `git diff src-tauri/src/backend/hub_contract.golden.json` shows exactly one added key under `SessionRow`; `CONTRACT_REVISION` unchanged.

- [x] **Step 9: Full suite and commit**

```bash
git add crates/fleet-core/migrations/061_stale_working.sql crates/fleet-core/src/store/schema.rs crates/fleet-core/src/store/rows.rs crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/store/reconcile.rs crates/fleet-core/src/service/sessions/reconcile.rs crates/fleet-core/src/service/sessions/tests.rs crates/fleet-core/src/service/tick.rs crates/fleet-core/src/service/settings.rs crates/fleet-core/src/service/health.rs crates/fleet-core/src/service/playbooks.rs crates/fleet-core/src/service/gc.rs crates/fleet-core/src/service/attention.rs src-tauri/src/backend/tests_remote.rs src-tauri/src/backend/tests_contract.rs src-tauri/src/backend/hub_contract.golden.json src/lib/sessions.ts src/lib/fleet_settings.ts src/lib/SettingsDialog.svelte
git commit -m "feat(reconcile): a stale working row ages out to idle (stale_working_at, reconcile.stale_working_secs)"
```

---

### Task 6: Attention model — `stop_failed`, `context_full`, `stale_working`, `ci_failing`; one context threshold

**Files:**
- Modify: `crates/fleet-core/src/service/attention.rs` (`Reason` `:38-64`, `needs_attention` `:85-123`, tests)
- Modify: `crates/fleet-core/src/service/health.rs` (`Health` `:26-75`, `CONTEXT_RED_THRESHOLD` `:354-356`, `summarize` `:364-394`, `health_from_store` `:396-428`, `health_check` `:452-470`, tests `:563-720`, `:786-830`, `:890-900`)
- Modify: `crates/fleet-core/src/mcp/tools/session_ops.rs` (`:55-59`, `:100-111`, `:270-289`), `crates/fleet-core/src/mcp/tools/support.rs` (`:908-935`)
- Modify: `crates/fleet-core/src/service/settings.rs`, `src/lib/fleet_settings.ts`, `src/lib/SettingsDialog.svelte`
- Modify: `src/lib/attention.ts` (`TRIAGE_BUCKETS` `:123-132`, `NEEDS_YOU_*` `:139-149`, `classify` `:194-204`, `bucketSince` `:207-220`), `src/lib/attention.test.ts` (`:117-130`, `:224-227`, `:251-268`)
- Modify: `src-tauri/src/backend/tests_contract.rs` (`sample_health` `:246-282`), `src-tauri/src/backend/hub_contract.golden.json` (regenerated)
- Test: `attention.rs`, `health.rs`, `mcp/tools/tests.rs`, `attention.test.ts`, contract

**Interfaces:**
- Consumes: `SessionRow.{claude_status, stuck_kind, context_pct, stale_working_at, ci_status, kind, idle_since, last_stop_at, context.context_at}`, `store::has_no_pane`.
- Produces: `Reason::{Waiting, Stuck, StopFailed, Failed, ContextFull, StaleWorking, CiFailing, Lifecycle}` (wire `stop_failed`, `context_full`, `stale_working`, `ci_failing`); `pub const DEFAULT_CONTEXT_RED_PCT: f64 = 85.0`; `pub fn needs_attention_with(row, context_red_pct: f64) -> Option<Attention>` (`needs_attention(row)` keeps its signature, using the default); `pub fn health::context_red_pct(s: &Store) -> f64`; `summarize(sessions, hosts, context_red_pct: f64)`; `Health.context_red_pct: u32` (`#[serde(default)]`); `SessionWithController::with_threshold(is_controller, row, context_red_pct)`; setting `health.context_red_pct` (`Kind::Int { min: 1, max: 100 }`, default `"85"`); desktop `TRIAGE_BUCKETS` = `waiting, stuck, stop_failed, failed, context_full, stale_working, ci_failing, done_unread, lifecycle, idle_long, working, idle`.

- [x] **Step 1: Failing Rust tests** — `attention.rs` `mod tests`: extend `the_wire_spellings_match_the_desktop_buckets` (`:263-277`) to iterate all eight variants (`Reason::Waiting, Reason::Stuck, Reason::StopFailed, Reason::Failed, Reason::ContextFull, Reason::StaleWorking, Reason::CiFailing, Reason::Lifecycle`) and add:

```rust
    /// F7: the model flagged three ghosts a person can do nothing about and
    /// missed five context-red rows, two stale `working` rows, a 429 and
    /// four idle sessions with failing CI.
    #[test]
    fn a_failed_turn_a_full_context_a_stale_row_and_failing_ci_each_qualify() {
        let mut r = row();
        r.claude_status = Some("failed".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::StopFailed);
        let mut r = row();
        r.kind = "bg".into();
        r.claude_status = Some("failed".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Failed, "a bg agent's exit is `claude agents`' verdict");

        let mut r = row();
        r.context_pct = Some(85.0);
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::ContextFull);
        assert_eq!(needs_attention_with(&r, 90.0), None, "the threshold is the hub's setting");

        let mut r = row();
        r.claude_status = Some("idle".into());
        r.stale_working_at = Some(50);
        assert_eq!(
            needs_attention(&r).unwrap(),
            Attention { reason: Reason::StaleWorking, since: 50 }
        );

        let mut r = row();
        r.claude_status = Some("idle".into());
        r.ci_status = Some("failing".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::CiFailing);
        let mut r = row();
        r.ci_status = Some("failing".into());
        assert_eq!(needs_attention(&r), None, "a working session may be fixing its CI");

        let mut r = row();
        r.kind = "shell".into();
        r.context_pct = Some(99.0);
        assert_eq!(needs_attention(&r), None, "a shell has no Claude in it");
    }

    /// Every reason a person can act on outranks a broken lifecycle.
    #[test]
    fn every_actionable_reason_outranks_lifecycle() {
        for set in [
            |r: &mut SessionRow| r.context_pct = Some(99.0),
            |r: &mut SessionRow| {
                r.claude_status = Some("idle".into());
                r.stale_working_at = Some(1);
            },
            |r: &mut SessionRow| {
                r.claude_status = Some("idle".into());
                r.ci_status = Some("failing".into());
            },
            |r: &mut SessionRow| r.claude_status = Some("failed".into()),
        ] {
            let mut r = row();
            set(&mut r);
            r.safe_kill_state = Some("failed".into());
            assert_ne!(needs_attention(&r).unwrap().reason, Reason::Lifecycle);
        }
    }
```

`health.rs` tests: change every `summarize(&sessions, &hosts)` / `summarize(&sessions, &[])` / `summarize(&[], &[])` (`:582, :609, :667, :680, :688, :~715`) to pass `85.0` as a third argument, and in `summarize_threshold_is_inclusive_at_85` add:

```rust
        assert_eq!(summarize(&sessions, &[], 90.0).context_red, 0, "one threshold, the setting's");
```

Add to `an_older_healths_json_without_peer_links_down_still_parses_as_zero` (`:897`): `assert_eq!(h.context_red_pct, 0, "an older hub sends none; the desktop keeps its default");` and a new test:

```rust
    #[test]
    fn health_from_store_exports_the_context_threshold_it_counts_with() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(health_from_store(&store).context_red_pct, 85);
        store
            .set_setting(crate::service::settings::HEALTH_CONTEXT_RED_PCT, "95")
            .unwrap();
        assert_eq!(health_from_store(&store).context_red_pct, 95);
    }
```

(`Store::set_setting(key, value)` is `crates/fleet-core/src/store/mod.rs:497`.)

- [x] **Step 2: Run** — `cargo test -p fleet-core attention::` → compile errors (`StopFailed`, `needs_attention_with`); `cargo test -p fleet-core health::` → compile errors (arity, `context_red_pct`).

- [x] **Step 3: Implement `attention.rs`** — replace `Reason` (`:38-64`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Blocked on a dialog: a permission prompt, a question, an elicitation.
    /// The one state where an answer is all that is wanted.
    Waiting,
    /// Wedged in a way the REPL will not leave on its own — an auth menu, a
    /// reconnect, an OOM. `stuck_kind` says which.
    Stuck,
    /// The last turn ended in an API error (a `StopFailure`: rate limit,
    /// auth, …); the `stop_failure` timeline entry says which. Re-prompt.
    StopFailed,
    /// Claude reported a failed turn (`claude agents`, a pane-less agent).
    Failed,
    /// The context window is at or past `health.context_red_pct`: compact
    /// or hand over before the next turn does it for you.
    ContextFull,
    /// The tick demoted a `working` row nothing had moved for
    /// `reconcile.stale_working_secs`: look at what it was doing.
    StaleWorking,
    /// Idle with a PR whose checks are failing.
    CiFailing,
    /// The session's lifecycle is broken: a safe kill that failed or is still
    /// pending, a ghost row, or a row the fleet has lost track of.
    Lifecycle,
}

impl Reason {
    /// The wire spelling, matching the desktop's bucket names.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Waiting => "waiting",
            Reason::Stuck => "stuck",
            Reason::StopFailed => "stop_failed",
            Reason::Failed => "failed",
            Reason::ContextFull => "context_full",
            Reason::StaleWorking => "stale_working",
            Reason::CiFailing => "ci_failing",
            Reason::Lifecycle => "lifecycle",
        }
    }
}

/// The one context threshold, when no store is at hand to read
/// `health.context_red_pct`: `fleet_health.context_red`, `context_full`
/// here and the desktop's chip all count from the same number.
pub const DEFAULT_CONTEXT_RED_PCT: f64 = 85.0;
```

and `needs_attention` (`:85-123`):

```rust
/// [`needs_attention_with`] at [`DEFAULT_CONTEXT_RED_PCT`]. Callers with a
/// store read the setting (`service::health::context_red_pct`) instead.
pub fn needs_attention(row: &SessionRow) -> Option<Attention> {
    needs_attention_with(row, DEFAULT_CONTEXT_RED_PCT)
}

/// Whether this row needs a person, and why.
///
/// The order of the checks *is* the precedence: a session that is both
/// blocked and ghosted is reported as blocked, because that is the one a
/// person can do something about right now — and every reason a person can
/// act on comes before `Lifecycle`, which nobody can act on from a phone.
///
/// An `external` session — a Claude running outside fleet entirely — never
/// qualifies, whatever its fields say: it is read-only here, so reporting it
/// as needing a person offers an action that does not exist. Nor does a
/// `shell`: there is no Claude in it.
pub fn needs_attention_with(row: &SessionRow, context_red_pct: f64) -> Option<Attention> {
    if row.kind == "external" || row.kind == "shell" {
        return None;
    }
    let failed = row.claude_status.as_deref() == Some("failed");
    let idle = matches!(
        row.claude_status.as_deref(),
        Some("idle") | Some("completed") | Some("stopped")
    );
    let reason = if row.claude_status.as_deref() == Some("blocked") {
        Reason::Waiting
    } else if row.stuck_kind.is_some() {
        Reason::Stuck
    } else if failed && !crate::store::has_no_pane(&row.kind) {
        Reason::StopFailed
    } else if failed {
        Reason::Failed
    } else if row.context_pct.is_some_and(|p| p >= context_red_pct) {
        Reason::ContextFull
    } else if row.stale_working_at.is_some() {
        Reason::StaleWorking
    } else if idle && row.ci_status.as_deref() == Some("failing") {
        Reason::CiFailing
    } else if is_lifecycle_broken(row) {
        Reason::Lifecycle
    } else {
        return None;
    };
    Some(Attention {
        reason,
        since: since_for(row, reason),
    })
}

fn since_for(row: &SessionRow, reason: Reason) -> i64 {
    match reason {
        Reason::Stuck => row.stuck_since.unwrap_or(row.last_activity_at),
        Reason::StopFailed => row.last_stop_at.unwrap_or(row.last_activity_at),
        Reason::ContextFull => row.context.context_at.unwrap_or(row.last_activity_at),
        Reason::StaleWorking => row.stale_working_at.unwrap_or(row.last_activity_at),
        Reason::CiFailing => row.idle_since.unwrap_or(row.last_activity_at),
        Reason::Lifecycle => row
            .lost_at
            .or(row.safe_kill_requested_at)
            .unwrap_or(row.last_activity_at),
        _ => row.last_activity_at,
    }
}
```

- [x] **Step 4: Implement `health.rs`** — delete `CONTEXT_RED_THRESHOLD` (`:354-356`); add after `Health` (`:75`):

```rust
/// The context threshold in force (`health.context_red_pct`; the
/// registry default is [`crate::service::attention::DEFAULT_CONTEXT_RED_PCT`]).
/// `context_red` here, `needs_attention`'s `context_full` and — exported on
/// [`Health::context_red_pct`] — the desktop's chip all read this one number.
pub fn context_red_pct(s: &Store) -> f64 {
    crate::service::settings::get_string(s, crate::service::settings::HEALTH_CONTEXT_RED_PCT)
        .parse::<f64>()
        .unwrap_or(crate::service::attention::DEFAULT_CONTEXT_RED_PCT)
}
```

In `Health`, after `context_red` (`:43`):

```rust
    /// The percent `context_red` counts from (`health.context_red_pct`), so a
    /// client draws its context chip at the hub's line, not its own. Per-field
    /// default: an older hub omits it (reads `0`; a client then keeps its own).
    #[serde(default)]
    pub context_red_pct: u32,
```

`summarize` becomes `pub fn summarize(sessions: &[SessionRow], hosts: &[HostRow], context_red_pct: f64) -> FleetSummary` with `if s.context_pct.is_some_and(|p| p >= context_red_pct)`; `health_from_store` reads `let red = context_red_pct(s);`, calls `summarize(&sessions, &hosts, red)` and sets `context_red_pct: red as u32,`; `health_check`'s poisoned arm sets `context_red_pct: crate::service::attention::DEFAULT_CONTEXT_RED_PCT as u32,`; the `whole` literal in `a_partial_health_is_rejected_rather_than_zeroed` (`:807-826`) gets `context_red_pct: 85,`. `src-tauri/src/backend/tests_contract.rs::sample_health` gets `context_red_pct: 85,` after `context_red: 1,`.

Setting — `service/settings.rs` (a new group after `REPORTS_MAX_AGE_SECS`, `:163`):

```rust
/// Percent of the context window at or past which a session counts as
/// `context_red` in `fleet_health`, reads `context_full` in
/// `needs_attention`, and draws red on the desktop. One number for all
/// three (ux F-09: the hub said 85 while the desktop said 70/90).
pub const HEALTH_CONTEXT_RED_PCT: &str = "health.context_red_pct";
```

spec after the `REPORTS_MAX_AGE_SECS` spec (`:383`):

```rust
    Spec {
        key: HEALTH_CONTEXT_RED_PCT,
        default: "85",
        kind: Kind::Int { min: 1, max: 100 },
    },
```

`src/lib/fleet_settings.ts`: `  healthContextRedPct: 'health.context_red_pct',` after `reportsMaxAgeSecs` and `  'health.context_red_pct': '85',` after `'reports.max_age_secs': '604800',`. `src/lib/SettingsDialog.svelte`, in the Limits section after the `restore delay` row (`:988`):

```svelte
      <div class="mcp-field">
        <label class="lbl" for="limit-context-red-pct">context red</label>
        <input class="port" id="limit-context-red-pct" type="number" min="1" max="100" step="1"
          value={settingInt($fleetSettings, SETTING_KEYS.healthContextRedPct)}
          disabled={limitsBusy}
          aria-describedby="limit-context-red-pct-desc"
          data-testid="health-context-red-pct"
          onchange={(e) => onLimitIntChange(SETTING_KEYS.healthContextRedPct, 'Context red threshold (%)', e)} />
        <span class="hook-desc" id="limit-context-red-pct-desc">percent of the context window at which a session needs you (the chip turns red here, amber 15 points below)</span>
      </div>
```

- [x] **Step 5: MCP callers read the setting** — `support.rs:908-935`:

```rust
impl SessionWithController {
    pub(super) fn new(is_controller: bool, row: crate::store::SessionRow) -> Self {
        Self::with_threshold(
            is_controller,
            row,
            crate::service::attention::DEFAULT_CONTEXT_RED_PCT,
        )
    }

    /// [`Self::new`] at the store's `health.context_red_pct`
    /// (`service::health::context_red_pct`), which every caller holding the
    /// store should pass so `context_full` and `fleet_health.context_red`
    /// agree.
    pub(super) fn with_threshold(
        is_controller: bool,
        row: crate::store::SessionRow,
        context_red_pct: f64,
    ) -> Self {
        Self {
            is_controller,
            needs_attention: crate::service::attention::needs_attention_with(&row, context_red_pct),
            row,
        }
    }
}
```

`session_ops.rs:55-59`: the lock block returns the threshold too — `(controller, caller.org_scope(&s).map_err(to_mcp_err)?, crate::service::health::context_red_pct(&s))` bound as `let (controller, scope, context_red_pct) = { … };`; `:101` becomes `if crate::service::attention::needs_attention_with(row, context_red_pct).is_some() != want`; `:111` becomes `SessionWithController::with_threshold(is_controller, row, context_red_pct)`. In the single-session path (`:270-289`), read `let context_red_pct = crate::service::health::context_red_pct(&s);` inside the same lock block that computes `is_controller` and return it alongside, then `SessionWithController::with_threshold(is_controller, row, context_red_pct)` at `:288`.

- [x] **Step 6: Run the Rust side** — `cargo test -p fleet-core attention::` → PASS; `cargo test -p fleet-core health::` → PASS; `cargo test -p fleet-core mcp::tools::tests` → PASS (`:135` still expects `{"reason":"waiting","since":1}`); `cargo test -p fleet-core every_spec_has_a_settings_dialog_row` → PASS. `the_wire_spellings_match_the_desktop_buckets` FAILS until Step 7 (`src/lib/attention.ts does not name the bucket stop_failed`).

- [x] **Step 7: Desktop buckets** — `src/lib/attention.ts:123-149`:

```ts
export const TRIAGE_BUCKETS = [
  'waiting',
  'stuck',
  'stop_failed',
  'failed',
  'context_full',
  'stale_working',
  'ci_failing',
  'done_unread',
  'lifecycle',
  'idle_long',
  'working',
  'idle',
] as const;

export type TriageBucket = (typeof TRIAGE_BUCKETS)[number];

/** Buckets the "Needs you" FILTER shows: everything above `working`. */
export const NEEDS_YOU_BUCKETS: readonly TriageBucket[] = TRIAGE_BUCKETS.slice(0, 10);

/** Buckets the "Needs you" COUNTER reports — one narrower than the filter,
 *  excluding `idle_long` (see the note below, which still stands). */
export const NEEDS_YOU_COUNTED_BUCKETS: readonly TriageBucket[] = TRIAGE_BUCKETS.slice(0, 9);
```

(keep the two explanatory comment blocks that precede them). `classify` (`:194-204`):

```ts
export function classify(s: SessionRow, opts: AttentionOptions): TriageBucket {
  if (s.kind === 'external') return s.claude_status === 'working' ? 'working' : 'idle';
  if (s.kind === 'shell') return 'idle';
  if (isWaiting(s)) return 'waiting';
  if (s.stuck_kind) return 'stuck';
  if (s.claude_status === 'failed') return s.kind === 'bg' ? 'failed' : 'stop_failed';
  if (contextLevel(s.context_pct) === 'crit') return 'context_full';
  if ((s.stale_working_at ?? null) !== null) return 'stale_working';
  if (s.ci_status === 'failing' && isIdleStatus(s.claude_status)) return 'ci_failing';
  if (isDoneUnread(s)) return 'done_unread';
  if (isLifecycleBroken(s)) return 'lifecycle';
  if (isIdleLong(s, opts)) return 'idle_long';
  if (s.claude_status === 'working') return 'working';
  return 'idle';
}

function isIdleStatus(status: ClaudeStatus | null): boolean {
  return status === 'idle' || status === 'completed' || status === 'stopped';
}
```

`bucketSince` (`:207-220`): add cases `'stop_failed'` → `s.last_stop_at ?? s.last_activity_at`, `'stale_working'` → `s.stale_working_at ?? s.last_activity_at`, `'ci_failing'` → `s.idle_since ?? s.last_activity_at` (`context_full` falls to the `default`).

`src/lib/attention.test.ts`: `:252` → `slice(0, 10)`, `:267` → `slice(0, 9)`; in the severity order test (`:120-127`) insert after the `stuck_kind` row: `row({ claude_status: 'failed' })` is already there — change the list to

```ts
    const order = [
      row({ claude_status: 'blocked' }),
      row({ stuck_kind: 'press_enter' }),
      row({ claude_status: 'failed' }),
      row({ kind: 'bg', claude_status: 'failed' }),
      row({ context_pct: 90 }),
      row({ claude_status: 'idle', stale_working_at: 5 }),
      row({ claude_status: 'idle', ci_status: 'failing' }),
      row({ status: 'ghost' }),
      row({ claude_status: 'working' }),
      row({ claude_status: 'idle' }),
    ].map(severity);
```

and add next to the `lifecycle` classify assertions (`:224-227`):

```ts
    expect(classify(row({ claude_status: 'failed' }), opts)).toBe('stop_failed');
    expect(classify(row({ kind: 'bg', claude_status: 'failed' }), opts)).toBe('failed');
    expect(classify(row({ context_pct: 85 }), opts)).toBe('context_full');
    expect(classify(row({ context_pct: 84.9 }), opts)).toBe('idle');
    expect(classify(row({ claude_status: 'idle', stale_working_at: 5 }), opts)).toBe('stale_working');
    expect(classify(row({ claude_status: 'idle', ci_status: 'failing' }), opts)).toBe('ci_failing');
    expect(classify(row({ claude_status: 'working', ci_status: 'failing' }), opts)).toBe('working');
    expect(classify(row({ kind: 'shell', claude_status: 'idle', context_pct: 99 }), opts)).toBe('idle');
```

Note `contextLevel` still uses `CONTEXT_CRIT_PCT = 90` until Task 8; the `context_pct: 85` assertions above pass only after Task 8 — write them as `context_pct: 90` / `89.9` here and change them to `85` / `84.9` in Task 8.

- [x] **Step 8: Run** — `npx vitest run src/lib/attention.test.ts` → PASS; `npx svelte-check` → 0 errors; `cargo test -p fleet-core the_wire_spellings_match_the_desktop_buckets` → PASS.

- [x] **Step 9: Hub contract golden** — `cargo test -p claude-fleet --lib contract` → FAILS (`Health: gained ["context_red_pct"]`); `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`; re-run → PASS; the golden diff is one added key under `Health`.

- [x] **Step 10: Full suite and commit**

```bash
git add crates/fleet-core/src/service/attention.rs crates/fleet-core/src/service/health.rs crates/fleet-core/src/service/settings.rs crates/fleet-core/src/mcp/tools/session_ops.rs crates/fleet-core/src/mcp/tools/support.rs src-tauri/src/backend/tests_contract.rs src-tauri/src/backend/hub_contract.golden.json src/lib/attention.ts src/lib/attention.test.ts src/lib/fleet_settings.ts src/lib/SettingsDialog.svelte
git commit -m "feat(attention): stop_failed, context_full, stale_working and ci_failing reasons; one context threshold (health.context_red_pct)"
```

---

### Task 7: Timeline hygiene — one event per transition, hook transitions recorded

**Files:**
- Modify: `crates/fleet-core/src/store/timeline.rs` (`write_session_event` `:105-135`; doc `:24-50`; tests after `:882`)
- Modify: `crates/fleet-core/src/store/sessions.rs` (the five hook recorders)
- Test: `timeline.rs`, `hooks.rs` tests

**Interfaces:**
- Consumes: `session_events(session_id, at, kind, detail)`, `in_savepoint`.
- Produces: `insert_session_event*` skips a `status_change` whose detail equals the session's newest `status_change`, and a `stuck` whose detail equals a `stuck` written within `STUCK_EVENT_WINDOW_SECS = 3600`; `Store::record_{stop, prompt_submit, session_end, stop_failure, notification}_hook_for_row` write `status_change <new status>`.

- [x] **Step 1: Failing tests** — `timeline.rs` `mod tests`:

```rust
    /// F10: 2674 of 2948 events were `status_change`, hundreds of them
    /// `busy → busy` — reconcile and the hooks both write status, and
    /// `claude agents` re-reports the same value every minute.
    #[test]
    fn a_status_change_that_repeats_the_newest_one_is_not_written() {
        let s = Store::open_in_memory().expect("open");
        s.insert_session_event(7, "status_change", Some("working")).unwrap();
        s.insert_session_event(7, "status_change", Some("working")).unwrap();
        s.insert_session_event(7, "status_change", Some("idle")).unwrap();
        s.insert_session_event(7, "status_change", Some("working")).unwrap();
        let details: Vec<Option<String>> = s
            .list_session_events(7, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.detail)
            .collect();
        assert_eq!(
            details,
            vec![Some("working".into()), Some("idle".into()), Some("working".into())],
            "newest first; the repeat was dropped, the return was kept"
        );
        // Other kinds are never deduplicated.
        s.insert_session_event(7, "prompt_sent", Some("x")).unwrap();
        s.insert_session_event(7, "prompt_sent", Some("x")).unwrap();
        assert_eq!(s.list_session_events(7, 10).unwrap().len(), 5);
    }

    /// F1: `stuck oom` was written on every detection, 8 rows in 83 min.
    #[test]
    fn a_stuck_event_repeats_only_after_its_window() {
        let s = Store::open_in_memory().expect("open");
        s.insert_session_event(7, "stuck", Some("oom")).unwrap();
        s.insert_session_event(7, "stuck", Some("oom")).unwrap();
        assert_eq!(s.list_session_events(7, 10).unwrap().len(), 1);
        // Age it past the window: the next detection is a new episode.
        s.conn
            .execute("UPDATE session_events SET at = at - 3601 WHERE session_id = 7", [])
            .unwrap();
        s.insert_session_event(7, "stuck", Some("oom")).unwrap();
        assert_eq!(s.list_session_events(7, 10).unwrap().len(), 2);
        // A different kind is always news.
        s.insert_session_event(7, "stuck", Some("press_enter")).unwrap();
        assert_eq!(s.list_session_events(7, 10).unwrap().len(), 3);
    }
```

`hooks.rs` `mod tests`:

```rust
    /// F10 (the two-writer gap): the hooks set status without recording
    /// it; reconcile recorded only what IT changed. Now each transition is
    /// on the timeline exactly once, whoever wrote it.
    #[test]
    fn hook_transitions_land_on_the_timeline_once() {
        let store = make_store();
        let id = hooked(&store);
        let c = ctx(&Caller::master(), None);
        apply_hook(&store, &make_ssh(), &make_payload("UserPromptSubmit", "uuid-1"), &c).unwrap();
        apply_hook(&store, &make_ssh(), &make_payload("UserPromptSubmit", "uuid-1"), &c).unwrap();
        apply_hook(&store, &make_ssh(), &make_payload("Stop", "uuid-1"), &c).unwrap();
        let changes: Vec<Option<String>> = events(&store, id)
            .into_iter()
            .filter(|(k, _)| k == "status_change")
            .map(|(_, d)| d)
            .collect();
        assert_eq!(
            changes,
            vec![Some("idle".into()), Some("working".into())],
            "newest first; the repeated prompt wrote nothing"
        );
    }
```

`cargo test -p fleet-core a_status_change_that_repeats` → FAILS (4 rows); `a_stuck_event_repeats_only_after_its_window` → FAILS (2 rows); `hook_transitions_land_on_the_timeline_once` → FAILS (empty).

- [x] **Step 2: Dedupe in the store** — `timeline.rs`, above `write_session_event`:

```rust
/// A `stuck` event that repeats one written within this window is the same
/// episode re-detected (the `oom` playbook's spacing is the same hour).
const STUCK_EVENT_WINDOW_SECS: i64 = 3600;

impl Store {
    /// The newest `kind` event's detail for the session — `None` when there
    /// is none (at/after `since`, when given).
    fn last_event_detail(
        &self,
        session_id: i64,
        kind: &str,
        since: Option<i64>,
    ) -> rusqlite::Result<Option<Option<String>>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT detail FROM session_events \
                 WHERE session_id = ?1 AND kind = ?2 AND at >= COALESCE(?3, 0) \
                 ORDER BY at DESC, id DESC LIMIT 1",
                rusqlite::params![session_id, kind, since],
                |r| r.get(0),
            )
            .optional()
    }
}
```

and at the top of `write_session_event`, before the savepoint (`:113`):

```rust
        let at = now_unix();
        // Timeline hygiene (F10): a `status_change` that repeats the newest
        // one is no transition — reconcile and the hooks both write status
        // now, and `claude agents` re-reports the same value every minute;
        // a `stuck` that repeats within the hour is the same episode
        // re-detected (F1). Everything else is written as before.
        let repeat = match kind {
            "status_change" => self.last_event_detail(session_id, kind, None)?,
            "stuck" => {
                self.last_event_detail(session_id, kind, Some(at - STUCK_EVENT_WINDOW_SECS))?
            }
            _ => None,
        };
        if repeat.is_some_and(|d| d.as_deref() == detail) {
            return Ok(());
        }
```

Add to the kind vocabulary doc (`:48`): "- hooks (`service::hooks` via the `record_*_hook_for_row` writers): `notification`, `status_change`, `stop_failure`." and "- the tick (`Store::age_out_stale_working`): `stale_working`."

- [x] **Step 3: Hooks record their transitions** — `store/sessions.rs`: in each recorder, after `if changed == 0 { return Ok(None); }` and before `emit_session`, add a best-effort insert:

```rust
        if let Err(e) = self.insert_session_event(row_id, "status_change", Some("idle")) {
            tracing::warn!(session_id = row_id, error = %e, "[hook] status_change not recorded");
        }
```

with `"idle"` in `record_stop_hook_for_row`, `"working"` in `record_prompt_submit_hook_for_row_with`, `"stopped"` in `record_session_end_hook_for_row`, `"failed"` in `record_stop_failure_hook_for_row`, and `Some(status)` (the already-bound `&str`) in `record_notification_hook_for_row`.

- [x] **Step 4: Run** — `cargo test -p fleet-core timeline::` → PASS; `cargo test -p fleet-core hooks::` → PASS (`a_nested_claude_in_the_pane_never_touches_the_parent_row`'s `events_before == events` still holds: the child's hooks never reach the parent's recorders); `cargo test -p fleet-core reconcile_tests` → PASS (`status_transitions_emit_session_events_and_stamp_lifecycle_columns` asserts distinct transitions); `cargo test -p fleet-core store::` → PASS (`insert_session_event_caps_timeline_per_session` uses distinct details).

- [x] **Step 5: Full suite and commit**

```bash
git add crates/fleet-core/src/store/timeline.rs crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/service/hooks.rs
git commit -m "fix(timeline): one status_change per transition from either writer, one stuck event per episode"
```

---

### Task 8: The desktop reads the hub's context threshold

**Files:**
- Modify: `src/lib/ipc.ts` (`Health` `:4-10`), `src/lib/attention.ts` (`:71-83`), `src/App.svelte` (`:8`, `:216-217`), `src/lib/tracker_health.ts` (`:63-66`)
- Test: `src/lib/attention.test.ts` (`:94-104`, the Task 6 classify assertions)

**Interfaces:**
- Consumes: `Health.context_red_pct` (Task 6).
- Produces: `setContextRedPct(pct: number | undefined): void`, `contextRedThreshold(): number`; `contextLevel(pct)` reads `crit` at the hub's threshold and `warn` 15 points below (`CONTEXT_WARN_MARGIN`); `CONTEXT_WARN_PCT` / `CONTEXT_CRIT_PCT` are removed (nothing outside `attention.ts` imports them — `grep -rn "CONTEXT_WARN_PCT\|CONTEXT_CRIT_PCT" src` is empty after the change).

- [x] **Step 1: Failing tests** — `src/lib/attention.test.ts`: add `afterEach` to the vitest import and `setContextRedPct` to the `./attention` import; replace the `contextLevel` block (`:94-104`):

```ts
describe('contextLevel', () => {
  afterEach(() => setContextRedPct(85));

  it('reads the hub threshold: red at 85, amber 15 points below', () => {
    expect(contextLevel(null)).toBeNull();
    expect(contextLevel(0)).toBe('ok');
    expect(contextLevel(69.9)).toBe('ok');
    expect(contextLevel(70)).toBe('warn');
    expect(contextLevel(84.9)).toBe('warn');
    expect(contextLevel(85)).toBe('crit');
    expect(contextLevel(100)).toBe('crit');
    expect(contextLevel(Number.NaN)).toBeNull();
  });

  it('follows fleet_health.context_red_pct and ignores an older hub that sends none', () => {
    setContextRedPct(95);
    expect(contextLevel(90)).toBe('warn');
    expect(contextLevel(95)).toBe('crit');
    setContextRedPct(undefined);
    expect(contextLevel(95)).toBe('crit');
    setContextRedPct(0);
    expect(contextLevel(95)).toBe('crit');
  });
});
```

and change Task 6's classify assertions to `context_pct: 85` → `'context_full'` and `context_pct: 84.9` → `'idle'`. `npx vitest run src/lib/attention.test.ts` → FAILS (`setContextRedPct` is not exported; `contextLevel(85)` is `'warn'`).

- [x] **Step 2: Implement** — `src/lib/attention.ts:71-83`:

```ts
// ── context pressure ──

export type ContextLevel = 'ok' | 'warn' | 'crit';

/** The hub's one context threshold (`health.context_red_pct`, sent as
 *  `fleet_health.context_red_pct`). 85 until the first health read; a hub
 *  too old to send it (0 / undefined) leaves it alone. */
let contextRedPct = 85;
/** `warn` starts this many points below `crit`. */
const CONTEXT_WARN_MARGIN = 15;

export function setContextRedPct(pct: number | undefined): void {
  if (typeof pct === 'number' && Number.isFinite(pct) && pct > 0 && pct <= 100) contextRedPct = pct;
}

export function contextRedThreshold(): number {
  return contextRedPct;
}

export function contextLevel(pct: number | null): ContextLevel | null {
  if (pct === null || !Number.isFinite(pct)) return null;
  if (pct >= contextRedPct) return 'crit';
  if (pct >= contextRedPct - CONTEXT_WARN_MARGIN) return 'warn';
  return 'ok';
}
```

`src/lib/ipc.ts` `Health`: add `/** The hub's context threshold (percent); absent from an older hub. */ context_red_pct?: number;`. `src/App.svelte:8` → `import { healthCheck, type Health } from './lib/ipc';` plus `import { setContextRedPct } from './lib/attention';` and after `health = hr0.value;` (`:216`) add `setContextRedPct(hr0.value.context_red_pct);`. `src/lib/tracker_health.ts:64-66`:

```ts
export async function refreshTrackersHealth(): Promise<void> {
  const r = await healthCheck();
  if (r.ok) {
    trackersHealth.set(r.value.trackers ?? null);
    setContextRedPct(r.value.context_red_pct);
  }
}
```

with `import { setContextRedPct } from './attention';`.

- [x] **Step 3: Run** — `npx vitest run src/lib/attention.test.ts src/lib/ipc.test.ts src/lib/tracker_health.test.ts` → PASS; `npx vitest run` → PASS; `npx svelte-check` → 0 errors.

- [x] **Step 4: Commit**

```bash
git add src/lib/attention.ts src/lib/attention.test.ts src/lib/ipc.ts src/App.svelte src/lib/tracker_health.ts
git commit -m "feat(desktop): draw the context chip at the hub's context_red_pct instead of a local 70/90"
```

---

## Self-review

**Spec coverage**

| Finding | Where | Task |
|---|---|---|
| lifecycle F1 (playbook kills a live session; no budget; episode restarts) | README row 1, T4 | Task 1 (guard, budget, episode pin), Task 2 (detector), Task 7 (one `stuck` per episode) |
| lifecycle F2 (`working` never ages out; `claude agents` overrides) | README row 4 | Task 5 |
| lifecycle F3 (`StopFailure` → plain idle, nothing surfaced) | README row 3 | Task 4 (status + classed event), Task 6 (`stop_failed`) |
| lifecycle F4 (ghosts keep `claude_status`/`blocked`) | README row 2, T2 | Task 3 |
| lifecycle F6 (external ghosts kept 14 d) | README row 2, T2 | Task 3 (`gc.external_lost_ttl_secs`) |
| lifecycle F7 (attention flags only ghosts) | README row 5 | Task 6 |
| lifecycle F8 (shells in `by_status`, pane-derived status) | README row 2 | Task 3 |
| lifecycle F10 (same-value `status_change` spam; two-writer gap) | README row 25 | Task 7 |
| ux F-09 (hub 85 vs desktop 70/90; no `context_full` bucket) | README row 5 | Task 6 (setting, `Health.context_red_pct`, bucket), Task 8 (desktop reads it) |

**Deliberately left out**

- F5 (hidden `local` host: no pass, immortal ghosts, duplicates on `mac`) and F9 (the controller pair) are not in slice A; F5 is the host rename/merge work in README row 9.
- F1 fix 4 (a timeline tombstone on reap) — retention, not the state machine; the post-mortem for the next false positive now exists on the hub's timeline while the row lives (`oom:recreate:skipped:<why>`, one `stuck` per episode).
- F3's opt-in `rate_limit` retry playbook — a new automated prompt send; the brief asked for surfacing, and the hub refuses unconfirmed sends for the operator.
- F4's `last_claude_status` column for the UI — the ghost row now shows nothing rather than a stale verdict; a column would be a second wire field for a cosmetic.
- A `pane_dead` / `pane_current_command` probe signal for the detector — it needs a change to the per-host reconcile script (`crates/fleet-core/src/tmux.rs:502`) and a `HostProbe` field; the heap block and the kill-verdict-then-shell rule cover both live incidents without it.
- `fleet_health.working_stale` / per-reason attention counts — `list_sessions { needs_attention: true }` already answers it per row; a roll-up field is README row 21's `fleet_health` work.
- `docs/hub.md:1239-1262` names `needs_attention` but does not enumerate its reasons, so no doc edit is needed; `docs/control-api-reference.md` lists `needs_attention` only as a parameter.
