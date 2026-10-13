# Identity Sub-project 1 Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the leftovers of the identity realm model (PR #885, #886) listed in fleet ticket TASK-264: the audit log covers every administrative write, the trust and grant views read the person's effective roles, two tooling gaps, and the review's technical debts.

**Architecture:** No new subsystem. Each task is a bounded change to a flow that exists: the admin-events recorder (`store/admin_events.rs`), the pairing registry (`mcp/pairing.rs`) and its two mint callers, the `work_admin` org dispatch, the client and catalog views, the token cache, the realm store. Tests live beside the code they change, in the style the file already uses.

**Tech Stack:** Rust (fleet-core, fleet-hub), rusqlite, serde_json; bash for `scripts/ci-local.sh`. Builds and tests run on mefistos through the relay (`.superpowers/sdd/task-264/mef-run '<cmd>'`); there is no cargo on this container.

**Spec:** fleet ticket TASK-264 (its notes are the requirement list) and the design approved in chat on 2026-10-13, reproduced per task below. The realm model's spec is `docs/superpowers/specs/2026-10-11-identity-realm-model-design.md` (§3 "every write records one row", §4 views read effective roles, device columns stay a mirror for one release).

## Global Constraints

- Validate with the ladder in `CLAUDE.md`: `cargo fleet-lint` after every task, `scripts/verify.sh` before each commit, both through `mef-run`. Pipe test output through `grep -E "^test .*FAILED$|panicked at|test result: " -A2`. fleet-agent's `report_frames_stay_under_the_frame_cap_and_carry_the_rest_over` is a known load flake: re-run it alone before blaming a change.
- Commits go `git -c core.hooksPath=/dev/null commit` (the pre-commit hook runs clippy locally, where there is no cargo) only after `mef-run 'scripts/verify.sh'` passed on the same tree. Conventional Commits, one commit per task.
- Every admin event's `representation` passes through `store::admin_events::scrub`; a pairing code, a token or a secret never reaches the log.
- The per-device mirror columns `client_tokens.trusted_at`, `assets_admin_at` and the `client_catalog_grants` rows of a PERSONAL catalog are not dropped in this plan; views stop deciding by them.
- No new migration. No change to the hub↔mobile contract (`CONTRACT_REVISION`).
- When a tool description changes: `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`, then run without the variable. The budget test `the_served_definition_budget_stays_bounded` must stay green.
- Docs that describe a changed behaviour change in the same task (`docs/hub.md`, `docs/control-api.md`).

---

## File map

| Area | Files |
|---|---|
| Tooling | `scripts/ci-local.sh`; `crates/fleet-core/src/mcp/tools/tests.rs` (budget test), `crates/fleet-core/src/mcp/guard.rs` (two comments) |
| Caller predicates, role_ref, fence | `crates/fleet-core/src/mcp/auth.rs`, `mcp/guard.rs`, `mcp/tools/fleet.rs`, `mcp/tools/updates.rs`, `service/orgs.rs`, `store/realm.rs`, `service/realm_admin.rs`, `service/org_admin.rs` |
| Token cache | `crates/fleet-core/src/mcp/token_cache.rs`, `mcp/mod.rs` |
| Group depth | `crates/fleet-core/src/store/realm.rs`, `store/realm/tests.rs` |
| Audit scrub | `crates/fleet-core/src/store/admin_events.rs` |
| Projection tests | `crates/fleet-core/src/store/realm/tests.rs` |
| Views | `crates/fleet-core/src/mcp/tools/support.rs`, `mcp/tools/fleet.rs`, `store/clients.rs`, `service/org_admin.rs`, `service/orgs.rs`, `crates/fleet-hub/src/pair.rs`, `docs/hub.md` |
| Audit coverage | `crates/fleet-core/src/mcp/pairing.rs`, `mcp/tools/fleet.rs`, `mcp/oidc.rs`, `mcp/mod.rs` (tests), `service/trackers/admin.rs`, `crates/fleet-hub/src/org.rs`, `docs/hub.md` |

---

### Task 1: `ci-local.sh` honours `CARGO_TARGET_DIR` in the update-scripts step

**Files:**
- Modify: `scripts/ci-local.sh:174-183` (`run_update_scripts`)

**Interfaces:** none.

- [ ] **Step 1: Make the change**

Replace the body of `run_update_scripts` so it reads:

```bash
run_update_scripts() {
  if ! command -v minisign >/dev/null 2>&1; then
    echo "ci-local: minisign not found; skipping scripts/release-update-scripts-test.sh" >&2
    return
  fi
  # The same target dir cargo built into: CARGO_TARGET_DIR when set (a relay
  # or a shared builder), else the repo's own.
  local target_dir="${CARGO_TARGET_DIR:-$ROOT/target}"
  step cargo build -p fleet-update --bin fleet-release --locked
  step env FLEET_RELEASE="$target_dir/debug/fleet-release" FLEET_HUB="$target_dir/debug/fleet-hub" \
    bash scripts/release-update-scripts-test.sh
}
```

`run_hub_e2e` (just below) already computes `target_dir` the same way; keep the two spellings identical.

- [ ] **Step 2: Check it**

Run: `bash -n scripts/ci-local.sh && shellcheck -s bash scripts/ci-local.sh` (shellcheck is on mefistos: `mef-run 'shellcheck -s bash scripts/ci-local.sh'`). Expected: no output.
Run through the relay, where `CARGO_TARGET_DIR` is set: `mef-run 'cargo build -p fleet-hub --locked -q && bash -c "source <(sed -n \"/^run_update_scripts()/,/^}/p\" scripts/ci-local.sh); ROOT=\$PWD; step(){ \"\$@\"; }; run_update_scripts"'`. Expected: the update-scripts test passes (it fails on main under the relay, R28 in the CF-1 ledger).

- [ ] **Step 3: Commit**

```bash
git add scripts/ci-local.sh
git -c core.hooksPath=/dev/null commit -m "build(ci-local): the update-scripts step uses the cargo target dir it built into"
```

---

