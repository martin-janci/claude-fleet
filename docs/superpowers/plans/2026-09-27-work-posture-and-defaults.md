# Work graph posture and decision defaults Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A tracker credential cannot sit in plaintext on a hub that is reachable from outside it, an org's boundary is closed by default, and two decisions stop waiting.

**Architecture:** Three independent changes and one documentation close-out. A reachable hub (`hub.public_url` set) refuses an inline `secret` and takes only a `credential_ref`; health degrades any tracker still holding a stored value so it gets moved. A migration closes `orgs.bound_sees_unassigned` for every org and records which ones changed, so Settings can say so rather than letting a bound phone silently lose rows. D5 is closed as *no* in the roadmap.

**Tech Stack:** Rust (fleet-core), SQLite (rusqlite), Svelte 5 (one banner).

**Spec:** `docs/superpowers/specs/2026-09-24-work-graph-design.md` — §0 is authoritative; §0.2 (`tracker_secrets`) and §0.7 / the roadmap's decisions table are what this plan deltas. The re-examination that produced this plan is in *Findings*; the owner declined a separate umbrella spec (2026-09-27), so this section is the delta of record.

## Findings (why this plan exists)

**F1 — a plaintext tracker token in a hub's SQLite is a different risk class than on a desktop.** `tracker_secrets` holds either `value` (stored) or `credential_ref` (`env:NAME` / `file:/absolute/path`, read at use). `credential_ref` is the right path and it exists, but nothing requires it, and `work_admin` is the tool that only ever executes on the hub — the deployment that, on a `fleet-hub` daemon, is published behind a reverse proxy. `hub.public_url` is written by `fleet-hub` only, so it is the precise discriminator for "this deployment is reachable beyond loopback".

**F2 — D31 defaults open on a boundary that is derived.** Org scope comes from `projects.owner` through text-keyed `org_rules`. A repo transferred between owners silently re-derives it, and `067_org_bound_sees_unassigned.sql` ships `DEFAULT 1` — so a mis-derived item is visible to a bound client by default. For the one flag that *is* the security boundary, closed is the right default.

Decision taken by the owner on 2026-09-27: **flip everywhere**, existing orgs included — with the consequence accepted, and therefore to be announced, because a bound phone losing rows on an upgrade otherwise reads as a bug.

**F3 — D5 has blocked on a measurement nobody has taken, across ten milestones.** The work context already rides the first hook's `additionalContext` on `UserPromptSubmit`. A synchronous `SessionStart` hook buys only "Claude knows the work before the first prompt" and costs up to about 2 s at start-up whenever the hub is unreachable — a worse failure mode than a brief arriving one prompt later.

**F4 — `Caps.write` stays a bool.** There is exactly one `WriteOp` (the PR remote link, D29). A bool cannot express "may comment, may not create", but it is not yet a lie; replacing it with a list belongs in the same PR as the second write op, not before. No work in this plan — recorded so the next reader does not re-derive it.

## Global Constraints

- Take the next unused migration number in `crates/fleet-core/migrations/`. Three sibling plans want numbers; check before creating.
- `hub.public_url` (`service::hub::SETTING_PUBLIC_URL`) is written by `fleet-hub` only. Read it through `service::hub`, never by string.
- A new `work.*` setting MUST get a row in `docs/work-graph.md` → Settings with its exact default, or `settings::tests::work_settings_are_in_the_user_guide` fails.
- Any change to a `#[tool(...)]` description: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- A credential never appears in a `Debug`, a request line, a snapshot, an error or an audit summary — conformance scenario 10 asserts it, and `WorkAdminArgs`'s hand-written `Debug` masks `secret`. Nothing in this plan may weaken that.
- Changing `bound_sees_unassigned` bumps `auth_epoch` (the trigger in `067`), which invalidates cached callers. The migration's bulk `UPDATE` will fire it once per row — that is correct and cheap, but assert the epoch moved exactly as a re-binding does.
- Verify with `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm check && pnpm test`.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/service/trackers/admin.rs` | refuse an inline `secret` on a reachable hub |
| `crates/fleet-core/src/service/health.rs` | `TRACKER_REASON_PLAINTEXT_SECRET`, degraded |
| `crates/fleet-core/src/store/trackers.rs` | `trackers_with_stored_secret()` |
| `crates/fleet-core/migrations/0NN_close_org_boundary.sql` | flip `bound_sees_unassigned` for every org |
| `crates/fleet-core/src/service/settings.rs` | `work.d31_notice` |
| `src/lib/…` + the Organisations settings pane | the one-time banner |
| `docs/work-graph.md`, `docs/hub.md`, `CHANGELOG.md` | what changed, for whom |
| `docs/superpowers/2026-09-24-work-graph-roadmap.md` | D5 closed; D31's row records the flip |

---

### Task 1: A reachable hub takes a reference, not a secret

**Files:**
- Modify: `crates/fleet-core/src/service/trackers/admin.rs`
- Modify: `docs/hub.md`
- Test: `crates/fleet-core/src/service/trackers/admin.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `service::hub::SETTING_PUBLIC_URL`, `settings::get_string` (existing).
- Produces: `fn reachable_hub(s: &Store) -> bool` in `admin.rs` — `hub.public_url` is set and non-empty.

