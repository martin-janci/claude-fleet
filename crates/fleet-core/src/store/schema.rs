//! Schema migrations: the ordered `MIGRATIONS` list, `migrate`, and the
//! guards that let a migration run over an existing database.

use super::*;

/// One `PRAGMA foreign_key_check` row: a child row whose foreign key names a
/// missing parent. `(table, rowid, parent, fkid)` identifies it stably across
/// a migration: rowids survive the 024 rebuild (the id column is the rowid)
/// and the rebuilt table declares its foreign keys in the same order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FkViolation {
    pub table: String,
    pub rowid: Option<i64>,
    pub parent: String,
    pub fkid: i64,
}

/// Every dangling foreign key in the database.
fn fk_violations(conn: &Connection) -> rusqlite::Result<Vec<FkViolation>> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let rows = stmt.query_map([], |r| {
        Ok(FkViolation {
            table: r.get(0)?,
            rowid: r.get(1)?,
            parent: r.get(2)?,
            fkid: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// `already_applied` guard of migration 024: `worktrees` already has its
/// `host_alias` column. 024 rebuilds the table, and running it again would
/// reset every row's host to 'local' (and collide remote rows with
/// same-named local ones), so on such a table a re-run only records the
/// version. See [`Migration`].
fn worktrees_has_host_alias(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('worktrees') WHERE name = 'host_alias'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 071: `usage_daily` already has its
/// `backfill` column. 071 rebuilds the table, and running it again would
/// collapse backfill rows into live ones, so on such a table a re-run only
/// records the version. See [`Migration`].
fn usage_daily_has_backfill(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('usage_daily') WHERE name = 'backfill'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 075: `sessions` already has its
/// `launch_model` column.
fn sessions_has_launch_model(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'launch_model'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 072: `sessions` already has its
/// `usage_backfill_until` column.
fn sessions_has_usage_backfill_until(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'usage_backfill_until'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 025 (session usage): it adds its
/// columns in one transaction, so its last column present means the whole
/// migration is, and a re-run (`ALTER TABLE ... ADD COLUMN` again) would
/// fail. See [`Migration`].
fn sessions_has_usage_columns(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') \
         WHERE name = 'usage_last_msg_usage'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 026: `worktrees` already has its
/// `updated_at_ms` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn worktrees_has_updated_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('worktrees') WHERE name = 'updated_at_ms'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 027: `projects` already has its
/// `adopted` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn projects_has_adopted(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'adopted'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 038: `projects` already has its
/// `system` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn client_tokens_has_trusted_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('client_tokens') WHERE name = 'trusted_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 042: `sessions` already has its
/// `row_version` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn sessions_has_row_version(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'row_version'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 081: `sessions` already has its
/// `pane_working_at` column, and `ALTER TABLE ... ADD COLUMN` would fail
/// again. See [`Migration`].
fn sessions_has_pr_evidence(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'pr_evidence'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn sessions_has_pane_working_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'pane_working_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 065: `sessions` already has its
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

/// `already_applied` guard of migration 080: `sessions` already has its
/// `stale_demoted_at` column, and `ALTER TABLE ... ADD COLUMN` would fail
/// again (the backfill is `backfill_stale_demoted`, idempotent, outside the
/// migration). See [`Migration`].
fn sessions_has_stale_demoted_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'stale_demoted_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 076: `hosts` already has its
/// `claude_version_at` column, and `ALTER TABLE ... ADD COLUMN` would fail
/// again. See [`Migration`].
fn hosts_has_claude_version_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'claude_version_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 077: `hosts` already has its
/// `health_at` column (and the eight beside it), and `ALTER TABLE ... ADD
/// COLUMN` would fail again. See [`Migration`].
fn hosts_has_health_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'health_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 078: `hosts` already has its
/// `provision_fingerprint` column (and `provisioned_at` beside it), and
/// `ALTER TABLE ... ADD COLUMN` would fail again. See [`Migration`].
fn hosts_has_provision_fingerprint(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'provision_fingerprint'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 043: `session_messages` already has
/// its `to_participant_id` column, and `ALTER TABLE ... ADD COLUMN` would
/// fail again. See [`Migration`].
fn messages_have_participant_columns(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('session_messages') \
         WHERE name = 'to_participant_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 054: `participants.address` exists,
/// so its `ALTER TABLE ... ADD COLUMN` lines would fail again.
fn participants_have_address(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('participants') WHERE name = 'address'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 056: `conversations` already has
/// `classify_nudged_at`. See [`Migration`].
fn conversations_have_nudge_stamp(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = 'classify_nudged_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 047: `work_links` already has its
/// `role` column, and `ALTER TABLE ... ADD COLUMN` would fail again. See
/// [`Migration`].
fn work_links_has_role(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_links') WHERE name = 'role'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 048: `work_items` already has its
/// `aliases` column, and `ALTER TABLE ... ADD COLUMN` would fail again. See
/// [`Migration`].
fn work_items_has_aliases(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = 'aliases'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 049: `work_links` already has its
/// `evidence` column, and `ALTER TABLE ... ADD COLUMN` would fail again. See
/// [`Migration`].
fn work_links_has_evidence(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_links') WHERE name = 'evidence'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 050 (work graph M5): its last ADD
/// COLUMN (`work_links.snap_org_id`) present means the whole migration is.
fn work_links_has_snap_org(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_links') WHERE name = 'snap_org_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 051: `trackers` already has its
/// `settings` column (the last of the two it adds).
fn trackers_has_settings(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('trackers') WHERE name = 'settings'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 155: `deferred_prompts` already has
/// its `not_before` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn deferred_prompts_has_not_before(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('deferred_prompts') WHERE name = 'not_before'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 052: `work_links` already has its
/// `archived_at` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn work_links_has_archived_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_links') WHERE name = 'archived_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 053: `orgs` already has its
/// `auto_tidy` column.
fn orgs_has_auto_tidy(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('orgs') WHERE name = 'auto_tidy'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 068: `orgs` already has its
/// `jev_allowed` column.
fn orgs_has_jev_allowed(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('orgs') WHERE name = 'jev_allowed'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 107 (org administration phase D):
/// `orgs` already has `admins_see_unclaimed`, the last of its two
/// `ADD COLUMN`s; everything else in the script is `IF NOT EXISTS`.
fn orgs_has_admins_see_unclaimed(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('orgs') WHERE name = 'admins_see_unclaimed'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 066 (work graph M14.1b): its last
/// ADD COLUMN (`client_tokens.org_id`) present means the whole migration is.
fn client_tokens_has_org(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('client_tokens') WHERE name = 'org_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 074: `client_tokens` already has
/// `assets_admin_at`, and `ALTER TABLE ... ADD COLUMN` would fail again.
fn client_tokens_has_assets_admin(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('client_tokens') WHERE name = 'assets_admin_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 098 (multi-user M1): `client_tokens`
/// already has `person_id`. That `ALTER TABLE ... ADD COLUMN` is the one
/// statement in 094 that is not idempotent — the table, both indexes and the
/// trigger are `IF NOT EXISTS`, and both writes are conditional — so the
/// column being there means the whole migration is.
fn client_tokens_has_person(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('client_tokens') WHERE name = 'person_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 101 (multi-user M1, T9d): the one
/// `ADD COLUMN` in the script. The trigger is `IF NOT EXISTS` and the
/// backfill `UPDATE`s are idempotent, so only the column needs the guard.
fn tasks_has_role(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'role'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn tasks_has_detached_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'detached_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 101 (multi-user M1): `sessions`
/// already has `visibility`, the LAST of 097's two `ADD COLUMN`s — the
/// `work_items_has_status_set_at` convention. Everything else in the script
/// is `IF NOT EXISTS` or a `DROP`/`CREATE` of the row-version trigger, and
/// the two `ADD COLUMN`s are the statements that cannot be written
/// idempotently, so the last of them being there means the whole migration
/// is. The attribution of pre-M1 rows is NOT in the script (see
/// [`Store::backfill_session_owner`]), so this guard skipping the script
/// never skips the backfill.
fn sessions_has_visibility(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'visibility'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 075 (native item status): its last
/// ADD COLUMN (`work_items.status_set_at`) present means the whole migration
/// is — the `ALTER TABLE ... ADD COLUMN`s are not idempotent on their own.
fn work_items_has_status_set_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = 'status_set_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 086 (shared work context).
fn work_items_has_origin(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = 'origin'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 091 (`catalog_id` on
/// `asset_inventory`; `host_layers` is rebuilt unconditionally by the same
/// migration, keyed off this column since `ADD COLUMN` is not idempotent).
fn asset_inventory_has_catalog_id(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'catalog_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 096 (`drift_side` on
/// `asset_inventory`, Assets M5).
fn asset_inventory_has_drift_side(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'drift_side'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 097 (`decided_at` on
/// `changeset_items`, Assets M5).
fn changeset_items_has_decided_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('changeset_items') WHERE name = 'decided_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 102 (`outcome` on
/// `changeset_items`, Assets M6).
fn changeset_items_has_outcome(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('changeset_items') WHERE name = 'outcome'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 103 (`withdrawn_at` on `changesets`,
/// Assets M6).
fn changesets_has_withdrawn_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('changesets') WHERE name = 'withdrawn_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 105: no worktree row is named with
/// a `/`. A data rewrite with nothing left to rewrite is skipped rather than
/// run, and it must be: its `UPDATE sessions` compiles every `sessions`
/// trigger, which on a database whose skipped branch migrations are not yet
/// repaired names columns that do not exist yet.
fn no_slash_named_worktrees(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM worktrees WHERE instr(name, '/') > 0)",
        [],
        |r| r.get(0),
    )
}

/// `already_applied` guard of migration 087 (`secret_like` / `fleet_owned`
/// on `asset_inventory`).
fn asset_inventory_has_fleet_owned(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'fleet_owned'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 089 (`hosts.harnesses`,
/// multi-harness F3a).
fn hosts_has_harnesses(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'harnesses'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 092.
fn hosts_has_provision_warning(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'provision_warning'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 114.
fn hosts_has_claude_profiles(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'claude_profiles'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 129.
fn grants_have_profile(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('orchestration_grants') WHERE name = 'profile'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn sessions_has_turn_outcome(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'turn_outcome'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 125.
fn sessions_has_last_viewed_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'last_viewed_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 124.
fn sessions_has_origin(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'origin'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 121.
fn sessions_has_agent(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'agent'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 113.
fn sessions_has_claude_profile(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'claude_profile'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 115.
fn work_items_has_orchestration_project(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') \
         WHERE name = 'orchestration_project_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 126.
fn host_tokens_has_rotated_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('host_tokens') WHERE name = 'rotated_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 139.
fn routine_runs_has_outcome_source(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('routine_runs') WHERE name = 'outcome_source'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 132.
fn peer_links_has_msgs_total(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('peer_links') WHERE name = 'msgs_total'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 142: `session_grants`' CHECK
/// already admits 'answer'. 142 rebuilds the table, so a re-run only records
/// the version.
fn session_grants_has_answer(conn: &Connection) -> rusqlite::Result<bool> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'session_grants'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(sql.is_some_and(|s| s.contains("'answer'")))
}

/// `already_applied` guard of migration 117.
/// `already_applied` guard of migration 134.
fn hosts_has_agents_on_path(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'agents_on_path'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 150.
fn hosts_has_last_reachable_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'last_reachable_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// already_applied guard of migration 148.
fn downloads_has_owner(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('downloads') WHERE name = 'owner_person_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn hosts_has_worktree_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'worktree_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn work_items_has_done_when(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = 'done_when'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 116.
fn work_items_has_held_at(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = 'held_at'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 111.
fn hosts_has_auth_overrides(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'auth_overrides'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 112.
fn local_workspaces_has_driver_since(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('local_workspaces') WHERE name = 'driver_since'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 067 (work graph M14.1b, D31).
fn orgs_has_bound_sees_unassigned(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('orgs') WHERE name = 'bound_sees_unassigned'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn projects_has_system(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'system'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 028: `accounts` already has BOTH its
/// `nickname` and `has_extra_usage` columns, and `ALTER TABLE ... ADD COLUMN`
/// would fail again. See [`Migration`].
fn accounts_has_nickname_and_extra_usage(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('accounts') \
         WHERE name IN ('nickname', 'has_extra_usage')",
        [],
        |r| r.get(0),
    )?;
    Ok(n >= 2)
}

/// `already_applied` guard of migration 031: `asset_inventory` already has
/// its `managed` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn asset_inventory_has_managed(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'managed'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 036: `sessions` already has its
/// `lost_reason` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn sessions_has_lost_reason(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'lost_reason'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 034: `hosts` already has its
/// `transport` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
/// See [`Migration`].
fn hosts_has_transport(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'transport'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 037: `sessions` already has its
/// `tmux_pane_id` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
fn sessions_has_tmux_pane_id(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'tmux_pane_id'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `already_applied` guard of migration 040: `sessions` already has its
/// `pending_input` column, and `ALTER TABLE ... ADD COLUMN` would fail again.
fn sessions_has_pending_input(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'pending_input'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Ordered schema migrations, `(version, sql)`. Versions are contiguous from
/// 1 and every script must end by recording its own version with
/// `INSERT OR IGNORE INTO schema_version (version) VALUES (N)` — the tests
/// enforce both. `migrate()` runs entry 0 unconditionally (it is idempotent
/// and bootstraps `schema_version`) and each later entry in its own
/// transaction iff its version is above the recorded maximum. To add one:
/// drop `NNN_name.sql` into `migrations/` and append it here.
const MIGRATIONS: &[Migration] = &[
    Migration::plain(1, include_str!("../../migrations/001_init.sql")),
    Migration::plain(2, include_str!("../../migrations/002_hosts_ssh.sql")),
    Migration::plain(3, include_str!("../../migrations/003_accounts.sql")),
    Migration::plain(4, include_str!("../../migrations/004_session_account.sql")),
    Migration::plain(5, include_str!("../../migrations/005_session_reviews.sql")),
    Migration::plain(
        6,
        include_str!("../../migrations/006_session_worktree_key.sql"),
    ),
    Migration::plain(7, include_str!("../../migrations/007_indexes.sql")),
    Migration::plain(8, include_str!("../../migrations/008_ghost_sessions.sql")),
    Migration::plain(
        9,
        include_str!("../../migrations/009_session_claude_id.sql"),
    ),
    Migration::plain(
        10,
        include_str!("../../migrations/010_claude_agent_fields.sql"),
    ),
    Migration::plain(
        11,
        include_str!("../../migrations/011_host_provisioned.sql"),
    ),
    Migration::plain(
        12,
        include_str!("../../migrations/012_session_context_pressure.sql"),
    ),
    Migration::plain(13, include_str!("../../migrations/013_session_events.sql")),
    Migration::plain(
        14,
        include_str!("../../migrations/014_last_reconciled_at.sql"),
    ),
    Migration::plain(
        15,
        include_str!("../../migrations/015_session_messages.sql"),
    ),
    Migration::plain(
        16,
        include_str!("../../migrations/016_session_friendly_name.sql"),
    ),
    Migration::plain(17, include_str!("../../migrations/017_safe_kill.sql")),
    Migration::plain(18, include_str!("../../migrations/018_host_tokens.sql")),
    Migration::plain(
        19,
        include_str!("../../migrations/019_lifecycle_fields.sql"),
    ),
    Migration::plain(20, include_str!("../../migrations/020_tasks_and_turns.sql")),
    Migration::plain(21, include_str!("../../migrations/021_repair_backoff.sql")),
    Migration::plain(
        22,
        include_str!("../../migrations/022_drop_handoff_freeze.sql"),
    ),
    Migration::plain(
        23,
        include_str!("../../migrations/023_worktree_parent_fingerprints.sql"),
    ),
    // A table rebuild cannot be written idempotently in SQL, and re-running
    // it would reset every row's host to 'local'.
    Migration {
        version: 24,
        sql: include_str!("../../migrations/024_worktree_host.sql"),
        already_applied: Some(worktrees_has_host_alias),
    },
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 25,
        sql: include_str!("../../migrations/025_session_usage.sql"),
        already_applied: Some(sessions_has_usage_columns),
    },
    Migration {
        version: 26,
        sql: include_str!("../../migrations/026_worktree_updated_at.sql"),
        already_applied: Some(worktrees_has_updated_at),
    },
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 27,
        sql: include_str!("../../migrations/027_project_adopted.sql"),
        already_applied: Some(projects_has_adopted),
    },
    // `ALTER TABLE ... ADD COLUMN` fails if either column is already there.
    Migration {
        version: 28,
        sql: include_str!("../../migrations/028_account_nickname.sql"),
        already_applied: Some(accounts_has_nickname_and_extra_usage),
    },
    // `CREATE TABLE IF NOT EXISTS` is safe to run as written on a re-run.
    Migration::plain(
        29,
        include_str!("../../migrations/029_dismissed_agents.sql"),
    ),
    // Asset catalog: two fresh `CREATE TABLE IF NOT EXISTS`, safe to re-run.
    Migration::plain(30, include_str!("../../migrations/030_asset_catalog.sql")),
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 31,
        sql: include_str!("../../migrations/031_asset_sync.sql"),
        already_applied: Some(asset_inventory_has_managed),
    },
    // `CREATE TABLE IF NOT EXISTS` plus two `CREATE UNIQUE INDEX IF NOT
    // EXISTS`, safe to re-run.
    Migration::plain(32, include_str!("../../migrations/032_client_tokens.sql")),
    // `CREATE TABLE IF NOT EXISTS` + `CREATE UNIQUE INDEX IF NOT EXISTS`,
    // safe to re-run.
    Migration::plain(33, include_str!("../../migrations/033_asset_layers.sql")),
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 34,
        sql: include_str!("../../migrations/034_host_transport.sql"),
        already_applied: Some(hosts_has_transport),
    },
    // Re-runs 033's layer DDL for databases the pre-merge host-agent branch
    // created, which recorded 33 for a different migration and so never get
    // offered 033. `IF NOT EXISTS` throughout, safe to re-run.
    Migration::plain(
        35,
        include_str!("../../migrations/035_host_layers_repair.sql"),
    ),
    // Adds the reboot safety net's columns (`sessions.lost_reason`, the host
    // boot identity). Guarded like 034: `ALTER TABLE ... ADD COLUMN` fails if
    // the column is already there.
    Migration {
        version: 36,
        sql: include_str!("../../migrations/036_host_boot_identity.sql"),
        already_applied: Some(sessions_has_lost_reason),
    },
    // Conversations table + `sessions.tmux_pane_id`; the ADD COLUMN needs
    // the same guard as 034.
    Migration {
        version: 37,
        sql: include_str!("../../migrations/037_conversations.sql"),
        already_applied: Some(sessions_has_tmux_pane_id),
    },
    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there.
    Migration {
        version: 38,
        sql: include_str!("../../migrations/038_project_system.sql"),
        already_applied: Some(projects_has_system),
    },
    // `client_tokens.trusted_at`; ADD COLUMN, so the same guard as 038.
    Migration {
        version: 39,
        sql: include_str!("../../migrations/039_client_trust.sql"),
        already_applied: Some(client_tokens_has_trusted_at),
    },
    // `sessions.pending_input`; ADD COLUMN, so the same guard as 039.
    Migration {
        version: 40,
        sql: include_str!("../../migrations/040_pending_input.sql"),
        already_applied: Some(sessions_has_pending_input),
    },
    // `CREATE TABLE IF NOT EXISTS` plus two `CREATE INDEX IF NOT EXISTS`,
    // safe to re-run.
    Migration::plain(41, include_str!("../../migrations/041_error_reports.sql")),
    // `sessions.row_version` (+ trigger) and `sessions.prompt_submit_seq`;
    // ADD COLUMN, so the same guard as 038/039/040.
    Migration {
        version: 42,
        sql: include_str!("../../migrations/042_row_version_and_prompt_ack.sql"),
        already_applied: Some(sessions_has_row_version),
    },
    // `participants` (CREATE TABLE IF NOT EXISTS, re-runnable) plus four
    // ADD COLUMNs, which are not — so the same guard shape as 038-042.
    Migration {
        version: 43,
        sql: include_str!("../../migrations/043_participants.sql"),
        already_applied: Some(messages_have_participant_columns),
    },
    // `read_cursors` (smart caching, cycle 2): CREATE TABLE / INDEX IF NOT
    // EXISTS only, so re-running it is a no-op — no `already_applied` guard.
    Migration::plain(44, include_str!("../../migrations/044_read_cursors.sql")),
    // A participant for every session row, minted by trigger on INSERT, plus
    // a backfill. `CREATE TRIGGER IF NOT EXISTS` and an `INSERT ... WHERE
    // NOT IN`, so re-running it is a no-op — no `already_applied` guard.
    Migration::plain(
        45,
        include_str!("../../migrations/045_session_participants.sql"),
    ),
    // Work graph M1b: `work_items`, `work_links` and the trigger that ends a
    // link (with its snapshot) when the session's participant retires. All
    // `IF NOT EXISTS`, so re-running it is a no-op.
    Migration::plain(46, include_str!("../../migrations/046_work_graph.sql")),
    // Work graph M2: `work_journal` (+ its conversation triggers), and
    // `work_links.role` / `.resumable` — ADD COLUMNs, so the 038-043 guard.
    Migration {
        version: 47,
        sql: include_str!("../../migrations/047_work_journal.sql"),
        already_applied: Some(work_links_has_role),
    },
    // Work graph M3: `trackers`, `tracker_secrets`, `tracker_views` and the
    // tracker columns of `work_items` — ADD COLUMNs, so the 038-047 guard.
    Migration {
        version: 48,
        sql: include_str!("../../migrations/048_trackers.sql"),
        already_applied: Some(work_items_has_aliases),
    },
    // Work graph M4: the live branch and PR signals on `sessions`, and the
    // explanation columns of `work_links` — ADD COLUMNs, so the same guard.
    Migration {
        version: 49,
        sql: include_str!("../../migrations/049_work_detection.sql"),
        already_applied: Some(work_links_has_evidence),
    },
    // Work graph M5: `orgs`, `org_rules`, `hosts.org_id`,
    // `work_links.snap_org_id` and its retirement trigger — ADD COLUMNs, so
    // the same guard.
    Migration {
        version: 50,
        sql: include_str!("../../migrations/050_orgs.sql"),
        already_applied: Some(work_links_has_snap_org),
    },
    // Work graph M6: `tracker_views.sync_mark` (sync tokens) and
    // `trackers.settings` (admin-owned provider settings) — ADD COLUMNs, so
    // the same guard. (Written as 050 on the M6 branch; renumbered when M5,
    // which took 050, was merged.)
    Migration {
        version: 51,
        sql: include_str!("../../migrations/051_tracker_providers.sql"),
        already_applied: Some(trackers_has_settings),
    },
    // Work graph M7: archive / snooze / never on `work_links`, the last
    // touch on `sessions`, `work_items.reopened_at` — ADD COLUMNs, so the
    // same guard. (Written as 051 and 052 on the M7 branch; renumbered when
    // M6, which took 051, was merged.)
    Migration {
        version: 52,
        sql: include_str!("../../migrations/052_work_lifecycle.sql"),
        already_applied: Some(work_links_has_archived_at),
    },
    // Work graph M7 on M5: `orgs.auto_tidy` — one ADD COLUMN, its own guard.
    Migration {
        version: 53,
        sql: include_str!("../../migrations/053_org_auto_tidy.sql"),
        already_applied: Some(orgs_has_auto_tidy),
    },
    // Hub↔hub federation (cycle 3): `peer_links`, remote participants, and the
    // remote / outbox columns on `session_messages`. Guarded: ADD COLUMN.
    Migration {
        version: 54,
        sql: include_str!("../../migrations/054_peer_links.sql"),
        already_applied: Some(participants_have_address),
    },
    // `participants(peer_link_id)` index (federation review G17):
    // `CREATE INDEX IF NOT EXISTS`, safe to re-run.
    Migration::plain(55, include_str!("../../migrations/055_peer_link_index.sql")),
    // Work graph M4.6: `conversations.classify_nudged_at`, the once-per-
    // conversation stamp of the classification nudge. Guarded: ADD COLUMN.
    Migration {
        version: 56,
        sql: include_str!("../../migrations/056_classify_nudge.sql"),
        already_applied: Some(conversations_have_nudge_stamp),
    },
    // Re-issues 044's `trg_read_cursors_on_session_delete` for a database
    // migrated by an intermediate build of 044 that lacked it:
    // `CREATE TRIGGER IF NOT EXISTS`, safe to re-run.
    Migration::plain(
        57,
        include_str!("../../migrations/057_read_cursors_trigger.sql"),
    ),
    // Work graph M12.2 (scale): `work_links(ended_at)` and the handover
    // loop guard's `work_journal(participant_id)`. `CREATE INDEX IF NOT
    // EXISTS`, safe to re-run.
    Migration::plain(
        58,
        include_str!("../../migrations/058_work_graph_scale_indexes.sql"),
    ),
    // `sessions(worktree_id)` partial index for live rows (hub store latency,
    // task 6): `CREATE INDEX IF NOT EXISTS`, safe to re-run.
    Migration::plain(
        59,
        include_str!("../../migrations/059_session_worktree_index.sql"),
    ),
    // The auth epoch and its triggers on the token tables (hub store
    // latency, task 7): `IF NOT EXISTS` / `OR IGNORE`, safe to re-run.
    Migration::plain(60, include_str!("../../migrations/060_auth_epoch.sql")),
    // Work graph M13.4e: the tracker write-back outbox. A new table and two
    // indexes, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(61, include_str!("../../migrations/061_tracker_writes.sql")),
    // Work graph M13.4f: a tracker's webhook secret. A new table,
    // `IF NOT EXISTS`, safe to re-run. The feature was removed again (D13
    // stays no); the entry stays so a database that ran it is not refused
    // as newer, and 064 drops the table.
    Migration::plain(
        62,
        include_str!("../../migrations/062_tracker_webhooks.sql"),
    ),
    // `sessions_row_version_bump` rebuilt to bump only on a change to a
    // watched column (not the reconcile's per-pass `last_reconciled_at`
    // stamp). DROP + CREATE of a trigger, no row touched: safe to re-run.
    Migration::plain(
        63,
        include_str!("../../migrations/063_row_version_on_visible_change.sql"),
    ),
    // Drops 062's `tracker_webhooks`: nothing reads it since the webhook
    // nudges were removed, and a secret no code can rotate must not stay.
    Migration::plain(
        64,
        include_str!("../../migrations/064_drop_tracker_webhooks.sql"),
    ),
    // Lifecycle F2: `sessions.stale_working_at` (one ADD COLUMN, its own
    // guard) and 063's `sessions_row_version_bump` rebuilt to watch it —
    // it is a `SessionRow` field, so a change to it must bump `row_version`.
    Migration {
        version: 65,
        sql: include_str!("../../migrations/065_stale_working.sql"),
        already_applied: Some(sessions_has_stale_working_at),
    },
    // Work graph M14.1b: the Work view's reads (link versions, placements,
    // placement rules, saved views, a local item's org, a conflict's review
    // ack, org-bound paired clients). `ALTER TABLE ... ADD COLUMN` fails if
    // the column is already there.
    Migration {
        version: 66,
        sql: include_str!("../../migrations/066_work_view.sql"),
        already_applied: Some(client_tokens_has_org),
    },
    // D31: `orgs.bound_sees_unassigned` (and its auth-epoch trigger). An
    // ADD COLUMN, guarded like 053's.
    Migration {
        version: 67,
        sql: include_str!("../../migrations/067_org_bound_sees_unassigned.sql"),
        already_applied: Some(orgs_has_bound_sees_unassigned),
    },
    // Jev evaluation (D31 / D36): `orgs.jev_allowed`, an org's consent to
    // decision-model calls. One ADD COLUMN, its own guard.
    Migration {
        version: 68,
        sql: include_str!("../../migrations/068_org_jev_allowed.sql"),
        already_applied: Some(orgs_has_jev_allowed),
    },
    // Jev evaluation (D35 / D37): `decision_runs` and `decision_secrets`.
    // New tables and indexes, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(69, include_str!("../../migrations/069_decision_runs.sql")),
    // D34 label hygiene: `work_unlinks`, a person's "Clear work" held
    // against the unchanged state signal (R9u). A new table, index and
    // trigger, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(70, include_str!("../../migrations/070_work_unlinks.sql")),
    // usage_daily keyed by (day, host_alias, backfill): a table rebuild, so
    // guarded — re-running the INSERT…SELECT would collapse backfill rows.
    Migration {
        version: 71,
        sql: include_str!("../../migrations/071_usage_daily_backfill.sql"),
        already_applied: Some(usage_daily_has_backfill),
    },
    // `sessions.usage_backfill_until`: a multi-pass first read of a large
    // transcript books every chunk's history as backfill, not only the
    // first. One ADD COLUMN, its own guard.
    Migration {
        version: 72,
        sql: include_str!("../../migrations/072_usage_backfill_until.sql"),
        already_applied: Some(sessions_has_usage_backfill_until),
    },
    // Task 5: the describe cache (`work_item_descriptions`) — one item's
    // whole description, held for `work.describe_cache_secs`. `CREATE TABLE
    // IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` are idempotent on their
    // own, so this needs no `already_applied` guard.
    Migration::plain(73, include_str!("../../migrations/073_describe_cache.sql")),
    // `client_tokens.assets_admin_at` (a client allowed `catalog_admin`)
    // and its auth-epoch trigger. One ADD COLUMN, its own guard.
    Migration {
        version: 74,
        sql: include_str!("../../migrations/074_client_assets_admin.sql"),
        already_applied: Some(client_tokens_has_assets_admin),
    },
    // `sessions.launch_model`: the `claude --model` recreate / restart pass
    // again. One ADD COLUMN, its own guard.
    Migration {
        version: 75,
        sql: include_str!("../../migrations/075_session_launch_model.sql"),
        already_applied: Some(sessions_has_launch_model),
    },
    // Host identity & health, task 1: `hosts.claude_version_at`. Guarded:
    // ADD COLUMN. (Numbered at merge time — `migrations_are_contiguous_from_one`
    // allows no gap — so a sibling plan merged first shifts these.)
    Migration {
        version: 76,
        sql: include_str!("../../migrations/076_host_claude_version_at.sql"),
        already_applied: Some(hosts_has_claude_version_at),
    },
    // Host identity & health, task 2: the per-pass health sample, the last
    // accepted hook and the agent version on `hosts`. Guarded: ADD COLUMN.
    Migration {
        version: 77,
        sql: include_str!("../../migrations/077_host_health.sql"),
        already_applied: Some(hosts_has_health_at),
    },
    // Host identity & health, task 6: the provisioning content fingerprint
    // and its stamp on `hosts`. Guarded: ADD COLUMN.
    Migration {
        version: 78,
        sql: include_str!("../../migrations/078_host_provision_fingerprint.sql"),
        already_applied: Some(hosts_has_provision_fingerprint),
    },
    // Application updates (S4): desired / observed / events / the signed
    // document cache. `CREATE TABLE IF NOT EXISTS` only.
    Migration::plain(79, include_str!("../../migrations/079_update_state.sql")),
    // Stale-working acknowledgement: `sessions.stale_demoted_at`, the
    // reconcile veto's memory apart from the attention stamp (one ADD
    // COLUMN, its own guard; the backfill is `backfill_stale_demoted`,
    // after the collision repair). Read into `SessionRow` but never
    // serialized nor compared, so 065's row_version trigger (and 082's
    // rebuild of it) does not watch it.
    Migration {
        version: 80,
        sql: include_str!("../../migrations/080_stale_demoted.sql"),
        already_applied: Some(sessions_has_stale_demoted_at),
    },
    // `sessions.pane_working_at`: the stale-working sweep's evidence that
    // the pane still shows a live turn, so one long tool call is not
    // demoted every tick. One ADD COLUMN, its own guard.
    Migration {
        version: 81,
        sql: include_str!("../../migrations/081_pane_working_at.sql"),
        already_applied: Some(sessions_has_pane_working_at),
    },
    // Result evidence: `sessions.pr_evidence` / `pr_checked_at` (two ADD
    // COLUMNs, one guard) and 065's `sessions_row_version_bump` rebuilt to
    // watch them: both are `SessionRow` fields.
    Migration {
        version: 82,
        sql: include_str!("../../migrations/082_result_evidence.sql"),
        already_applied: Some(sessions_has_pr_evidence),
    },
    // Declarative pages P5: `setting_proposals` and `setting_audit`. New
    // tables and indexes, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(83, include_str!("../../migrations/083_setting_review.sql")),
    // Native item status (design 2026-09-28 §2): `status_set_by` /
    // `status_set_at`. The `ADD COLUMN`s are not idempotent (unlike the
    // partial index), so this needs the same guard 072/074's ADD COLUMNs use.
    Migration {
        version: 84,
        sql: include_str!("../../migrations/084_native_item_status.sql"),
        already_applied: Some(work_items_has_status_set_at),
    },
    // An index on `work_unlinks.item_id` for its `work_items` cascade.
    Migration::plain(
        85,
        include_str!("../../migrations/085_work_unlinks_item_index.sql"),
    ),
    // Multi-user M1's four scripts were written as 086-089, renumbered to
    // 094-097 at the first `main` merge, and RENUMBERED AGAIN to 096-099 at
    // the second: `main` had meanwhile shipped 086-093 (shared work context,
    // inventory flags, guides, host harnesses, the catalogs) and then 094
    // (changesets) and 095 (downloads). The scripts themselves are unchanged
    // — every set touches disjoint tables (M1: `people`, `sessions`,
    // `session_grants`, `tasks`) — so the renumber is the whole of it, and
    // M1 deliberately has NO arm in `repair_skipped_main_migrations` below:
    // no `fleet-hub` binary was ever built from an M1 worktree and the branch
    // was first pushed on 2026-10-04, so no database in the wild has ever
    // recorded 86-89 or 94-97 for these scripts. There is nothing to repair.
    // Shared work context (design 2026-09-29): origin, project, notes, job
    // and proposal columns on `work_items`. The ADD COLUMNs are not
    // idempotent, so the same guard 084 uses; the backfill and indexes are.
    Migration {
        version: 86,
        sql: include_str!("../../migrations/086_shared_work_context.sql"),
        already_applied: Some(work_items_has_origin),
    },
    // Assets S1a: `secret_like` / `fleet_owned` on `asset_inventory`.
    Migration {
        version: 87,
        sql: include_str!("../../migrations/087_inventory_flags.sql"),
        already_applied: Some(asset_inventory_has_fleet_owned),
    },
    // Declarative pages, guides: `guide_proposals`. A new table and index,
    // `IF NOT EXISTS`, safe to re-run.
    Migration::plain(88, include_str!("../../migrations/088_guides.sql")),
    // Multi-harness F3a: which harnesses the asset catalog syncs on a host
    // (NULL = auto). ADD COLUMN is not idempotent: the same guard 087 uses.
    Migration {
        version: 89,
        sql: include_str!("../../migrations/089_host_harnesses.sql"),
        already_applied: Some(hosts_has_harnesses),
    },
    // Assets S1b M1: `catalogs`, backfilled from `catalog_config` as
    // `personal`. CREATE IF NOT EXISTS + INSERT OR IGNORE: safe to re-run.
    Migration::plain(90, include_str!("../../migrations/090_catalogs.sql")),
    // Assets S1b M2: `catalog_id` on `host_layers` (rebuilt) and
    // `asset_inventory`.
    Migration {
        version: 91,
        sql: include_str!("../../migrations/091_catalog_ids.sql"),
        already_applied: Some(asset_inventory_has_catalog_id),
    },
    // The warning a degraded provisioning left behind, so it survives the
    // call that produced it. ADD COLUMN is not idempotent: the same guard
    // 087 and 089 use.
    Migration {
        version: 92,
        sql: include_str!("../../migrations/092_host_provision_warning.sql"),
        already_applied: Some(hosts_has_provision_warning),
    },
    // Assets S1b M3: `host_catalogs` (admissions) and `client_catalog_grants`
    // (personal backfilled from `assets_admin_at`). IF NOT EXISTS + INSERT
    // OR IGNORE: safe to re-run.
    Migration::plain(93, include_str!("../../migrations/093_catalog_access.sql")),
    // Assets S1b+S2 M4: changeset cards, their items, triage verdicts (the
    // spec's DDL verbatim). CREATE IF NOT EXISTS: safe to re-run.
    Migration::plain(94, include_str!("../../migrations/094_changesets.sql")),
    // File downloads: `downloads` (a file a session sent from its host,
    // copied to the data dir). CREATE IF NOT EXISTS: safe to re-run.
    Migration::plain(95, include_str!("../../migrations/095_downloads.sql")),
    // Assets M5: which side moved on a drifted managed row. ADD COLUMN is
    // not idempotent: the same guard 087 uses.
    Migration {
        version: 96,
        sql: include_str!("../../migrations/096_inventory_drift_side.sql"),
        already_applied: Some(asset_inventory_has_drift_side),
    },
    // Assets M5: when a changeset item was decided. ADD COLUMN is not
    // idempotent: guarded.
    Migration {
        version: 97,
        sql: include_str!("../../migrations/097_changeset_item_decided_at.sql"),
        already_applied: Some(changeset_items_has_decided_at),
    },
    // Multi-user M1 (T1): `people`, `client_tokens.person_id` with its own
    // narrow auth-epoch trigger, this hub's personal owner, and the backfill
    // that leaves no live device person-less. Guarded: the ADD COLUMN is the
    // one statement here that is not idempotent.
    Migration {
        version: 98,
        sql: include_str!("../../migrations/098_people.sql"),
        already_applied: Some(client_tokens_has_person),
    },
    // Multi-user M1 (T3): `sessions.owner_person_id` / `visibility`,
    // `idx_sessions_owner`, the durable `conversation_owners` record with
    // the two triggers that fill it, and 082's `sessions_row_version_bump`
    // re-issued to watch both new `SessionRow` fields. Guarded: the second
    // ADD COLUMN. The attribution of pre-M1 rows is deliberately NOT in the
    // script — it is `backfill_session_owner`, after the collision repair,
    // for migration 080's reason.
    Migration {
        version: 99,
        sql: include_str!("../../migrations/099_session_owner.sql"),
        already_applied: Some(sessions_has_visibility),
    },
    // Multi-user M1 (T4): `session_grants` — the owner's explicit, revocable,
    // downward-only share — with its NULL-safe one-live-grant index, the hot
    // per-person read and the index the session cascade needs. Plain: every
    // statement is `IF NOT EXISTS` and there is no ADD COLUMN, so re-running
    // the script changes nothing. The rules it cannot express as constraints
    // are in `store/session_grants.rs`.
    Migration::plain(100, include_str!("../../migrations/100_session_grants.sql")),
    // Multi-user M1 (T9d): `tasks.detached_at` plus the `AFTER DELETE ON
    // sessions` trigger that NULLs a reaped session's id out of both ends and
    // stamps the task, so a recycled `sessions.id` can never make another
    // person's task read as theirs. Guarded: the ADD COLUMN.
    Migration {
        version: 101,
        sql: include_str!("../../migrations/101_tasks_detach.sql"),
        already_applied: Some(tasks_has_detached_at),
    },
    // Assets M6's two scripts were written as 098-099 and renumbered to
    // 102-103 at the `main` merge that brought multi-user M1's 098-101; the
    // scripts are unchanged (M6 touches only `changeset_items` and
    // `changesets`).
    //
    // Assets M6: what a host-writing card left undone on an item's host.
    // ADD COLUMN is not idempotent: guarded.
    Migration {
        version: 102,
        sql: include_str!("../../migrations/102_changeset_item_outcome.sql"),
        already_applied: Some(changeset_items_has_outcome),
    },
    // Assets M6: when the system withdrew a card, so it is pruned a week
    // after withdrawal. ADD COLUMN is not idempotent: guarded.
    Migration {
        version: 103,
        sql: include_str!("../../migrations/103_changeset_withdrawn_at.sql"),
        already_applied: Some(changesets_has_withdrawn_at),
    },
    // The New session picker: a person's pin / visibility / group per
    // project, keyed by owner/repo TEXT. A new table only, so plain.
    Migration::plain(104, include_str!("../../migrations/104_project_picks.sql")),
    // Worktree rows named after a branch with a `/` (`feat/imports`) take a
    // flat name (`feat-imports`), and so do their sessions' keys. Rewrites
    // rows only; guarded so it runs only while such a row exists.
    Migration {
        version: 105,
        sql: include_str!("../../migrations/105_worktree_slash_names.sql"),
        already_applied: Some(no_slash_named_worktrees),
    },
    Migration::plain(
        106,
        include_str!("../../migrations/106_org_settings_and_spend.sql"),
    ),
    // Org administration phase D: memberships, the company that owns the
    // hub, and the unclaimed-count switch. ADD COLUMN is not idempotent:
    // guarded on the last one.
    Migration {
        version: 107,
        sql: include_str!("../../migrations/107_org_members.sql"),
        already_applied: Some(orgs_has_admins_see_unclaimed),
    },
    // Sprints and releases (design 2026-09-28 §1): three new tables, so
    // plain.
    Migration::plain(108, include_str!("../../migrations/108_work_buckets.sql")),
    // Local workspace sync, Phase 1: three new tables, so plain.
    Migration::plain(
        109,
        include_str!("../../migrations/109_local_workspaces.sql"),
    ),
    // Orchestration O0: a task names the work item it is an attempt at, with
    // its attempt number and role. ADD COLUMN is not idempotent: guarded on
    // the last one.
    Migration {
        version: 110,
        sql: include_str!("../../migrations/110_task_runs.sql"),
        already_applied: Some(tasks_has_role),
    },
    // Auth-override names per host (multi-account groundwork): one ADD
    // COLUMN, guarded.
    Migration {
        version: 111,
        sql: include_str!("../../migrations/111_host_auth_overrides.sql"),
        already_applied: Some(hosts_has_auth_overrides),
    },
    // Local workspace, Phases 2 and 3: who drives the worktree (two ADD
    // COLUMNs, guarded on the last) and the per-path activity log.
    Migration {
        version: 112,
        sql: include_str!("../../migrations/112_local_workspace_handoff.sql"),
        already_applied: Some(local_workspaces_has_driver_since),
    },
    // A session's credential profile (multi-account phase 2): one ADD
    // COLUMN, guarded, and the row-version trigger re-issued to watch it.
    Migration {
        version: 113,
        sql: include_str!("../../migrations/113_session_claude_profile.sql"),
        already_applied: Some(sessions_has_claude_profile),
    },
    // A host's login profiles and their accounts: one ADD COLUMN, guarded.
    Migration {
        version: 114,
        sql: include_str!("../../migrations/114_host_claude_profiles.sql"),
        already_applied: Some(hosts_has_claude_profiles),
    },
    // Orchestration O1: the mission container, its repos and its event log
    // (new tables), and work_items.orchestration_project_id (ADD COLUMN,
    // so guarded on it).
    Migration {
        version: 115,
        sql: include_str!("../../migrations/115_orchestration_projects.sql"),
        already_applied: Some(work_items_has_orchestration_project),
    },
    // Orchestration O2: the dependency graph (a new table) and a person's
    // hold on an item (ADD COLUMN, so guarded on it).
    Migration {
        version: 116,
        sql: include_str!("../../migrations/116_work_item_deps.sql"),
        already_applied: Some(work_items_has_held_at),
    },
    // Orchestration O3: a task's report and git evidence, an item's
    // done_when (three ADD COLUMNs, guarded on the last) and the journal of
    // a person's checks (a new table).
    Migration {
        version: 117,
        sql: include_str!("../../migrations/117_task_evidence.sql"),
        already_applied: Some(work_items_has_done_when),
    },
    // Orchestration O4–O6: the loop's cards and a person's grants (new
    // tables only, so idempotent).
    Migration::plain(
        118,
        include_str!("../../migrations/118_orchestration_loop.sql"),
    ),
    // Chat forms: `form_requests`. A new table and indexes, `IF NOT EXISTS`,
    // safe to re-run.
    Migration::plain(119, include_str!("../../migrations/119_form_requests.sql")),
    // Debug devices: `debug_devices`, `debug_device_scans` and a host-delete
    // trigger. New objects only, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(120, include_str!("../../migrations/120_debug_devices.sql")),
    // Orbit Fleet 2.1: which agent runs in a session (one ADD COLUMN,
    // guarded, a shell backfill, and the row-version trigger re-issued).
    Migration {
        version: 121,
        sql: include_str!("../../migrations/121_session_agent.sql"),
        already_applied: Some(sessions_has_agent),
    },
    // Account usage history: `account_usage_snapshots` and its index. New
    // objects only, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(
        122,
        include_str!("../../migrations/122_account_usage_snapshots.sql"),
    ),
    // Orbit Fleet 4.6, the Hosts page: CPU, total memory, boot time,
    // latency and worktree size on `hosts` (six ADD COLUMNs, guarded on the
    // last).
    Migration {
        version: 123,
        sql: include_str!("../../migrations/123_host_probe_facts.sql"),
        already_applied: Some(hosts_has_worktree_at),
    },
    // Orbit Fleet 2.2: who or what started a session (two ADD COLUMNs,
    // guarded, and the row-version trigger re-issued). No backfill.
    Migration {
        version: 124,
        sql: include_str!("../../migrations/124_session_origin.sql"),
        already_applied: Some(sessions_has_origin),
    },
    // Orbit Fleet 2.3: when a person last looked at a session (one ADD
    // COLUMN, guarded, every row backfilled as viewed now, and the
    // row-version trigger re-issued).
    Migration {
        version: 125,
        sql: include_str!("../../migrations/125_session_last_viewed.sql"),
        already_applied: Some(sessions_has_last_viewed_at),
    },
    // Orbit Fleet 11.4, the Control API tokens table: `last_used_at` and
    // `rotated_at` on `host_tokens` (two ADD COLUMNs, guarded on the last),
    // and its update trigger narrowed to leave liveness alone.
    Migration {
        version: 126,
        sql: include_str!("../../migrations/126_host_token_use.sql"),
        already_applied: Some(host_tokens_has_rotated_at),
    },
    // Orbit Fleet 8.2: `aux_usage`, the cost of fleet's own `claude -p`
    // runs. A new table and indexes, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(127, include_str!("../../migrations/127_aux_usage.sql")),
    // Pull requests (redesign 6.4): `pull_requests` and two indexes. New
    // objects only, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(128, include_str!("../../migrations/128_pull_requests.sql")),
    // Orbit Fleet 2.8: a finished turn's outcome (J2) and the subject index
    // proposals are read through (one ADD COLUMN, guarded, an index, and the
    // row-version trigger re-issued).
    Migration {
        version: 129,
        sql: include_str!("../../migrations/129_session_turn_outcome.sql"),
        already_applied: Some(sessions_has_turn_outcome),
    },
    // Orbit Fleet 4.2, cost per account and per model:
    // `usage_daily_account` and its index. New objects only, `IF NOT
    // EXISTS`, safe to re-run.
    Migration::plain(
        130,
        include_str!("../../migrations/130_usage_daily_account.sql"),
    ),
    // Orbit Fleet 8.5: routines and their runs (two new tables).
    Migration::plain(131, include_str!("../../migrations/131_routines.sql")),
    // Orbit Fleet 11.5, the Federation page: a link's latency and message
    // counts on `peer_links` (four ADD COLUMNs, guarded on the last).
    Migration {
        version: 132,
        sql: include_str!("../../migrations/132_peer_link_traffic.sql"),
        already_applied: Some(peer_links_has_msgs_total),
    },
    // Orbit Fleet 5.10, Send prompt: `deferred_prompts`, prompts typed in
    // once a busy session is idle. New objects only, `IF NOT EXISTS`, safe
    // to re-run.
    Migration::plain(
        133,
        include_str!("../../migrations/133_deferred_prompts.sql"),
    ),
    // Orbit Fleet 12.4: which agent CLIs a host has on its PATH (one ADD
    // COLUMN, guarded).
    Migration {
        version: 134,
        sql: include_str!("../../migrations/134_host_agents_on_path.sql"),
        already_applied: Some(hosts_has_agents_on_path),
    },
    // Orbit Fleet 4.9: the add-host wizard's saved drafts (`host_setups`)
    // and the fleet-agent install jobs (`agent_installs`). New tables only,
    // `IF NOT EXISTS`, safe to re-run.
    Migration::plain(135, include_str!("../../migrations/135_host_setup.sql")),
    // PR shepherd: a person's standing rule per project and one row per
    // problem the shepherd saw on a session's PR (two CREATE TABLE IF NOT
    // EXISTS, idempotent as written).
    Migration::plain(136, include_str!("../../migrations/136_pr_shepherd.sql")),
    // Orbit Fleet 8.7: a mission grant names its login (one ADD COLUMN,
    // guarded).
    Migration {
        version: 137,
        sql: include_str!("../../migrations/137_grant_profile.sql"),
        already_applied: Some(grants_have_profile),
    },
    // Orbit Fleet 11.8: `usage_daily_person`, an org's spend by person. A
    // new table, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(
        138,
        include_str!("../../migrations/138_usage_daily_person.sql"),
    ),
    // Orbit Fleet 8.10: what a routine run came to (two ADD COLUMNs,
    // guarded on the last).
    Migration {
        version: 139,
        sql: include_str!("../../migrations/139_routine_run_outcome.sql"),
        already_applied: Some(routine_runs_has_outcome_source),
    },
    // Orbit Fleet 9.3: `control_handoffs`, what Control's agent sent where.
    // A new table only, `IF NOT EXISTS`, safe to re-run.
    Migration::plain(
        140,
        include_str!("../../migrations/140_control_handoffs.sql"),
    ),
    // Orbit Fleet 8.3: the indexes behind `runs { list }` (one list over
    // tasks, mission actions, Jev, `claude -p` and routine runs). Indexes only,
    // `IF NOT EXISTS`, safe to re-run.
    Migration::plain(141, include_str!("../../migrations/141_runs_indexes.sql")),
    // Orbit Fleet 11.7, the Answer share level: `session_grants.level`'s
    // CHECK gains 'answer'. A table rebuild, so guarded.
    Migration {
        version: 142,
        sql: include_str!("../../migrations/142_share_level_answer.sql"),
        already_applied: Some(session_grants_has_answer),
    },
    // Orbit Fleet 9.7: Control's Library indexes the files a person put on
    // a host (one CREATE TABLE IF NOT EXISTS, idempotent as written).
    Migration::plain(143, include_str!("../../migrations/143_library_items.sql")),
    // Orbit Fleet 8.11, from AI to rule: `start_rules`, the key patterns
    // that pick a start's project before history and Jev. New objects only.
    Migration::plain(144, include_str!("../../migrations/144_start_rules.sql")),
    // PR shepherd, step 3: the merge queue's record (a new table and an
    // index, `IF NOT EXISTS`, safe to re-run).
    Migration::plain(
        145,
        include_str!("../../migrations/145_pr_shepherd_merges.sql"),
    ),
    // Review r02: routines whose project or host is gone are paused with
    // the reason, and a partial index serves the scheduler's scan of runs
    // without an outcome. Idempotent as written (the UPDATEs match only
    // enabled orphans; the index is `IF NOT EXISTS`).
    Migration::plain(
        146,
        include_str!("../../migrations/146_routine_orphans.sql"),
    ),
    // Review r16: a partial index on the live org grants, for the org half
    // of `grants_for_person` (`IF NOT EXISTS`, safe to re-run).
    Migration::plain(
        147,
        include_str!("../../migrations/147_session_grants_org.sql"),
    ),
    // Review r04 F3: Library items and downloads record their owner, so a
    // reaped session's files stay that person's. Two ALTERs, so guarded.
    Migration {
        version: 148,
        sql: include_str!("../../migrations/148_file_owner.sql"),
        already_applied: Some(downloads_has_owner),
    },
    // Redesign: a background agent's "stop after" limits, enforced by the
    // reconcile tick (`service::bg_sessions::spawn_enforce_stop_limits`).
    // `IF NOT EXISTS`, safe to re-run.
    Migration::plain(149, include_str!("../../migrations/149_bg_stop_limits.sql")),
    // Review r13: the host row's last probe error and last-reachable stamp
    // (three ADD COLUMNs, guarded on the first).
    Migration {
        version: 150,
        sql: include_str!("../../migrations/150_host_probe_error.sql"),
        already_applied: Some(hosts_has_last_reachable_at),
    },
    // Update design S9: staged rollouts, one active per component.
    // `IF NOT EXISTS`, safe to re-run.
    Migration::plain(
        151,
        include_str!("../../migrations/151_update_rollouts.sql"),
    ),
    // Update design S9: per-org update policy. `IF NOT EXISTS`, safe to re-run.
    Migration::plain(
        152,
        include_str!("../../migrations/152_update_org_policy.sql"),
    ),
    // Redesign 10.12: a chat form while its agent is still writing it
    // (`ask { draft }`). `IF NOT EXISTS`, safe to re-run.
    Migration::plain(153, include_str!("../../migrations/153_form_drafts.sql")),
    // M15 step G1.8: Send later's time choices on `deferred_prompts` — ADD
    // COLUMNs, so a guard.
    Migration {
        version: 155,
        sql: include_str!("../../migrations/155_deferred_prompt_timing.sql"),
        already_applied: Some(deferred_prompts_has_not_before),
    },
];

/// One schema migration. `already_applied`, when set, reports whether the
/// migration's change is already in the schema, for a migration that cannot
/// be written idempotently in SQL. Tests roll the recorded version back and
/// migrate again, which re-runs every later migration; such a migration is
/// then only recorded, not re-run. `None` (the usual case) always runs it.
#[derive(Clone, Copy)]
struct Migration {
    version: i64,
    sql: &'static str,
    already_applied: Option<fn(&Connection) -> rusqlite::Result<bool>>,
}

impl Migration {
    /// A migration that is safe to run as written.
    const fn plain(version: i64, sql: &'static str) -> Self {
        Migration {
            version,
            sql,
            already_applied: None,
        }
    }
}

/// The schema version a fully migrated database reports. `pub(crate)` (not
/// `pub`) so other crate-internal tests — e.g. `service::health`'s — can
/// assert against the authoritative value instead of a literal that rots on
/// every new migration; re-exported from `store::mod` since `schema` itself
/// is a private submodule.
#[cfg(test)]
pub(crate) const LATEST_SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].version;

/// The newest schema this build knows: the downgrade guard's bound.
const KNOWN_SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].version;

/// [`KNOWN_SCHEMA_VERSION`], for the release manifest's `store.schema_to`
/// (`fleet-hub compat`): the schema this build migrates a database to, and
/// the newest it will open.
pub fn known_schema_version() -> i64 {
    KNOWN_SCHEMA_VERSION
}

/// `(version, sql)` of every migration up to and including `version`, in
/// order: the historical files, for a test that builds an older database
/// (`store::testgen`).
#[cfg(test)]
pub(super) fn migrations_through(version: i64) -> impl Iterator<Item = (i64, &'static str)> {
    MIGRATIONS
        .iter()
        .filter(move |m| m.version <= version)
        .map(|m| (m.version, m.sql))
}

/// How [`newer_schema_error`]'s message starts; [`is_newer_schema_error`]
/// recognises the refusal by it.
const NEWER_SCHEMA_PREFIX: &str = "this database is at schema version ";

/// The downgrade guard's refusal: `recorded` came from a newer build.
fn newer_schema_error(recorded: i64) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
        Some(format!(
            "{NEWER_SCHEMA_PREFIX}{recorded}, but this build of claude-fleet \
             only knows up to {KNOWN_SCHEMA_VERSION}: it was last opened by a newer release. \
             It is not corrupt; do not delete it. Run that release (or a newer one) again, \
             or restore the copy of state.db you backed up before upgrading."
        )),
    )
}

/// Whether `e` is the downgrade guard's refusal (a database a newer release
/// migrated). An opener's own "if the file is corrupt, delete it" advice must
/// not follow it: that database is intact, and deleting it loses every
/// token, pairing and work item a rollback was meant to keep.
pub fn is_newer_schema_error(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, Some(msg))
            if f.code == rusqlite::ErrorCode::CannotOpen && msg.starts_with(NEWER_SCHEMA_PREFIX)
    )
}

/// The advice an opener adds to a failure to open `state.db`: none for the
/// downgrade refusal (its message already says what to do), else `corrupt`.
pub fn open_failure_advice<'a>(e: &rusqlite::Error, corrupt: &'a str) -> &'a str {
    if is_newer_schema_error(e) {
        ""
    } else {
        corrupt
    }
}

impl Store {
    pub(super) fn migrate(&self) -> Result<()> {
        // Downgrade guard, before anything is written: a database a newer
        // build migrated has a schema this build does not know, so this
        // build refuses it rather than running against it. A fresh file has
        // no `schema_version` yet, which reads as nothing recorded.
        let recorded: Option<i64> = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten();
        if let Some(recorded) = recorded.filter(|&v| v > KNOWN_SCHEMA_VERSION) {
            return Err(newer_schema_error(recorded));
        }
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        // The bootstrap migration is idempotent (`CREATE TABLE IF NOT EXISTS`
        // + `INSERT OR IGNORE`) and always runs: on a fresh DB it also creates
        // `schema_version`, which the gate below needs to exist.
        let bootstrap = MIGRATIONS[0].sql;
        self.conn.execute_batch(bootstrap)?;
        // Newer migrations are applied only if not yet recorded. We can't
        // wrap them in CREATE-OR-IGNORE because they ALTER existing tables.
        let v: i64 = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0);
        // Each ALTER-based migration runs in its OWN transaction, together
        // with the `schema_version` row it inserts. SQLite DDL is
        // transactional, so an interrupted migration rolls back entirely —
        // it can never leave a column half-added, which on the next launch
        // would re-run the migration and fail with "duplicate column",
        // bricking startup.
        //
        // Pending migrations run with foreign keys OFF. A table rebuild (024
        // changes `worktrees`' UNIQUE key) must drop a parent table that
        // `sessions.worktree_id` references, which SQLite only allows that
        // way. The pragma is a no-op inside a transaction, so it is set here,
        // outside them; `apply_migrations` then refuses to commit any
        // migration that ADDS a dangling reference, and foreign keys go back
        // on even when a migration fails. Dangling references that were
        // already there (a hand edit in the sqlite3 CLI, where foreign keys
        // default to off, can leave one) are logged and never fatal: failing
        // on them would stop the app from starting with no way out short of
        // manual SQL.
        let pending: Vec<Migration> = MIGRATIONS
            .iter()
            .copied()
            .filter(|m| m.version > v)
            .collect();
        if !pending.is_empty() {
            self.conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
            let applied = self.apply_migrations(&pending);
            let restored = self.conn.execute_batch("PRAGMA foreign_keys = ON;");
            for violation in applied? {
                tracing::warn!(
                    "migration left a pre-existing dangling foreign key in place: {violation:?}"
                );
            }
            restored?;
        }
        self.repair_skipped_main_migrations()?;
        self.backfill_stale_demoted()?;
        self.backfill_session_owner()?;
        self.reap_orphan_session_events()?;
        Ok(())
    }

    /// Migration 080's backfill: a row the tick demoted before the veto had
    /// its own column (`stale_working_at` set, `stale_demoted_at` not) keeps
    /// its veto. Runs on every open, after the collision repair, because an
    /// UPDATE of `sessions` compiles 065's row_version trigger, which names
    /// `lost_reason` — a column a conversation-branch database only has once
    /// the repair has added it. Idempotent: every write that sets
    /// `stale_working_at` sets `stale_demoted_at` with it, and nothing clears
    /// the veto while the stamp stays, so after the first run this matches
    /// no row. The column is not watched by the trigger, so no
    /// `row_version` moves.
    fn backfill_stale_demoted(&self) -> Result<usize> {
        self.conn.execute(
            "UPDATE sessions SET stale_demoted_at = stale_working_at \
             WHERE stale_working_at IS NOT NULL AND stale_demoted_at IS NULL",
            [],
        )
    }

    /// Migration 099's backfill: on a hub with exactly ONE person, every
    /// session fleet itself started becomes that person's, and private.
    ///
    /// **Why it is Rust and not an `UPDATE` in the script.** Migration 080
    /// wrote the reason down: any `UPDATE sessions` compiles the row-version
    /// trigger, which names `lost_reason` — a column that on a
    /// conversations-branch database exists only once
    /// [`Store::repair_skipped_main_migrations`] has added it, and that runs
    /// *after* every pending migration. An inline `UPDATE` in 095 aborts
    /// such an upgrade with `no such column: lost_reason`. So this runs
    /// beside [`Store::backfill_stale_demoted`], after the repair.
    ///
    /// **Why `started_at`.** It is documented "when fleet created the
    /// session (NULL for tmux-discovered rows)" (`store/rows.rs`), so it
    /// separates exactly the two populations the spec's table (§4.3,
    /// *`'unclaimed'`: the safe holding state*) distinguishes: a row fleet
    /// started, which the hub's one person demonstrably owns, from a row
    /// reconcile found on a host, which nobody can speak for. A
    /// hand-started tmux session on a shared host is precisely the row that
    /// must come out of the upgrade `unclaimed`. The companion spec's Q10
    /// asks for "every session" to carry the person; that is the half of
    /// Q10 that was wrong, and §4.3's own table is the authority.
    ///
    /// **Why one person.** With two or more people on the hub there is no
    /// fact that says which of them started a pre-M1 row, and guessing
    /// would widen (rule 7: the upgrade widens nothing). Those rows stay
    /// `unclaimed`, which is a count and nothing else.
    ///
    /// `(SELECT id FROM people WHERE is_personal_owner = 1) IS NOT NULL`
    /// looks redundant beside the count — migration 098 mints the flagged
    /// row — and is there so that a database where the one person is
    /// somehow not the flagged owner attributes NOTHING rather than
    /// stamping `private` with a NULL owner, which no caller could ever
    /// read. Fail closed, as `personal_owner_id()` does.
    ///
    /// Idempotent on two keys (`owner_person_id IS NULL` and
    /// `visibility = 'unclaimed'`): after the first run it matches no row,
    /// so the second `migrate()` of a session writes nothing. The two keys
    /// also mean it never revisits a row somebody has since decided — a
    /// session created after M1 carries its owner from its own create path
    /// (T5), and a row deliberately left owner-less and `private` stays
    /// unreadable rather than being handed to the hub's owner on the next
    /// open.
    ///
    /// Both columns ARE watched by the row-version trigger, so each
    /// attributed row's `row_version` moves once — which is correct: an
    /// open client holding a cached row must learn that its visibility
    /// changed.
    fn backfill_session_owner(&self) -> Result<usize> {
        self.conn.execute(
            "UPDATE sessions \
                SET owner_person_id = (SELECT id FROM people WHERE is_personal_owner = 1), \
                    visibility = 'private' \
              WHERE owner_person_id IS NULL \
                AND visibility = 'unclaimed' \
                AND started_at IS NOT NULL \
                AND (SELECT COUNT(*) FROM people) = 1 \
                AND (SELECT id FROM people WHERE is_personal_owner = 1) IS NOT NULL",
            [],
        )
    }

    /// Repair for the conversations-migration collision. The
    /// conversation-tracking branch numbered its migration 034, then 036,
    /// while `main` shipped 034 (`hosts.transport`), 035 (layers repair) and
    /// 036 (host boot identity); it is now 037. A database created by an
    /// earlier build of that branch recorded 34 or 36 for the conversations
    /// migration, so `migrate()` (which only offers `version >
    /// MAX(schema_version)`) never runs the `main` migrations numbered at or
    /// below that, and 037 is then skipped by its own guard.
    ///
    /// SQLite has no `ADD COLUMN IF NOT EXISTS`, so each column those
    /// migrations add is checked and added here when missing, and 035's
    /// `IF NOT EXISTS` DDL is re-run. Every step is idempotent, in one
    /// transaction; a no-op on any database that went through the numbered
    /// migrations.
    ///
    /// The same collision, a second time: the hub-ops-accounting branch
    /// (PR #344) numbered its `usage_daily` rebuild 064, 065, 066 and then
    /// 068 before it landed as 071, while `main` shipped 064–068. A database
    /// a build of that branch opened recorded the branch's number, so
    /// `main`'s migrations at or below it never ran (071's own guard then
    /// only records it). Each of 064–068 is detected by its artefact —
    /// 065–068 by their `already_applied` guards, 064 by
    /// `tracker_webhooks` still existing — and run here when the recorded
    /// version is past it and the artefact is missing. Their scripts are
    /// ADD COLUMN plus `IF NOT EXISTS` / `DROP … IF EXISTS` DDL, and 065's
    /// trigger rebuild is still the latest one, so running them late is
    /// what running them in order would have left.
    fn repair_skipped_main_migrations(&self) -> Result<()> {
        /// `(table, column, column definition)` added by `main`'s 034 and 036.
        const COLUMNS: &[(&str, &str, &str)] = &[
            ("hosts", "transport", "TEXT NOT NULL DEFAULT 'ssh'"),
            ("hosts", "boot_id", "TEXT"),
            ("hosts", "tmux_server_pid", "INTEGER"),
            ("sessions", "lost_reason", "TEXT"),
        ];
        // Reads, then writes (035's `INSERT OR IGNORE`): IMMEDIATE, or a
        // CLI opening this file beside the running hub fails on its write lock.
        let tx = self.immediate_transaction()?;
        for (table, column, def) in COLUMNS {
            let n: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                rusqlite::params![table, column],
                |r| r.get(0),
            )?;
            if n == 0 {
                tracing::warn!(
                    "{table}.{column} missing despite schema version; adding it (migration collision repair)"
                );
                tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {def};"))?;
            }
        }
        // 035 is `IF NOT EXISTS` throughout; re-running it is a no-op when
        // its tables are there.
        tx.execute_batch(include_str!("../../migrations/035_host_layers_repair.sql"))?;
        let recorded: i64 = tx
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0);
        for m in MIGRATIONS
            .iter()
            .filter(|m| (64..=68).contains(&m.version) && m.version <= recorded)
        {
            let missing = match m.already_applied {
                Some(applied) => !applied(&tx)?,
                None => {
                    // 064 drops `tracker_webhooks`.
                    let n: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM sqlite_master \
                         WHERE type = 'table' AND name = 'tracker_webhooks'",
                        [],
                        |r| r.get(0),
                    )?;
                    n > 0
                }
            };
            if missing {
                tracing::warn!(
                    "migration {} missing despite schema version {recorded}; running it \
                     (usage_daily numbering collision repair)",
                    m.version
                );
                tx.execute_batch(m.sql)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Run `pending` migrations in order, each in its own transaction, and
    /// return the dangling references that were ALREADY in the database.
    /// `PRAGMA foreign_key_check` covers the whole database, so it is taken
    /// before and after each migration: only rows the migration added roll it
    /// back (SQLite's table-rebuild procedure); rows present before it are
    /// left alone and returned for the caller to log.
    fn apply_migrations(&self, pending: &[Migration]) -> Result<Vec<FkViolation>> {
        let mut preexisting: Vec<FkViolation> = Vec::new();
        for &Migration {
            version,
            sql,
            already_applied,
        } in pending
        {
            let tx = self.immediate_transaction()?;
            if already_applied
                .map(|applied| applied(&tx))
                .transpose()?
                .unwrap_or(false)
            {
                // Re-run on a schema that already has this change (a test
                // rolled the recorded version back): only record it.
                tx.execute(
                    "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
                    rusqlite::params![version],
                )?;
                tx.commit()?;
                continue;
            }
            let before: std::collections::HashSet<FkViolation> =
                fk_violations(&tx)?.into_iter().collect();
            tx.execute_batch(sql)?;
            let mut added: Vec<FkViolation> = Vec::new();
            for violation in fk_violations(&tx)? {
                if !before.contains(&violation) {
                    added.push(violation);
                } else if !preexisting.contains(&violation) {
                    preexisting.push(violation);
                }
            }
            if let Some(v) = added.first() {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                    Some(format!(
                        "migration {version} left a dangling foreign key: {} row {:?} -> {} (fk {})",
                        v.table, v.rowid, v.parent, v.fkid
                    )),
                ));
            }
            tx.commit()?;
        }
        Ok(preexisting)
    }

    /// Drop `session_events` rows whose session no longer exists. Deletes that
    /// predate `delete_session` reaping the timeline left such orphans behind;
    /// a reused session id would inherit them. Idempotent, runs on open.
    fn reap_orphan_session_events(&self) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM session_events \
             WHERE NOT EXISTS (SELECT 1 FROM sessions WHERE sessions.id = session_events.session_id)",
            [],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_TABLES: &[&str] = &[
        "hosts",
        "projects",
        "worktrees",
        "sessions",
        "settings",
        "schema_version",
        "session_events",
        "session_messages",
        "host_tokens",
        "tasks",
        "worktree_parent_fingerprints",
        "catalog_config",
        "catalogs",
        "asset_inventory",
        "catalog_secrets",
        "catalog_secrets_host",
        "sync_runs",
        "client_tokens",
        "conversations",
        "error_reports",
        "participants",
        "read_cursors",
        "peer_links",
        "host_layers",
        "host_catalogs",
        "client_catalog_grants",
        "changesets",
        "changeset_items",
        "asset_triage_verdicts",
    ];

    #[test]
    fn open_in_memory_creates_all_tables() {
        let store = Store::open_in_memory().expect("open");
        for t in EXPECTED_TABLES {
            assert!(store.has_table(t).expect("has_table"), "missing table: {t}");
        }
    }

    fn session_columns(s: &Store) -> Vec<String> {
        let mut stmt = s.conn.prepare("PRAGMA table_info(sessions)").unwrap();
        let mut out = Vec::new();
        for c in stmt.query_map([], |r| r.get::<_, String>(1)).unwrap() {
            out.push(c.unwrap());
        }
        out
    }

    /// The handoff/freeze drop (W5 G2) on an existing database at version
    /// 20 that still has the dead `handoffs` table (with a row) and
    /// `sessions.frozen_scrollback` (with a value): every later migration
    /// applies cleanly, both are gone, the session row survives, and a
    /// relaunch — which re-runs the 001 bootstrap — is a strict no-op and
    /// does not bring the table back.
    #[test]
    fn handoff_freeze_drop_applies_on_an_existing_v20_db_and_is_idempotent() {
        const SEED_AT: i64 = 20;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for &Migration { version, sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= SEED_AT) {
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("migration {version}: {e}"));
        }
        // The table exactly as the pre-022 bootstrap created it.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS handoffs (
               id INTEGER PRIMARY KEY,
               session_id INTEGER NOT NULL REFERENCES sessions(id),
               from_host TEXT NOT NULL, to_host TEXT NOT NULL, mode TEXT NOT NULL,
               started_at INTEGER NOT NULL, finished_at INTEGER,
               status TEXT NOT NULL, error TEXT);
             INSERT INTO hosts (alias) VALUES ('h');
             INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status, frozen_scrollback)
               VALUES ('s', 'h', 1, 1, 'running', 'frozen text');
             INSERT INTO handoffs (session_id, from_host, to_host, mode, started_at, status)
               VALUES (1, 'h', 'x', 'mirror', 1, 'done');",
        )
        .unwrap();
        let store = Store {
            conn,
            bus: StoreBus::new(Arc::new(NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            peer_generations: Default::default(),
            instance: super::next_instance(),
        };
        assert!(store.has_table("handoffs").unwrap());
        assert!(session_columns(&store).contains(&"frozen_scrollback".to_string()));
        assert_eq!(store.schema_version().unwrap(), SEED_AT);

        store.migrate().expect("migrate a v20 db");
        assert!(!store.has_table("handoffs").unwrap());
        assert!(!session_columns(&store).contains(&"frozen_scrollback".to_string()));
        let row = store.get_session("s", "h").unwrap().expect("row survives");
        assert_eq!(row.status, "running");
        assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

        // Relaunch: the bootstrap re-runs; nothing changes, the table stays gone.
        let objects = |s: &Store| -> i64 {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        let before = (objects(&store), session_columns(&store));
        store.migrate().expect("second migrate");
        assert!(!store.has_table("handoffs").unwrap());
        assert_eq!((objects(&store), session_columns(&store)), before);
        assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// A fresh database: 001 still lists `frozen_scrollback` (so the drop
    /// can be an unconditional DROP COLUMN) and no longer creates
    /// `handoffs`; after the full migrate neither exists, and a second
    /// migrate keeps it that way.
    #[test]
    fn fresh_db_has_no_handoffs_or_frozen_scrollback() {
        let store = Store::open_in_memory().expect("open");
        assert!(!store.has_table("handoffs").unwrap());
        assert!(!session_columns(&store).contains(&"frozen_scrollback".to_string()));
        store.migrate().expect("second migrate");
        assert!(!store.has_table("handoffs").unwrap());
        assert!(!session_columns(&store).contains(&"frozen_scrollback".to_string()));
    }

    /// Migration 037 backfills exactly one open `conversations` row (matching
    /// `sessions.claude_session_id`) per session bound to a conversation at
    /// upgrade time, stamped `start_source = 'unknown'` at the session's
    /// `created_at`.
    #[test]
    fn migration_037_backfills_one_open_conversation_per_bound_session() {
        let conn = Connection::open_in_memory().unwrap();
        for &Migration { version, sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= 36) {
            let _ = version;
            conn.execute_batch(sql).unwrap();
        }
        conn.execute("INSERT INTO hosts (alias) VALUES ('local')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status, claude_session_id)
             VALUES ('s', 'local', 7, 7, 'running', '11111111-1111-1111-1111-111111111111')",
            [],
        )
        .unwrap();
        let store = Store {
            conn,
            bus: StoreBus::new(Arc::new(NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            peer_generations: Default::default(),
            instance: super::next_instance(),
        };
        store.migrate().unwrap();
        let (n, src, started): (i64, String, i64) = store
            .conn
            .query_row(
                "SELECT COUNT(*), start_source, started_at FROM conversations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((n, src.as_str(), started), (1, "unknown", 7));
    }

    /// A freshly created session row carries `SessionContext::default()` —
    /// every context_* column is NULL / false until a hook writes one.
    #[test]
    fn session_row_carries_context_defaults() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        store
            .upsert_session("s", "local", None, None, 0, 0, "running", None)
            .unwrap();
        let row = store.get_session("s", "local").unwrap().unwrap();
        assert_eq!(row.context, SessionContext::default());
    }

    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(
            store.schema_version().expect("version"),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrations_are_contiguous_from_one() {
        assert!(!MIGRATIONS.is_empty());
        for (i, &Migration { version, .. }) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                version,
                i as i64 + 1,
                "MIGRATIONS[{i}] has version {version}"
            );
        }
        assert_eq!(LATEST_SCHEMA_VERSION, MIGRATIONS.len() as i64);
    }

    #[test]
    fn every_migration_records_its_own_version() {
        for &Migration { version, sql, .. } in MIGRATIONS {
            // Normalise whitespace so formatting differences between scripts
            // (line breaks, double spaces) do not matter.
            let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            let stamp =
                format!("INSERT OR IGNORE INTO schema_version (version) VALUES ({version});");
            assert!(
                flat.contains(&stamp),
                "migration {version} must contain `{stamp}`"
            );
            // …and must not stamp any *other* version by mistake.
            let stamps = flat
                .matches("INTO schema_version (version) VALUES (")
                .count();
            assert_eq!(stamps, 1, "migration {version} stamps {stamps} versions");
        }
    }

    #[test]
    fn second_migrate_on_fresh_db_is_a_noop() {
        let store = Store::open_in_memory().expect("open");
        let before = store.schema_version().expect("version");
        let count = |s: &Store| -> i64 {
            s.conn
                .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
                .unwrap()
        };
        let rows_before = count(&store);
        let tables_before = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        store.migrate().expect("second migrate");
        assert_eq!(store.schema_version().expect("version"), before);
        assert_eq!(count(&store), rows_before);
        let tables_after = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(tables_after, tables_before);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().expect("open");
        let on: i64 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma");
        assert_eq!(on, 1, "foreign_keys pragma should be ON");
    }

    #[test]
    fn migration_002_adds_ssh_alias_column_to_hosts() {
        let s = Store::open_in_memory().expect("open");
        // sqlite_master pragma_table_info path
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "ssh_alias"),
            "expected `ssh_alias` column; got: {cols:?}"
        );
    }

    #[test]
    fn schema_version_is_latest_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn migration_004_adds_account_uuid_column_to_sessions() {
        let s = Store::open_in_memory().expect("open");
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('sessions')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on sessions; got: {cols:?}"
        );
    }

    #[test]
    fn migration_003_adds_accounts_table_and_host_account_uuid_column() {
        let s = Store::open_in_memory().expect("open");
        assert!(
            s.has_table("accounts").expect("has_table"),
            "expected accounts table"
        );
        let mut stmt = s
            .conn
            .prepare_cached("SELECT name FROM pragma_table_info('hosts')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on hosts; got: {cols:?}"
        );
    }

    #[test]
    fn open_reaps_orphaned_session_events_idempotently() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let live = store
            .upsert_session("live", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .insert_session_event(live, "status_change", Some("idle"))
            .unwrap();
        // Left behind by a delete that predates the reap in delete_session.
        store
            .insert_session_event(live + 1000, "killed", None)
            .unwrap();

        store.migrate().unwrap();
        store.migrate().unwrap();

        assert_eq!(store.list_session_events(live, 10).unwrap().len(), 1);
        assert!(store
            .list_session_events(live + 1000, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn migration_005_adds_kind_and_reviews_columns_with_defaults() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let rows = store.list_sessions_for_host("alpha").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "work");
        assert_eq!(rows[0].reviews_session_id, None);
    }

    /// A database that stopped at `version`: the migrations up to it, with
    /// foreign keys on as a real install ran them.
    fn store_at_version(version: i64) -> Store {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for &Migration { sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= version) {
            conn.execute_batch(sql).unwrap();
        }
        Store {
            conn,
            bus: StoreBus::new(Arc::new(NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            peer_generations: Default::default(),
            instance: super::next_instance(),
        }
    }

    #[test]
    fn migration_024_fresh_db_has_host_scoped_worktrees() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        let local = s
            .upsert_worktree(pid, "feat", "/p/r/.worktrees/feat", None)
            .unwrap();
        // Same project and name on another host: its own row, no clash.
        let remote = s
            .upsert_worktree_on(
                "vps",
                pid,
                "feat",
                "/home/u/r/.worktrees/feat",
                Some("feat"),
            )
            .unwrap();
        assert_ne!(local, remote);
        // Upserting again on the same host updates that row in place.
        let again = s
            .upsert_worktree_on("vps", pid, "feat", "/home/u/r/.worktrees/feat2", None)
            .unwrap();
        assert_eq!(again, remote);
        let row = s.get_worktree_row(remote).unwrap().unwrap();
        assert_eq!(
            (row.host_alias.as_str(), row.path.as_str()),
            ("vps", "/home/u/r/.worktrees/feat2")
        );
        assert_eq!(
            s.get_worktree_row(local).unwrap().unwrap().host_alias,
            "local"
        );
        let default: String = s
            .conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('worktrees') WHERE name='host_alias'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(default, "'local'");
    }

    /// 024 on a database with data: every worktree row keeps its id and
    /// becomes `local`, sessions keep their `worktree_id`, foreign keys come
    /// back on (and still guard the rebuilt table), and a second migrate is
    /// a no-op.
    #[test]
    fn migration_024_on_an_existing_db_backfills_local_and_keeps_ids() {
        let old = store_at_version(23);
        old.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('local');
                 INSERT INTO projects (id, owner, repo, base_path) VALUES (1, 'o', 'r', '/p/r');
                 INSERT INTO worktrees (id, project_id, name, path, branch)
                   VALUES (7, 1, 'main', '/p/r', 'main');
                 INSERT INTO worktrees (id, project_id, name, path, branch)
                   VALUES (9, 1, 'feat', '/p/r/.worktrees/feat', NULL);
                 INSERT INTO sessions
                   (tmux_name, host_alias, project_id, worktree_id,
                    created_at, last_activity_at, status)
                   VALUES ('dev', 'local', 1, 9, 1, 1, 'running');",
            )
            .unwrap();
        old.migrate().expect("024 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let got: Vec<(i64, String, String)> = old
            .list_worktrees_for_project(1)
            .unwrap()
            .into_iter()
            .map(|w| (w.id, w.host_alias, w.name))
            .collect();
        assert_eq!(
            got,
            vec![
                (9, "local".to_string(), "feat".to_string()),
                (7, "local".to_string(), "main".to_string())
            ]
        );
        let wid: Option<i64> = old
            .conn
            .query_row(
                "SELECT worktree_id FROM sessions WHERE tmux_name='dev'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(wid, Some(9));
        let fk: i64 = old
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign keys are back on");
        assert!(
            old.conn
                .execute("DELETE FROM worktrees WHERE id = 9", [])
                .is_err(),
            "the referenced row is still protected"
        );
        // The new key: a remote row may share the local row's name.
        old.upsert_worktree_on("vps", 1, "main", "/home/u/p/r", None)
            .unwrap();
        assert_eq!(old.list_worktrees_for_project(1).unwrap().len(), 2);
        assert_eq!(old.list_worktrees_on_host("vps").unwrap().len(), 1);
        // Double migrate: nothing re-runs, nothing is lost.
        old.migrate().expect("second migrate");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(old.list_worktrees_on_host("vps").unwrap().len(), 1);
        assert_eq!(old.list_worktrees_for_project(1).unwrap().len(), 2);
    }

    /// Pending migrations run with foreign keys off, so the runner itself
    /// must refuse to commit one that leaves a dangling reference.
    #[test]
    fn apply_migrations_rolls_back_a_dangling_foreign_key() {
        let s = Store::open_in_memory().unwrap();
        s.conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        let bad = "INSERT INTO worktrees (project_id, name, path) VALUES (424242, 'x', '/x');";
        let err = s
            .apply_migrations(&[Migration::plain(999, bad)])
            .unwrap_err();
        s.conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        assert!(err.to_string().contains("dangling"), "{err}");
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM worktrees WHERE project_id = 424242",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "the migration rolled back");
    }

    /// The per-entry `already_applied` guard: when it says the change is
    /// already in the schema, the entry is only recorded and its SQL never
    /// runs (here it would fail); when it says no, the SQL runs as usual.
    #[test]
    fn apply_migrations_only_records_an_entry_its_guard_says_is_applied() {
        fn yes(_: &Connection) -> rusqlite::Result<bool> {
            Ok(true)
        }
        fn no(_: &Connection) -> rusqlite::Result<bool> {
            Ok(false)
        }
        let s = Store::open_in_memory().unwrap();
        let guarded = Migration {
            version: 998,
            sql: "THIS IS NOT SQL;",
            already_applied: Some(yes),
        };
        s.apply_migrations(&[guarded])
            .expect("an entry its guard says is applied is not run");
        let stamped: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version = 998",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stamped, 1, "but its version is recorded");
        let unguarded = Migration {
            version: 997,
            sql: "THIS IS NOT SQL;",
            already_applied: Some(no),
        };
        assert!(
            s.apply_migrations(&[unguarded]).is_err(),
            "a guard that says no runs the SQL"
        );
    }

    /// Seed one dangling reference, as a hand edit in the sqlite3 CLI (foreign
    /// keys default to off there) can leave.
    fn seed_dangling_session(s: &Store) {
        s.conn
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO hosts (alias) VALUES ('local');
                 INSERT INTO sessions
                   (tmux_name, host_alias, project_id, worktree_id,
                    created_at, last_activity_at, status)
                   VALUES ('stale', 'local', NULL, 4242, 1, 1, 'running');
                 PRAGMA foreign_keys = ON;",
            )
            .unwrap();
    }

    /// `PRAGMA foreign_key_check` covers the whole database: a dangling
    /// reference that was ALREADY there must not block 024 (or brick
    /// startup). It is reported, keyed stably, and left in place; only rows a
    /// migration adds roll it back.
    #[test]
    fn migration_024_applies_over_a_preexisting_dangling_reference() {
        let old = store_at_version(23);
        seed_dangling_session(&old);
        let pending: Vec<Migration> = MIGRATIONS
            .iter()
            .copied()
            .filter(|m| m.version > 23)
            .collect();
        old.conn
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .unwrap();
        let reported = old
            .apply_migrations(&pending)
            .expect("a pre-existing dangling row is not fatal");
        old.conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert_eq!(
            (reported[0].table.as_str(), reported[0].parent.as_str()),
            ("sessions", "worktrees")
        );
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            old.has_table("worktrees").unwrap()
                && old.list_worktrees_on_host("local").unwrap().is_empty(),
            "the rebuilt table is in place"
        );
        // The app start path too: `migrate()` logs it and succeeds.
        let again = store_at_version(23);
        seed_dangling_session(&again);
        again.migrate().expect("the app still starts");
        assert_eq!(again.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Tests roll the recorded version back and migrate again (#61's upgrade
    /// test does), which re-runs 024. On a table that already has
    /// `host_alias` it must not rebuild again: remote rows keep their host,
    /// even one named like a local row.
    #[test]
    fn migration_024_rerun_keeps_remote_rows() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        s.upsert_worktree(pid, "main", "/p/r", None).unwrap();
        let remote = s
            .upsert_worktree_on("vps", pid, "main", "/home/u/r", None)
            .unwrap();
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 24;")
            .unwrap();
        s.migrate().expect("re-running 024 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            s.get_worktree_row(remote).unwrap().unwrap().host_alias,
            "vps"
        );
        assert_eq!(s.list_worktrees_for_project(pid).unwrap().len(), 1);
    }

    /// Rolling the recorded version back re-runs 025 (session usage); it
    /// must not try to add its columns again, and counted usage survives.
    #[test]
    fn migration_025_rerun_is_a_no_op_and_keeps_usage() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.apply_usage(
            id,
            "local",
            &UsageDelta {
                reset: false,
                totals: UsageTotals {
                    input_tokens: 9,
                    cost_micros: 45,
                    ..Default::default()
                },
                model: None,
                offset: 3,
                source: "x.jsonl".into(),
                last_msg_id: None,
                last_msg_usage: None,
                now: 86_400,
                by_day: Vec::new(),
                backfill_until: None,
            },
        )
        .unwrap();
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 25;")
            .unwrap();
        s.migrate().expect("re-running 025 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.usage.usage_input_tokens, 9);
        assert_eq!(s.usage_daily_since(0, None).unwrap().len(), 1);
    }

    /// 026 on a database with worktree rows (stopped at 025): the column is
    /// added, existing rows keep their data with no stamp (written before
    /// 026), and their next write stamps them.
    #[test]
    fn migration_026_on_an_existing_db_keeps_rows_and_stamps_on_next_write() {
        let old = store_at_version(25);
        old.conn
            .execute_batch(
                "INSERT INTO projects (id, owner, repo, base_path) VALUES (1, 'o', 'r', '/p/r');
                 INSERT INTO worktrees (id, project_id, host_alias, name, path, branch)
                   VALUES (7, 1, 'vps', 'feat', '/h/r/feat', 'feat');",
            )
            .unwrap();
        old.migrate().expect("026 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = old.get_worktree_row(7).unwrap().expect("the row survives");
        assert_eq!(
            (
                row.host_alias.as_str(),
                row.path.as_str(),
                row.branch.as_deref()
            ),
            ("vps", "/h/r/feat", Some("feat"))
        );
        assert_eq!(
            old.worktree_updated_at_ms(7).unwrap(),
            None,
            "written before 026: no stamp"
        );
        old.upsert_worktree_on("vps", 1, "feat", "/h/r/feat", Some("feat"))
            .unwrap();
        assert!(
            old.worktree_updated_at_ms(7).unwrap().is_some(),
            "the next write stamps it"
        );
    }

    /// 026 stamps every worktree write, insert and update, with
    /// `updated_at_ms`, and a re-run on a table that already has the column
    /// is only recorded: the stamp survives.
    #[test]
    fn migration_026_stamps_worktree_writes_and_reruns_safely() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/r").unwrap();
        let id = s
            .upsert_worktree_on("vps", pid, "feat", "/h/r/feat", None)
            .unwrap();
        assert!(
            s.worktree_updated_at_ms(id)
                .unwrap()
                .is_some_and(|ms| ms > 0),
            "stamped on insert"
        );
        s.set_worktree_updated_at_ms(id, 1).unwrap();
        s.upsert_worktree_on("vps", pid, "feat", "/h/r/feat2", None)
            .unwrap();
        let stamped = s
            .worktree_updated_at_ms(id)
            .unwrap()
            .expect("stamped on update");
        assert!(stamped > 1, "an update restamps: {stamped}");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 26;")
            .unwrap();
        s.migrate().expect("re-running 026 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            s.worktree_updated_at_ms(id).unwrap(),
            Some(stamped),
            "the stamp survives a re-run"
        );
        assert_eq!(s.worktree_updated_at_ms(999_999).unwrap(), None);
    }

    /// 027 on a database with project rows (stopped at 026): the column is
    /// added, defaults to 0 (not adopted) for existing rows, and a re-run
    /// (tests roll the recorded version back and migrate again) is a no-op
    /// that keeps a row's `adopted` flag.
    #[test]
    fn migration_027_adds_adopted_column_defaulting_to_unset_and_reruns_safely() {
        let old = store_at_version(26);
        old.conn
            .execute_batch(
                "INSERT INTO projects (id, owner, repo, base_path) VALUES (1, 'o', 'r', '/p/r');",
            )
            .unwrap();
        old.migrate().expect("027 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            !old.list_projects().unwrap()[0].adopted,
            "a pre-027 row is not adopted"
        );
        let pid = old
            .upsert_adopted_project("a", "b", "/out/of/root")
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 27;")
            .unwrap();
        old.migrate().expect("re-running 027 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            old.list_projects()
                .unwrap()
                .into_iter()
                .find(|p| p.id == pid)
                .unwrap()
                .adopted,
            "the adopted flag survives a re-run"
        );
    }

    /// 038 on a database with project rows (stopped at 037): the column is
    /// added, defaults to 0 (not a system row) for existing rows, and a
    /// re-run (tests roll the recorded version back and migrate again) is a
    /// no-op that keeps a row's `system` flag.
    #[test]
    fn migration_038_adds_system_column_defaulting_to_unset_and_reruns_safely() {
        let old = store_at_version(37);
        old.conn
            .execute_batch(
                "INSERT INTO projects (id, owner, repo, base_path) VALUES (1, 'o', 'r', '/p/r');",
            )
            .unwrap();
        old.migrate().expect("038 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            !old.list_projects().unwrap()[0].system,
            "a pre-038 row is not a system row"
        );
        let pid = old
            .upsert_system_project("fleet", "operator", "~/.claude-fleet/operator")
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 38;")
            .unwrap();
        old.migrate().expect("re-running 038 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            old.list_projects()
                .unwrap()
                .into_iter()
                .find(|p| p.id == pid)
                .unwrap()
                .system,
            "the system flag survives a re-run"
        );
    }

    /// 028 on a database with an account row (stopped at 027): both columns
    /// are added (`nickname` NULL, `has_extra_usage` defaulting to false for
    /// existing rows), and a re-run (tests roll the recorded version back and
    /// migrate again) is a no-op that keeps a nickname set after the upgrade.
    #[test]
    fn migration_028_adds_nickname_and_extra_usage_columns_and_reruns_safely() {
        let old = store_at_version(27);
        old.conn
            .execute_batch("INSERT INTO accounts (uuid, email) VALUES ('u1', 'a@b.com');")
            .unwrap();
        old.migrate().expect("028 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = old
            .get_account_by_uuid("u1")
            .unwrap()
            .expect("row survives");
        assert_eq!(row.nickname, None, "a pre-028 row has no nickname");
        assert!(
            !row.has_extra_usage,
            "a pre-028 row defaults to no extra usage"
        );
        old.set_account_nickname("u1", Some("Home")).unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 28;")
            .unwrap();
        old.migrate().expect("re-running 028 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            old.get_account_by_uuid("u1")
                .unwrap()
                .unwrap()
                .nickname
                .as_deref(),
            Some("Home"),
            "the nickname survives a re-run"
        );
    }

    /// 029 on a database stopped at 028: `dismissed_agents` is created and a
    /// row survives a re-run (tests roll the recorded version back and
    /// migrate again). Its SQL is `CREATE TABLE IF NOT EXISTS`, so unlike 024
    /// (rebuild) or 025/027/028 (`ALTER TABLE ADD COLUMN`) it needs no
    /// `already_applied` guard — a re-run is safe as written.
    #[test]
    fn migration_029_is_idempotent() {
        let old = store_at_version(28);
        old.migrate().expect("029 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        old.conn
            .execute_batch(
                "INSERT INTO dismissed_agents (host_alias, claude_session_id, dismissed_at) \
                 VALUES ('local', 'u1', 100);",
            )
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 29;")
            .unwrap();
        old.migrate().expect("re-running 029 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let dismissed_at: i64 = old
            .conn
            .query_row(
                "SELECT dismissed_at FROM dismissed_agents WHERE host_alias='local' AND claude_session_id='u1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dismissed_at, 100, "the row survives a re-run");
    }

    /// 030 on a database stopped at 029: `catalog_config` and
    /// `asset_inventory` are created and their rows survive a re-run. Both
    /// statements are `CREATE TABLE IF NOT EXISTS`, so — like 029 — the
    /// migration needs no `already_applied` guard.
    #[test]
    fn migration_030_is_idempotent() {
        let old = store_at_version(29);
        old.migrate().expect("030 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        old.conn
            .execute_batch(
                "INSERT INTO catalog_config (id, repo_path) VALUES (1, '/tmp/assets');\
                 INSERT INTO asset_inventory \
                   (host_alias, harness, kind, name, state, scanned_at) \
                   VALUES ('local', 'claude', 'skill', 's', 'in_sync', 7);",
            )
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 30;")
            .unwrap();
        old.migrate().expect("re-running 030 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let repo_path: String = old
            .conn
            .query_row(
                "SELECT repo_path FROM catalog_config WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(repo_path, "/tmp/assets", "the config row survives a re-run");
        let scanned_at: i64 = old
            .conn
            .query_row(
                "SELECT scanned_at FROM asset_inventory WHERE host_alias='local' AND name='s'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scanned_at, 7, "the inventory row survives a re-run");
    }

    /// 031 on a database stopped at 030 with an inventory row: `managed`
    /// is added (defaulting to false for the existing row) and the new
    /// `catalog_secrets`, `catalog_secrets_host`, `sync_runs` tables appear.
    /// Rolling the recorded version back and migrating again (tests do this
    /// to simulate re-running an already-applied migration) is a no-op that
    /// keeps the row and does not re-add the column.
    #[test]
    fn migration_031_is_idempotent() {
        let old = store_at_version(30);
        old.conn
            .execute_batch(
                "INSERT INTO asset_inventory \
                   (host_alias, harness, kind, name, state, scanned_at) \
                   VALUES ('local', 'claude', 'skill', 's', 'in_sync', 7);",
            )
            .unwrap();
        old.migrate().expect("031 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            !old.list_inventory().unwrap()[0].managed,
            "a pre-031 row is not managed"
        );
        let n: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'managed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "the managed column exists exactly once");
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 31;")
            .unwrap();
        old.migrate().expect("re-running 031 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let n: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('asset_inventory') WHERE name = 'managed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "re-running 031 does not duplicate the column");
        let scanned_at: i64 = old
            .conn
            .query_row(
                "SELECT scanned_at FROM asset_inventory WHERE host_alias='local' AND name='s'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scanned_at, 7, "the inventory row survives a re-run");
    }

    /// 090 on a database stopped at 089 with a `catalog_config` row: the row
    /// becomes the `personal` catalog, and a re-run changes nothing.
    #[test]
    fn migration_90_copies_catalog_config_into_personal() {
        let old = Store::open_in_memory().expect("open");
        old.conn
            .execute_batch(
                "DROP TABLE catalogs;\
                 INSERT INTO catalog_config (id, repo_path, remote_url, head_commit, last_loaded_at) \
                   VALUES (1, '/r', 'git@a:b.git', 'h1', 5);\
                 DELETE FROM schema_version WHERE version >= 90;",
            )
            .unwrap();
        old.migrate().expect("090 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let p = old.personal_catalog().unwrap().expect("personal row");
        assert_eq!((p.name.as_str(), p.repo_path.as_str()), ("personal", "/r"));
        assert_eq!(p.remote_url.as_deref(), Some("git@a:b.git"));
        assert_eq!(
            (p.head_commit.as_deref(), p.last_loaded_at, p.org_id),
            (Some("h1"), Some(5), None)
        );
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 90;")
            .unwrap();
        old.migrate().expect("re-running 090 is safe");
        assert_eq!(
            old.list_catalogs().unwrap().len(),
            1,
            "no second personal row"
        );
    }

    /// 121 on a database stopped at 120: every row gets an agent, a shell
    /// session's is `shell`, the row-version trigger watches the column,
    /// and a re-run (the column already there) changes nothing.
    #[test]
    fn migration_121_gives_every_session_an_agent() {
        let old = Store::open_in_memory().expect("open");
        old.upsert_host("h").unwrap();
        let work = old
            .upsert_session("w", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let shell = old
            .upsert_session("sh", "h", None, None, 1, 1, "running", None)
            .unwrap();
        old.conn
            .execute_batch(&format!(
                "UPDATE sessions SET kind = 'shell' WHERE id = {shell};\
                 DROP TRIGGER sessions_row_version_bump;\
                 ALTER TABLE sessions DROP COLUMN agent;\
                 DELETE FROM schema_version WHERE version >= 121;"
            ))
            .unwrap();
        old.migrate().expect("121 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let agent = |id| old.get_session_by_id(id).unwrap().unwrap().agent;
        assert_eq!(
            (agent(work), agent(shell)),
            ("claude".into(), "shell".into())
        );
        let version = |id| old.get_session_by_id(id).unwrap().unwrap().row_version;
        let before = version(work);
        old.conn
            .execute("UPDATE sessions SET agent = 'codex' WHERE id = ?1", [work])
            .unwrap();
        assert_eq!(version(work), before + 1, "the trigger watches agent");
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 121;")
            .unwrap();
        old.migrate().expect("re-running 121 is safe");
        assert_eq!(agent(work), "codex", "a re-run does not backfill again");
    }

    /// 124 on a database stopped at 123: the columns arrive empty (no
    /// guessed backfill), the CHECK refuses an unknown origin, and the
    /// row-version trigger watches both.
    #[test]
    fn migration_124_adds_an_empty_origin() {
        let old = Store::open_in_memory().expect("open");
        old.upsert_host("h").unwrap();
        let id = old
            .upsert_session("w", "h", None, None, 1, 1, "running", None)
            .unwrap();
        old.conn
            .execute_batch(
                "DROP TRIGGER sessions_row_version_bump;\
                 ALTER TABLE sessions DROP COLUMN origin_ref;\
                 ALTER TABLE sessions DROP COLUMN origin;\
                 DELETE FROM schema_version WHERE version >= 124;",
            )
            .unwrap();
        old.migrate().expect("124 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = || old.get_session_by_id(id).unwrap().unwrap();
        assert_eq!((row().origin, row().origin_ref), (None, None));
        let before = row().row_version;
        old.set_session_origin(id, &crate::store::SessionOrigin::token(Some(3)))
            .unwrap();
        assert_eq!(row().row_version, before + 1, "the trigger watches origin");
        assert_eq!(row().origin_ref.as_deref(), Some("3"));
        assert!(old
            .conn
            .execute("UPDATE sessions SET origin = 'cron' WHERE id = ?1", [id])
            .is_err());
    }

    /// 125 on an existing DB: every row reads as viewed at upgrade time, so
    /// nothing turns unread by upgrading; the stamp only moves forward.
    #[test]
    fn migration_125_counts_every_session_as_viewed() {
        let old = Store::open_in_memory().expect("open");
        old.upsert_host("h").unwrap();
        let id = old
            .upsert_session("w", "h", None, None, 1, 1, "running", None)
            .unwrap();
        old.conn
            .execute_batch(
                "DROP TRIGGER sessions_row_version_bump;\
                 ALTER TABLE sessions DROP COLUMN last_viewed_at;\
                 DELETE FROM schema_version WHERE version >= 125;",
            )
            .unwrap();
        old.migrate().expect("125 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = || old.get_session_by_id(id).unwrap().unwrap();
        let stamped = row().last_viewed_at.expect("backfilled as viewed");
        assert!(stamped > 1_600_000_000);
        let before = row().row_version;
        assert!(!old.touch_session_viewed(id, stamped - 5).unwrap());
        assert!(old.touch_session_viewed(id, stamped + 5).unwrap());
        assert_eq!(row().last_viewed_at, Some(stamped + 5));
        assert_eq!(row().row_version, before + 1, "the trigger watches it");
    }

    /// 129 on a database stopped at 128: `turn_outcome` arrives empty, the
    /// CHECK refuses a word J2 never answers, the row-version trigger
    /// watches it, and proposals have their subject index.
    #[test]
    fn migration_129_adds_an_empty_turn_outcome() {
        let old = Store::open_in_memory().expect("open");
        old.upsert_host("h").unwrap();
        let id = old
            .upsert_session("w", "h", None, None, 1, 1, "running", None)
            .unwrap();
        old.conn
            .execute_batch(
                "DROP TRIGGER sessions_row_version_bump;\
                 DROP INDEX decision_runs_subject;\
                 ALTER TABLE sessions DROP COLUMN turn_outcome;\
                 DELETE FROM schema_version WHERE version >= 129;",
            )
            .unwrap();
        old.migrate().expect("129 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = || old.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row().turn_outcome, None);
        let before = row().row_version;
        assert!(old.set_turn_outcome(id, Some("asked")).unwrap());
        assert!(
            !old.set_turn_outcome(id, Some("asked")).unwrap(),
            "no change"
        );
        assert_eq!(row().turn_outcome.as_deref(), Some("asked"));
        assert_eq!(row().row_version, before + 1, "the trigger watches it");
        assert!(old.set_turn_outcome(id, Some("done")).is_err());
        assert!(old
            .conn
            .execute(
                "UPDATE sessions SET turn_outcome = 'done' WHERE id = ?1",
                [id]
            )
            .is_err());
        assert!(old.set_turn_outcome(id, None).unwrap());
        let indexed: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
                 AND name = 'decision_runs_subject'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
    }

    /// 091 on a database stopped at 090: existing host_layers rows and managed
    /// inventory rows get the personal catalog's id; a re-run is safe; the
    /// copy preserves axis/position/active, not just the key columns; and
    /// the migrated table still enforces one active role per `(host,
    /// catalog)` while letting two different catalogs each have their own.
    #[test]
    fn migration_91_backfills_catalog_ids() {
        let old = Store::open_in_memory().expect("open");
        old.set_catalog_config("/p", None).unwrap();
        let personal = old.personal_catalog().unwrap().unwrap().id;
        old.upsert_host("h").unwrap();
        // Recreate the 090 shape of host_layers and drop the new inventory
        // column. Non-default axis/position/active values (a context layer,
        // inactive) so the copy is checked on more than the key columns.
        old.conn
            .execute_batch(
                "DROP TABLE host_layers;\
                 CREATE TABLE host_layers (host_alias TEXT NOT NULL REFERENCES hosts(alias), layer_name TEXT NOT NULL, \
                   axis TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0, active INTEGER NOT NULL DEFAULT 1, \
                   PRIMARY KEY (host_alias, layer_name));\
                 INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                   VALUES ('h', 'core', 'role', 2, 1);\
                 INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                   VALUES ('h', 'extra', 'context', 5, 0);\
                 ALTER TABLE asset_inventory DROP COLUMN catalog_id;\
                 INSERT INTO asset_inventory (host_alias, harness, kind, name, state, scanned_at, managed) \
                   VALUES ('h','claude','skill','a','in_sync',1,1), ('h','claude','skill','u','unmanaged',1,0);\
                 DELETE FROM schema_version WHERE version >= 91;",
            )
            .unwrap();
        old.migrate().expect("091");
        // `get_host_layers_for` only returns active rows, so read `extra`
        // (inactive) straight from the table to check its copy too.
        let rows = old.get_host_layers_for("h", personal).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            (
                rows[0].layer_name.as_str(),
                rows[0].axis.as_str(),
                rows[0].position,
                rows[0].active
            ),
            ("core", "role", 2, true),
            "axis/position/active survive the copy"
        );
        let extra: (String, i64, bool) = old
            .conn
            .query_row(
                "SELECT axis, position, active FROM host_layers \
                 WHERE host_alias='h' AND layer_name='extra'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
            .unwrap();
        assert_eq!(extra, ("context".to_string(), 5, false));
        let inv = old.list_inventory().unwrap();
        assert_eq!(
            inv.iter().find(|r| r.name == "a").unwrap().catalog_id,
            Some(personal)
        );
        assert_eq!(inv.iter().find(|r| r.name == "u").unwrap().catalog_id, None);
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 91;")
            .unwrap();
        old.migrate().expect("re-running 091 is safe");
        assert_eq!(old.get_host_layers("h").unwrap().len(), 1);

        // The migrated (not freshly created) table still enforces one active
        // role per (host, catalog): a second active role in `personal` is
        // rejected...
        let dup_in_personal = old.conn.execute(
            "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
             VALUES ('h', ?1, 'core2', 'role', 0, 1)",
            rusqlite::params![personal],
        );
        assert!(
            dup_in_personal.is_err(),
            "a second active role in the same catalog must be rejected"
        );
        // ...but a second catalog gets its own slot: one active role each,
        // both accepted.
        old.conn
            .execute(
                "INSERT INTO orgs (id, name, created_at) VALUES (77, 'acme', 0)",
                [],
            )
            .unwrap();
        old.conn
            .execute(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) \
                 VALUES ('acme', '/acme', 77, 0)",
                [],
            )
            .unwrap();
        let acme: i64 = old
            .conn
            .query_row("SELECT id FROM catalogs WHERE name='acme'", [], |r| {
                r.get(0)
            })
            .unwrap();
        old.conn
            .execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                 VALUES ('h', ?1, 'acme-core', 'role', 0, 1)",
                rusqlite::params![acme],
            )
            .expect("a different catalog gets its own active-role slot");
        let dup_in_acme = old.conn.execute(
            "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
             VALUES ('h', ?1, 'acme-core2', 'role', 0, 1)",
            rusqlite::params![acme],
        );
        assert!(
            dup_in_acme.is_err(),
            "two active roles in the same (host, catalog) must still be rejected"
        );
    }

    /// A `host_layers` row already dangling before 091 (its `hosts` row gone,
    /// as a hand edit with `foreign_keys = OFF` can leave) must not resurface
    /// as a brand-new "added" FK violation after 091's copy-out/drop/recreate
    /// rebuild. The rebuild renumbers FK ids (the `hosts` FK moves from fkid
    /// 0 to fkid 1 on the new two-FK table) and reassigns rowids, both of
    /// which `apply_migrations` uses to tell "pre-existing" from "added" —
    /// so a naive copy would turn an old, harmless dangling row into a fatal
    /// startup error (final-review I1). 091 copies forward only rows whose
    /// host still exists, so the dangling row is dropped, never carried.
    #[test]
    fn migration_91_drops_a_preexisting_dangling_host_layers_row() {
        let old = store_at_version(90);
        // Raw SQL, not `set_catalog_config`: at version 90 there is no
        // `client_catalog_grants` table for its grant backfill to write to.
        old.conn
            .execute_batch(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('personal', '/p', NULL, 0);",
            )
            .unwrap();
        // Raw SQL, not `upsert_host`: a typed host read selects HOST_COLUMNS,
        // which names every column of the CURRENT schema, so calling it on a
        // store pinned to an older version breaks as soon as any later
        // migration adds a host column. The 089 test inserts this way for the
        // same reason; this test is about `host_layers`, not the host API.
        old.conn
            .execute_batch(
                "INSERT INTO hosts (alias, reachable) VALUES ('h', 1);\
                 INSERT INTO host_layers (host_alias, layer_name, axis) VALUES ('h', 'core', 'role');\
                 PRAGMA foreign_keys = OFF;\
                 INSERT INTO host_layers (host_alias, layer_name, axis) VALUES ('ghost', 'core', 'role');",
            )
            .unwrap();
        old.migrate()
            .expect("a pre-existing dangling host_layers row must not brick startup");
        old.conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let rows = old.get_host_layers("h").unwrap();
        assert_eq!(rows.len(), 1, "the real row survives");
        let ghost: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM host_layers WHERE host_alias = 'ghost'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ghost, 0, "the dangling row is not carried forward");
    }

    /// 093 on a database stopped at 092: every client with `assets_admin_at`
    /// gets a grant on `personal` at that time (spec, Migration step 3); a
    /// client without one gets none; re-running is a no-op.
    #[test]
    fn migration_93_backfills_personal_grants_from_assets_admin_at() {
        let old = store_at_version(92);
        old.conn
            .execute_batch(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('personal', '/p', NULL, 0);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('desk', 'h1', 'full', 1, 77);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at) VALUES ('plain', 'h2', 'full', 1);",
            )
            .unwrap();
        old.migrate().expect("093");
        let grants = |s: &Store| -> Vec<(String, i64)> {
            s.conn
                .prepare(
                    "SELECT t.name, g.granted_at FROM client_catalog_grants g \
                     JOIN client_tokens t ON t.id = g.client_id ORDER BY t.name",
                )
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(grants(&old), vec![("desk".to_string(), 77)]);
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 93;")
            .unwrap();
        old.migrate().expect("re-running 093 is safe");
        assert_eq!(grants(&old).len(), 1);
    }

    /// No personal catalog yet: nothing to attach a grant to, so 093 writes
    /// none (the grant waits in `assets_admin_at`, Rulings R2).
    #[test]
    fn migration_93_without_a_personal_catalog_backfills_nothing() {
        let old = store_at_version(92);
        old.conn
            .execute_batch(
                "INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('desk', 'h1', 'full', 1, 77);",
            )
            .unwrap();
        old.migrate().expect("093");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let n: i64 = old
            .conn
            .query_row("SELECT COUNT(*) FROM client_catalog_grants", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0);
    }

    /// Carry 3c (Rulings R30): 093 backfills a grant for every
    /// `assets_admin_at` holder, eligible or not — revoked, readonly and
    /// org-bound ones too — and those rows grant nothing: the live predicate
    /// refuses them and `catalog_grantees` never lists them.
    #[test]
    fn migration_93_backfills_ineligible_holders_but_they_grant_nothing() {
        let old = store_at_version(92);
        old.conn
            .execute_batch(
                "INSERT INTO catalogs (id, name, repo_path, org_id, created_at) VALUES (1, 'personal', '/p', NULL, 0);\
                 INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at, revoked_at) \
                   VALUES ('gone', 'h1', 'full', 1, 5, 9);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('kiosk', 'h2', 'readonly', 1, 5);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at, org_id) \
                   VALUES ('contractor', 'h3', 'full', 1, 5, 10);\
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, assets_admin_at) \
                   VALUES ('desk', 'h4', 'full', 1, 5);",
            )
            .unwrap();
        old.migrate().expect("093 and 094");
        let n: i64 = old
            .conn
            .query_row("SELECT COUNT(*) FROM client_catalog_grants", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 4, "the backfill copies every holder");
        let id = |name: &str| -> i64 {
            old.conn
                .query_row(
                    "SELECT id FROM client_tokens WHERE name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap()
        };
        for name in ["gone", "kiosk", "contractor"] {
            assert!(
                !old.client_may_admin_catalog(id(name), 1).unwrap(),
                "{name} is not eligible"
            );
        }
        assert!(old.client_may_admin_catalog(id("desk"), 1).unwrap());
        assert_eq!(old.catalog_grantees(1).unwrap(), vec!["desk".to_string()]);
    }

    /// 094 on a database stopped at 093 creates the spec's three tables,
    /// column for column; re-running it is a no-op.
    #[test]
    fn migration_94_creates_changesets_items_and_verdicts() {
        let old = store_at_version(93);
        old.migrate().expect("094");
        let cols = |t: &str| -> Vec<String> {
            old.conn
                .prepare(&format!(
                    "SELECT name FROM pragma_table_info('{t}') ORDER BY cid"
                ))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            cols("changesets"),
            [
                "id",
                "kind",
                "summary",
                "state",
                "created_at",
                "applied_at",
                "commits",
                "layers_snapshot",
                "error",
                // 103 (Assets M6): `migrate` runs on to the latest.
                "withdrawn_at"
            ]
        );
        assert_eq!(
            cols("changeset_items"),
            [
                "changeset_id",
                "position",
                "grp",
                "catalog_id",
                "kind",
                "name",
                "action",
                "params",
                "decider",
                "state",
                // 097 (Assets M5), 102 (Assets M6): `migrate` runs on to the latest.
                "decided_at",
                "outcome"
            ]
        );
        assert_eq!(
            cols("asset_triage_verdicts"),
            [
                "catalog_id",
                "kind",
                "name",
                "content_hash",
                "verdict",
                "decider",
                "decided_at"
            ]
        );
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 94;")
            .unwrap();
        old.migrate().expect("re-running 094 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// M2 carry 6 / Rulings R1: 091 must not assume `host_layers` exists — a
    /// database from the 033 collision family can reach 091 without it, and
    /// `repair_skipped_main_migrations` (which recreates it) runs only after
    /// every pending migration.
    #[test]
    fn migration_91_recreates_a_missing_host_layers_table() {
        let old = store_at_version(90);
        old.conn.execute_batch("DROP TABLE host_layers;").unwrap();
        old.migrate()
            .expect("091 must not assume host_layers exists");
        let n: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('host_layers') WHERE name = 'catalog_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    /// 034 on a database stopped at 033 with a host row: `transport` is
    /// added, defaulting to `ssh` for the existing row. Rolling the recorded
    /// version back and migrating again (tests do this to simulate re-running
    /// an already-applied migration) is a no-op that keeps a changed value
    /// and does not re-add the column.
    /// The 033 collision, forward direction: a database at main's RELEASED
    /// schema (33, with `host_layers` and no `transport`) migrates to the
    /// merged head, and 034's `ALTER TABLE` runs exactly once.
    #[test]
    fn a_released_main_db_migrates_to_the_merged_head() {
        let old = store_at_version(33);
        old.conn
            .execute_batch("INSERT INTO hosts (alias) VALUES ('h');")
            .unwrap();
        // main's 033 really did run: the layers table is already there.
        old.conn
            .execute_batch(
                "INSERT INTO host_layers (host_alias, layer_name, axis) \
                 VALUES ('h', 'base', 'role');",
            )
            .unwrap();
        // 091 attaches a pre-existing host_layers row to the personal
        // catalog; without a `catalog_config` row to seed it, 090 creates no
        // personal catalog and the row would be dropped by 091's rebuild.
        old.conn
            .execute_batch("INSERT INTO catalog_config (id, repo_path) VALUES (1, '/p');")
            .unwrap();
        old.migrate().expect("merged head on a released main DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let transport: String = old
            .conn
            .query_row("SELECT transport FROM hosts WHERE alias='h'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(transport, "ssh", "034 added the column, defaulting to ssh");
        let n: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'transport'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "034's ALTER TABLE ran exactly once");
        // 035 is a no-op here: the pre-existing layer row survives.
        let rows: i64 = old
            .conn
            .query_row("SELECT COUNT(*) FROM host_layers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "035 did not clobber the existing layers table");
    }

    /// The 033 collision, from the other side: a database created by the
    /// PRE-MERGE host-agent branch, where `033` was host_transport. It
    /// records version 33 and already has `transport`, so on the merged head
    /// `migrate()` never offers 033 (asset_layers) — 33 is not `> 33` — and
    /// 034 is skipped by its `already_applied` guard. Migration 035 is what
    /// puts `host_layers` there anyway.
    #[test]
    fn a_pre_merge_branch_db_still_gets_the_layers_table() {
        let old = store_at_version(32);
        // Replay exactly what the pre-merge branch's 033 did.
        old.conn
            .execute_batch(
                "ALTER TABLE hosts ADD COLUMN transport TEXT NOT NULL DEFAULT 'ssh';
                 INSERT OR IGNORE INTO schema_version (version) VALUES (33);",
            )
            .unwrap();
        old.migrate().expect("merged head on a pre-merge branch DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        // The table main's 033 would have created is present…
        old.conn
            .execute_batch("INSERT INTO hosts (alias) VALUES ('h');")
            .unwrap();
        // host_layers now carries a NOT NULL catalog_id (091): a catalog
        // must exist before a row can reference one.
        old.set_catalog_config("/p", None).unwrap();
        let personal = old.personal_catalog().unwrap().unwrap().id;
        old.conn
            .execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis) \
                 VALUES ('h', ?1, 'base', 'role');",
                [personal],
            )
            .expect("host_layers exists and accepts a row");
        // …and its unique-active-role index came with it.
        let err = old.conn.execute(
            "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis) \
             VALUES ('h', ?1, 'other', 'role');",
            [personal],
        );
        assert!(err.is_err(), "the active-role index is in place");
        // The column the branch had already added was not added twice.
        let n: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'transport'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "034 did not re-add a column the branch already had");
    }

    /// A database from an earlier build of the conversation-tracking branch,
    /// which recorded the conversations migration as `recorded_as` after
    /// running `main`'s migrations up to `main_upto`.
    fn conversation_branch_db(main_upto: i64, recorded_as: i64) -> Store {
        let old = store_at_version(main_upto);
        old.conn
            .execute_batch("INSERT INTO hosts (alias) VALUES ('h');")
            .unwrap();
        let conv = include_str!("../../migrations/037_conversations.sql");
        let branch = conv.replace(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (37);",
            &format!("INSERT OR IGNORE INTO schema_version (version) VALUES ({recorded_as});"),
        );
        assert_ne!(branch, conv);
        old.conn.execute_batch(&branch).unwrap();
        old
    }

    /// Every column `main`'s 034 and 036 add, and 035's tables, are there.
    fn assert_main_034_to_036_applied(s: &Store) {
        for (table, column) in [
            ("hosts", "transport"),
            ("hosts", "boot_id"),
            ("hosts", "tmux_server_pid"),
            ("sessions", "lost_reason"),
        ] {
            let n: i64 = s
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                    [table, column],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table}.{column}");
        }
        assert!(s.has_table("host_layers").unwrap());
        let transport: String = s
            .conn
            .query_row("SELECT transport FROM hosts WHERE alias = 'h'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(transport, "ssh");
        assert!(s.has_table("conversations").unwrap());
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Pre-merge branch: `main` up to 033, conversations recorded as 34.
    /// 035 and 036 run as pending migrations, 037 is guarded out, and the
    /// repair adds the `hosts.transport` 034 never got to add.
    #[test]
    fn a_conversation_branch_db_recorded_as_34_gets_main_034_to_036() {
        let old = conversation_branch_db(33, 34);
        old.migrate()
            .expect("merged head on a pre-merge conversation-branch DB");
        assert_main_034_to_036_applied(&old);
        // Idempotent: a second start changes nothing and does not fail.
        old.migrate().expect("second migrate");
        assert_main_034_to_036_applied(&old);
    }

    /// Intermediate branch: `main` up to 035, conversations recorded as 36.
    /// Only 037 is pending and it is guarded out, so without the repair
    /// `main`'s 036 columns would never be added.
    #[test]
    fn a_conversation_branch_db_recorded_as_36_gets_main_036() {
        let old = conversation_branch_db(35, 36);
        old.migrate()
            .expect("merged head on an intermediate conversation-branch DB");
        assert_main_034_to_036_applied(&old);
        old.migrate().expect("second migrate");
        assert_main_034_to_036_applied(&old);
    }

    #[test]
    fn migration_034_is_idempotent() {
        let old = store_at_version(33);
        old.conn
            .execute_batch("INSERT INTO hosts (alias) VALUES ('h');")
            .unwrap();
        old.migrate().expect("034 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let transport: String = old
            .conn
            .query_row("SELECT transport FROM hosts WHERE alias='h'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(transport, "ssh", "a pre-034 row defaults to ssh");
        old.conn
            .execute("UPDATE hosts SET transport='agent' WHERE alias='h'", [])
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 34;")
            .unwrap();
        old.migrate().expect("re-running 034 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let n: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('hosts') WHERE name = 'transport'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "re-running 034 does not duplicate the column");
        let transport: String = old
            .conn
            .query_row("SELECT transport FROM hosts WHERE alias='h'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(transport, "agent", "the value survives a re-run");
    }

    #[test]
    fn migration_008_adds_lost_at_column() {
        let store = Store::open_in_memory().expect("store");
        let v: i64 = store
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            v, LATEST_SCHEMA_VERSION,
            "schema_version should be current after migration"
        );
        // Column exists and defaults to NULL
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let lost: Option<i64> = store
            .conn
            .query_row(
                "SELECT lost_at FROM sessions WHERE tmux_name='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lost, None, "lost_at should be NULL for a fresh session");
    }

    // ── migration 020: orchestration fields, tasks, reply_to ──

    #[test]
    fn migration_020_adds_orchestration_columns_with_defaults() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.turn_seq, 0);
        assert_eq!(row.last_stop_at, None);
        assert_eq!(row.parent_session_id, None);
        assert!(row.tags.is_empty());
        assert!(s.has_table("tasks").unwrap());
    }

    #[test]
    fn migration_023_fresh_db_stores_and_updates_parent_fingerprints() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.has_table("worktree_parent_fingerprints").unwrap());
        assert_eq!(s.parent_fingerprint("local", "/r/w").unwrap(), None);
        s.record_parent_fingerprint("local", "/r/w", "42:7", 1)
            .unwrap();
        assert_eq!(
            s.parent_fingerprint("local", "/r/w").unwrap().as_deref(),
            Some("42:7")
        );
        // Re-recording replaces; other hosts / paths are separate keys.
        s.record_parent_fingerprint("local", "/r/w", "42:9", 2)
            .unwrap();
        s.record_parent_fingerprint("vps", "/r/w", "1:1", 2)
            .unwrap();
        assert_eq!(
            s.parent_fingerprint("local", "/r/w").unwrap().as_deref(),
            Some("42:9")
        );
        assert_eq!(
            s.parent_fingerprint("vps", "/r/w").unwrap().as_deref(),
            Some("1:1")
        );
    }

    #[test]
    fn migration_023_upgrades_an_existing_db_and_keeps_its_rows() {
        // Seed a real v22 database from MIGRATIONS (as the 022 test does), so
        // no later migration ever needs an edit here.
        const SEED_AT: i64 = 22;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for &Migration { version, sql, .. } in MIGRATIONS.iter().filter(|m| m.version <= SEED_AT) {
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("migration {version}: {e}"));
        }
        conn.execute_batch(
            "INSERT OR IGNORE INTO hosts (alias) VALUES ('local');
             INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
               VALUES ('sess', 'local', 1, 1, 'running');",
        )
        .unwrap();
        let s = Store {
            conn,
            bus: StoreBus::new(Arc::new(NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            peer_generations: Default::default(),
            instance: super::next_instance(),
        };
        assert_eq!(s.schema_version().unwrap(), SEED_AT);
        assert!(!s.has_table("worktree_parent_fingerprints").unwrap());
        s.migrate().unwrap();
        assert!(s.has_table("worktree_parent_fingerprints").unwrap());
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(
            s.get_session("sess", "local").unwrap().is_some(),
            "rows survive"
        );
    }

    #[test]
    fn migration_023_double_migrate_keeps_recorded_fingerprints() {
        let s = Store::open_in_memory().unwrap();
        s.record_parent_fingerprint("local", "/r/w", "42:7", 1)
            .unwrap();
        s.migrate().unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            s.parent_fingerprint("local", "/r/w").unwrap().as_deref(),
            Some("42:7")
        );
    }

    // ── migration 019: lifecycle + outcome fields ──

    #[test]
    fn migration_019_adds_lifecycle_columns_defaulting_to_null() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.idle_since, None);
        assert_eq!(row.stuck_since, None);
        assert_eq!(row.last_playbook_at, None);
        assert_eq!(row.last_prompt, None);
        assert_eq!(row.started_at, None);
        assert_eq!(row.last_turn_at, None);
        assert_eq!(row.ci_status, None);
    }

    // ── migration 043: participants, delivery columns, block streak ──

    /// Migration 043's shape: `participants` exists with `retired_at`, the
    /// delivery columns land on `session_messages`, and `sessions` gets its
    /// block-streak counter. Deliberately shape-only — the empty in-memory
    /// store has no sessions, so a join against `participants` would be
    /// vacuous; the backfill itself is proved by
    /// `migration_043_backfills_existing_sessions` below.
    #[test]
    fn migration_043_creates_participants_shape() {
        let s = Store::open_in_memory().unwrap();
        for (table, col) in [
            ("participants", "retired_at"),
            ("session_messages", "from_participant_id"),
            ("session_messages", "to_participant_id"),
            ("session_messages", "delivered_at"),
            ("sessions", "stop_block_streak"),
        ] {
            let n: i64 = s
                .conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{col}'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table}.{col} missing after migration 043");
        }
    }

    /// A session that predates migration 043 gets a `participants` row of
    /// `kind='session'`, not retired, once 043 runs — the whole point of the
    /// backfill (a message can address it). Also proves the backfill
    /// `INSERT ... WHERE id NOT IN (SELECT session_id FROM participants
    /// ...)` is itself idempotent: re-running just that statement, and
    /// separately a full guarded second pass through `migrate()`, must not
    /// duplicate the row.
    /// Insert a bare `sessions` row with raw SQL. For a store held at an old
    /// schema version: `upsert_session` reads the row back through
    /// `SESSION_COLUMNS`, which names tables (participants, work_links) that
    /// an old version does not have yet.
    fn raw_session(s: &Store, name: &str) -> i64 {
        s.conn
            .execute(
                "INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status) \
                 VALUES (?1, 'h', 1, 1, 'running')",
                rusqlite::params![name],
            )
            .unwrap();
        s.conn.last_insert_rowid()
    }

    #[test]
    fn migration_043_backfills_existing_sessions() {
        let old = store_at_version(42);
        old.conn
            .execute("INSERT INTO hosts (alias) VALUES ('h')", [])
            .unwrap();
        let sid = raw_session(&old, "sess");

        old.migrate().expect("043 backfill");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

        let (kind, retired): (String, Option<i64>) = old
            .conn
            .query_row(
                "SELECT kind, retired_at FROM participants WHERE session_id = ?1",
                rusqlite::params![sid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "session");
        assert_eq!(retired, None);

        // Re-run just the backfill INSERT (what a second pass would execute
        // if the ADD COLUMNs were not already there to guard it out): the
        // `WHERE id NOT IN (...)` clause must keep this a no-op.
        old.conn
            .execute_batch(
                "INSERT INTO participants (kind, session_id, created_at)
                   SELECT 'session', id, strftime('%s','now') FROM sessions
                   WHERE id NOT IN (SELECT session_id FROM participants WHERE session_id IS NOT NULL);",
            )
            .unwrap();
        let count: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM participants WHERE session_id = ?1",
                rusqlite::params![sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "backfill INSERT ... WHERE NOT IN must not duplicate the participant"
        );

        // A full guarded second pass: roll the recorded version back and
        // migrate again. `messages_have_participant_columns` now sees the
        // ADD COLUMNs already applied, so 043's whole body is skipped and
        // only its version is re-recorded — must not error or duplicate.
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 43;")
            .unwrap();
        old.migrate().expect("guarded second pass over 043");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let count: i64 = old
            .conn
            .query_row(
                "SELECT COUNT(*) FROM participants WHERE session_id = ?1",
                rusqlite::params![sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "guarded second pass over 043 must not duplicate the participant"
        );
    }

    /// Migration 045 backfills the sessions created after 043 that never
    /// messaged anyone (and so never got a participant), and from then on the
    /// trigger mints one per insert. Re-running it duplicates nothing.
    #[test]
    fn migration_045_gives_every_session_a_participant() {
        let old = store_at_version(44);
        old.conn
            .execute("INSERT INTO hosts (alias) VALUES ('h')", [])
            .unwrap();
        let quiet = raw_session(&old, "quiet");
        let count = |s: &Store, sid: i64| -> i64 {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM participants WHERE session_id = ?1",
                    rusqlite::params![sid],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(count(&old, quiet), 0, "044 minted nothing on insert");

        old.migrate().expect("045");
        assert_eq!(count(&old, quiet), 1, "the backfill gave it one");
        let later = old
            .upsert_session("later", "h", None, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(count(&old, later), 1, "the trigger mints on insert");

        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 45;")
            .unwrap();
        old.migrate().expect("re-run 045");
        assert_eq!(count(&old, quiet), 1);
        assert_eq!(count(&old, later), 1);
    }

    /// Migration 048 (work graph M3): the tracker tables and the new
    /// `work_items` columns land on a database with work items, and a re-run
    /// (the guard) keeps both the tracker and the item's new attributes.
    #[test]
    fn migration_048_adds_trackers_and_item_columns_and_reruns_safely() {
        let old = store_at_version(47);
        old.conn
            .execute_batch(
                "INSERT INTO work_items (source, key, title, created_at, updated_at) \
                 VALUES ('local', 'ABC-1', 'x', 1, 1);",
            )
            .unwrap();
        old.migrate().expect("048 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let t = old
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap();
        old.conn
            .execute_batch(
                "UPDATE work_items SET aliases = '[\"OLD-1\"]', status_name = 'In Review';",
            )
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 48;")
            .unwrap();
        old.migrate().expect("re-running 048 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(old.get_tracker(t.id).unwrap().is_some());
        let item = old.work_item_by_key("ABC-1").unwrap().unwrap();
        assert_eq!(item.aliases, vec!["OLD-1"]);
        assert_eq!(item.status_name.as_deref(), Some("In Review"));
    }

    /// Migration 049 (work graph M4): the detection columns land on a
    /// database with links, old links read with no strength / evidence, and a
    /// re-run (the guard) keeps what was written since.
    #[test]
    fn migration_049_adds_detection_columns_and_reruns_safely() {
        let old = store_at_version(48);
        // Raw SQL: the store's own session reads already expect 049.
        old.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h'); \
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status) \
                 VALUES ('dev', 'h', 1, 1, 'running');",
            )
            .unwrap();
        let sid: i64 = old
            .conn
            .query_row("SELECT id FROM sessions", [], |r| r.get(0))
            .unwrap();
        old.conn
            .execute_batch(
                "INSERT INTO work_links (ref_key, participant_id, state, source, is_primary, created_at) \
                 SELECT 'ABC-1', id, 'confirmed', 'manual', 1, 1 FROM participants;",
            )
            .unwrap();
        old.migrate().expect("049 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let links = old.session_work_links(sid).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].strength, None);
        assert!(links[0].evidence.is_empty());
        old.conn
            .execute_batch("UPDATE sessions SET current_branch = 'abc-2-x';")
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 49;")
            .unwrap();
        old.migrate().expect("re-running 049 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let branch: Option<String> = old
            .conn
            .query_row(
                "SELECT current_branch FROM sessions WHERE id = ?1",
                [sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(branch.as_deref(), Some("abc-2-x"));
    }

    /// Migration 051 (work graph M6): a tracker with views keeps them and
    /// its watermark, gains an empty sync mark and settings, and a re-run
    /// (the guard) keeps what was written since.
    #[test]
    fn migration_051_adds_sync_marks_and_settings_and_reruns_safely() {
        let old = store_at_version(50);
        old.conn
            .execute_batch(
                "INSERT INTO trackers (id, provider, name, site_url, created_at) \
                 VALUES (3, 'jira', 'Acme', 'https://acme.atlassian.net', 1); \
                 INSERT INTO tracker_views (tracker_id, view_id, label, query, watermark) \
                 VALUES (3, 'mine', 'My work', 'q', 1700000000);",
            )
            .unwrap();
        old.migrate().expect("051 on an existing DB");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let views = old.list_tracker_views(3).unwrap();
        assert_eq!(views[0].watermark, Some(1_700_000_000));
        assert_eq!(views[0].sync_mark, None);
        assert_eq!(
            old.get_tracker(3).unwrap().unwrap().settings,
            Default::default()
        );
        old.set_tracker_view_mark(3, "mine", Some("tok-1")).unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 51;")
            .unwrap();
        old.migrate().expect("re-running 051 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            old.list_tracker_views(3).unwrap()[0].sync_mark.as_deref(),
            Some("tok-1")
        );
    }

    /// Migrations 052 and 053 (work graph M7) on a database that already ran
    /// M6's 051: the lifecycle columns land on a database with a live link, a
    /// done item and a tracker with M6's sync mark and settings; nothing is
    /// archived, snoozed or reopened by the migration itself, M6's columns
    /// are untouched, and a re-run (the guards) keeps what was written since.
    #[test]
    fn migrations_052_053_add_lifecycle_columns_after_051_and_rerun_safely() {
        let old = store_at_version(51);
        assert_eq!(old.schema_version().unwrap(), 51);
        old.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h'); \
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status) \
                 VALUES ('dev', 'h', 1, 1, 'running'); \
                 INSERT INTO work_items (source, key, title, status_category, created_at, updated_at) \
                 VALUES ('local', 'ABC-1', 't', 'done', 1, 1); \
                 INSERT INTO work_links (item_id, participant_id, state, source, is_primary, created_at) \
                 SELECT 1, id, 'confirmed', 'manual', 1, 1 FROM participants; \
                 INSERT INTO trackers (id, provider, name, site_url, created_at) \
                 VALUES (3, 'jira', 'Acme', 'https://acme.atlassian.net', 1); \
                 INSERT INTO tracker_views (tracker_id, view_id, label, query, sync_mark) \
                 VALUES (3, 'mine', 'My work', 'q', 'tok-0');",
            )
            .unwrap();
        let sid: i64 = old
            .conn
            .query_row("SELECT id FROM sessions", [], |r| r.get(0))
            .unwrap();
        old.migrate().expect("052 and 053 on a database at 051");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            old.list_tracker_views(3).unwrap()[0].sync_mark.as_deref(),
            Some("tok-0")
        );
        let (archived, snoozed, never): (Option<i64>, Option<i64>, i64) = old
            .conn
            .query_row(
                "SELECT archived_at, tidy_snoozed_until, tidy_never FROM work_links",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((archived, snoozed, never), (None, None, 0));
        let row = old.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.work.unwrap().archived_at, None);
        old.conn
            .execute_batch(
                "UPDATE work_links SET archived_at = 7; UPDATE sessions SET last_touch_at = 9;",
            )
            .unwrap();
        old.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 52;")
            .unwrap();
        old.migrate().expect("re-running 052 is safe");
        assert_eq!(old.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let row = old.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.work.unwrap().archived_at, Some(7));
        let reopened: Option<i64> = old
            .conn
            .query_row("SELECT reopened_at FROM work_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reopened, None);
        // 053: an org's override, NULL (inherit) on existing orgs.
        old.conn
            .execute_batch(
                "INSERT INTO orgs (name, created_at) VALUES ('A', 1); \
                 DELETE FROM schema_version WHERE version >= 53;",
            )
            .unwrap();
        old.migrate().expect("re-running 053 is safe");
        assert_eq!(old.list_orgs().unwrap()[0].auto_tidy, None);
    }

    // ── migration 054: peer_links, remote participants, outbox columns ──

    /// A guarded second pass over 054: roll the recorded version back and
    /// migrate again. `participants_have_address` sees the ADD COLUMNs
    /// already applied, so 054's whole body is skipped and only its version
    /// is re-recorded — must not error, and `participants.address` must
    /// still exist exactly once.
    #[test]
    fn migration_054_guarded_second_pass_does_not_error() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 54;")
            .unwrap();
        s.migrate().expect("guarded second pass over 054");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('participants') WHERE name = 'address'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "participants.address must exist exactly once");
    }

    /// Migration 054 on a POPULATED v53 database: sessions, participants and
    /// messages survive with their ids, every new column reads NULL or its
    /// default, and the partial unique indexes exist.
    #[test]
    fn migration_054_upgrades_a_populated_v53_db_and_keeps_its_rows() {
        const SEED_AT: i64 = 53;
        let s = store_at_version(SEED_AT);
        // Since 045 a session's participant is minted by trigger on insert;
        // renumber the two it mints so the messages below can name them.
        s.conn
            .execute_batch(
                "INSERT OR IGNORE INTO hosts (alias) VALUES ('local');
             INSERT INTO sessions (id, tmux_name, host_alias, created_at, last_activity_at, status)
               VALUES (11, 'a1', 'local', 1, 1, 'running'),
                      (12, 'b1', 'local', 1, 1, 'running');
             UPDATE participants SET id = 21 WHERE session_id = 11;
             UPDATE participants SET id = 22 WHERE session_id = 12;
             INSERT INTO session_messages
               (id, from_session_id, to_session_id, from_participant_id, to_participant_id,
                body, kind, sent_at, reply_to, delivered_at)
               VALUES (31, 11, 12, 21, 22, 'hello', 'message', 5, NULL, 6),
                      (32, 12, 11, 22, 21, 'back', 'task_result', 7, 31, NULL);",
            )
            .unwrap();
        assert_eq!(s.schema_version().unwrap(), SEED_AT);
        assert!(!s.has_table("peer_links").unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(s.has_table("peer_links").unwrap());

        let sessions: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id IN (11, 12)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sessions, 2, "sessions survive");
        type Part = (i64, Option<i64>, Option<String>, Option<i64>);
        let parts: Vec<Part> = s
            .conn
            .prepare("SELECT id, session_id, address, peer_link_id FROM participants ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            parts,
            vec![(21, Some(11), None, None), (22, Some(12), None, None)],
            "participants survive; address and peer_link_id are NULL"
        );
        type Row = (
            i64,
            String,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<i64>,
            Option<String>,
            i64,
            Option<String>,
        );
        let msgs: Vec<Row> = s
            .conn
            .prepare(
                "SELECT id, body, reply_to, delivered_at, remote_fleet_id, remote_message_id, \
                        peer_state, peer_wake, peer_from_addr \
                 FROM session_messages ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            msgs,
            vec![
                (31, "hello".into(), None, Some(6), None, None, None, 0, None),
                (32, "back".into(), Some(31), None, None, None, None, 0, None),
            ],
            "messages survive; the remote and outbox columns are NULL / 0"
        );
        // Local rows read as before: the inbox is unchanged.
        let inbox = s.list_inbox(12, false, 10).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].body, "hello");
        assert!(inbox[0].from_addr.is_none());

        let partial_unique = |name: &str| -> (i64, Option<String>) {
            s.conn
                .query_row(
                    "SELECT il.\"unique\", m.sql FROM sqlite_master m \
                       JOIN pragma_index_list(m.tbl_name) il ON il.name = m.name \
                      WHERE m.type = 'index' AND m.name = ?1",
                    [name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap_or_else(|e| panic!("index {name}: {e}"))
        };
        for name in [
            "idx_peer_links_live_fleet",
            "idx_peer_links_client",
            "idx_participants_address",
            "idx_session_messages_remote",
        ] {
            let (unique, sql) = partial_unique(name);
            assert_eq!(unique, 1, "{name} is unique");
            assert!(
                sql.as_deref().unwrap_or("").contains(" WHERE "),
                "{name} is partial: {sql:?}"
            );
        }
        let (unique, _) = partial_unique("idx_session_messages_peer_pending");
        assert_eq!(unique, 0, "the pending index is a plain partial index");
    }

    /// G17: `pending_outbox`, `has_pending_outbox`, `mark_peer_accepted`,
    /// `handover_upto` and `peer_links_down`'s listener branch all join
    /// `participants ON participants.peer_link_id = peer_links.id` — every
    /// poll of every dialer loop and every listener exchange. Migration 055
    /// gives that join an index; this pins that it exists, is partial (only
    /// the `remote` rows that ever set the column), and is a plain (not
    /// unique) index — more than one participant can belong to the same
    /// link.
    #[test]
    fn migration_055_adds_a_partial_index_on_participants_peer_link_id() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let (unique, sql): (i64, Option<String>) = s
            .conn
            .query_row(
                "SELECT il.\"unique\", m.sql FROM sqlite_master m \
                   JOIN pragma_index_list(m.tbl_name) il ON il.name = m.name \
                  WHERE m.type = 'index' AND m.name = 'idx_participants_peer_link'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(unique, 0, "more than one participant can share a link");
        let sql = sql.unwrap_or_default();
        assert!(sql.contains("peer_link_id"), "{sql}");
        assert!(sql.contains(" WHERE "), "partial: {sql}");
    }

    /// Migration 044 was rewritten after it landed; a database whose 044
    /// ran without `trg_read_cursors_on_session_delete` recorded 44 all the
    /// same and never got the trigger. Migration 057 re-issues it: such a
    /// database gains it on the next open, and a session's cursors then die
    /// with its row as designed.
    #[test]
    fn migration_057_restores_the_read_cursor_trigger_a_rewritten_044_left_out() {
        let trigger_exists = |s: &Store| -> bool {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                      WHERE type = 'trigger' AND name = 'trg_read_cursors_on_session_delete'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
                == 1
        };
        let s = store_at_version(56);
        // The intermediate 044: table and indexes, no trigger.
        s.conn
            .execute_batch("DROP TRIGGER trg_read_cursors_on_session_delete;")
            .unwrap();
        assert!(!trigger_exists(&s));
        assert_eq!(s.schema_version().unwrap(), 56);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(trigger_exists(&s), "057 re-issued the trigger");
        // And it does its job: a deleted session takes its cursors with it,
        // as the reader and as the target.
        s.conn
            .execute_batch(
                "INSERT OR IGNORE INTO hosts (alias) VALUES ('local');
                 INSERT INTO sessions (id, tmux_name, host_alias, created_at, last_activity_at, status)
                   VALUES (1, 'a1', 'local', 1, 1, 'running'),
                          (2, 'a2', 'local', 1, 1, 'running');
                 INSERT INTO read_cursors (reader_session_id, tool, resource_key, target_session_id, watermark, updated_at)
                   VALUES (1, 'inbox', '2:false', 2, 5, 1),
                          (2, 'inbox', '1:false', 1, 5, 1),
                          (2, 'list_sessions', '', NULL, NULL, 1);
                 DELETE FROM sessions WHERE id = 1;",
            )
            .unwrap();
        let left: Vec<(i64, String)> = s
            .conn
            .prepare("SELECT reader_session_id, tool FROM read_cursors ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(left, vec![(2, "list_sessions".to_string())]);
        // A database that has the trigger already is untouched by a re-run.
        s.conn
            .execute_batch(include_str!(
                "../../migrations/057_read_cursors_trigger.sql"
            ))
            .unwrap();
        assert!(trigger_exists(&s));
    }

    /// Migration 055 on a database that already has migration 054 and rows
    /// in `participants`: it must not disturb them, and running it twice
    /// (`CREATE INDEX IF NOT EXISTS`) is a no-op.
    #[test]
    fn migration_055_on_a_populated_v54_database_is_a_safe_reindex() {
        const SEED_AT: i64 = 54;
        let s = store_at_version(SEED_AT);
        // The session's own participant is minted by the 045 trigger.
        s.conn
            .execute_batch(
                "INSERT OR IGNORE INTO hosts (alias) VALUES ('local');
                 INSERT INTO sessions (id, tmux_name, host_alias, created_at, last_activity_at, status)
                   VALUES (1, 'a1', 'local', 1, 1, 'running');
                 INSERT INTO participants (id, kind, address, peer_link_id, created_at)
                   VALUES (2, 'remote', 'fleet-b/session/h/b1', NULL, 1);",
            )
            .unwrap();
        assert_eq!(s.schema_version().unwrap(), SEED_AT);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        // Re-running the migration script directly (as `migrate()` would on
        // a database that already recorded 55) must not fail.
        s.conn
            .execute_batch(include_str!("../../migrations/055_peer_link_index.sql"))
            .unwrap();
        let rows: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM participants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "the re-run touched no rows");
    }

    /// Hub store latency, task 6: `alive_sessions_for_worktree` (called once
    /// per worktree by the old `list_worktrees` N+1, and inline by
    /// `delete_worktree`'s occupant guard) filters
    /// `worktree_id=? AND status='running' AND lost_at IS NULL` with no index
    /// on `worktree_id`. Migration 059 gives that a partial index; this pins
    /// that it exists, is partial (only live rows — the minority once a
    /// fleet has run a while), and is a plain (not unique) index — more than
    /// one session can point at the same worktree while races settle.
    #[test]
    fn migration_059_adds_a_partial_index_on_sessions_worktree_id() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let (unique, sql): (i64, Option<String>) = s
            .conn
            .query_row(
                "SELECT il.\"unique\", m.sql FROM sqlite_master m \
                   JOIN pragma_index_list(m.tbl_name) il ON il.name = m.name \
                  WHERE m.type = 'index' AND m.name = 'idx_sessions_worktree_live'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(unique, 0, "more than one session can share a worktree_id");
        let sql = sql.unwrap_or_default();
        assert!(sql.contains("worktree_id"), "{sql}");
        assert!(sql.contains(" WHERE "), "partial: {sql}");
    }

    /// Migration 059 on a database that already has migration 058 and rows in
    /// `sessions`: it must not disturb them, and running it twice
    /// (`CREATE INDEX IF NOT EXISTS`) is a no-op.
    #[test]
    fn migration_059_on_a_populated_v58_database_is_a_safe_reindex() {
        const SEED_AT: i64 = 58;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch(
                "INSERT OR IGNORE INTO hosts (alias) VALUES ('local');
                 INSERT INTO sessions (id, tmux_name, host_alias, created_at, last_activity_at, status)
                   VALUES (1, 'a1', 'local', 1, 1, 'running');",
            )
            .unwrap();
        assert_eq!(s.schema_version().unwrap(), SEED_AT);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        // Re-running the migration script directly (as `migrate()` would on a
        // database that already recorded 59) must not fail.
        s.conn
            .execute_batch(include_str!(
                "../../migrations/059_session_worktree_index.sql"
            ))
            .unwrap();
        let rows: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the re-run touched no rows");
    }

    /// The triggers of migration 060, by name. Each is what keeps the hub's
    /// token cache honest for one kind of write; a migration that rebuilds
    /// `host_tokens` or `client_tokens` (CREATE new / copy / DROP / RENAME)
    /// drops them silently, and this is what then fails.
    const AUTH_EPOCH_TRIGGERS: [&str; 6] = [
        "auth_epoch_host_tokens_insert",
        "auth_epoch_host_tokens_update",
        "auth_epoch_host_tokens_delete",
        "auth_epoch_client_tokens_insert",
        "auth_epoch_client_tokens_update",
        "auth_epoch_client_tokens_delete",
    ];

    /// Hub store latency, task 7: the token cache in `authorize` keys on
    /// `auth_epoch`, which these triggers bump in the writing transaction —
    /// in whichever process wrote.
    #[test]
    fn migration_060_adds_the_auth_epoch_and_its_triggers() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let epoch: i64 = s
            .conn
            .query_row("SELECT epoch FROM auth_epoch WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(epoch, 0, "a fresh database starts at epoch 0");
        for name in AUTH_EPOCH_TRIGGERS {
            let n: i64 = s
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "trigger {name} is missing");
        }
    }

    /// The `client_tokens` update trigger lists the columns that change what
    /// a token resolves to (everything but `last_seen_at`). A new column on
    /// either token table must be judged against it: this pins both tables'
    /// columns so adding one fails here until the trigger is reviewed.
    #[test]
    fn the_token_tables_columns_are_the_ones_the_auth_epoch_triggers_know() {
        let s = Store::open_in_memory().unwrap();
        let cols = |t: &str| -> Vec<String> {
            let mut stmt = s
                .conn
                .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
                .unwrap();
            stmt.query_map([t], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(
            cols("host_tokens"),
            [
                "host_alias",
                "token",
                "created_at",
                "mode",
                // Orbit Fleet 11.4 (migration 126): liveness, left out of
                // auth_epoch_host_tokens_update like client_tokens.last_seen_at.
                "last_used_at",
                // Listed in the trigger: a rotation replaces the token.
                "rotated_at"
            ],
            "host_tokens changed: add the column to auth_epoch_host_tokens_update \
             (migration 126) unless it is liveness-only like last_used_at"
        );
        assert_eq!(
            cols("client_tokens"),
            [
                "id",
                "name",
                "token_sha256",
                "mode",
                "created_at",
                "last_seen_at",
                "revoked_at",
                "trusted_at",
                // Work graph M14 (migration 066): its own trigger,
                // `auth_epoch_client_tokens_org`.
                "org_id",
                // Migration 074: its own trigger,
                // `auth_epoch_client_tokens_assets_admin`.
                "assets_admin_at",
                // Multi-user M1 (migration 098): its own trigger,
                // `auth_epoch_client_tokens_person`. Whose device this is
                // decides which sessions the caller may read at all, so a
                // re-binding MUST invalidate every cached caller.
                "person_id"
            ],
            "client_tokens changed: add the column to auth_epoch_client_tokens_update \
             (migration 060) unless it is liveness-only like last_seen_at"
        );
    }

    /// Work graph M14: re-binding a paired client to another org (or
    /// unbinding it) is a change of who it is — it must invalidate every
    /// cached caller, or a re-bound phone would keep reading its old org
    /// until the cache aged out.
    #[test]
    fn rebinding_a_client_bumps_the_auth_epoch() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_org("A", None, false).unwrap();
        let b = s.add_org("B", None, false).unwrap();
        s.insert_client_token("phone", &"0".repeat(64), "full")
            .unwrap();
        let at = |s: &Store| s.auth_epoch().unwrap();
        let e0 = at(&s);
        s.set_client_org("phone", Some(a.id)).unwrap();
        let e1 = at(&s);
        assert!(e1 > e0, "bound");
        s.set_client_org("phone", Some(a.id)).unwrap();
        assert_eq!(at(&s), e1, "the same binding again is no change");
        s.set_client_org("phone", Some(b.id)).unwrap();
        let e2 = at(&s);
        assert!(e2 > e1, "re-bound");
        s.set_client_org("phone", None).unwrap();
        assert!(at(&s) > e2, "unbound");
        assert_eq!(
            s.set_client_org("phone", Some(9_999)).unwrap_err().code,
            crate::ipc_error::codes::E_NOTFOUND
        );
        // A deleted org leaves the client bound to its id: fail closed.
        s.set_client_org("phone", Some(b.id)).unwrap();
        s.remove_org(b.id).unwrap();
        assert_eq!(s.active_client_tokens().unwrap()[0].org_id, Some(b.id));
    }

    /// Multi-user M1 (migration 098): re-binding a paired device to another
    /// person — or unbinding it — changes WHOSE token it is, and therefore
    /// which sessions the caller may read at all. It must invalidate every
    /// cached caller, or a device handed to a colleague would go on reading
    /// the previous person's private work until the cache aged out.
    #[test]
    fn binding_a_client_to_a_person_bumps_the_auth_epoch() {
        let s = Store::open_in_memory().unwrap();
        let owner = s.personal_owner_id().unwrap().expect("096 mints one");
        let ada = s.create_person("ada", None).unwrap();
        s.insert_client_token("phone", &"0".repeat(64), "full")
            .unwrap();
        // A device paired AFTER the upgrade starts person-less: 096's
        // backfill only reaches the rows that were there when it ran, and
        // binding the new one is T2's pairing change.
        assert_eq!(s.active_client_tokens().unwrap()[0].person_id, None);
        let at = |s: &Store| s.auth_epoch().unwrap();
        let e0 = at(&s);
        s.set_client_person("phone", Some(owner)).unwrap();
        let e1 = at(&s);
        assert!(e1 > e0, "bound");
        s.set_client_person("phone", Some(owner)).unwrap();
        assert_eq!(at(&s), e1, "the same binding again is no change");
        s.set_client_person("phone", Some(ada.id)).unwrap();
        let e2 = at(&s);
        assert!(e2 > e1, "re-bound");
        s.set_client_person("phone", None).unwrap();
        assert!(at(&s) > e2, "unbound");
        assert_eq!(
            s.set_client_person("phone", Some(9_999)).unwrap_err().code,
            crate::ipc_error::codes::E_NOTFOUND
        );
        // Disabling the person revokes the device but leaves the binding:
        // the audit trail keeps saying whose it was.
        s.set_client_person("phone", Some(ada.id)).unwrap();
        s.disable_person(ada.id).unwrap();
        let binding = |s: &Store| -> Option<i64> {
            s.list_client_tokens(true)
                .unwrap()
                .into_iter()
                .find(|c| c.name == "phone")
                .unwrap()
                .person_id
        };
        assert_eq!(binding(&s), Some(ada.id));
        assert!(s.active_client_tokens().unwrap().is_empty(), "revoked");
        // And a person row removed outright leaves the device bound to an id
        // nothing has — fail closed, exactly as a deleted org does. There is
        // no FK to widen it to NULL and no cascade to hand it to somebody.
        s.conn
            .execute("DELETE FROM people WHERE id = ?1", [ada.id])
            .unwrap();
        assert_eq!(binding(&s), Some(ada.id));
    }

    /// Migration 098 on a populated v95 database: every LIVE DEVICE comes
    /// out of the upgrade bound to this hub's personal owner (no row is left
    /// at the `person: None` privilege level), a revoked row is left exactly
    /// as it was, a `peer` and an `updater` row are left person-less because
    /// neither is a device (`TokenMode::is_single_purpose`, and
    /// `set_client_person` refuses both), and rolling the recorded version
    /// back and migrating again — which the guard turns into a record-only
    /// pass — changes nothing.
    #[test]
    fn migration_098_on_a_populated_v97_database_is_safe_to_rerun() {
        const SEED_AT: i64 = 97;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch(
                "INSERT INTO client_tokens (name, token_sha256, mode, created_at) \
                   VALUES ('phone', 'h1', 'full', 1);
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at, revoked_at) \
                   VALUES ('old', 'h2', 'full', 1, 2);
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at) \
                   VALUES ('hub-b', 'h3', 'peer', 1);
                 INSERT INTO client_tokens (name, token_sha256, mode, created_at) \
                   VALUES ('updater', 'h4', 'updater', 1);",
            )
            .unwrap();
        assert_eq!(s.schema_version().unwrap(), SEED_AT);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

        let owner = s.personal_owner_id().unwrap().expect("096 mints one");
        assert_eq!(s.list_people().unwrap().len(), 1);
        let unowned: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM client_tokens \
                 WHERE person_id IS NULL AND revoked_at IS NULL \
                   AND mode NOT IN ('peer', 'updater')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            unowned, 0,
            "no live device survives the upgrade person-less"
        );
        let binding = |s: &Store, name: &str| -> Option<i64> {
            s.conn
                .query_row(
                    "SELECT person_id FROM client_tokens WHERE name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(binding(&s, "phone"), Some(owner));
        assert_eq!(binding(&s, "old"), None, "a revoked row is left as it was");
        // The two machine tokens. A peer is ANOTHER FLEET and an updater is
        // `fleet-updater` acting for this hub: giving either a person would
        // make it a reader of that person's private sessions the moment T3
        // keys session reads on the caller's person. The backfill's mode
        // filter and `set_client_person`'s refusal are the same rule, and
        // this is the half that holds at upgrade time.
        for machine in ["hub-b", "updater"] {
            assert_eq!(
                binding(&s, machine),
                None,
                "{machine} is not a person's device"
            );
            assert_eq!(
                s.set_client_person(machine, Some(owner)).unwrap_err().code,
                crate::ipc_error::codes::E_VALIDATE,
                "{machine}"
            );
        }

        // Re-migrate over a schema that already has the column: the guard
        // records the version and runs not one of 096's statements — no
        // second owner, no rename undone, no binding rewritten.
        s.rename_person(owner, Some("Martin"), None).unwrap();
        s.set_client_person("phone", None).unwrap();
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 86;")
            .unwrap();
        s.migrate().expect("re-running 094 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(s.list_people().unwrap().len(), 1, "no second owner");
        assert_eq!(s.personal_owner_id().unwrap(), Some(owner));
        assert_eq!(
            s.get_person(owner).unwrap().unwrap().name,
            "Martin",
            "the rename survives the re-run"
        );
        assert_eq!(
            binding(&s, "phone"),
            None,
            "the guard ran nothing, the backfill included"
        );
        let rows: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM client_tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 4, "the re-run touched no rows");
    }

    /// Migration 099 on a populated v96 database: the two columns arrive
    /// with the safe default, the backfill attributes exactly the rows fleet
    /// started (`started_at IS NOT NULL`) to the hub's one person and leaves
    /// a reconcile-discovered row `unclaimed`, each attributed row's
    /// `row_version` moves once (an open client must learn its visibility
    /// changed), the `CHECK` makes a third visibility value unrepresentable,
    /// the conversation-owner record is written and survives the session's
    /// deletion, and rolling the recorded version back and migrating again —
    /// which the guard turns into a record-only pass — changes nothing.
    #[test]
    fn migration_099_on_a_populated_v98_database_is_safe_to_rerun() {
        const SEED_AT: i64 = 98;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 -- fleet started this one, and it has a conversation
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, started_at, claude_session_id) \
                   VALUES ('fleet-made', 'h', 1, 1, 'running', 10, 'conv-1');
                 -- fleet started this one too, but it never bound a conversation
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, started_at) \
                   VALUES ('fleet-quiet', 'h', 1, 1, 'running', 11);
                 -- reconcile found this one: a hand-started tmux session
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, claude_session_id) \
                   VALUES ('hand-made', 'h', 1, 1, 'running', 'conv-2');",
            )
            .unwrap();
        assert_eq!(s.schema_version().unwrap(), SEED_AT);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

        let owner = s.personal_owner_id().unwrap().expect("096 mints one");
        let row = |s: &Store, name: &str| -> (Option<i64>, String, i64) {
            s.conn
                .query_row(
                    "SELECT owner_person_id, visibility, row_version FROM sessions \
                     WHERE tmux_name = ?1",
                    [name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        };
        for started in ["fleet-made", "fleet-quiet"] {
            assert_eq!(
                row(&s, started),
                (Some(owner), "private".to_string(), 1),
                "{started}: a row fleet started is the one person's, private, \
                 and its row_version moved once"
            );
        }
        assert_eq!(
            row(&s, "hand-made"),
            (None, "unclaimed".to_string(), 0),
            "a row reconcile found is nobody's: unclaimed, and untouched"
        );

        // The conversation-owner record: written by 097's UPDATE trigger
        // when the backfill gave the row an owner, and only for a row that
        // has both halves.
        let owners: Vec<(String, i64)> = {
            let mut stmt = s
                .conn
                .prepare(
                    "SELECT claude_session_id, owner_person_id FROM conversation_owners \
                     ORDER BY claude_session_id",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(
            owners,
            vec![("conv-1".to_string(), owner)],
            "only the owned conversation is recorded"
        );

        // A third visibility value is unrepresentable, which is stronger
        // than any test asserting that no migration produces one. In
        // particular `'org'` — team sharing under another name — cannot be
        // written by a hand edit, a future UPDATE, or a mistake here.
        let err = s
            .conn
            .execute(
                "UPDATE sessions SET visibility = 'org' WHERE tmux_name = 'hand-made'",
                [],
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("CHECK constraint failed"), "{err}");

        // The record outlives the session: `delete_session` deletes the
        // `sessions` row outright and `conversations` cascades with it, so a
        // resume gate that read live rows would see nothing. This one stays.
        let id: i64 = s
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE tmux_name = 'fleet-made'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        s.delete_session(id).unwrap();
        assert_eq!(
            s.conn
                .query_row(
                    "SELECT owner_person_id FROM conversation_owners \
                     WHERE claude_session_id = 'conv-1'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            owner,
            "a reaped session's conversation owner survives"
        );

        // Re-migrate over a schema that already has the columns: the guard
        // records the version and runs not one of 097's statements, and the
        // backfill — which is outside the script and therefore DOES run
        // again — matches no row.
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 87;")
            .unwrap();
        let before = row(&s, "hand-made");
        s.migrate().expect("re-running 087 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            row(&s, "hand-made"),
            before,
            "the unclaimed row is still unclaimed after a second open"
        );
        assert_eq!(row(&s, "fleet-quiet"), (Some(owner), "private".into(), 1));
        assert_eq!(
            count_rows(&s, "conversation_owners"),
            1,
            "the re-run recorded nothing twice"
        );
    }

    /// `COUNT(*)` of one table, for the migration tests below.
    fn count_rows(s: &Store, table: &str) -> i64 {
        s.conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
                r.get(0)
            })
            .unwrap()
    }

    /// 099's backfill attributes NOTHING once the hub has more than one
    /// person: there is no fact saying which of them started a pre-M1 row,
    /// and the upgrade widens nothing (rule 7). Those rows stay `unclaimed`,
    /// which is a per-host count and not one byte more.
    #[test]
    fn the_099_backfill_attributes_nothing_on_a_hub_with_two_people() {
        let s = store_at_version(98);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO people (name, is_personal_owner, created_at) \
                   VALUES ('ada', 0, 1);
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, started_at) \
                   VALUES ('fleet-made', 'h', 1, 1, 'running', 10);",
            )
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.list_people().unwrap().len(), 2);
        let (owner, vis, version): (Option<i64>, String, i64) = s
            .conn
            .query_row(
                "SELECT owner_person_id, visibility, row_version FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (owner, vis.as_str(), version),
            (None, "unclaimed", 0),
            "two people: nobody is guessed, and the row is not even touched"
        );
    }

    /// Migration 100 on a populated database: the table and its three indexes
    /// arrive, the grants a hub already holds survive a second open, and the
    /// NULL-safe live index still fires afterwards.
    ///
    /// Every statement in the script is `IF NOT EXISTS` and there is no
    /// `ALTER TABLE ... ADD COLUMN`, which is why it is registered as a
    /// `Migration::plain` with no `already_applied` guard — this is the test
    /// that says so rather than the comment claiming it.
    #[test]
    fn migration_100_on_a_populated_database_is_safe_to_rerun() {
        const SEED_AT: i64 = 99;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch("INSERT INTO hosts (alias) VALUES ('h');")
            .unwrap();
        assert!(!s.has_table("session_grants").unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(s.has_table("session_grants").unwrap());
        let index = |name: &str| -> bool {
            s.conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [name],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
                == 1
        };
        for name in [
            "idx_session_grants_live",
            "idx_session_grants_person",
            "idx_session_grants_session",
        ] {
            assert!(index(name), "{name} is missing");
        }

        // A hub that has already shared something: one live grant and one
        // revoked row, which is what the audit trail looks like.
        let ada = s.create_person("ada", None).unwrap().id;
        let bo = s.create_person("bo", None).unwrap().id;
        s.conn
            .execute(
                "INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, started_at, owner_person_id, visibility) \
                 VALUES ('s', 'h', 1, 1, 'running', 1, ?1, 'private')",
                [ada],
            )
            .unwrap();
        let sid = s.conn.last_insert_rowid();
        s.grant_session(sid, crate::store::GrantRecipient::Person(bo), "watch", ada)
            .unwrap();
        s.revoke_session_grant(sid, bo, ada).unwrap();
        s.grant_session(sid, crate::store::GrantRecipient::Person(bo), "drive", ada)
            .unwrap();
        let grants = |s: &Store| -> (i64, Option<String>) {
            (
                count_rows(s, "session_grants"),
                s.grants_for_person(bo).unwrap().get(&sid).cloned(),
            )
        };
        let before = grants(&s);
        assert_eq!(before, (2, Some("drive".to_string())));

        // Re-migrate over a schema that already has the table: the script
        // runs again in full (no guard) and changes nothing.
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 88;")
            .unwrap();
        s.migrate().expect("re-running 096 is safe");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(grants(&s), before, "the re-run touched no grant");

        // And the live index is still the live index: a second grant to the
        // same recipient is refused, which is the invariant the whole table
        // exists to carry.
        assert_eq!(
            s.grant_session(sid, crate::store::GrantRecipient::Person(bo), "watch", ada)
                .unwrap_err()
                .code,
            crate::ipc_error::codes::E_EXISTS
        );
    }

    /// Migration 060 on a populated v59 database: the counter starts at 0,
    /// the tokens are untouched, and running it again (as the tests' roll
    /// back and re-migrate does) neither fails nor resets the counter.
    #[test]
    fn migration_060_on_a_populated_v59_database_is_safe_to_rerun() {
        const SEED_AT: i64 = 59;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch(
                "INSERT INTO host_tokens (host_alias, token, created_at) VALUES ('box', 't', 1);
                 INSERT INTO client_tokens (name, token_sha256, created_at) VALUES ('p', 'h', 1);",
            )
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let read_epoch = |s: &Store| -> i64 {
            s.conn
                .query_row("SELECT epoch FROM auth_epoch WHERE id = 1", [], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        // The baseline is whatever the rest of the chain left, not 0: a
        // later migration that legitimately re-binds a token moves the
        // counter too (096's backfill binds this live row to the hub's
        // personal owner). What this test pins is the DELTA — one token
        // write bumps it once, and re-running 060 does not reset it.
        let base = read_epoch(&s);
        s.conn
            .execute("UPDATE host_tokens SET token = 't2'", [])
            .unwrap();
        s.conn
            .execute_batch(include_str!("../../migrations/060_auth_epoch.sql"))
            .unwrap();
        let epoch = read_epoch(&s);
        assert_eq!(epoch, base + 1, "the re-run kept the counter");
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM client_tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "the re-run touched no rows");
    }

    /// The `sessions` columns migration 063's `sessions_row_version_bump`
    /// deliberately does NOT watch: `row_version` itself (an explicit
    /// `row_version + 1` must not re-trigger), and per-pass bookkeeping that
    /// is not on the wire (the reconcile's stamp, 072's usage backfill mark,
    /// 080's stale-working veto memory — a `#[serde(skip)]` `SessionRow`
    /// field — and 081's pane spinner stamp). Every other column is
    /// watched, so a write that changes it bumps the counter.
    const ROW_VERSION_UNWATCHED: [&str; 6] = [
        "row_version",
        "last_reconciled_at",
        "usage_backfill_until",
        "pane_working_at",
        "launch_model",
        "stale_demoted_at",
    ];

    /// The SQL of `sessions_row_version_bump`, as the database holds it.
    fn row_version_trigger_sql(s: &Store) -> String {
        s.conn
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'trigger' AND name = 'sessions_row_version_bump'",
                [],
                |r| r.get(0),
            )
            .expect("sessions_row_version_bump is missing")
    }

    /// Migration 063 on a fresh database: an UPDATE that changes nothing a
    /// client sees (same values, or only `last_reconciled_at`) leaves
    /// `row_version` alone; a real change bumps it once; an explicit
    /// `row_version + 1` bumps it exactly once.
    #[test]
    fn migration_063_bumps_row_version_only_on_a_watched_change() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
                 VALUES ('s', 'h', 1, 1, 'running');",
            )
            .unwrap();
        let v = || -> i64 {
            s.conn
                .query_row("SELECT row_version FROM sessions", [], |r| r.get(0))
                .unwrap()
        };
        let run = |sql: &str| {
            s.conn.execute(sql, []).unwrap();
        };
        assert_eq!(v(), 0);
        run("UPDATE sessions SET status = 'running', last_activity_at = 1");
        assert_eq!(v(), 0, "a same-value UPDATE is not a change");
        run("UPDATE sessions SET last_reconciled_at = 77");
        assert_eq!(v(), 0, "the reconcile stamp alone is not a change");
        run("UPDATE sessions SET last_activity_at = 2, last_reconciled_at = 78");
        assert_eq!(v(), 1, "a changed column bumps once");
        run("UPDATE sessions SET notes = NULL");
        assert_eq!(v(), 1, "NULL to NULL is no change (IS NOT, not <>)");
        run("UPDATE sessions SET notes = 'n'");
        assert_eq!(v(), 2, "NULL to a value is a change");
        run("UPDATE sessions SET row_version = row_version + 1");
        assert_eq!(v(), 3, "an explicit bump counts exactly once");
    }

    /// Migration 063's trigger names every `sessions` column it watches.
    /// A new column must be judged against it: this reads the table's
    /// columns and fails for one the trigger neither watches nor lists in
    /// [`ROW_VERSION_UNWATCHED`] — and for a table rebuild that dropped the
    /// trigger.
    ///
    /// SQLite has no ALTER TRIGGER, so "add a line" means re-issuing the
    /// whole body in a new migration; 095 is the newest such re-issue (065,
    /// 082 before it), and this test is the one that fails if a column is
    /// added without one.
    #[test]
    fn the_sessions_columns_are_the_ones_the_row_version_trigger_knows() {
        let s = Store::open_in_memory().unwrap();
        let sql = row_version_trigger_sql(&s);
        let mut stmt = s
            .conn
            .prepare("SELECT name FROM pragma_table_info('sessions') ORDER BY cid")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for u in ROW_VERSION_UNWATCHED {
            assert!(cols.iter().any(|c| c == u), "{u} is not a sessions column");
        }
        for c in &cols {
            let watched = sql.contains(&format!("NEW.{c} IS NOT OLD.{c}"));
            if ROW_VERSION_UNWATCHED.contains(&c.as_str()) {
                assert!(
                    !watched,
                    "{c} is listed as unwatched but the trigger watches it"
                );
            } else {
                assert!(
                    watched,
                    "sessions.{c} is not watched by sessions_row_version_bump: add \
                     `NEW.{c} IS NOT OLD.{c}` to a new migration's trigger (it bumps \
                     row_version on a client-visible change) or, for per-pass \
                     bookkeeping that is not a SessionRow field, to ROW_VERSION_UNWATCHED"
                );
            }
        }
    }

    /// Migration 063 on a populated v62 database: the rows are untouched
    /// (no `row_version` moves), the new trigger is in place, and running
    /// the script again (as the tests' roll back and re-migrate does) is
    /// harmless.
    #[test]
    fn migration_063_on_a_populated_v62_database_is_safe_to_rerun() {
        const SEED_AT: i64 = 62;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
                 VALUES ('s', 'h', 1, 1, 'running');
                 UPDATE sessions SET claude_status = 'idle';",
            )
            .unwrap();
        let v = |s: &Store| -> i64 {
            s.conn
                .query_row("SELECT row_version FROM sessions", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(v(&s), 1, "the v42 trigger bumps on the seeded change");
        s.conn
            .execute("UPDATE sessions SET last_reconciled_at = 5", [])
            .unwrap();
        assert_eq!(v(&s), 2, "the v42 trigger bumps on any UPDATE");
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(v(&s), 2, "the migration moved no row_version");
        s.conn
            .execute("UPDATE sessions SET last_reconciled_at = 6", [])
            .unwrap();
        assert_eq!(v(&s), 2, "the new trigger ignores the reconcile stamp");
        s.conn
            .execute_batch(include_str!(
                "../../migrations/063_row_version_on_visible_change.sql"
            ))
            .unwrap();
        let triggers: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'trigger' AND name = 'sessions_row_version_bump'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(triggers, 1, "the re-run left exactly one bump trigger");
        s.conn
            .execute("UPDATE sessions SET claude_status = 'working'", [])
            .unwrap();
        assert_eq!(v(&s), 3, "after the re-run a real change still bumps once");
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "the re-run touched no rows");
    }

    #[test]
    fn migration_065_adds_stale_working_at_and_is_safe_to_rerun() {
        let s = store_at_version(64);
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
        // The rebuilt trigger watches the new column: stamping it is a
        // client-visible change, so `row_version` moves once.
        let v = || -> i64 {
            s.conn
                .query_row("SELECT row_version FROM sessions", [], |r| r.get(0))
                .unwrap()
        };
        let v0 = v();
        s.conn
            .execute("UPDATE sessions SET stale_working_at = 5", [])
            .unwrap();
        assert_eq!(v(), v0 + 1, "a stale_working_at change bumps row_version");
        s.conn
            .execute("UPDATE sessions SET stale_working_at = 5", [])
            .unwrap();
        assert_eq!(v(), v0 + 1, "a same-value write does not");
        // Rolling the recorded version back re-runs 065 (the idiom of the
        // 024–026 tests): the guard skips the ADD COLUMN.
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 65;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Migration 080 on a populated v79 database: a row already demoted
    /// keeps its veto (backfilled from `stale_working_at`), an unstamped row
    /// stays unarmed, no `row_version` moves (the column is not watched),
    /// and re-running it (the tests' roll back and re-migrate, or any later
    /// open) neither fails nor overwrites a veto already set.
    #[test]
    fn migration_080_backfills_stale_demoted_at_and_is_safe_to_rerun() {
        let s = store_at_version(79);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
                 VALUES ('a', 'h', 1, 1, 'running'), ('b', 'h', 1, 1, 'running');
                 UPDATE sessions SET stale_working_at = 40 WHERE tmux_name = 'a';",
            )
            .unwrap();
        let versions = |s: &Store| -> Vec<i64> {
            let mut st = s
                .conn
                .prepare("SELECT row_version FROM sessions ORDER BY tmux_name")
                .unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let demoted = |s: &Store| -> Vec<Option<i64>> {
            let mut st = s
                .conn
                .prepare("SELECT stale_demoted_at FROM sessions ORDER BY tmux_name")
                .unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let before = versions(&s);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(sessions_has_stale_demoted_at(&s.conn).unwrap());
        assert_eq!(
            demoted(&s),
            vec![Some(40), None],
            "the demoted row keeps its veto"
        );
        assert_eq!(versions(&s), before, "the backfill moved no row_version");
        s.conn
            .execute(
                "UPDATE sessions SET stale_demoted_at = 99 WHERE tmux_name = 'a'",
                [],
            )
            .unwrap();
        assert_eq!(
            versions(&s),
            before,
            "a stale_demoted_at change is not visible"
        );
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 80;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert_eq!(
            demoted(&s),
            vec![Some(99), None],
            "the re-run neither fails nor overwrites a set veto"
        );
    }

    #[test]
    fn migration_071_rekeys_usage_daily_by_backfill_and_keeps_the_rows() {
        const SEED_AT: i64 = 70;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch(
                "INSERT INTO usage_daily (day, host_alias, input_tokens, output_tokens, \
                 cache_write_tokens, cache_read_tokens, cost_micros) VALUES (20714, 'trn', 1, 2, 3, 4, 5);",
            )
            .unwrap();
        assert!(!usage_daily_has_backfill(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(usage_daily_has_backfill(&s.conn).unwrap());
        let (backfill, cost): (i64, i64) = s
            .conn
            .query_row(
                "SELECT backfill, cost_micros FROM usage_daily WHERE day = 20714 AND host_alias = 'trn'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((backfill, cost), (0, 5), "existing rows are live rows");
        // The new key admits a backfill row beside the live one for the same day.
        s.conn
            .execute(
                "INSERT INTO usage_daily (day, host_alias, backfill, cost_micros) VALUES (20714, 'trn', 1, 7)",
                [],
            )
            .unwrap();
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM usage_daily WHERE day = 20714",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    /// 071 once stamped version 68, not 71 (fixed in b75d596). A database
    /// that ran that script holds the rebuilt `usage_daily` but no 71 row:
    /// the next launch offers 071 again, and its guard must only record it
    /// — re-running the rebuild would collapse backfill rows into live ones.
    #[test]
    fn a_database_that_ran_the_misstamped_071_keeps_its_backfill_rows() {
        let s = store_at_version(71);
        s.conn
            .execute_batch(
                "INSERT INTO usage_daily (day, host_alias, backfill, cost_micros) \
                 VALUES (20714, 'trn', 1, 850000000), (20714, 'trn', 0, 5);
                 DELETE FROM schema_version WHERE version = 71;",
            )
            .unwrap();
        assert_eq!(s.schema_version().unwrap(), 70);
        s.migrate().unwrap();
        let has_71: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version = 71",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_71, 1, "071 is recorded");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let rows: Vec<(i64, i64)> = s
            .conn
            .prepare("SELECT backfill, cost_micros FROM usage_daily ORDER BY backfill")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(0, 5), (1, 850_000_000)],
            "the backfill row survives"
        );
    }

    #[test]
    fn migration_072_adds_the_usage_backfill_mark_and_is_safe_to_rerun() {
        let s = store_at_version(71);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
                 VALUES ('a', 'h', 1, 1, 'running');",
            )
            .unwrap();
        assert!(!sessions_has_usage_backfill_until(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let (until, rv): (i64, i64) = s
            .conn
            .query_row(
                "SELECT usage_backfill_until, row_version FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(until, 0, "an existing cursor has no history pending");
        s.conn
            .execute("UPDATE sessions SET usage_backfill_until = 9", [])
            .unwrap();
        let rv2: i64 = s
            .conn
            .query_row("SELECT row_version FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rv2, rv, "the mark is bookkeeping: no row_version bump");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 72;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn migration_081_adds_the_pane_working_stamp_and_is_safe_to_rerun() {
        let s = store_at_version(80);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
                 VALUES ('a', 'h', 1, 1, 'running');",
            )
            .unwrap();
        assert!(!sessions_has_pane_working_at(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let (at, rv): (Option<i64>, i64) = s
            .conn
            .query_row(
                "SELECT pane_working_at, row_version FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(at, None, "no pane has been seen working yet");
        s.conn
            .execute("UPDATE sessions SET pane_working_at = 9", [])
            .unwrap();
        let rv2: i64 = s
            .conn
            .query_row("SELECT row_version FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rv2, rv, "the stamp is bookkeeping: no row_version bump");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 81;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn migration_086_adds_the_shared_work_columns_backfills_origin_and_is_safe_to_rerun() {
        let s = store_at_version(85);
        s.conn
            .execute_batch(
                "INSERT INTO work_items (source, key, title, created_at, updated_at) VALUES ('local', 'OPS', 'named', 1, 1);
                 INSERT INTO work_items (source, key, title, created_at, updated_at) VALUES ('jira', 'TK-1', 'ticket', 1, 1);",
            )
            .unwrap();
        assert!(!work_items_has_origin(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let got: Vec<(String, String)> = s
            .conn
            .prepare("SELECT title, origin FROM work_items ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            got,
            vec![
                ("named".into(), "manual".into()),
                ("ticket".into(), "detected".into())
            ]
        );
        for col in [
            "project_id",
            "notes",
            "task_id",
            "proposal_state",
            "proposed_by",
            "proposal_why",
        ] {
            let n: i64 = s
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = ?1",
                    [col],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{col}");
        }
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 86;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn migration_089_adds_hosts_harnesses_as_auto_and_is_safe_to_rerun() {
        let s = store_at_version(88);
        s.conn
            .execute_batch("INSERT INTO hosts (alias, reachable) VALUES ('h', 1);")
            .unwrap();
        assert!(!hosts_has_harnesses(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(hosts_has_harnesses(&s.conn).unwrap());
        let v: Option<String> = s
            .conn
            .query_row("SELECT harnesses FROM hosts WHERE alias = 'h'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, None, "an existing host starts on auto");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 89;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn migration_092_adds_the_provision_warning_as_none_and_is_safe_to_rerun() {
        let s = store_at_version(91);
        s.conn
            .execute_batch("INSERT INTO hosts (alias, reachable, provisioned) VALUES ('h', 1, 1);")
            .unwrap();
        assert!(!hosts_has_provision_warning(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(hosts_has_provision_warning(&s.conn).unwrap());
        let v: Option<String> = s
            .conn
            .query_row(
                "SELECT provision_warning FROM hosts WHERE alias = 'h'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, None, "an existing host carries no warning");
        // the ADD COLUMN is not idempotent, so a re-run must be guarded
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 92;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Assets M5 (R4): `asset_inventory.drift_side`, NULL on every existing
    /// row, and a re-run is guarded (ADD COLUMN is not idempotent).
    #[test]
    fn migration_096_adds_drift_side_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(95);
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias, reachable, provisioned) VALUES ('h', 1, 1); \
                 INSERT INTO asset_inventory (host_alias, harness, kind, name, state, scanned_at) \
                   VALUES ('h', 'claude', 'skill', 's', 'drifted', 1);",
            )
            .unwrap();
        assert!(!asset_inventory_has_drift_side(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(asset_inventory_has_drift_side(&s.conn).unwrap());
        let side: Option<String> = s
            .conn
            .query_row("SELECT drift_side FROM asset_inventory", [], |r| r.get(0))
            .unwrap();
        assert_eq!(side, None);
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 96;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Assets M5 (R8): `changeset_items.decided_at`, NULL on existing items,
    /// guarded on re-run.
    #[test]
    fn migration_097_adds_decided_at_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(96);
        s.conn
            .execute_batch(
                "INSERT INTO changesets (id, kind, summary, state, created_at) \
                   VALUES (1, 'new', 'New on oci', 'dismissed', 1); \
                 INSERT INTO changeset_items \
                   (changeset_id, position, grp, kind, name, action, decider, state) \
                   VALUES (1, 0, 'core', 'skill', 's', 'import', 'person', 'rejected');",
            )
            .unwrap();
        assert!(!changeset_items_has_decided_at(&s.conn).unwrap());
        s.migrate().unwrap();
        assert!(changeset_items_has_decided_at(&s.conn).unwrap());
        let at: Option<i64> = s
            .conn
            .query_row("SELECT decided_at FROM changeset_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(at, None, "an item decided before 097 has no time");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 97;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Assets M6 (R1): `changeset_items.outcome`, NULL on existing items,
    /// guarded on re-run.
    #[test]
    fn migration_102_adds_item_outcome_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(101);
        s.conn
            .execute_batch(
                "INSERT INTO changesets (id, kind, summary, state, created_at) \
                   VALUES (1, 'rollout', 'Roll out core to oci', 'applied', 1); \
                 INSERT INTO changeset_items \
                   (changeset_id, position, grp, kind, name, action, decider, state) \
                   VALUES (1, 0, 'core', 'host', 'oci', 'sync', 'rule', 'skipped');",
            )
            .unwrap();
        assert!(!changeset_items_has_outcome(&s.conn).unwrap());
        s.migrate().unwrap();
        assert!(changeset_items_has_outcome(&s.conn).unwrap());
        let o: Option<String> = s
            .conn
            .query_row("SELECT outcome FROM changeset_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(o, None, "an item recorded before 102 has no outcome");
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 102;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// Assets M6 (R3): `changesets.withdrawn_at`, NULL on existing cards,
    /// guarded on re-run.
    #[test]
    fn migration_103_adds_withdrawn_at_as_null_and_is_safe_to_rerun() {
        let s = store_at_version(102);
        s.conn
            .execute_batch(
                "INSERT INTO changesets (id, kind, summary, state, created_at, error) \
                   VALUES (1, 'new', 'New on oci', 'dismissed', 1, 'withdrawn: no longer applies');",
            )
            .unwrap();
        assert!(!changesets_has_withdrawn_at(&s.conn).unwrap());
        s.migrate().unwrap();
        assert!(changesets_has_withdrawn_at(&s.conn).unwrap());
        let at: Option<i64> = s
            .conn
            .query_row("SELECT withdrawn_at FROM changesets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(at, None);
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 103;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// 105: worktree rows named after a branch with a `/` (the live
    /// `feat/imports` on claude-fleet-trn, `feat/abc` on mefistos) take a
    /// flat name; the path, the branch and the session's link stay, and the
    /// session's key follows the row. A name already taken gets the row id.
    #[test]
    fn migration_105_flattens_slash_named_worktrees_and_their_session_keys() {
        let s = store_at_version(104);
        // Raw rows: `upsert_host` reads back every current `hosts` column,
        // and a 104 store predates the later ones (111's `auth_overrides`).
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias, reachable) VALUES ('trn', 1), ('mefistos', 1);",
            )
            .unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let nested = "/home/dev/projects/r/.worktrees/feat/imports";
        let wid = s
            .upsert_worktree_on("trn", pid, "feat/imports", nested, Some("feat/imports"))
            .unwrap();
        let abc = s
            .upsert_worktree_on(
                "mefistos",
                pid,
                "feat/abc",
                "/m/r/.worktrees/feat/abc",
                None,
            )
            .unwrap();
        let taken = s
            .upsert_worktree_on(
                "trn",
                pid,
                "x-y",
                "/home/dev/projects/r/.worktrees/x-y",
                None,
            )
            .unwrap();
        let clash = s
            .upsert_worktree_on(
                "trn",
                pid,
                "x/y",
                "/home/dev/projects/r/.worktrees/x/y",
                None,
            )
            .unwrap();
        // Raw too: `upsert_session` reads the row back with every current
        // `sessions` column (113's `claude_profile` among them).
        s.conn
            .execute(
                "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id, \
                 created_at, last_activity_at, status, worktree_key) \
                 VALUES ('dev-r--feat/imports', 'trn', ?1, ?2, 1, 1, 'lost', 'feat')",
                rusqlite::params![pid, wid],
            )
            .unwrap();
        let sid = s.conn.last_insert_rowid();

        s.migrate().unwrap();
        let row = |id| s.get_worktree_row(id).unwrap().unwrap();
        let w = row(wid);
        assert_eq!(w.name, "feat-imports");
        assert_eq!(w.path, nested, "the checkout stays where it is");
        assert_eq!(w.branch.as_deref(), Some("feat/imports"));
        assert_eq!(row(abc).name, "feat-abc");
        assert_eq!(row(taken).name, "x-y", "an existing flat name is kept");
        assert_eq!(row(clash).name, format!("x-y-{clash}"));
        let sess = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(sess.worktree_id, Some(wid));
        assert_eq!(sess.worktree_key.as_deref(), Some("feat-imports"));

        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 105;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(row(wid).name, "feat-imports", "a re-run changes nothing");
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// The hub-ops-accounting branch numbered its `usage_daily` rebuild
    /// 064–068 before it landed as 071. A database that branch's build
    /// opened at `main`'s 063 recorded (say) 68 for it, so `main`'s 064–068
    /// never ran. `repair_skipped_main_migrations` finds each by its
    /// artefact and runs it.
    #[test]
    fn a_branch_numbered_usage_migration_does_not_leave_main_064_to_068_unapplied() {
        let s = store_at_version(63);
        let branch_071 = include_str!("../../migrations/071_usage_daily_backfill.sql").replace(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (71);",
            "INSERT OR IGNORE INTO schema_version (version) VALUES (68);",
        );
        s.conn.execute_batch(&branch_071).unwrap();
        s.conn
            .execute_batch(
                "INSERT INTO hosts (alias) VALUES ('h');
                 INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, status)
                 VALUES ('a', 'h', 1, 1, 'running');
                 INSERT INTO usage_daily (day, host_alias, backfill, cost_micros) VALUES (1, 'h', 1, 7);",
            )
            .unwrap();
        assert_eq!(s.schema_version().unwrap(), 68);
        assert!(!sessions_has_stale_working_at(&s.conn).unwrap());
        assert!(s.has_table("tracker_webhooks").unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(sessions_has_stale_working_at(&s.conn).unwrap(), "065");
        assert!(client_tokens_has_org(&s.conn).unwrap(), "066");
        assert!(orgs_has_bound_sees_unassigned(&s.conn).unwrap(), "067");
        assert!(orgs_has_jev_allowed(&s.conn).unwrap(), "068");
        assert!(!s.has_table("tracker_webhooks").unwrap(), "064");
        assert!(
            row_version_trigger_sql(&s)
                .contains("NEW.stale_working_at IS NOT OLD.stale_working_at"),
            "065's trigger rebuild ran"
        );
        let backfill: i64 = s
            .conn
            .query_row("SELECT backfill FROM usage_daily", [], |r| r.get(0))
            .unwrap();
        assert_eq!(backfill, 1, "071 was only recorded, not re-run");
        // A second open is a no-op.
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }

    /// 142 (Orbit Fleet 11.7) rebuilds `session_grants` to widen its CHECK:
    /// the rows it held come through with their ids, the indexes are back,
    /// `answer` is admitted, and a re-run is only recorded.
    #[test]
    fn migration_142_keeps_every_grant_and_admits_answer() {
        let s = Store::open_in_memory().unwrap();
        let ada = s.create_person("ada", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        s.upsert_host("h").unwrap();
        let sid = s
            .upsert_session("t", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(sid, Some(ada)).unwrap();
        let g = s
            .grant_session(sid, crate::store::GrantRecipient::Person(bob), "drive", ada)
            .unwrap();
        // Back to the table as 100 wrote it, still holding the grant.
        s.conn
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 CREATE TEMP TABLE keep AS SELECT * FROM session_grants;
                 DROP TABLE session_grants;",
            )
            .unwrap();
        s.conn
            .execute_batch(include_str!("../../migrations/100_session_grants.sql"))
            .unwrap();
        s.conn
            .execute_batch(
                "INSERT INTO session_grants SELECT * FROM keep; DROP TABLE keep;
                 PRAGMA foreign_keys = ON;",
            )
            .unwrap();
        assert!(!session_grants_has_answer(&s.conn).unwrap());
        let level_of = |s: &Store| -> String {
            s.conn
                .query_row(
                    "SELECT level FROM session_grants WHERE id = ?1",
                    [g.id],
                    |r| r.get(0),
                )
                .unwrap()
        };
        s.conn
            .execute("DELETE FROM schema_version WHERE version >= 142", [])
            .unwrap();
        s.migrate().unwrap();
        assert!(session_grants_has_answer(&s.conn).unwrap());
        assert_eq!(level_of(&s), "drive", "the grant came through under its id");
        let indexes: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
                  AND tbl_name = 'session_grants' AND name LIKE 'idx_session_grants_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // 142's three, rebuilt with the table, and 147's org index (review
        // r16), which runs again after it.
        assert_eq!(indexes, 4);
        s.conn
            .execute(
                "UPDATE session_grants SET level = 'answer' WHERE id = ?1",
                [g.id],
            )
            .expect("the CHECK admits answer");
        // A re-run is recorded, not rebuilt: the row keeps its new level.
        s.conn
            .execute("DELETE FROM schema_version WHERE version >= 142", [])
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(level_of(&s), "answer");
    }
}

#[cfg(test)]
mod tests_upgrade;
