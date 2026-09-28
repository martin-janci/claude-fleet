# Stale-working acknowledgement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `stale_working` attention reason ends when a person has looked (an attach), when the row is working or blocked again, or after a TTL, instead of staying until the session's next hook.

**Architecture:** Store-only lifecycle, no new command or tool. `Store::touch_session` (the attach path) clears `sessions.stale_working_at`; a new `Store::expire_stale_working` sweep on the reconcile tick clears stamps on rows that are working/blocked again or older than a new setting `reconcile.stale_working_ttl_secs`. Both attention classifiers (`service/attention.rs`, `src/lib/attention.ts`) already derive the reason from the column, so clearing it is the whole fix.

**Tech Stack:** Rust (`crates/fleet-core`, rusqlite), Svelte 5 settings dialog, Vitest.

**Spec:** none separate. This plan argues from the live-instance findings of 2026-09-28 (below) and extends `docs/superpowers/plans/2026-09-27-session-state-machine.md` (lifecycle F2, which introduced `stale_working_at`).

## Why (findings, 2026-09-28, hub 0.3.3)

- The F2 sweep (`Store::age_out_stale_working`, first shipped in v0.3.3 by `25a45650`) ran for the first time 25 s after the hub upgrade and demoted rows stuck in `working` for days (session 21639: last hook `turn_done` + `status_change working` at 1790179712, then nothing for 107 h). Correct behaviour.
- But the stamp is cleared ONLY by a hook (`record_stop_hook_for_row`, `record_prompt_submit_hook_for_row_with`, `record_session_end_hook_for_row`, `record_stop_failure_hook_for_row`, `record_notification_hook_for_row` in `store/sessions.rs`). A session nobody means to prompt again stays in "needs you" forever.
- Archiving does not help: `src/lib/tidy.ts:293` `splitArchived` keeps an archived row live while `needsYou(s)` — the operator archived 21639 at 1790590451 and it stayed in the attention list.
- Edge: a pane read can lift the demotion (`stale_working_veto`, `service/sessions/reconcile.rs`, `(Working, Working) => Working`) without clearing the stamp, so a working row shows `stale_working`.

## Global Constraints

- Execute on a branch fresh from `origin/main` (the worktree this plan was written in is ~628 commits behind). `git fetch` and check `main..origin/main` before branching.
- No new Tauri command, MCP tool, `SessionRow` field or wire change: no `REGEN_DOCS`, no `REGEN_HUB_VERDICTS`, no hub contract regen, `CONTRACT_REVISION` unchanged.
- New setting: key `reconcile.stale_working_ttl_secs`, `Kind::Secs`, default `"86400"`, `0` = a stamp never expires by age.
- SQLite writes go through `Store`; never hold the store guard across an `.await`.
- Subagents: sonnet or better for TDD steps; no `git pull/push/rebase/checkout/stash`; run the FULL crate suite per task, unpiped.
- Tests: `cargo test -p fleet-core <name>`; frontend via `npx vitest run` and `npx svelte-check` (not `pnpm test`).

## File Structure

- `crates/fleet-core/src/store/work_tidy.rs` — `touch_session` clears the stamp (Task 1).
- `crates/fleet-core/src/store/work_tidy/tests.rs` — Task 1 test.
- `crates/fleet-core/src/store/sessions.rs` — `Store::expire_stale_working` next to `age_out_stale_working`, plus its test (Task 2).
- `crates/fleet-core/src/service/sessions/reconcile.rs` — service wrapper `expire_stale_working(store)` next to `age_out_stale_working(store)` (Task 2).
- `crates/fleet-core/src/service/tick.rs` — call it right after the age-out sweep (Task 2).
- `crates/fleet-core/src/service/settings.rs`, `src/lib/fleet_settings.ts`, `src/lib/SettingsDialog.svelte` — the new setting, its desktop mirror and dialog row (Task 2; `every_spec_has_a_settings_dialog_row` forces them into one commit).
- `docs/hub.md` — one sentence on how the reason ends (Task 2).

---

### Task 1: An attach acknowledges a stale-working stamp

**Files:**
- Modify: `crates/fleet-core/src/store/work_tidy.rs` (`touch_session`, ~`:302` on origin/main)
- Test: `crates/fleet-core/src/store/work_tidy/tests.rs`

**Interfaces:**
- Consumes: `Store::unarchive_session_work(&self, i64) -> Result<usize, IpcError>` (existing), `Store::emit_session(&self, i64)` (existing, `pub(super)` in `store/sessions.rs`), `Store::bump_row_for_lifecycle(&self, i64)` (existing, used by the archive path).
- Produces: `Store::touch_session(&self, session_id: i64) -> Result<bool, IpcError>` — unchanged signature; now also sets `stale_working_at = NULL` and emits `session:updated` when it cleared one.