- [ ] **Step 1: Write the failing tests**

In `admin.rs`'s existing `mod tests`, beside the `set_credential` tests already there:

```rust
#[tokio::test]
async fn a_reachable_hub_refuses_an_inline_secret() {
    let w = harness();
    w.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "https://fleet.example.com");
    let e = w
        .run(WorkAdminArgs {
            secret: Some("shhh".into()),
            ..args("set_credential")
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(e.message.contains("env:"), "{}", e.message);
    assert!(!e.message.contains("shhh"), "the refusal echoed the secret");
}

#[tokio::test]
async fn a_reachable_hub_accepts_a_reference() {
    let w = harness();
    w.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "https://fleet.example.com");
    let text = w
        .run(WorkAdminArgs {
            credential_ref: Some("env:JIRA_TOKEN".into()),
            ..args("set_credential")
        })
        .await
        .unwrap();
    assert!(text.contains("\"has_credential\":true"));
}

#[tokio::test]
async fn a_loopback_deployment_still_takes_an_inline_secret() {
    let w = harness(); // no hub.public_url
    let text = w
        .run(WorkAdminArgs {
            secret: Some("shhh".into()),
            ..args("set_credential")
        })
        .await
        .unwrap();
    assert!(text.contains("\"has_credential\":true"));
}

#[tokio::test]
async fn an_existing_stored_secret_keeps_working_on_a_reachable_hub() {
    let w = harness();
    w.run(WorkAdminArgs { secret: Some("shhh".into()), ..args("set_credential") }).await.unwrap();
    w.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "https://fleet.example.com");
    // A sync must not start failing because the deployment became reachable.
    assert!(w.store_lock().resolve_tracker_credential(w.tracker).unwrap().is_some());
}
```

Use the file's own harness and `args(...)` helper; do not add a second one.

- [ ] **Step 2: Run them and watch the first fail**

Run: `cargo test -p fleet-core a_reachable_hub_refuses_an_inline_secret`
Expected: FAIL — the secret is stored.

- [ ] **Step 3: Add the guard**

In `admin.rs`:

```rust
/// Whether this deployment is reachable from outside its own machine.
///
/// `hub.public_url` is written by `fleet-hub` only (see
/// [`crate::service::hub`]), so it is exactly "a daemon someone put behind a
/// proxy" — and a stored credential then sits at rest in a database beside a
/// published endpoint. A desktop, whose hub is loopback plus a reverse
/// tunnel, is not that, and keeps taking an inline secret.
fn reachable_hub(s: &Store) -> bool {
    !crate::service::settings::get_string(s, crate::service::hub::SETTING_PUBLIC_URL)
        .trim()
        .is_empty()
}
```

and in the `SetCredential` arm, before the write:

```rust
    if args.secret.is_some() && args.credential_ref.is_none() && reachable_hub(&s) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "this hub is reachable from outside: store the credential outside \
             the database and pass credential_ref (env:NAME or file:/absolute/path)",
        ));
    }
```

The refusal must not name the secret — the message above is a constant, and `WorkAdminArgs`'s `Debug` already masks it.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p fleet-core admin`
Expected: PASS

- [ ] **Step 5: Document it**

In `docs/hub.md`, in the section about configuring trackers:

```markdown
A hub with `hub.public_url` set takes tracker credentials **by reference
only**: `--credential-ref env:JIRA_TOKEN` or
`--credential-ref file:/run/secrets/jira`. The token is read when a request
is made and never stored in `state.db`. A credential already stored inline
keeps working; fleet health marks the tracker until you move it.
```

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/trackers/admin.rs docs/hub.md
git commit -m "feat(work): a reachable hub takes a credential reference only"
```

---

### Task 2: Health says which trackers still hold one

**Files:**
- Modify: `crates/fleet-core/src/store/trackers.rs`
- Modify: `crates/fleet-core/src/service/health.rs`
- Test: `crates/fleet-core/src/service/health.rs` (inline tests)

**Interfaces:**
- Consumes: `TrackersHealth`, `TrackerHealth`, the `TRACKER_REASON_*` constants (existing).
- Produces: `Store::trackers_with_stored_secret(&self) -> Result<Vec<i64>, IpcError>`; `pub const TRACKER_REASON_PLAINTEXT_SECRET: &str = "plaintext_secret";`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_reachable_hub_degrades_a_tracker_holding_a_stored_secret() {
    let w = health_harness();
    w.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "https://fleet.example.com");
    let id = w.tracker_with_stored_secret();
    let h = trackers_health(&w.store, &OrgScope::All).unwrap();
    let t = h.trackers.iter().find(|t| t.id == id).unwrap();
    assert_eq!(t.reason.as_deref(), Some(TRACKER_REASON_PLAINTEXT_SECRET));
    assert_eq!(h.degraded, 1);
    assert_eq!(h.failing, 0, "the tracker works; this is posture, not a fault");
}

#[test]
fn a_reference_is_not_degraded() {
    let w = health_harness();
    w.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "https://fleet.example.com");
    w.tracker_with_credential_ref();
    let h = trackers_health(&w.store, &OrgScope::All).unwrap();
    assert_eq!(h.degraded, 0);
}

#[test]
fn a_loopback_deployment_says_nothing_about_stored_secrets() {
    let w = health_harness();
    w.tracker_with_stored_secret();
    let h = trackers_health(&w.store, &OrgScope::All).unwrap();
    assert_eq!(h.degraded, 0);
}

#[test]
fn a_credential_fault_outranks_posture() {
    let w = health_harness();
    w.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "https://fleet.example.com");
    let id = w.tracker_with_stored_secret_and_auth_failed();
    let h = trackers_health(&w.store, &OrgScope::All).unwrap();
    let t = h.trackers.iter().find(|t| t.id == id).unwrap();
    assert_eq!(t.reason.as_deref(), Some(TRACKER_REASON_CREDENTIAL));
}
```

Use the real names of the health entry point and its harness from the surrounding tests.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p fleet-core plaintext_secret`
Expected: FAIL to compile — the constant does not exist.

- [ ] **Step 3: Add the store read**

In `crates/fleet-core/src/store/trackers.rs` — the only module that may touch `tracker_secrets` — add a read that returns **ids only**, never a value:

```rust
    /// Trackers whose credential is stored in the database rather than held
    /// by reference. Ids only: this module is the only reader of
    /// `tracker_secrets`, and nothing outside it needs more than the fact.
    pub fn trackers_with_stored_secret(&self) -> Result<Vec<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT tracker_id FROM tracker_secrets \
             WHERE value IS NOT NULL AND credential_ref IS NULL",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?)
    }