### Task 2: the tool-definition budget covers the full-client surface

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs:4505-4590` (`the_served_definition_budget_stays_bounded`)
- Modify: `crates/fleet-core/src/mcp/guard.rs:99-101` and `:176-178` (the two comments that say only the master's surface is budgeted)
- Possibly modify: tool descriptions of `Access::Device` / `Access::PersonDevice` tools (`org_admin`, `realm_admin`, `update_policy`, the settings tools) in `crates/fleet-core/src/mcp/tools/*.rs`, then `docs/control-api-reference.md` via REGEN_DOCS.

**Interfaces:** none.

- [ ] **Step 1: Measure first**

Run: `mef-run 'cargo fleet-test -- the_served_definition_budget_stays_bounded -- --nocapture 2>&1 | grep -E "tools / |measured"'`. Note the "client full" line: `N tools / B bytes`. The ticket measured about 829 bytes a tool against the 825 budget.

- [ ] **Step 2: Make the test assert the client surface**

In the test, after the `(served, bytes)` and `(ro_served, ro_bytes)` lines, add:

```rust
    let (client_served, client_bytes) =
        definition_bytes(&client_caller("phone", TokenMode::Full));
    let client_budget = client_served * BYTES_PER_TOOL;
```

Replace the single master assertion with one loop over both budgeted surfaces, keeping the message's shape (it tells the next person what to raise):

```rust
    for (label, n, b) in [
        ("master", served, bytes),
        ("client full", client_served, client_bytes),
    ] {
        let budget = n * BYTES_PER_TOOL;
        println!(
            "{label} surface measured at {b} bytes for {n} tools ({} a tool), \
             of the {budget} budget ({} bytes of headroom)",
            b / n.max(1),
            budget.saturating_sub(b)
        );
        assert!(
            b <= budget,
            "the {label} tool surface grew to {b} bytes for {n} tools, over the {budget} \
             budget ({BYTES_PER_TOOL} a tool): trim a description, or raise \
             BYTES_PER_TOOL on purpose — to {} (the measurement plus 10 bytes a tool), \
             and say in the commit message what was measured and when",
            b / n.max(1) + 10
        );
    }
```

Remove the now-duplicated `println!("master surface measured …")` and the old `assert!(bytes <= budget, …)`; keep `assert!(ro_bytes < bytes / 2, …)` and the per-label listing loop. Update the test's doc comment: the budget is per tool for BOTH the master and a full client, because a phone pays for the client surface on every request; a `Device`-only tool was never measured before.

- [ ] **Step 3: Run it, expect the client surface to fail**

Run: `mef-run 'cargo fleet-test -- the_served_definition_budget_stays_bounded 2>&1 | grep -E "panicked at|over the|test result" -A2'`. Expected: FAILED on "client full".

- [ ] **Step 4: Close the gap by trimming, not raising**

Trim the descriptions of the client-only tools until the client surface fits. Start with the longest: run `mef-run 'cargo fleet-test -- the_served_definition_budget_stays_bounded -- --nocapture'` after each edit. Candidates are the `#[tool(description = …)]` strings of `org_admin` and `realm_admin` in `mcp/tools/fleet.rs` and `update_policy` in `mcp/tools/updates.rs`; shorten repeated phrases, drop parentheticals, keep every action name and parameter name (the reference doc and the agents read them). If the gap cannot be closed without dropping information, raise `BYTES_PER_TOOL` to the figure the failure prints and quote the measurement in the constant's doc comment and the commit message, as the comment convention requires.

- [ ] **Step 5: Regenerate the reference and re-run**

Run: `mef-run 'REGEN_DOCS=1 cargo fleet-test -- reference_is_current >/dev/null 2>&1; cargo fleet-test -- reference_is_current the_served_definition_budget_stays_bounded 2>&1 | grep -E "^test |test result"'`. Apply the relay's `last-remote.patch` (it carries `docs/control-api-reference.md`). Expected: both tests `ok`.

- [ ] **Step 6: Reword the two guard comments**

In `mcp/guard.rs` at the `HostToken` doc (~line 99-101) and the `PersonDevice` arm (~line 176-178), change "every byte of the master's tool surface is budgeted" to "every byte of the master's and a full client's tool surface is budgeted" so the comments stop claiming the client surface is free.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/mcp docs/control-api-reference.md
git -c core.hooksPath=/dev/null commit -m "test(mcp): the definition budget bounds the full-client surface too"
```

(Say in the body what the client surface measured before and after.)

---

### Task 3: one predicate for a fleet admin's device, one `role_ref`, one via-fence loop

**Files:**
- Modify: `crates/fleet-core/src/mcp/auth.rs:269-273` (add two methods after `is_person_device`)
- Modify: `crates/fleet-core/src/mcp/guard.rs:162`, `:182`; `mcp/tools/fleet.rs:377`, `:1146`, `:1223`; `mcp/tools/updates.rs:239`; `service/orgs.rs:1176`
- Modify: `crates/fleet-core/src/store/realm.rs` (add `role_ref` near `RoleRow`, line 37), `service/realm_admin.rs:376-381`, `service/org_admin.rs:482-493`
- Modify: `crates/fleet-core/src/store/realm.rs:1331-1355` (`OrgGroupFence::keep_role_vias`), `service/org_admin.rs:1095-1113` (`fence_realm_to_org`), `service/realm_admin.rs:427-441` (`roles_json`)
- Test: `crates/fleet-core/src/mcp/auth.rs` tests module; `crates/fleet-core/src/store/realm/tests.rs`

**Interfaces:**
- Produces: `Caller::is_fleet_admin_device(&self) -> bool`, `Caller::is_master_or_fleet_admin_device(&self) -> bool`; `store::realm::role_ref(client: Option<&str>, name: &str) -> String` (re-exported as `crate::store::role_ref`); `OrgGroupFence::keep_role_vias(&self, via: &mut Vec<String>) -> bool`.

- [ ] **Step 1: Write the failing tests**

In `mcp/auth.rs`'s test module:

```rust
    #[test]
    fn a_fleet_admins_device_is_one_predicate() {
        let mut c = Caller::master();
        assert!(!c.is_fleet_admin_device(), "the master is not a device");
        assert!(c.is_master_or_fleet_admin_device());
        c.client = Some(ClientRef {
            id: 1,
            name: "phone".into(),
            trusted: false,
            org_id: None,
            person_id: Some(1),
        });
        c.is_fleet_admin = true;
        assert!(c.is_fleet_admin_device());
        assert!(c.is_master_or_fleet_admin_device());
        c.is_fleet_admin = false;
        assert!(!c.is_fleet_admin_device());
        assert!(!c.is_master_or_fleet_admin_device());
    }
```

(If `Caller::master()` sets `client: None` and `is_master()` keys on that, the first assertions hold; read `is_master` and adjust the second half so the device caller is not also a master.)

In `store/realm/tests.rs`:

```rust
#[test]
fn role_ref_spells_a_client_role_with_its_client() {
    assert_eq!(role_ref(Some("fleet"), "org-admin"), "fleet:org-admin");
    assert_eq!(role_ref(None, "fleet-admin"), "fleet-admin");
}

#[test]
fn a_fence_keeps_a_role_only_while_one_via_is_inside() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap().id;
    let fence = s.org_group_fence(org).unwrap();
    let mut direct = vec!["direct".to_string()];
    assert!(fence.keep_role_vias(&mut direct));
    let mut outside = vec!["group:/elsewhere".to_string()];
    assert!(!fence.keep_role_vias(&mut outside), "every via was fenced out");
    let mut mixed = vec!["group:/elsewhere".to_string(), "group:/orgs/acme/admins".to_string()];
    assert!(fence.keep_role_vias(&mut mixed));
    assert_eq!(mixed, vec!["group:/orgs/acme/admins".to_string()]);
    let mut none: Vec<String> = Vec::new();
    assert!(fence.keep_role_vias(&mut none), "a role with no via is kept, as before");
}
```

- [ ] **Step 2: Run them, expect compile failures**

Run: `mef-run 'cargo fleet-test -- a_fleet_admins_device_is_one_predicate role_ref_spells a_fence_keeps 2>&1 | grep -E "^error|test result" | head'`. Expected: `error[E0599]` no method / no function.

- [ ] **Step 3: Implement**

`mcp/auth.rs`, after `is_person_device`:

```rust
    /// A fleet admin on a paired, org-free device of their own — the shape
    /// `Access::PersonDevice` admits. WHOSE and WHAT in one predicate, so no
    /// gate spells half of it.
    pub fn is_fleet_admin_device(&self) -> bool {
        self.is_fleet_admin && self.is_person_device()
    }

    /// [`Self::is_fleet_admin_device`] or the master: the two callers that
    /// administer the fleet (`Access::Person`).
    pub fn is_master_or_fleet_admin_device(&self) -> bool {
        self.is_master() || self.is_fleet_admin_device()
    }
```

Replace the sites: `guard.rs:162` → `caller.is_master_or_fleet_admin_device()`; `guard.rs:182` → `caller.is_fleet_admin_device()`; `fleet.rs:377` → `!caller.is_master_or_fleet_admin_device()`; `fleet.rs:1146`, `fleet.rs:1223`, `updates.rs:239` → `caller.is_fleet_admin_device(),`; `orgs.rs:1176` → `if caller.is_master_or_fleet_admin_device() {`. Keep the comments that explain the two axes; point them at the method.

`store/realm.rs`, next to `RoleRow`:

```rust
/// `client:name` for a client role, the bare name for a realm role: the one
/// spelling of a role reference (`fleet:org-admin`, `fleet-admin`).
pub fn role_ref(client: Option<&str>, name: &str) -> String {
    match client {
        Some(c) => format!("{c}:{name}"),
        None => name.to_string(),
    }
}
```

Export it where `RoleRow` is exported from `store/mod.rs` (`pub use realm::{…, role_ref}`). Then `realm_admin.rs:376` becomes `fn role_ref(r: &RoleRow) -> String { crate::store::role_ref(r.client.as_deref(), &r.name) }` (keep the local wrapper, its 8 callers pass a `RoleRow`), and `EffectiveRoleSummary::display` becomes:

```rust
    pub fn display(&self) -> String {
        let role = crate::store::role_ref(self.client.as_deref(), &self.name);
        if self.via.is_empty() { role } else { format!("{role} (via {})", self.via.join(", ")) }
    }
```

`OrgGroupFence`, after `shows_via`:

```rust
    /// Fence a role's `via` list in place: drop every `via` the fence does
    /// not show. Returns whether the role is still shown — a role whose every
    /// via was fenced out is not (a role with no via never had one to fence).
    pub fn keep_role_vias(&self, via: &mut Vec<String>) -> bool {
        let had = !via.is_empty();
        via.retain(|v| self.shows_via(v));
        !(had && via.is_empty())
    }
```

`org_admin::fence_realm_to_org`: `p.roles.retain_mut(|r| fence.keep_role_vias(&mut r.via));`. `realm_admin::roles_json`: build `let mut via: Vec<String> = e.via.clone();` and `if let Some(f) = fence { if !f.keep_role_vias(&mut via) { continue; } }`, then emit `via`.

- [ ] **Step 4: Lint and run the realm, org_admin and auth tests**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head -40; cargo fleet-test --no-fail-fast -- auth:: realm org_admin orgs::tests guard 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`. Expected: lint clean; no FAILED.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src
git -c core.hooksPath=/dev/null commit -m "refactor(identity): one fleet-admin-device predicate, one role_ref, one via fence"
```

---

### Task 4: `TokenCache::untouch` restores the previous stamp

**Files:**
- Modify: `crates/fleet-core/src/mcp/token_cache.rs:227-290`
- Modify: `crates/fleet-core/src/mcp/mod.rs:414-425`, `:454-457`, `:486-489`
- Test: `crates/fleet-core/src/mcp/token_cache.rs` tests module

**Interfaces:**
- Produces: `TokenCache::touch_due(&self, id: i64, now: i64) -> Option<Option<i64>>` (`None`: not due; `Some(prev)`: due, `prev` is the stamp it replaced), `TokenCache::untouch(&self, id: i64, prev: Option<i64>)`; the same shapes for `host_touch_due`/`host_untouch` (keyed by `&str`) and `control_touch_due`/`control_untouch`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn a_stamp_the_writer_was_too_busy_for_is_restored_not_erased() {
        let c = TokenCache::default();
        assert_eq!(c.touch_due(7, 1_000), Some(None), "first touch: due, nothing before");
        assert_eq!(c.last_touched(7), Some(1_000));
        // Well past the interval: due again, and the old stamp comes back.
        let later = 1_000 + TOUCH_INTERVAL_SECS + 1;
        let prev = c.touch_due(7, later);
        assert_eq!(prev, Some(Some(1_000)));
        c.untouch(7, prev.unwrap());
        assert_eq!(c.last_touched(7), Some(1_000), "the good stamp survives a busy writer");
        // And a first stamp the writer was busy for is forgotten.
        let first = c.touch_due(8, later).unwrap();
        c.untouch(8, first);
        assert_eq!(c.last_touched(8), None);
        assert_eq!(c.touch_due(7, later + 1), None, "not due inside the interval");
    }
```

(`TokenCache::default()`: use whatever constructor the module's existing tests use.)

- [ ] **Step 2: Run it, expect a type error**

Run: `mef-run 'cargo fleet-test -- a_stamp_the_writer_was_too_busy_for 2>&1 | grep -E "^error|test result" | head -5'`.

- [ ] **Step 3: Implement**

```rust
    /// Whether client `id` is due a `last_seen_at` stamp at `now`. Marks it
    /// stamped when it is, so concurrent requests from one phone do not all
    /// go for the writer; the previous stamp is returned so that
    /// [`Self::untouch`] can put it back when the write could not happen.
    pub fn touch_due(&self, id: i64, now: i64) -> Option<Option<i64>> {
        let mut t = self.touched.lock().unwrap_or_else(|p| p.into_inner());
        match t.get(&id) {
            Some(&at) if now - at < TOUCH_INTERVAL_SECS => None,
            _ => Some(t.insert(id, now)),
        }
    }

    /// Put back the stamp [`Self::touch_due`] replaced (the writer was busy):
    /// the good stamp before it is kept, a first one is forgotten, and the
    /// next request tries again either way.
    pub fn untouch(&self, id: i64, prev: Option<i64>) {
        let mut t = self.touched.lock().unwrap_or_else(|p| p.into_inner());
        match prev {
            Some(at) => {
                t.insert(id, at);
            }
            None => {
                t.remove(&id);
            }
        }
    }
```

Mirror for `host_touch_due`/`host_untouch` (`t.insert(alias.to_string(), now)` returns the previous `Option<i64>`) and `control_touch_due`/`control_untouch`. In `mcp/mod.rs` change the three call sites to `if let Some(prev) = cache.touch_due(client.id, now) { match state.store.try_lock() { Ok(s) => {…} Err(_) => cache.untouch(client.id, prev) } }` and the host/control equivalents.

- [ ] **Step 4: Lint and test**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test -- token_cache mcp::tests 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/mcp
git -c core.hooksPath=/dev/null commit -m "fix(mcp): a liveness stamp the writer was busy for is restored, not erased"
```

---

### Task 5: `move_group` bounds the moved subtree's height

**Files:**
- Modify: `crates/fleet-core/src/store/realm.rs:685-720` (`move_group_in_tx`), add `subtree_height_in_tx` next to `group_path_in_tx` (~line 288)
- Test: `crates/fleet-core/src/store/realm/tests.rs`

**Interfaces:**
- Produces: `pub(crate) fn subtree_height_in_tx(conn: &Connection, id: i64) -> Result<usize, IpcError>` (the group itself counts 1).

- [ ] **Step 1: Write the failing test**

```rust
/// A subtree moved under a deep parent must not push any group past the
/// depth cap: `list_groups` walks every path and would refuse the whole
/// realm.
#[test]
fn moving_a_tall_subtree_under_a_deep_parent_is_refused() {
    let s = s();
    // A chain of MAX_GROUP_DEPTH - 2 groups under the top.
    let mut deep = None;
    for i in 0..(MAX_GROUP_DEPTH - 2) {
        deep = Some(s.create_group(deep, &format!("d{i}"), None).unwrap().id);
    }
    // A subtree three groups tall.
    let a = s.create_group(None, "a", None).unwrap().id;
    let b = s.create_group(Some(a), "b", None).unwrap().id;
    let _c = s.create_group(Some(b), "c", None).unwrap().id;
    let err = s.move_group(a, deep).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    assert!(err.message.contains("too deep"), "{}", err.message);
    assert!(s.list_groups().is_ok(), "the listing still works after the refusal");
    // One group tall fits where three did not.
    let lone = s.create_group(None, "lone", None).unwrap().id;
    s.move_group(lone, deep).unwrap();
    assert!(s.list_groups().is_ok());
}
```

(`MAX_GROUP_DEPTH` is private; make it `pub(crate)` or re-state `64` in the test with a comment.)

- [ ] **Step 2: Run it, expect the move to succeed wrongly**

Run: `mef-run 'cargo fleet-test -- moving_a_tall_subtree 2>&1 | grep -E "panicked at|test result" -A2'`. Expected: FAILED at `unwrap_err` (the move is accepted).

- [ ] **Step 3: Implement**

Next to `group_path_in_tx`:

```rust
/// How tall the subtree rooted at `id` is: the group itself counts 1, a leaf
/// child 2, and so on. Capped at `MAX_GROUP_DEPTH + 1`, where the walk stops.
pub(crate) fn subtree_height_in_tx(conn: &Connection, id: i64) -> Result<usize, IpcError> {
    let h: i64 = conn.query_row(
        "WITH RECURSIVE t(id, d) AS (
             SELECT ?1, 1
             UNION ALL
             SELECT g.id, t.d + 1 FROM groups g JOIN t ON g.parent_id = t.id WHERE t.d <= ?2
         ) SELECT max(d) FROM t",
        rusqlite::params![id, MAX_GROUP_DEPTH as i64],
        |r| r.get(0),
    )?;
    Ok(h.max(1) as usize)
}
```

In `move_group_in_tx`, inside `if let Some(p) = parent { … }` after the ancestor loop (where `steps` is the new parent's depth, parent counted):

```rust
            // The whole subtree moves: its tallest branch must still fit
            // under the parent, or `list_groups` (which walks every path)
            // refuses the realm.
            if steps + subtree_height_in_tx(c, id)? > MAX_GROUP_DEPTH {
                return Err(invalid("group nesting too deep"));
            }
```

- [ ] **Step 4: Lint and test**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test -- store::realm 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/store
git -c core.hooksPath=/dev/null commit -m "fix(realm): moving a group bounds the moved subtree's height, not only the parent's depth"
```

---

### Task 6: the audit scrub keeps values, redacts URL credentials and stays JSON

**Files:**
- Modify: `crates/fleet-core/src/store/admin_events.rs:21-105`
- Test: `crates/fleet-core/src/store/admin_events.rs` tests module (lines ~238-365 hold the existing ones)

**Interfaces:**
- Produces: `pub fn scrub(v: &Value) -> Value` (same signature, new rules), `pub const STRING_LEAF_MAX_CHARS: usize = 512`.

Rules the implementation follows:
1. Key matching is case-insensitive (`k.to_ascii_lowercase()` against both lists).
2. `value` is dropped only when a sibling key `key`, `name` or `setting` names a secret: its lowercase text contains `secret`, `password`, `token` or `api_key`. Otherwise it is kept, so a `set_org_setting` event says what the setting became.
3. Every string leaf has URL userinfo redacted: a match of `^[A-Za-z][A-Za-z0-9+.-]*://[^/@\s]+@` has the `userinfo@` replaced by `***@`.
4. A string leaf longer than `STRING_LEAF_MAX_CHARS` is cut there and ends in `…`. `representation_text` then stores `scrub(v).to_string()`; if the whole text is still longer than `REPRESENTATION_MAX_CHARS`, it stores `{"truncated":true,"chars":<len>}` instead of cutting JSON mid-way. The `AdminEventRow.representation` doc comment is updated accordingly.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn the_scrub_keeps_a_plain_value_and_drops_a_secret_one() {
        let kept = scrub(&json!({ "key": "work.auto_tidy", "value": "on" }));
        assert_eq!(kept["value"], "on");
        let dropped = scrub(&json!({ "key": "tracker.api_token", "value": "abc" }));
        assert!(dropped.get("value").is_none());
        let dropped = scrub(&json!({ "name": "Client_Secret", "value": "abc" }));
        assert!(dropped.get("value").is_none());
        let case = scrub(&json!({ "Token": "abc", "Password": "x", "CODE": "1" }));
        assert_eq!(case, json!({}));
    }

    #[test]
    fn the_scrub_redacts_url_credentials_and_keeps_the_rest_of_the_url() {
        let v = scrub(&json!({ "remote": "https://user:tok@github.com/o/r.git", "note": "a@b" }));
        assert_eq!(v["remote"], "https://***@github.com/o/r.git");
        assert_eq!(v["note"], "a@b", "an e-mail is not a URL");
    }

    #[test]
    fn a_long_representation_stays_json() {
        let long = "x".repeat(10_000);
        let v = scrub(&json!({ "notes": long }));
        let s = v["notes"].as_str().unwrap();
        assert!(s.chars().count() <= STRING_LEAF_MAX_CHARS + 1 && s.ends_with('…'));
        let many: serde_json::Map<String, Value> = (0..200)
            .map(|i| (format!("k{i}"), json!("y".repeat(400))))
            .collect();
        let text = representation_text(&Value::Object(many));
        let parsed: Value = serde_json::from_str(&text).expect("still JSON");
        assert_eq!(parsed["truncated"], true);
    }
```

- [ ] **Step 2: Run them, expect failures**

Run: `mef-run 'cargo fleet-test -- admin_events 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

- [ ] **Step 3: Implement**

```rust
/// Longest string leaf kept in a representation; longer ones end in `…`.
pub const STRING_LEAF_MAX_CHARS: usize = 512;

fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    crate::mcp::guard::SKIP_KEYS.iter().any(|s| *s == k) && k != "value"
        || SECRET_KEYS.iter().any(|s| *s == k)
}

/// Does an object's `key` / `name` / `setting` name a secret, so that its
/// `value` is one too?
fn names_a_secret(m: &serde_json::Map<String, serde_json::Value>) -> bool {
    ["key", "name", "setting"].iter().any(|k| {
        m.get(*k).and_then(|v| v.as_str()).is_some_and(|s| {
            let s = s.to_ascii_lowercase();
            ["secret", "password", "token", "api_key"].iter().any(|w| s.contains(w))
        })
    })
}

/// `user:pass@` after a URL scheme becomes `***@`; anything else is left.
fn redact_url_userinfo(s: &str) -> String {
    let Some(scheme_end) = s.find("://") else { return s.to_string() };
    if scheme_end == 0 || !s[..scheme_end].chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) {
        return s.to_string();
    }
    let rest = &s[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) if !rest[..at].contains(char::is_whitespace) => {
            format!("{}://***@{}", &s[..scheme_end], &rest[at + 1..])
        }
        _ => s.to_string(),
    }
}

fn scrub_str(s: &str) -> serde_json::Value {
    let s = redact_url_userinfo(s);
    if s.chars().count() > STRING_LEAF_MAX_CHARS {
        let mut cut: String = s.chars().take(STRING_LEAF_MAX_CHARS).collect();
        cut.push('…');
        serde_json::Value::String(cut)
    } else {
        serde_json::Value::String(s)
    }
}

pub fn scrub(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let secret_value = names_a_secret(m);
            serde_json::Value::Object(
                m.iter()
                    .filter(|(k, _)| !is_secret_key(k))
                    .filter(|(k, _)| !(secret_value && k.eq_ignore_ascii_case("value")))
                    .map(|(k, v)| (k.clone(), scrub(v)))
                    .collect(),
            )
        }
        serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(scrub).collect()),
        serde_json::Value::String(s) => scrub_str(s),
        other => other.clone(),
    }
}

fn representation_text(v: &serde_json::Value) -> String {
    let text = scrub(v).to_string();
    let n = text.chars().count();
    if n > REPRESENTATION_MAX_CHARS {
        serde_json::json!({ "truncated": true, "chars": n }).to_string()
    } else {
        text
    }
}
```

Update the `SECRET_KEYS` doc comment (`value` is no longer unconditionally dropped; say why) and `AdminEventRow.representation`'s. Check the existing tests in the module that asserted `value` is dropped from an unrelated object and change them to the new rule (a `set_org_setting` representation keeps its value; an `env_vars`-style `{ "name": "API_TOKEN", "value": … }` drops it).

- [ ] **Step 4: Lint and test**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test --no-fail-fast -- admin_events org_admin realm_admin oidc 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`. Any test elsewhere that asserted a scrubbed `value` is absent must be read and, if it was asserting the old over-scrub, updated.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/store/admin_events.rs
git -c core.hooksPath=/dev/null commit -m "fix(audit): the scrub keeps plain values, redacts URL credentials and stores JSON always"
```

---

### Task 7: the projection property test covers every writer, and the guard reads every spelling

**Files:**
- Modify: `crates/fleet-core/src/store/realm/tests.rs:338-420`

**Interfaces:** none.

- [ ] **Step 1: Widen the property test**

Replace `the_projection_equals_the_direct_computation`'s loop body with one over eight operations. The store functions exist: `set_org_member(org, person, role, None)`, `remove_org_member(org, person)`, `disable_person(id)` / `enable_person(id)` (in `store/people.rs`), `remove_org(id)` + `add_org(name, None, false)` (in `store/orgs.rs`), `create_group(parent, name, by)`, `move_group(id, parent)`, `add_group_member(group, person)` (name it as `store/realm.rs` does — read the file). After every step the two computations must agree for every live person; a disabled person's direct computation is empty, so check only enabled people. Also assert `shares_since`: for every live membership row, `shares_since` is `Some` iff the person is enabled (read the column's rule in `store/realm.rs`'s projection and assert exactly that rule).

```rust
    let mut orgs: Vec<i64> = …;
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    let mut free_groups: Vec<i64> = Vec::new();
    for step in 0..600 {
        let o = orgs[rng.random_range(0..orgs.len())];
        let p = people[rng.random_range(0..people.len())];
        match rng.random_range(0..8) {
            0 => { s.remove_org_member(o, p).unwrap(); }
            1 | 2 | 3 => { s.set_org_member(o, p, ["admin", "member", "viewer"][rng.random_range(0..3)], None).unwrap(); }
            4 => { if s.get_person(p).unwrap().unwrap().disabled_at.is_none() { s.disable_person(p).unwrap() } else { s.enable_person(p).unwrap() } }
            5 => {
                // An unmanaged group, sometimes moved under an org's managed tree: it must not change the projection.
                let g = s.create_group(None, &format!("g{step}"), None).unwrap().id;
                free_groups.push(g);
                if let Some(&target) = free_groups.first() { let _ = s.move_group(g, Some(target)); }
            }
            6 => { let _ = s.remove_org(o); let n = s.add_org(&format!("o{step}"), None, false).unwrap().id; orgs.retain(|x| *x != o); orgs.push(n); }
            _ => { let _ = s.set_org_member(o, p, "member", None); }
        }
        for &p in &people {
            let enabled = s.get_person(p).unwrap().unwrap().disabled_at.is_none();
            let mut direct = s.org_roles_from_groups(p).unwrap();
            let mut projected: Vec<(i64, String)> = s.memberships_of(p).unwrap().into_iter().filter(|m| m.removed_at.is_none()).map(|m| (m.org_id, m.role)).collect();
            direct.sort(); projected.sort();
            if enabled { assert_eq!(direct, projected, "step {step}") } else { assert!(projected.is_empty(), "step {step}: a disabled person holds nothing") }
        }
    }
```

Adjust names to the real API (`get_person`, `disabled_at`, `remove_org` may refuse a non-empty org — then `let _ =` is right; `move_group` into a managed tree is refused — `let _ =` is right). Keep the test under two seconds on mefistos; lower the step count if needed.

- [ ] **Step 2: Fix the writer guard**

Replace the `writes` / `prod` logic in `only_the_projection_writes_org_members`:

```rust
    // Every spelling of a write, whitespace-insensitive, case-insensitive.
    let re = regex::Regex::new(
        r"(?i)\b(insert(\s+or\s+\w+)?\s+into|replace\s+into|update|delete\s+from)\s+org_members\b",
    )
    .unwrap();
    …
        let text = std::fs::read_to_string(&f).unwrap();
        // Production code only: `#[cfg(test)]` opens a module that runs to the
        // file's end in this crate (one test module per file, last), so the
        // text before the first marker is the production text. A file that
        // keeps production code after it is the bug this comment is for.
        let prod = text.split("#[cfg(test)]").next().unwrap();
        let flat = prod.split_whitespace().collect::<Vec<_>>().join(" ");
        if re.is_match(&flat) {
            offenders.push(rel);
        }
```

`regex` is a dependency of fleet-core already (check `Cargo.toml`; if it is dev-only or absent, use a manual lowercase `contains` over the five spellings `insert into org_members`, `insert or ignore into org_members`, `insert or replace into org_members`, `replace into org_members`, `update org_members`, `delete from org_members` on the whitespace-flattened lowercase text). Also replace `rel.contains("tests")` with `rel.ends_with("/tests.rs") || rel.contains("/tests/") || rel.ends_with("_tests.rs")` so a production file whose name merely contains `tests` is scanned. If `store/session_grants.rs`'s `UPDATE org_members SET shares_since` lines now trip the guard, add `"store/session_grants.rs"` to the allow-list with a comment that it writes only `shares_since` (a column of the projection's row the grants own), and add a second assertion that in that file every match is followed by `SET shares_since`.

- [ ] **Step 3: Run both**

Run: `mef-run 'cargo fleet-test -- the_projection_equals_the_direct_computation only_the_projection_writes_org_members 2>&1 | grep -E "^test |panicked at|test result" -A3'`. Expected: both `ok`. If the property test finds a real projection bug, STOP and report it (that is the test's job; the fix is a separate decision).

- [ ] **Step 4: Commit**

```bash
git add crates/fleet-core/src/store/realm/tests.rs
git -c core.hooksPath=/dev/null commit -m "test(realm): the projection property test covers every writer; the guard reads every spelling"
```

---

### Task 8: `group_by_path` fails closed on an ambiguous legacy path

**Files:**
- Modify: `crates/fleet-core/src/store/realm.rs:563-590`
- Test: `crates/fleet-core/src/store/realm/tests.rs`

**Interfaces:** none (same signature).

- [ ] **Step 1: Write the failing test**

A legacy org name with `/` cannot be created through `add_org` (refused), so write the root group row directly in the test:

```rust
/// An org named before `/` was refused has a root group `acme/eu` whose path
/// `/orgs/acme/eu` reads like group `eu` under org `acme`. The lookup must
/// never answer one for the other.
#[test]
fn group_by_path_never_answers_a_different_group_than_the_path_names() {
    let s = s();
    let acme = s.add_org("acme", None, false).unwrap().id;
    let acme_root = s.group_by_path("/orgs/acme").unwrap().unwrap();
    // A legacy root beside it, written as migration-era data would be.
    s.conn_for_test().execute(
        "INSERT INTO groups (uuid, name, parent_id, managed, org_id) VALUES ('u-legacy', 'acme/eu', (SELECT parent_id FROM groups WHERE id = ?1), 1, NULL)",
        rusqlite::params![acme_root.id],
    ).unwrap();
    // Group `eu` under acme: the path strings collide.
    let eu = s.create_group(Some(acme_root.id), "eu", None).unwrap();
    assert_eq!(eu.path, "/orgs/acme/eu");
    let found = s.group_by_path("/orgs/acme/eu").unwrap().unwrap();
    assert_eq!(found.id, eu.id, "the segment walk finds the nested group");
    assert_eq!(found.path, "/orgs/acme/eu");
    // Nothing answers for the legacy root by path; `group(id)` still does.
    assert!(s.group_by_path("/orgs/acme/eu/").unwrap().is_none());
    let _ = acme;
}
```

Adjust the INSERT to the real `groups` columns (read migration 181) and use whatever raw-connection accessor the tests module already uses (`s.conn` is reachable inside the crate's tests; `Store` fields are `pub(crate)` — check).

- [ ] **Step 2: Implement the fail-closed check**

At the end of `group_by_path`, replace `Some(id) => load_group(&self.conn, id)` with:

```rust
            Some(id) => {
                // Fail closed: a legacy name holding '/' can make two groups
                // spell one path; the group found by walking segments is
                // answered only when its own path is the path asked for.
                let g = load_group(&self.conn, id)?;
                Ok(g.filter(|g| g.path == path))
            }
```

(`GroupRow.path` is filled by `load_group` through `group_path_in_tx`; confirm.)

- [ ] **Step 3: Test, lint, commit**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test -- store::realm 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

```bash
git add crates/fleet-core/src/store
git -c core.hooksPath=/dev/null commit -m "fix(realm): group_by_path answers only a group whose own path is the one asked"
```

---

### Task 9: `list_clients` and `fleet-hub client list` show the person's trust and catalog grant

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/support.rs:1548-1584` (`ClientSummary` + a constructor that takes the role holders)
- Modify: `crates/fleet-core/src/mcp/tools/fleet.rs:756-777` (`list_clients`), `:804` and `:842` (`revoke_client`, `set_client_trust` build the summary too)
- Modify: `crates/fleet-hub/src/pair.rs:370-395`, `:545-556`, fixture `:1052`
- Modify: `docs/hub.md:1246-1251`, `:1277-1279`, `:3764`; `docs/control-api.md` (the `list_clients` row shape)
- Test: `crates/fleet-core/src/mcp/tools/tests.rs` (near `:3800-3850`), `crates/fleet-hub/src/pair.rs` tests

**Interfaces:**
- Produces: `ClientSummary { …, trusted: bool, assets_admin: bool }` (two new fields, always serialised); `ClientSummary::from_row(r: ClientTokenRow, trusted_people: &[i64], assets_admins: &[i64]) -> Self`. `From<ClientTokenRow>` is removed so no caller can build a summary without the role sets.
- Consumes: `ClientTokenRow::trusted_by(&self, &[i64]) -> bool` (`store/rows.rs:1602`), `Store::people_with_realm_role(name) -> Vec<i64>` (`store/realm.rs:1019`), `ROLE_FLEET_TRUSTED`, `ROLE_ASSETS_ADMIN` from `crate::store`.

- [ ] **Step 1: Write the failing test**

In `mcp/tools/tests.rs`, beside the existing `set_client_trust` test:

```rust
/// Trust and the catalog grant are the PERSON's (identity sub-project 1):
/// every full device of a trusted person lists as trusted, whatever its own
/// `trusted_at` mirror says, and a readonly one never does.
#[tokio::test]
async fn list_clients_reports_the_persons_trust_not_the_devices_column() {
    let s = Store::open_in_memory().unwrap();
    let owner = s.personal_owner_id().unwrap().unwrap();
    for (name, mode) in [("desk", "full"), ("phone", "full"), ("kiosk", "readonly")] {
        s.insert_client_token(name, &format!("h-{name}"), mode).unwrap();
        s.set_client_person(name, Some(owner)).unwrap();
    }
    // Trust granted through ONE device: the person becomes trusted.
    s.set_client_trust("desk", true).unwrap();
    let t = test_tools(s);
    let out = t
        .list_clients(Parameters(ListClientsParams { include_revoked: false }))
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_str(text_of(&out.content[0])).unwrap();
    let by = |n: &str| rows.iter().find(|r| r["name"] == n).unwrap().clone();
    assert_eq!(by("desk")["trusted"], true);
    assert!(by("desk")["trusted_at"].is_i64(), "the mirror column is still shown");
    assert_eq!(by("phone")["trusted"], true, "the person's other full device");
    assert!(by("phone")["trusted_at"].is_null(), "…though its own mirror says nothing");
    assert_eq!(by("kiosk")["trusted"], false, "a readonly device is never trusted");
    assert_eq!(by("desk")["assets_admin"], false);
}
```

(Use the module's existing param struct name and `text_of`; if `ListClientsParams` has more fields, `..Default::default()`.)

- [ ] **Step 2: Run it, expect a missing field**

Run: `mef-run 'cargo fleet-test -- list_clients_reports_the_persons_trust 2>&1 | grep -E "panicked at|test result" -A2'`.

- [ ] **Step 3: Implement**

`support.rs`: add the two fields with doc comments ("what the hub decides by: the person's `fleet-trusted` / `assets-admin` role; `trusted_at` / `assets_admin_at` are the per-device mirror kept one release") and replace the `From` impl with:

```rust
impl ClientSummary {
    pub(super) fn from_row(
        r: crate::store::ClientTokenRow,
        trusted_people: &[i64],
        assets_admins: &[i64],
    ) -> Self {
        let trusted = r.trusted_by(trusted_people);
        let assets_admin = r.revoked_at.is_none()
            && r.mode == "full"
            && r.org_id.is_none()
            && r.person_id.is_some_and(|p| assets_admins.contains(&p));
        Self { id: r.id, name: r.name, mode: r.mode, created_at: r.created_at, last_seen_at: r.last_seen_at, revoked_at: r.revoked_at, trusted_at: r.trusted_at, trusted, org_id: r.org_id, assets_admin_at: r.assets_admin_at, assets_admin, person_id: r.person_id }
    }
}
```

(The `assets_admin` eligibility mirrors `LIVE_GRANT_ELIGIBLE` in `store/clients.rs:466`; say so in a comment.) In `fleet.rs`, `list_clients`, `revoke_client` and `set_client_trust` read `let trusted_people = s.people_with_realm_role(ROLE_FLEET_TRUSTED)?; let assets_admins = s.people_with_realm_role(ROLE_ASSETS_ADMIN)?;` under the same lock and call `ClientSummary::from_row(row, &trusted_people, &assets_admins)`. Update the `list_clients` tool description's row shape to `{ id, name, mode, created_at, last_seen_at, revoked_at, trusted, trusted_at, org_id, person_id, assets_admin, assets_admin_at }` and REGEN_DOCS.

`fleet-hub/src/pair.rs`: the `client list` table gets columns `TRUSTED` (`yes`/`no` from `trusted`) and `ASSETS` (`yes`/`no` from `assets_admin`) in place of the two time columns; keep `trusted_at` out of the table (it is the mirror). Update the fixture at `:1052` and the table test. `set_client_trust`'s message (`:545-556`) keeps using `trusted_at` for "since", which `set_client_trust` still returns.

- [ ] **Step 4: Docs**

`docs/hub.md:1246-1251` and `:1277-1279`: replace "still reads the per-device …" with the new behaviour: `client list` and `list_clients` show the person's effective trust and grant; the `trusted_at` / `assets_admin_at` columns remain in `list_clients` as the mirror kept one release. `docs/control-api.md`: the `list_clients` row shape.

- [ ] **Step 5: Lint, test, regen, commit**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; REGEN_DOCS=1 cargo fleet-test -- reference_is_current >/dev/null 2>&1; cargo fleet-test --no-fail-fast -- list_clients set_client_trust revoke_client reference_is_current pair:: 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`. Apply `last-remote.patch`.

```bash
git add crates docs
git -c core.hooksPath=/dev/null commit -m "feat(clients): list_clients and client list show the person's trust and catalog grant"
```

---

### Task 10: a personal catalog's grantees are the `assets-admin` holders' devices

**Files:**
- Modify: `crates/fleet-core/src/store/clients.rs:686-700` (`catalog_grantees`)
- Test: `crates/fleet-core/src/store/clients.rs` tests (~1360-1400), `crates/fleet-core/src/store/schema.rs:4409` (migration test asserting `catalog_grantees(1) == ["desk"]`)
- Modify: `docs/hub.md:1246-1251` (already touched in Task 9; state that `catalog_grantees` for `personal` follows the role)

**Interfaces:**
- Produces: `Store::catalog_grantees(catalog_id) -> Result<Vec<String>, rusqlite::Error>` (same signature; new rule for `org_id IS NULL` catalogs).

- [ ] **Step 1: Write the failing test**

In `store/clients.rs` tests, next to the existing grantees test:

```rust
    #[test]
    fn a_personal_catalogs_grantees_are_every_eligible_device_of_an_assets_admin() {
        let s = Store::open_in_memory().unwrap();
        let owner = s.personal_owner_id().unwrap().unwrap();
        let personal = s.list_catalogs().unwrap().into_iter().find(|c| c.org_id.is_none()).unwrap().id;
        for (name, mode) in [("desk", "full"), ("phone", "full"), ("kiosk", "readonly")] {
            s.insert_client_token(name, &format!("h-{name}"), mode).unwrap();
            s.set_client_person(name, Some(owner)).unwrap();
        }
        assert!(s.catalog_grantees(personal).unwrap().is_empty());
        // The grant through one device is the PERSON's role (identity 1).
        s.set_client_assets_admin("desk", true).unwrap(); // the real writer's name: read clients.rs:597-620
        assert_eq!(s.catalog_grantees(personal).unwrap(), vec!["desk".to_string(), "phone".to_string()]);
        // An org-bound device of theirs is not eligible, as before.
        let acme = s.add_org("acme", None, false).unwrap().id;
        s.set_client_org("phone", Some(acme)).unwrap();
        assert_eq!(s.catalog_grantees(personal).unwrap(), vec!["desk".to_string()]);
    }
```

- [ ] **Step 2: Run it, expect `["desk"]` where `["desk","phone"]` is wanted**

Run: `mef-run 'cargo fleet-test -- a_personal_catalogs_grantees 2>&1 | grep -E "panicked at|test result" -A3'`.

- [ ] **Step 3: Implement**

```rust
    /// The live clients that may manage `catalog_id`, by name. A PERSONAL
    /// catalog (`org_id IS NULL`): every eligible device of a person holding
    /// the realm role `assets-admin` (identity sub-project 1; the grant row
    /// is a mirror). An org catalog: the devices holding a grant row on it.
    /// Both filter with `LIVE_GRANT_ELIGIBLE`: this must never list a client
    /// `client_may_admin_catalog` / `client_is_assets_admin` would refuse.
    pub fn catalog_grantees(&self, catalog_id: i64) -> Result<Vec<String>, rusqlite::Error> {
        let personal: bool = self.conn.query_row(
            "SELECT org_id IS NULL FROM catalogs WHERE id = ?1",
            [catalog_id],
            |r| r.get(0),
        )?;
        if !personal {
            return self.conn
                .prepare_cached(&format!(
                    "SELECT t.name FROM client_tokens t JOIN client_catalog_grants g ON g.client_id = t.id
                     WHERE g.catalog_id = ?1 AND {elig} ORDER BY t.name",
                    elig = Self::LIVE_GRANT_ELIGIBLE,
                ))?
                .query_map([catalog_id], |r| r.get(0))?
                .collect();
        }
        let holders = super::realm::realm_role_holders(&self.conn, super::ROLE_ASSETS_ADMIN)?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT t.name, t.person_id FROM client_tokens t WHERE {elig} ORDER BY t.name",
            elig = Self::LIVE_GRANT_ELIGIBLE,
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))?;
        rows.filter_map(|r| match r {
            Ok((name, Some(p))) if holders.contains(&p) => Some(Ok(name)),
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        })
        .collect()
    }
```

(The catalogs table and column names: read `store/catalogs.rs`. A catalog that does not exist returns an empty list as before: use `.optional()` and treat `None` as not personal with no rows.) Check the two existing assertions (`clients.rs` ~1360-1400 and `schema.rs:4409`): both grant through a device whose person then holds the role, so `["desk"]` still holds when the person has one eligible device; adjust only if a test built a device-only grant without a person.

- [ ] **Step 4: Lint, test, commit**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test --no-fail-fast -- catalog_grantees store::clients catalogs org_admin 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

```bash
git add crates docs/hub.md
git -c core.hooksPath=/dev/null commit -m "fix(catalog): a personal catalog's grantees follow the assets-admin role"
```

---

### Task 11: org-admin device lists hide every fleet admin's devices

**Files:**
- Modify: `crates/fleet-core/src/service/org_admin.rs:987-1030` (`list_devices`), `:775-796` (`require_org_device` uses the same set)
- Modify: `crates/fleet-core/src/service/orgs.rs:1647-1670` (`org_details_locked`'s `owner` filter; check `org_member_list` and `list_members` at `org_admin.rs:649`)
- Test: `crates/fleet-core/src/service/org_admin_tests.rs:559` (`an_org_admin_sees_and_acts_on_their_orgs_devices_only`)

**Interfaces:**
- Consumes: `Store::fleet_admins() -> Result<Vec<i64>, IpcError>` (`store/realm.rs:1043`, sorted, includes the owner).
- Produces: `fn is_fleet_admins_device(admins: &[i64], c: &ClientTokenRow) -> bool` in `service/org_admin.rs`, `pub(crate)` so `orgs.rs` uses it.

- [ ] **Step 1: Extend the failing test**

In `an_org_admin_sees_and_acts_on_their_orgs_devices_only`, after the fixture, add a second fleet admin with a device in the org: create person `root2`, map the `fleet-admin` realm role to them (`s.map_user_role(person, role_id)` or whatever `store/realm.rs` names it; the realm tests show the call), pair a `full` device `root2-phone` to them and make them a member of the org. Then assert that jane's `list_devices` does NOT list `root2-phone`, that `revoke_device root2-phone` as jane is `E_FORBIDDEN` with "is not a device of your org's members" (already the rule), and that the device count is unchanged from before (2). Run it and watch it fail on the list assertion.

- [ ] **Step 2: Implement**

In `org_admin.rs`:

```rust
/// A fleet admin's device, the hub owner's among them (`Store::fleet_admins`
/// lists the owner): an org admin neither lists nor acts on it.
pub(crate) fn is_fleet_admins_device(admins: &[i64], c: &ClientTokenRow) -> bool {
    c.person_id.is_some_and(|p| admins.binary_search(&p).is_ok())
}
```

`list_devices`: replace `let owner = s.personal_owner_id()?;` with `let admins = s.fleet_admins()?;` and the filter arm with `c.org_id == Some(org) && !is_fleet_admins_device(&admins, c)`. Update the function's comment ("never a fleet admin's, the hub owner's among them"). `require_org_device`: keep its per-row `is_fleet_admin` call or switch to `fleet_admins()` + the helper; either way the two must give one answer, so add a one-line comment naming the helper. `orgs.rs:1647`: `let admins = s.fleet_admins()?;` and `.filter(|c| admin || !crate::service::org_admin::is_fleet_admins_device(&admins, c))`; check `org_member_list` and `list_members` (`org_admin.rs:649`) for the same `owner` comparison and switch them too.

- [ ] **Step 3: Lint, test, commit**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test --no-fail-fast -- org_admin orgs:: 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

```bash
git add crates/fleet-core/src/service
git -c core.hooksPath=/dev/null commit -m "fix(org_admin): device lists hide every fleet admin's devices, as require_org_device already does"
```

---

### Task 12: a pairing code carries its minter; the mint and the redeem are audited

**Files:**
- Modify: `crates/fleet-core/src/mcp/pairing.rs:115-160` (`MintRequest`, `PairingRequest`), `:290-300` (`Pending`), `:325-350` (`mint`), `:368-392` (`consume`), `:624-675` (the `pair` event)
- Modify: `crates/fleet-core/src/mcp/tools/fleet.rs:612-760` (`mint_pairing` takes a `Minter`, records `pair_client`), `:604-610` (`pair_client`), `:1360-1375` (`pair_device` passes its minter)
- Modify: `crates/fleet-core/src/mcp/oidc.rs:1062` (the OIDC mint names `via: "oidc"`)
- Modify tests that build `MintRequest`: `mcp/mod.rs:1160` (`mint_req`), `:1439`, `:1443`, `:1487`, `:1596`; `mcp/pairing.rs:762`, `:1002`, `:1011`
- Modify: `docs/hub.md` (the audit paragraph that says pairing codes carry no minter)
- Test: `crates/fleet-core/src/mcp/mod.rs` (the `/pair` integration test around `:1496-1560`), `crates/fleet-core/src/mcp/tools/tests.rs`

**Interfaces:**
- Produces, in `mcp/pairing.rs`:

```rust
/// Who minted a code, carried on it so the redeem's event can say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Minter<'a> {
    /// The person who asked for the code; `None` for the operator on the hub.
    pub person: Option<i64>,
    /// The device they asked through, when through one (its client id).
    pub device: Option<i64>,
    /// Which surface: `pair_client` (master), `org_admin` (pair_device), `oidc` (a sign-in).
    pub via: &'a str,
    /// An org admin's mint is fenced to their org: the redeem re-checks a
    /// trusted code against it (Task 13). `None`: fleet authority.
    pub fenced_org: Option<i64>,
}
```

  `MintRequest` gains `pub minter: Minter<'a>`; `Pending` stores `minter_person: Option<i64>, minter_device: Option<i64>, via: String, fenced_org: Option<i64>`; `PairingRequest` gains `pub minted_by_person: Option<i64>, pub minted_by_device: Option<i64>, pub minted_via: String, pub fenced_org: Option<i64>`.
- `FleetTools::mint_pairing(&self, p: PairClientParams, tool: &str, minter: Minter<'_>)`.

- [ ] **Step 1: Write the failing tests**

In `mcp/mod.rs`'s `/pair` integration test (the one that asserts `("pair","device")` events), extend the `mint_req` helper to set `minter: pairing::Minter { person: Some(42), device: None, via: "test", fenced_org: None }` and, after the redeem, assert the `pair` event's representation carries `"minted_by_person": 42` and `"minted_via": "test"`.

In `mcp/tools/tests.rs`, a tool-level test:

```rust
/// A master mint is an administrative write: one `pair_client` event, with
/// no code in it, and the redeem's `pair` event names the minter (TASK-264).
#[tokio::test]
async fn pairing_a_client_records_the_mint_and_the_redeem_names_the_minter() {
    let s = Store::open_in_memory().unwrap();
    let t = test_tools(s);
    let out = t.pair_client(Parameters(PairClientParams { name: "phone".into(), mode: None, ttl_s: None, trusted: false, org_id: None, person: None })).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(text_of(&out.content[0])).unwrap();
    let ev = t.store.lock().unwrap().admin_events(&crate::store::AdminEventQuery { limit: 10, ..Default::default() }).unwrap();
    let mint = ev.iter().find(|e| e.action == "pair_client").expect("the mint is recorded");
    assert_eq!((mint.resource_type.as_str(), mint.resource_id.as_str()), ("pairing", "phone"));
    assert!(mint.representation.get("code").is_none(), "the code never reaches the log");
    assert_eq!(mint.representation["via"], "pair_client");
    let _ = v;
}
```

(`PairClientParams`'s exact fields: read `fleet.rs` near line 580.)

- [ ] **Step 2: Run them, expect compile errors**

Run: `mef-run 'cargo fleet-test -- pairing_a_client_records_the_mint 2>&1 | grep -E "^error" -A3 | head -20'`.

- [ ] **Step 3: Implement**

`pairing.rs`: add `Minter`, the fields, copy them in `mint` (into `Pending`) and `consume` (into `PairingRequest`). In `handle_pair`'s `pair` event representation add `"minted_by_person": req.minted_by_person, "minted_by_device": req.minted_by_device, "minted_via": req.minted_via`; rewrite the comment at `:624-626` ("who minted the code rides on it since TASK-264").

`fleet.rs`: `pair_client` calls `self.mint_pairing(p, "pair_client", Minter { person: None, device: None, via: "pair_client", fenced_org: None })`. `pair_device` builds `Minter { person: me.person, device: <id of me.device, resolved under the lock as `run` does>, via: "org_admin", fenced_org: match authority { Authority::Org { org, .. } => Some(org), Authority::Fleet => None } }`. In `mint_pairing`, after the mint and only when `tool == "pair_client"` (pair_device records its own `pair_device` event at `:1186`, one row per write):

```rust
        if tool == "pair_client" {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.record_admin_event_or_log(&crate::store::AdminEvent {
                actor_person: minter.person,
                actor_device: minter.device,
                action: "pair_client",
                resource_type: "pairing",
                resource_id: &name,
                representation: serde_json::json!({
                    "name": name, "mode": mode, "trusted": p.trusted, "org_id": p.org_id,
                    "person": person, "ttl_s": ttl.as_secs(), "via": minter.via, "by": "master",
                }),
            });
        }
```

`oidc.rs:1062`: `minter: Minter { person: Some(person.id), device: None, via: "oidc", fenced_org: None }`. Tests: `mint_req` and the direct literals get `minter: Minter::default()` with `via: "test"` (a `Default` with `via: ""` is fine for the literals; give `mint_req` the explicit one the integration test asserts).

`docs/hub.md`: in the audit-log section, the known gap "pairing codes carry no minter" becomes the new behaviour (`pair_client` event at the mint; the `pair` event names the minter and the surface).

- [ ] **Step 4: Lint, test, commit**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test --no-fail-fast -- pairing pair_client pair_device oidc mcp::tests 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

```bash
git add crates docs/hub.md
git -c core.hooksPath=/dev/null commit -m "feat(audit): a pairing code carries its minter; the mint is recorded and the redeem names it"
```

---

### Task 13: a trusted code is re-checked against the org at redeem

**Files:**
- Modify: `crates/fleet-core/src/mcp/pairing.rs:676-700` (before `set_client_trust` in `handle_pair`)
- Test: `crates/fleet-core/src/mcp/mod.rs` (`/pair` integration tests)

**Interfaces:**
- Consumes: `PairingRequest.fenced_org` (Task 12), `crate::service::org_admin::{require_trust_inside_org, Me, Authority}` (`service/org_admin.rs:806`).

- [ ] **Step 1: Write the failing test**

In `mcp/mod.rs`'s pairing tests, beside the trusted-pairing test: mint a trusted code for person `bob` with `fenced_org: Some(acme)` (bob a member of acme with no other device); BEFORE redeeming, give bob a device in another org (`insert_client_token("bob-other", …)`, `set_client_person`, `set_client_org(.., Some(other))`); redeem. Expected: the `/pair` response is a 4xx (`StatusCode::FORBIDDEN`) with the `require_trust_inside_org` wording, no `client_tokens` row for the new name exists (the transaction rolled back), and no `pair` event was recorded. A second test: the same mint with `fenced_org: None` redeems trusted as before.

- [ ] **Step 2: Run it, expect the redeem to succeed wrongly**

- [ ] **Step 3: Implement**

In `handle_pair`, inside `s.atomically`, replace `let row = if req.trusted { let row = s.set_client_trust(&row.name, true)?; … }` with:

```rust
            let row = if req.trusted {
                // An org admin's trusted code is checked against the org at
                // REDEEM, not only at mint: between the two the person can
                // gain a device or a membership outside it (TASK-264). A
                // fleet-authority mint (`fenced_org: None`) skips it, as at mint.
                if let Some(org) = req.fenced_org {
                    crate::service::org_admin::require_trust_inside_org(
                        s,
                        row.person_id,
                        crate::service::org_admin::Me {
                            device: None,
                            person: req.minted_by_person,
                            authority: crate::service::org_admin::Authority::Org { org, hub: false },
                        },
                    )?;
                }
                let row = s.set_client_trust(&row.name, true)?;
                …
```

Map the `E_FORBIDDEN` out of `inserted` to a `403` JSON response with the error's message (read how `handle_pair` maps other `IpcError`s below `match inserted` and follow it; the plaintext token must not be in a failed response). `hub: false` is right: the hub-owning flag changes host routing, not trust.

- [ ] **Step 4: Lint, test, commit**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test --no-fail-fast -- pairing mcp::tests org_admin 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

```bash
git add crates/fleet-core/src/mcp
git -c core.hooksPath=/dev/null commit -m "fix(pairing): a trusted code minted by an org admin is re-checked against the org at redeem"
```

---

### Task 14: `work_admin`'s org writes and the CLI's direct org commands record admin events

**Files:**
- Modify: `crates/fleet-core/src/service/trackers/admin.rs:603` (the `AdminAction::Org(o)` arm of the dispatch under `admin_sync`)
- Modify: `crates/fleet-hub/src/org.rs:424-560` (`run_direct`: `member add|rm|grants`, `own-hub`, `unclaimed-count` — each write calls `crate::person::record`)
- Modify: `docs/hub.md` (the audit-log known gaps: `fleet-hub client bind`, `fleet-hub org …`, master `work_admin` org actions)
- Test: `crates/fleet-core/src/service/trackers/admin.rs` tests or `crates/fleet-core/src/mcp/tools/tests_isolation.rs:400` (the `work_admin` harness), `crates/fleet-hub/src/org.rs` tests

**Interfaces:**
- Consumes: `OrgAction` (`service/orgs.rs:1405`), `WorkAdminArgs: Serialize` (`service/trackers/admin.rs:20`), `Store::record_admin_event_or_log`.
- Produces: `fn org_audit_target(action: OrgAction, args: &WorkAdminArgs) -> Option<(&'static str, String)>` in `service/trackers/admin.rs`.

- [ ] **Step 1: Write the failing test**

Against the service, with a store:

```rust
    /// Every org WRITE through `work_admin` (the master's path, and the one
    /// `fleet-hub org …` / `client bind` take) records one admin event; a
    /// read records none (TASK-264).
    #[test]
    fn work_admin_org_writes_record_one_admin_event_each() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let call = |action: &str, extra: serde_json::Value| {
            let mut v = serde_json::json!({ "action": action });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let args: WorkAdminArgs = serde_json::from_value(v).unwrap();
            admin_sync(&args, &st).unwrap()
        };
        let events = || st.lock().unwrap().admin_events(&crate::store::AdminEventQuery { limit: 50, ..Default::default() }).unwrap();
        call("list_orgs", serde_json::json!({}));
        assert!(events().is_empty(), "a read records nothing");
        let out = call("add_org", serde_json::json!({ "name": "acme" }));
        let org = out["id"].as_i64().unwrap();
        let ev = events();
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].action.as_str(), ev[0].resource_type.as_str(), ev[0].resource_id.as_str()), ("add_org", "org", "acme"));
        assert_eq!((ev[0].actor_person, ev[0].actor_device), (None, None));
        assert_eq!(ev[0].representation["by"], "master");
        st.lock().unwrap().insert_client_token("phone", "h", "full").unwrap();
        call("assign_client", serde_json::json!({ "name": "phone", "org_id": org }));
        let ev = events();
        assert_eq!(ev[0].action, "assign_client");
        assert_eq!((ev[0].resource_type.as_str(), ev[0].resource_id.as_str()), ("org", org.to_string().as_str()));
    }
```

(Read `orgs::admin`'s `add_org` output shape for the id key; `admin_sync`'s exact name and signature at `service/trackers/admin.rs` — it takes `&WorkAdminArgs, &Mutex<Store>`.)

- [ ] **Step 2: Run it, expect an empty log after `add_org`**

- [ ] **Step 3: Implement**

In `service/trackers/admin.rs`:

```rust
/// Which resource a `work_admin` org WRITE is recorded against — the same
/// mapping `org_admin::audited_resource` uses for the same actions, so the
/// two paths to one write log one shape. `None` for a read.
fn org_audit_target(action: OrgAction, args: &WorkAdminArgs) -> Option<(&'static str, String)> {
    let text = |v: &Option<String>| v.as_deref().map(str::trim).unwrap_or("").to_string();
    let org = || args.org_id.map(|i| i.to_string()).unwrap_or_else(|| text(&args.name));
    Some(match action {
        OrgAction::ListOrgs => return None,
        OrgAction::AddOrg => ("org", text(&args.name)),
        OrgAction::AddRule => ("org_rule", org()),
        OrgAction::RemoveRule => ("org_rule", args.rule_id.map(|i| i.to_string()).unwrap_or_default()),
        _ => ("org", org()),
    })
}
```

(If `WorkAdminArgs` has no `rule_id`, read how `orgs::admin` finds the rule to remove and use that field.) In the dispatch:

```rust
        AdminAction::Org(o) => {
            let out = crate::service::orgs::admin(o, args, &s)?;
            if let Some((resource_type, resource_id)) = org_audit_target(o, args) {
                let mut representation = serde_json::to_value(args).unwrap_or_default();
                if let Some(m) = representation.as_object_mut() { m.insert("by".into(), "master".into()); }
                s.record_admin_event_or_log(&crate::store::AdminEvent {
                    actor_person: None, actor_device: None,
                    action: args.action.as_str(), resource_type, resource_id: &resource_id, representation,
                });
            }
            Ok(out)
        }
```

`org_admin::run_action` calls `orgs::admin` directly and records its own event, so this arm (the `admin_sync` path) is the only one that gains a row: no double recording. Confirm by grepping `orgs::admin(` callers.

`fleet-hub/src/org.rs` `run_direct`: after each store write in `member add`, `member rm`, `member grants …`, `own-hub`, `unclaimed-count` (whichever write), call `crate::person::record(&store, "<subcommand>", "org_member" | "hub" | "org", &resource, serde_json::json!({ "org": org, "person": …, "role": …, "by": "operator" }))` with the same resource shapes `org_admin::audited_resource` uses (`org_member` → `"{org}/{person}"`, `own-hub` → `("hub", org)`). Add a test in `org.rs` that runs `member add` against a temp store and asserts one event (the existing `person.rs` CLI tests show how a temp data dir and `open_store` are set up).

`docs/hub.md`: the three known gaps become covered behaviour.

- [ ] **Step 4: Lint, test, commit**

Run: `mef-run 'cargo fleet-lint 2>&1 | grep -E "^(error|warning)" -A4 | head; cargo fleet-test --no-fail-fast -- work_admin trackers::admin orgs::tests org:: 2>&1 | grep -E "^test .*FAILED$|panicked at|test result: " -A2'`.

```bash
git add crates docs/hub.md
git -c core.hooksPath=/dev/null commit -m "feat(audit): work_admin's org writes and the CLI's direct org commands record admin events"
```

---

### Task 15: whole-branch verification and the ticket

- [ ] **Step 1:** `mef-run 'scripts/verify.sh full'` (with `FLEET_RELEASE`/`FLEET_HUB` no longer needed after Task 1). Expected: green, or only the documented fleet-agent flake, re-run alone.
- [ ] **Step 2:** `docs/status.md`: if it lists the identity known gaps, update the list to what remains (the mefistos repo item is not code).
- [ ] **Step 3:** Push, open one PR against `main` listing the four groups and the measurements from Task 2, and comment the PR link on TASK-264 (`work_link { action: "comment", item_id: 264 }`).