- [ ] **Step 1: Write the failing test** — append to `crates/fleet-core/src/store/work_tidy/tests.rs`:

```rust
/// 2026-09-28: a `stale_working` row stayed in "needs you" until its next
/// hook; archiving it did not help. An attach is a person looking — the
/// reason has done its job.
#[test]
fn an_attach_acknowledges_a_stale_working_stamp() {
    let (s, bus) = crate::store::test_support::store_with_recorder();
    let sid = seed(&s, "dev");
    s.conn
        .execute(
            "UPDATE sessions SET claude_status = 'idle', stale_working_at = 500 WHERE id = ?1",
            [sid],
        )
        .unwrap();
    let before = s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(
        crate::service::attention::needs_attention(&before).map(|a| a.reason),
        Some(crate::service::attention::Reason::StaleWorking)
    );
    bus.take();

    assert!(s.touch_session(sid).unwrap());
    let after = s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(after.stale_working_at, None);
    assert_eq!(crate::service::attention::needs_attention(&after), None);
    assert!(
        bus.take().contains(&format!("session:updated:{sid}")),
        "the cleared stamp must reach the clients"
    );

    // Nothing stamped, nothing archived: a touch announces nothing.
    assert!(s.touch_session(sid).unwrap());
    assert!(bus.take().is_empty());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p fleet-core an_attach_acknowledges_a_stale_working_stamp`
Expected: FAIL at `assert_eq!(after.stale_working_at, None)` (left: `Some(500)`).

- [ ] **Step 3: Implement** — replace the body of `touch_session` in `crates/fleet-core/src/store/work_tidy.rs` and extend its doc comment:

```rust
    /// A person is using the session (a prompt, or an attach): stamp
    /// `last_touch_at` (the tidy planner's one-hour protection), un-archive
    /// it, and clear a `stale_working` stamp — the reason asked a person to
    /// look, and one just did. Emits the row when it was archived or
    /// stamped. `false` for a row that does not exist.
    pub fn touch_session(&self, session_id: i64) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE sessions SET last_touch_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, now_unix()],
        )?;
        if n == 0 {
            return Ok(false);
        }
        let acknowledged = self.conn.execute(
            "UPDATE sessions SET stale_working_at = NULL \
             WHERE id = ?1 AND stale_working_at IS NOT NULL",
            rusqlite::params![session_id],
        )?;
        // `unarchive_session_work` bumps and emits the row itself when it
        // un-archived anything; emit here only when it did not.
        if self.unarchive_session_work(session_id)? == 0 && acknowledged > 0 {
            self.bump_row_for_lifecycle(session_id)?;
            self.emit_session(session_id)?;
        }
        Ok(true)
    }
```

- [ ] **Step 4: Run the test and the crate suite**

Run: `cargo test -p fleet-core an_attach_acknowledges_a_stale_working_stamp` → PASS.
Run: `cargo test -p fleet-core archive_is_ui_only_and_the_next_prompt_or_attach_unarchives` → PASS (unchanged behaviour for archive).
Run: `cargo test -p fleet-core` → all pass (read the summary line, do not pipe through `tail`).

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/store/work_tidy.rs crates/fleet-core/src/store/work_tidy/tests.rs
git commit -m "fix(attention): an attach acknowledges a stale_working stamp"
```

---

### Task 2: The tick lifts a resumed or expired stale-working stamp

**Files:**
- Modify: `crates/fleet-core/src/store/sessions.rs` (new fn after `age_out_stale_working`, ~`:1376`; test after `age_out_stale_working_demotes_a_quiet_working_row_and_leaves_the_rest`, ~`:3503`)
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs` (wrapper after `age_out_stale_working`, ~`:2018`)
- Modify: `crates/fleet-core/src/service/tick.rs` (~`:205`)
- Modify: `crates/fleet-core/src/service/settings.rs` (key after `RECONCILE_STALE_WORKING_SECS`, ~`:65`; spec after its spec, ~`:290`)
- Modify: `src/lib/fleet_settings.ts` (~`:11`, ~`:142`)
- Modify: `src/lib/SettingsDialog.svelte` (after the `stale working` row, ~`:996`)
- Modify: `docs/hub.md` (~`:1584`)