```

- [ ] **Step 4: Add the reason**

In `health.rs`, beside the other reasons:

```rust
/// [`TrackerHealth::reason`]: the deployment is reachable from outside
/// (`hub.public_url`) and this tracker's credential is stored in the
/// database instead of held by reference. The tracker works — this is
/// posture, so it reads `degraded`, never `failing`, and it never blocks a
/// sync.
pub const TRACKER_REASON_PLAINTEXT_SECRET: &str = "plaintext_secret";
```

Set it in the tracker-health builder only when `reachable_hub` holds and the tracker is in `trackers_with_stored_secret()`, and place the check **after** the credential and sync-failure reasons so a real fault always wins.

- [ ] **Step 5: Wording for the client**

The Attention text is "Move <tracker>'s credential out of the database" with the action pointing at Settings → Work → Trackers. Add it wherever the existing "Reconnect …" string is produced, in the same shape.

- [ ] **Step 6: Run everything and commit**

Run: `cargo test -p fleet-core health`
Expected: PASS

```bash
git add crates/fleet-core/src
git commit -m "feat(work): health flags a stored credential on a reachable hub"
```

---

### Task 3: Close the org boundary, and say so

**Files:**
- Create: `crates/fleet-core/migrations/0NN_close_org_boundary.sql`
- Modify: `crates/fleet-core/src/service/settings.rs`
- Modify: the Organisations settings pane (`src/lib/`) and `docs/work-graph.md`
- Modify: `CHANGELOG.md` (`[Unreleased]`)
- Test: `crates/fleet-core/src/service/orgs.rs` (the isolation tests), `crates/fleet-core/src/store` (the migration)

**Interfaces:**
- Produces: setting `work.d31_notice`, default `""` — a comma-separated list of org ids whose `bound_sees_unassigned` this migration closed, cleared when the operator dismisses the banner.

- [ ] **Step 1: Write the failing migration test**

```rust
#[test]
fn the_migration_closes_every_org_and_records_which() {
    let s = store_at_schema_version(67);
    let a = s.add_org_for_test("Company A"); // DEFAULT 1
    let b = s.add_org_for_test("Company B");
    s.set_bound_sees_unassigned_for_test(b, false); // already closed
    let before = s.auth_epoch();

    s.migrate().unwrap();

    assert!(!s.org(a).unwrap().bound_sees_unassigned);
    assert!(!s.org(b).unwrap().bound_sees_unassigned);
    // Only the org that CHANGED is named, so the banner does not claim
    // something happened to an org where nothing did.
    assert_eq!(
        crate::service::settings::get_string(&s, settings::WORK_D31_NOTICE),
        a.to_string()
    );
    assert!(s.auth_epoch() > before, "a changed boundary invalidates callers");
}