**Interfaces:**
- Consumes: `crate::service::settings::get_secs(&Store, &str) -> u64` (existing), `Store::emit_session` (existing).
- Produces: `pub const RECONCILE_STALE_WORKING_TTL_SECS: &str = "reconcile.stale_working_ttl_secs"`; `Store::expire_stale_working(&self, now: i64, ttl_secs: i64) -> Result<Vec<SessionRow>, rusqlite::Error>`; `pub fn expire_stale_working(store: &Mutex<Store>) -> usize` in `service::sessions` (re-exported the same way `age_out_stale_working` is); TS key `SETTING_KEYS.reconcileStaleWorkingTtlSecs`.

- [ ] **Step 1: Write the failing store test** — in `crates/fleet-core/src/store/sessions.rs` `mod tests`, after the age-out test:

```rust
    /// 2026-09-28: the stamp outlived its point. It lifts when the row is
    /// working or blocked again (a pane read lifted the demotion without a
    /// hook) and after `ttl_secs`; a fresh stamp on an idle row stays.
    #[test]
    fn expire_stale_working_lifts_a_resumed_or_old_stamp_and_keeps_a_fresh_one() {
        let s = store();
        let mk = |name: &str, status: &str, at: i64| -> i64 {
            let id = s
                .upsert_session(name, "local", None, None, 1, 1, "running", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET claude_status = ?2, stale_working_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, status, at],
                )
                .unwrap();
            id
        };
        let fresh = mk("fresh", "idle", 9_000);
        let old = mk("old", "idle", 1_000);
        let resumed = mk("resumed", "working", 9_000);
        let asking = mk("asking", "blocked", 9_000);

        let mut lifted: Vec<i64> = s
            .expire_stale_working(10_000, 3_600)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        lifted.sort();
        let mut want = vec![old, resumed, asking];
        want.sort();
        assert_eq!(lifted, want);
        assert_eq!(
            s.get_session_by_id(fresh).unwrap().unwrap().stale_working_at,
            Some(9_000),
            "a fresh stamp on an idle row still asks for a look"
        );

        // `0`: never by age — but a resumed row still lifts.
        let ancient = mk("ancient", "idle", 1);
        let back = mk("back", "working", 1);
        let lifted: Vec<i64> = s
            .expire_stale_working(10_000, 0)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(lifted, vec![back]);
        assert_eq!(
            s.get_session_by_id(ancient).unwrap().unwrap().stale_working_at,
            Some(1)
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p fleet-core expire_stale_working_lifts_a_resumed_or_old_stamp_and_keeps_a_fresh_one`
Expected: FAIL to compile — `no method named expire_stale_working found for struct Store`.

- [ ] **Step 3: Implement the store sweep** — in `crates/fleet-core/src/store/sessions.rs`, directly after `age_out_stale_working`:

```rust
    /// Lift `stale_working_at` where it no longer asks anything of a person:
    /// the row is `working` or `blocked` again (a pane read lifted the
    /// demotion without a hook — `stale_working_veto`), or the stamp is
    /// older than `ttl_secs` (nobody looked; `0` = never by age). An attach
    /// clears it too (`touch_session`), and every hook does. Returns the
    /// rows it changed; each gets `session_updated`.
    pub fn expire_stale_working(
        &self,
        now: i64,
        ttl_secs: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let cutoff = if ttl_secs > 0 { now - ttl_secs } else { i64::MIN };
        let ids: Vec<i64> = self
            .conn
            .prepare(
                "UPDATE sessions SET stale_working_at = NULL \
                 WHERE stale_working_at IS NOT NULL \
                   AND (claude_status IN ('working', 'blocked') OR stale_working_at < ?1) \
                 RETURNING id",
            )?
            .query_map(rusqlite::params![cutoff], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(row) = self.emit_session(id)? {
                out.push(row);
            }
        }
        Ok(out)
    }
```

- [ ] **Step 4: Run the store test** — `cargo test -p fleet-core expire_stale_working_lifts_a_resumed_or_old_stamp_and_keeps_a_fresh_one` → PASS.

- [ ] **Step 5: The setting** — `crates/fleet-core/src/service/settings.rs`, after `RECONCILE_STALE_WORKING_SECS`:

```rust
/// How long a `stale_working` stamp asks for a look before the tick lifts
/// it on its own. An attach or any hook lifts it sooner. `0` = never by age.
pub const RECONCILE_STALE_WORKING_TTL_SECS: &str = "reconcile.stale_working_ttl_secs";
```

and in `SPECS`, after the `RECONCILE_STALE_WORKING_SECS` spec:

```rust
    Spec {
        key: RECONCILE_STALE_WORKING_TTL_SECS,
        default: "86400",
        kind: Kind::Secs,
    },
```

`src/lib/fleet_settings.ts`: add `  reconcileStaleWorkingTtlSecs: 'reconcile.stale_working_ttl_secs',` after `reconcileStaleWorkingSecs` (~`:11`) and `  'reconcile.stale_working_ttl_secs': '86400',` after `'reconcile.stale_working_secs': '1800',` (~`:142`).

`src/lib/SettingsDialog.svelte`, after the `stale working` row (the `</div>` closing `data-testid="reconcile-stale-working-secs"`, ~`:996`):

```svelte
      <div class="mcp-field">
        <span class="lbl">stale ttl</span>
        <input class="port" type="number" min="0"
          value={settingSecs($fleetSettings, SETTING_KEYS.reconcileStaleWorkingTtlSecs)}
          disabled={automationBusy}
          data-testid="reconcile-stale-working-ttl-secs"
          onchange={(e) => onSecsChange(SETTING_KEYS.reconcileStaleWorkingTtlSecs, e)} />
        <span class="hook-desc">seconds a demoted session asks for a look before the tick drops the reason; opening its terminal drops it at once (0 = never)</span>
      </div>
```

- [ ] **Step 6: The service wrapper and the tick** — `crates/fleet-core/src/service/sessions/reconcile.rs`, after `age_out_stale_working`:

```rust
/// The tick's companion to [`age_out_stale_working`]: reads
/// `reconcile.stale_working_ttl_secs` and lifts every stamp that no longer
/// asks anything (`Store::expire_stale_working`). Best-effort; returns how
/// many rows were lifted.
pub fn expire_stale_working(store: &Mutex<Store>) -> usize {
    let Ok(s) = store.lock() else {
        return 0;
    };
    let ttl = crate::service::settings::get_secs(
        &s,
        crate::service::settings::RECONCILE_STALE_WORKING_TTL_SECS,
    ) as i64;
    match s.expire_stale_working(now_unix(), ttl) {
        Ok(rows) => rows.len(),
        Err(e) => {
            tracing::warn!(error = %e, "[reconcile] stale-working expiry failed");
            0
        }
    }
}
```

Export it wherever `age_out_stale_working` is exported from `service::sessions` (check with `git grep -n "age_out_stale_working" crates/fleet-core/src/service/sessions/mod.rs`; if it is `pub use reconcile::*` or a glob, nothing to do).

`crates/fleet-core/src/service/tick.rs`, right after the `if stale > 0 { … }` block (~`:208`):

```rust
                // …and its stamp lifts once it asks nothing: working or
                // blocked again, or older than `reconcile.stale_working_ttl_secs`.
                let lifted = service::sessions::expire_stale_working(store);
                if lifted > 0 {
                    tracing::debug!("reconcile tick: {lifted} stale working stamp(s) lifted");
                }
```

- [ ] **Step 7: Docs** — `docs/hub.md` ~`:1584`, change `` `stale_working` (a `working` row demoted after `reconcile.stale_working_secs` with no activity) `` to `` `stale_working` (a `working` row demoted after `reconcile.stale_working_secs` with no activity; it lifts on the next hook, when its terminal is opened, when the row works again, or after `reconcile.stale_working_ttl_secs`) ``.

- [ ] **Step 8: Run everything**

Run: `cargo test -p fleet-core expire_stale_working` → PASS.
Run: `cargo test -p fleet-core every_spec_has_a_settings_dialog_row` → PASS.
Run: `cargo test -p fleet-core` → all pass.
Run: `cargo clippy --workspace --all-targets -- -D warnings` → clean.
Run: `cargo fmt --all --check` → clean.
Run: `npx vitest run src/lib/fleet_settings.test.ts` → PASS; `npx svelte-check` → 0 errors.

- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/src/store/sessions.rs crates/fleet-core/src/service/sessions/reconcile.rs crates/fleet-core/src/service/tick.rs crates/fleet-core/src/service/settings.rs src/lib/fleet_settings.ts src/lib/SettingsDialog.svelte docs/hub.md
git commit -m "fix(attention): the tick lifts a resumed or expired stale_working stamp (reconcile.stale_working_ttl_secs)"
```

---

## Out of scope (noted, not planned)

- An explicit "Dismiss" button (new command + MCP tool + hub verdict). Only if the attach + TTL turn out not to be enough.
- Why 21639 stayed `working` 107 h without a Stop: likely an interrupted turn (the Stop hook does not run on an Esc interrupt) — unverified; the F2 sweep is the remedy either way.
- `splitArchived` keeping archived `needsYou` rows live is deliberate (a row that needs you must not hide); with this plan an archived stale row drops out once the stamp lifts.