#[test]
fn a_new_org_is_closed_too() {
    let s = migrated_store();
    let id = s.add_org_for_test("Company C");
    assert!(!s.org(id).unwrap().bound_sees_unassigned);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p fleet-core the_migration_closes_every_org`
Expected: FAIL — the migration does not exist.

- [ ] **Step 3: Write the migration**

```sql
-- Work graph, decision D31 revised (2026-09-27): the org boundary defaults
-- CLOSED.
--
-- 067 shipped `bound_sees_unassigned` with DEFAULT 1, so a client bound to an
-- org also saw work and sessions belonging to no org — as a host does. The
-- scope an item lands in is DERIVED (`projects.owner` through text-keyed
-- `org_rules`), so a repo moved between owners silently re-derives it; for
-- the one flag that is the security boundary, the safe default is closed.
--
-- Every existing org is closed, not only new ones: the owner accepted that a
-- bound client loses rows on this upgrade. It must therefore be ANNOUNCED —
-- `work.d31_notice` records the orgs that actually changed, and Settings →
-- Work → Organisations shows a one-time banner naming them, so it reads as a
-- decision and not as a bug.
--
-- 067's trigger bumps `auth_epoch` per changed row, which invalidates cached
-- callers exactly as a re-binding does. That is wanted here.

-- Record before changing: after the UPDATE there is nothing left to tell
-- which rows were open.
INSERT INTO settings (key, value)
SELECT 'work.d31_notice', COALESCE(group_concat(id), '')
FROM orgs WHERE bound_sees_unassigned = 1
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

UPDATE orgs SET bound_sees_unassigned = 0 WHERE bound_sees_unassigned <> 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (NN);
```

Check `settings`' real table and column names before writing that `INSERT` — use whatever `service::settings` writes through, and if it is not a plain `settings(key, value)` table, set the value from Rust in the migration's post-step instead of SQL.

The column's own default must change too. `067` cannot be edited (a database that ran it is already at 67), so add here:

```sql
-- New orgs: the DEFAULT of a column cannot be altered in SQLite, so the
-- default is enforced where orgs are created (`store::orgs::add_org` passes
-- 0 explicitly) and this trigger catches every other writer.
CREATE TRIGGER IF NOT EXISTS trg_orgs_bound_sees_unassigned_closed
AFTER INSERT ON orgs
WHEN NEW.bound_sees_unassigned = 1
BEGIN
  UPDATE orgs SET bound_sees_unassigned = 0 WHERE id = NEW.id;
END;
```

Then make `store::orgs`'s insert pass `0` explicitly, so the trigger is a backstop and not the mechanism.

- [ ] **Step 4: Register the setting and document it**

```rust
/// Orgs whose `bound_sees_unassigned` the D31 revision closed, as a
/// comma-separated list of ids. Read once by Settings to show what changed,
/// then cleared. Empty on a fresh install.
pub const WORK_D31_NOTICE: &str = "work.d31_notice";
```

with a `SPECS` entry defaulting to `""` and `Kind::Text` (use whichever kind the existing text settings use), and a row in `docs/work-graph.md`:

```markdown
| `work.d31_notice` | `` | Set once by the upgrade that closed every organisation's boundary: the organisations whose setting changed. Cleared when you dismiss the notice in Settings → Work → Organisations. |
```

- [ ] **Step 5: Run the guide check and the isolation tests**

Run: `cargo test -p fleet-core work_settings_are_in_the_user_guide`
Run: `cargo test -p fleet-core orgs`
Expected: PASS. D31's isolation test already covers both values of the flag (its row in the roadmap requires it); only the default in its fixtures changes.

- [ ] **Step 6: The banner**

In the Organisations settings pane, when `work.d31_notice` is non-empty, show one dismissible notice above the list:

> Clients bound to an organisation now see only that organisation's work. This changed for: **Company A**. Turn it back on per organisation below.

Dismissing writes the setting back to `""`. Follow the pane's existing pattern for a settings write; add no new store or event kind.

- [ ] **Step 7: The changelog**

In `CHANGELOG.md`'s `[Unreleased]` section (`scripts/release.sh` folds it into the next release):

```markdown
### Changed

- Organisations: a client bound to an organisation now sees only that
  organisation's work and sessions. Previously it also saw work belonging to
  no organisation. Existing organisations were changed by the upgrade; turn
  it back on per organisation in Settings → Work → Organisations.
```

- [ ] **Step 8: Full verification and commit**

Run: `cargo test --workspace`
Run: `pnpm check && pnpm test`
Expected: PASS

```bash
git add crates/fleet-core src docs CHANGELOG.md
git commit -m "feat(work)!: bound clients see only their organisation's work"
```

The `!` is deliberate: this changes what an existing deployment shows.

---

### Task 4: Close D5, and record D29

**Files:**
- Modify: `docs/superpowers/2026-09-24-work-graph-roadmap.md`

- [ ] **Step 1: Close D5**

In the decisions table, replace D5's current answer with:

> **Closed 2026-09-27: no.** The work context already rides the first hook's `additionalContext` on `UserPromptSubmit`; a synchronous `SessionStart` buys only "Claude knows the work before the first prompt" and costs up to ~2 s at start-up whenever the hub is unreachable — a worse failure mode than a brief arriving one prompt later. `scripts/measure-session-start.sh` stays in the repository; it is no longer on the critical path, and M13.4b is withdrawn.

- [ ] **Step 2: Record D31's revision**

Append to D31's row:

> **Revised 2026-09-27: the default is closed**, for existing orgs too, with the change announced (`work.d31_notice`, a Settings banner, a `CHANGELOG` entry). See `plans/2026-09-27-work-posture-and-defaults.md`.

- [ ] **Step 3: Record what D29 still holds**

Append to D29's row:

> `Caps.write` stays a `bool` while there is one write op. It becomes a list of op kinds in the same PR as the second one — a bool cannot express "may comment, may not create", but it is not yet wrong.

- [ ] **Step 4: Add the revision line**

At the bottom of *Revisions*:

```markdown
- 2026-09-27: **re-examination pass.** D5 closed as no; D31 revised to closed
  by default for every org; D29 gained the `Caps.write` note. Four plans
  written from the pass: visible truncation and `describe`, local-item merge,
  the correctness pass, and this posture work.
```

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/2026-09-24-work-graph-roadmap.md
git commit -m "docs(work): close D5, revise D31, note Caps.write"
```

---

## Self-Review

- **Coverage.** F1 → Tasks 1 and 2 (refuse, then surface what is already stored); F2 → Task 3, in the flip-everywhere shape the owner chose, with the announcement the choice requires; F3 → Task 4 Step 1; F4 → Task 4 Step 3, recorded rather than built, which is the finding's own conclusion.
- **Placeholders.** None. Two steps name the thing to check before writing (Task 3 Step 3's settings table shape, Task 2 Step 1's health entry-point name) and say where to look, rather than guessing an identifier.
- **Type consistency.** `reachable_hub(&Store) -> bool` is defined in Task 1 and reused by name in Task 2 Step 4; `trackers_with_stored_secret() -> Vec<i64>` returns ids in both the store and the health task; `TRACKER_REASON_PLAINTEXT_SECRET` has one spelling; `WORK_D31_NOTICE` / `work.d31_notice` is a comma-separated id list in the migration, the setting doc, the guide row and the banner.
