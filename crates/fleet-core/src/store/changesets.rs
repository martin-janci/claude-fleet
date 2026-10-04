//! Changeset cards, their items and triage verdicts (Assets M4, migration
//! 094). The rules — which cards exist, what applying one does — live in
//! `service::catalog::changesets`; this is the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};
use std::collections::{BTreeMap, BTreeSet};

/// One card (`changesets` row).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetRow {
    pub id: i64,
    /// bootstrap | new | drift | rollout
    pub kind: String,
    pub summary: String,
    /// proposed | applied | undone | dismissed | failed
    pub state: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix **milliseconds** (Rulings PF13), so "the latest applied card"
    /// orders by apply time — `(applied_at, id)` — even for two cards applied
    /// in the same second. Stamp it with [`super::now_unix_ms`].
    pub applied_at: Option<i64>,
    /// JSON `{catalog_id: sha}`: the commits an apply made.
    pub commits: Option<String>,
    /// JSON `[HostLayerRow]`: the touched catalogs' `host_layers` before apply.
    pub layers_snapshot: Option<String>,
    pub error: Option<String>,
}

/// One item of a card (`changeset_items` row).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesetItemRow {
    pub changeset_id: i64,
    pub position: i64,
    pub grp: String,
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    /// import | assign_layer | set_scope | hide | take_host | restore | sync
    pub action: String,
    pub params: Option<String>,
    /// rule | jev | haiku | person
    pub decider: String,
    /// pending | applied | skipped | rejected
    pub state: String,
    /// Assets M5 (migration 097, R8): when it left `pending`, Unix ms;
    /// `None` while pending and on items decided before 097 (absent on the
    /// wire then, as before).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
}

/// An item to insert; its position is its index. `state` is always
/// `pending` on insert, so it is not part of the value — two proposals with
/// the same items compare equal whatever their stored items' states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChangesetItem {
    pub grp: String,
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    pub action: String,
    pub params: Option<String>,
    pub decider: String,
}

impl From<&ChangesetItemRow> for NewChangesetItem {
    fn from(r: &ChangesetItemRow) -> Self {
        NewChangesetItem {
            grp: r.grp.clone(),
            catalog_id: r.catalog_id,
            kind: r.kind.clone(),
            name: r.name.clone(),
            action: r.action.clone(),
            params: r.params.clone(),
            decider: r.decider.clone(),
        }
    }
}

/// One `asset_triage_verdicts` row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TriageVerdictRow {
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    pub content_hash: String,
    /// ignored | rejected | host_local
    pub verdict: String,
    pub decider: String,
    /// Unix seconds.
    pub decided_at: i64,
}

/// What an item is across a refresh — `(grp, kind, name, action,
/// catalog_id)` — for carrying a person's decision time (Assets M5, R8).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ItemIdentity(String, String, String, String, Option<i64>);

impl ItemIdentity {
    fn of(r: &ChangesetItemRow) -> Self {
        ItemIdentity(
            r.grp.clone(),
            r.kind.clone(),
            r.name.clone(),
            r.action.clone(),
            r.catalog_id,
        )
    }

    fn of_new(n: &NewChangesetItem) -> Self {
        ItemIdentity(
            n.grp.clone(),
            n.kind.clone(),
            n.name.clone(),
            n.action.clone(),
            n.catalog_id,
        )
    }
}

const CARD_COLS: &str =
    "id, kind, summary, state, created_at, applied_at, commits, layers_snapshot, error";
const ITEM_COLS: &str =
    "changeset_id, position, grp, catalog_id, kind, name, action, params, decider, state, decided_at";

fn card_row(r: &rusqlite::Row<'_>) -> Result<ChangesetRow> {
    Ok(ChangesetRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        summary: r.get(2)?,
        state: r.get(3)?,
        created_at: r.get(4)?,
        applied_at: r.get(5)?,
        commits: r.get(6)?,
        layers_snapshot: r.get(7)?,
        error: r.get(8)?,
    })
}

fn item_row(r: &rusqlite::Row<'_>) -> Result<ChangesetItemRow> {
    Ok(ChangesetItemRow {
        changeset_id: r.get(0)?,
        position: r.get(1)?,
        grp: r.get(2)?,
        catalog_id: r.get(3)?,
        kind: r.get(4)?,
        name: r.get(5)?,
        action: r.get(6)?,
        params: r.get(7)?,
        decider: r.get(8)?,
        state: r.get(9)?,
        decided_at: r.get(10)?,
    })
}

fn insert_items(conn: &rusqlite::Connection, id: i64, items: &[NewChangesetItem]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO changeset_items \
           (changeset_id, position, grp, catalog_id, kind, name, action, params, decider, state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending')",
    )?;
    for (i, it) in items.iter().enumerate() {
        stmt.execute(rusqlite::params![
            id,
            i as i64,
            it.grp,
            it.catalog_id,
            it.kind,
            it.name,
            it.action,
            it.params,
            it.decider
        ])?;
    }
    Ok(())
}

/// The one item-state writer (with the re-reject in
/// [`Store::replace_changeset_items_keeping`]). Assets M5 (R8): leaving
/// `pending` is a decision, stamped in Unix ms; going back clears it.
fn set_item_states(
    conn: &rusqlite::Connection,
    id: i64,
    positions: &[i64],
    state: &str,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "UPDATE changeset_items SET state = ?3, \
           decided_at = CASE WHEN ?3 = 'pending' THEN NULL ELSE ?4 END \
         WHERE changeset_id = ?1 AND position = ?2",
    )?;
    let at = super::now_unix_ms();
    for p in positions {
        stmt.execute(rusqlite::params![id, p, state, at])?;
    }
    Ok(())
}

fn upsert_verdict(conn: &rusqlite::Connection, v: &TriageVerdictRow) -> Result<()> {
    conn.execute(
        "INSERT INTO asset_triage_verdicts \
           (catalog_id, kind, name, content_hash, verdict, decider, decided_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT (kind, name, content_hash) DO UPDATE SET \
           catalog_id = excluded.catalog_id, verdict = excluded.verdict, \
           decider = excluded.decider, decided_at = excluded.decided_at \
         WHERE asset_triage_verdicts.decider != 'person' OR excluded.decider = 'person'",
        rusqlite::params![
            v.catalog_id,
            v.kind,
            v.name,
            v.content_hash,
            v.verdict,
            v.decider,
            v.decided_at
        ],
    )?;
    Ok(())
}

fn mark_applied(
    conn: &rusqlite::Connection,
    id: i64,
    applied_at: i64,
    commits: &str,
    layers_snapshot: &str,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE changesets SET state = 'applied', applied_at = ?2, commits = ?3, \
         layers_snapshot = ?4, error = ?5 WHERE id = ?1",
        rusqlite::params![id, applied_at, commits, layers_snapshot, error],
    )?;
    Ok(())
}

/// Everything an apply records once its commits landed (Assets M4, Task 6
/// review): written by [`Store::record_changeset_applied`] in one
/// transaction, so the card is applied with all of it or with none of it.
#[derive(Debug, Clone, Default)]
pub struct AppliedRecord<'a> {
    /// Unix **milliseconds** (Rulings PF13).
    pub applied_at: i64,
    pub commits: &'a str,
    pub layers_snapshot: &'a str,
    pub applied: &'a [i64],
    pub skipped: &'a [i64],
    pub verdicts: &'a [TriageVerdictRow],
    /// A note on the applied card (a host card's skipped hosts), written
    /// with it — `None` clears any earlier error.
    pub error: Option<&'a str>,
}

impl Store {
    /// A card's apply recorded in ONE transaction: its verdicts, its items
    /// `applied` / `skipped`, the card `applied` with `r.error` as its note.
    /// Any failure rolls all of it back.
    pub fn record_changeset_applied(&self, id: i64, r: &AppliedRecord<'_>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for v in r.verdicts {
            upsert_verdict(&tx, v)?;
        }
        set_item_states(&tx, id, r.applied, "applied")?;
        set_item_states(&tx, id, r.skipped, "skipped")?;
        mark_applied(&tx, id, r.applied_at, r.commits, r.layers_snapshot, r.error)?;
        tx.commit()
    }

    /// A failed apply (Rulings R12), in ONE transaction: `pending` items back
    /// to pending, the card `failed` with `error`.
    pub fn fail_changeset(&self, id: i64, pending: &[i64], error: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        set_item_states(&tx, id, pending, "pending")?;
        tx.execute(
            "UPDATE changesets SET state = 'failed', error = ?2 WHERE id = ?1",
            rusqlite::params![id, error],
        )?;
        tx.commit()
    }

    /// A new `proposed` card with its items, in one transaction.
    /// `created_at` is stamped in Unix seconds.
    pub fn insert_changeset(
        &self,
        kind: &str,
        summary: &str,
        items: &[NewChangesetItem],
    ) -> Result<ChangesetRow> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO changesets (kind, summary, state, created_at) VALUES (?1, ?2, 'proposed', ?3)",
            rusqlite::params![kind, summary, now_unix()],
        )?;
        let id = tx.last_insert_rowid();
        insert_items(&tx, id, items)?;
        tx.commit()?;
        self.get_changeset(id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    /// Refresh an OPEN card (proposed or failed) in place: new summary, new
    /// items, every item pending. `false` (and nothing written) when the
    /// card is not open — applied, undone and dismissed cards are history.
    pub fn replace_changeset_items(
        &self,
        id: i64,
        summary: &str,
        items: &[NewChangesetItem],
    ) -> Result<bool> {
        self.replace_changeset_items_keeping(id, summary, items, |_| Vec::new())
    }

    /// [`Self::replace_changeset_items`], keeping what a person rejected
    /// (Rulings PF14): inside ONE transaction it reads the card's current
    /// items, asks `rejected` which positions of the NEW `items` stay
    /// `rejected` (the service matches them by `ItemKey`), replaces the
    /// items and re-rejects those positions. Any failure rolls back, so
    /// the old items — and their states — stay exactly as they were.
    pub fn replace_changeset_items_keeping(
        &self,
        id: i64,
        summary: &str,
        items: &[NewChangesetItem],
        rejected: impl FnOnce(&[ChangesetItemRow]) -> Vec<i64>,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(
            "UPDATE changesets SET summary = ?2 WHERE id = ?1 AND state IN ('proposed', 'failed')",
            rusqlite::params![id, summary],
        )?;
        if n == 0 {
            return Ok(false);
        }
        let old: Vec<ChangesetItemRow> = tx
            .prepare(&format!(
                "SELECT {ITEM_COLS} FROM changeset_items WHERE changeset_id = ?1 ORDER BY position"
            ))?
            .query_map([id], item_row)?
            .collect::<Result<_>>()?;
        let keep = rejected(&old);
        // Assets M5 (R8): a re-rejected item keeps when the person decided
        // it — matched on what the item is, since positions change across a
        // refresh. One it cannot match (or decided before 097) is stamped now.
        let decided: BTreeMap<ItemIdentity, i64> = old
            .iter()
            .filter(|i| i.state == "rejected")
            .filter_map(|i| Some((ItemIdentity::of(i), i.decided_at?)))
            .collect();
        let now = super::now_unix_ms();
        tx.execute("DELETE FROM changeset_items WHERE changeset_id = ?1", [id])?;
        insert_items(&tx, id, items)?;
        {
            let mut stmt = tx.prepare(
                "UPDATE changeset_items SET state = 'rejected', decided_at = ?3 \
                 WHERE changeset_id = ?1 AND position = ?2",
            )?;
            for p in keep {
                let at = usize::try_from(p)
                    .ok()
                    .and_then(|p| items.get(p))
                    .and_then(|it| decided.get(&ItemIdentity::of_new(it)).copied())
                    .unwrap_or(now);
                stmt.execute(rusqlite::params![id, p, at])?;
            }
        }
        tx.commit()?;
        Ok(true)
    }

    /// An undo recorded in ONE transaction (Assets M4, Rulings R20): each
    /// of `catalogs`' `host_layers` put back from `snapshot` (replacing that
    /// catalog's rows, hosts deleted since skipped — see
    /// [`Store::restore_host_layers`]) and the card `undone` with its error
    /// cleared — only while it is `applied`. `false`, and nothing written,
    /// when it is not. `applied_at` keeps the apply's time (milliseconds,
    /// PF13); an undo stamps no time of its own.
    pub fn record_changeset_undone(
        &self,
        id: i64,
        catalogs: &[i64],
        snapshot: &[super::HostLayerRow],
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(
            "UPDATE changesets SET state = 'undone', error = NULL WHERE id = ?1 AND state = 'applied'",
            [id],
        )?;
        if n == 0 {
            return Ok(false);
        }
        for c in catalogs {
            super::layers::restore_rows(&tx, *c, snapshot)?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// A person's no (Assets M4, Rulings R9/R10) in ONE transaction, only
    /// while the card is open (`proposed` or `failed`): `verdicts`
    /// recorded, the items at `positions` `rejected`, and the card
    /// `dismissed` when `close`. `false`, and nothing written, when the card
    /// is not open.
    pub fn reject_changeset_items(
        &self,
        id: i64,
        positions: &[i64],
        verdicts: &[TriageVerdictRow],
        close: bool,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let open: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM changesets WHERE id = ?1 AND state IN ('proposed', 'failed')",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if open.is_none() {
            return Ok(false);
        }
        for v in verdicts {
            upsert_verdict(&tx, v)?;
        }
        set_item_states(&tx, id, positions, "rejected")?;
        if close {
            tx.execute(
                "UPDATE changesets SET state = 'dismissed' WHERE id = ?1",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Withdraw an OPEN card (R2): `dismissed` with `error`, only while it
    /// is `proposed` or `failed` — a card that was applied, undone or
    /// dismissed meanwhile stays as it is. `true` when it was withdrawn.
    pub fn withdraw_changeset(&self, id: i64, error: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE changesets SET state = 'dismissed', error = ?2 \
             WHERE id = ?1 AND state IN ('proposed', 'failed')",
            rusqlite::params![id, error],
        )?;
        Ok(n > 0)
    }

    pub fn get_changeset(&self, id: i64) -> Result<Option<ChangesetRow>> {
        self.conn
            .query_row(
                &format!("SELECT {CARD_COLS} FROM changesets WHERE id = ?1"),
                [id],
                card_row,
            )
            .optional()
    }

    /// Every card, newest first.
    pub fn list_changesets(&self) -> Result<Vec<ChangesetRow>> {
        self.conn
            .prepare(&format!(
                "SELECT {CARD_COLS} FROM changesets ORDER BY id DESC"
            ))?
            .query_map([], card_row)?
            .collect()
    }

    /// A card's items, by position.
    pub fn changeset_items(&self, id: i64) -> Result<Vec<ChangesetItemRow>> {
        self.conn
            .prepare(&format!(
                "SELECT {ITEM_COLS} FROM changeset_items WHERE changeset_id = ?1 ORDER BY position"
            ))?
            .query_map([id], item_row)?
            .collect()
    }

    pub fn set_changeset_state(&self, id: i64, state: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE changesets SET state = ?2, error = ?3 WHERE id = ?1",
            rusqlite::params![id, state, error],
        )?;
        Ok(())
    }

    /// The card applied: when (`applied_at`, Unix **milliseconds** — stamp
    /// it with [`super::now_unix_ms`], Rulings PF13), its commits and its
    /// `host_layers` snapshot (both JSON), and any warning (a reload or push
    /// that failed after the commits, Rulings R12).
    pub fn mark_changeset_applied(
        &self,
        id: i64,
        applied_at: i64,
        commits: &str,
        layers_snapshot: &str,
        error: Option<&str>,
    ) -> Result<()> {
        mark_applied(&self.conn, id, applied_at, commits, layers_snapshot, error)
    }

    pub fn set_changeset_item_states(&self, id: i64, positions: &[i64], state: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        set_item_states(&tx, id, positions, state)?;
        tx.commit()
    }

    /// Record a verdict on `(kind, name, content_hash)`. A `person` verdict
    /// is never replaced by another decider's (spec: "an agent never
    /// overturns a person's verdict", Rulings R10); a person may replace
    /// anything.
    pub fn upsert_triage_verdict(&self, v: &TriageVerdictRow) -> Result<()> {
        upsert_verdict(&self.conn, v)
    }

    pub fn triage_verdicts(&self) -> Result<Vec<TriageVerdictRow>> {
        self.conn
            .prepare(
                "SELECT catalog_id, kind, name, content_hash, verdict, decider, decided_at \
                 FROM asset_triage_verdicts ORDER BY kind, name, content_hash",
            )?
            .query_map([], |r| {
                Ok(TriageVerdictRow {
                    catalog_id: r.get(0)?,
                    kind: r.get(1)?,
                    name: r.get(2)?,
                    content_hash: r.get(3)?,
                    verdict: r.get(4)?,
                    decider: r.get(5)?,
                    decided_at: r.get(6)?,
                })
            })?
            .collect()
    }

    /// `(catalog_id, layer)` of every layer some rollout card has applied a
    /// `sync` item for (Rulings R16) — on an applied or a failed card.
    pub fn rolled_out_layers(&self) -> Result<BTreeSet<(i64, String)>> {
        self.conn
            .prepare(
                "SELECT DISTINCT i.catalog_id, json_extract(i.params, '$.layer') \
                 FROM changeset_items i JOIN changesets c ON c.id = i.changeset_id \
                 WHERE c.kind = 'rollout' AND i.action = 'sync' AND i.state = 'applied' \
                   AND i.catalog_id IS NOT NULL AND json_extract(i.params, '$.layer') IS NOT NULL",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::collections::BTreeSet;

    fn item(
        grp: &str,
        action: &str,
        catalog_id: Option<i64>,
        params: Option<&str>,
    ) -> NewChangesetItem {
        NewChangesetItem {
            grp: grp.into(),
            catalog_id,
            kind: "skill".into(),
            name: format!("{grp}-{action}"),
            action: action.into(),
            params: params.map(String::from),
            decider: "rule".into(),
        }
    }

    fn store() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let personal = s.personal_catalog().unwrap().unwrap().id;
        (s, personal)
    }

    #[test]
    fn a_card_round_trips_and_only_an_open_card_is_refreshed() {
        let (s, p) = store();
        let card = s
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 1 layers",
                &[
                    item("core", "import", Some(p), Some(r#"{"from_host":"oci"}"#)),
                    item("core", "assign_layer", Some(p), None),
                ],
            )
            .unwrap();
        assert_eq!(
            (card.kind.as_str(), card.state.as_str(), card.applied_at),
            ("bootstrap", "proposed", None)
        );
        let items = s.changeset_items(card.id).unwrap();
        assert_eq!(items.iter().map(|i| i.position).collect::<Vec<_>>(), [0, 1]);
        assert!(items.iter().all(|i| i.state == "pending"));
        assert_eq!(items[0].params.as_deref(), Some(r#"{"from_host":"oci"}"#));
        assert_eq!(
            NewChangesetItem::from(&items[1]),
            item("core", "assign_layer", Some(p), None)
        );

        assert!(s
            .replace_changeset_items(
                card.id,
                "Adopt 1 as 1 layers",
                &[item("core", "import", Some(p), None)]
            )
            .unwrap());
        assert_eq!(s.changeset_items(card.id).unwrap().len(), 1);
        assert_eq!(
            s.get_changeset(card.id).unwrap().unwrap().summary,
            "Adopt 1 as 1 layers"
        );

        s.set_changeset_item_states(card.id, &[0], "applied")
            .unwrap();
        s.mark_changeset_applied(card.id, 50, r#"{"1":"abc"}"#, "[]", None)
            .unwrap();
        let applied = s.get_changeset(card.id).unwrap().unwrap();
        assert_eq!(
            (
                applied.state.as_str(),
                applied.applied_at,
                applied.commits.as_deref()
            ),
            ("applied", Some(50), Some(r#"{"1":"abc"}"#))
        );
        assert!(
            !s.replace_changeset_items(card.id, "x", &[]).unwrap(),
            "an applied card is history"
        );
        assert_eq!(s.changeset_items(card.id).unwrap()[0].state, "applied");
        let newer = s.insert_changeset("new", "New", &[]).unwrap();
        assert_eq!(
            s.list_changesets()
                .unwrap()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            [newer.id, card.id],
            "newest first"
        );
        s.set_changeset_state(card.id, "undone", None).unwrap();
        assert_eq!(s.get_changeset(card.id).unwrap().unwrap().state, "undone");
    }

    /// Assets M4 undo / reject: each writes only while the card is in the
    /// state it expects — an undo only an applied card (restoring its
    /// catalogs' host_layers with it), a rejection only an open one.
    #[test]
    fn an_undo_and_a_rejection_write_only_in_the_state_they_expect() {
        let (s, p) = store();
        s.upsert_host("oci").unwrap();
        let card = s
            .insert_changeset(
                "new",
                "New",
                &[item("core", "import", Some(p), Some(r#"{"hash":"h"}"#))],
            )
            .unwrap();
        s.set_host_layers_for("oci", p, None, &["core"]).unwrap();
        assert!(!s.record_changeset_undone(card.id, &[p], &[]).unwrap());
        assert_eq!(
            s.get_host_layers_for("oci", p).unwrap().len(),
            1,
            "nothing written"
        );
        s.mark_changeset_applied(card.id, 5, "{}", "[]", Some("note"))
            .unwrap();
        let verdict = TriageVerdictRow {
            catalog_id: Some(p),
            kind: "skill".into(),
            name: "w".into(),
            content_hash: "h".into(),
            verdict: "rejected".into(),
            decider: "person".into(),
            decided_at: 1,
        };
        assert!(!s
            .reject_changeset_items(card.id, &[0], std::slice::from_ref(&verdict), true)
            .unwrap());
        assert!(s.triage_verdicts().unwrap().is_empty(), "nothing written");
        assert!(s.record_changeset_undone(card.id, &[p], &[]).unwrap());
        let undone = s.get_changeset(card.id).unwrap().unwrap();
        assert_eq!(
            (undone.state.as_str(), undone.error, undone.applied_at),
            ("undone", None, Some(5))
        );
        assert!(
            s.get_host_layers_for("oci", p).unwrap().is_empty(),
            "back to the empty snapshot"
        );

        let open = s
            .insert_changeset(
                "new",
                "New",
                &[
                    item("core", "import", Some(p), None),
                    item("core", "set_scope", Some(p), None),
                ],
            )
            .unwrap();
        assert!(s
            .reject_changeset_items(open.id, &[0], &[verdict], false)
            .unwrap());
        let row = s.get_changeset(open.id).unwrap().unwrap();
        let states: Vec<String> = s
            .changeset_items(open.id)
            .unwrap()
            .into_iter()
            .map(|i| i.state)
            .collect();
        assert_eq!(
            (row.state.as_str(), states),
            (
                "proposed",
                vec!["rejected".to_string(), "pending".to_string()]
            )
        );
        assert_eq!(s.triage_verdicts().unwrap().len(), 1);
        assert!(s.reject_changeset_items(open.id, &[1], &[], true).unwrap());
        assert_eq!(
            s.get_changeset(open.id).unwrap().unwrap().state,
            "dismissed"
        );
    }

    /// PF14: the replace and the re-reject are one transaction — a failure
    /// part-way leaves the old items and their states intact.
    #[test]
    fn a_refresh_keeps_rejections_atomically() {
        let (s, p) = store();
        let card = s
            .insert_changeset(
                "bootstrap",
                "Adopt 2",
                &[
                    item("core", "import", Some(p), None),
                    item("core", "assign_layer", Some(p), None),
                ],
            )
            .unwrap();
        s.set_changeset_item_states(card.id, &[1], "rejected")
            .unwrap();
        let new = [
            item("core", "set_scope", Some(p), None),
            item("core", "assign_layer", Some(p), None),
        ];
        assert!(s
            .replace_changeset_items_keeping(card.id, "Adopt 3", &new, |old| {
                assert_eq!(old[1].state, "rejected", "it sees the current items");
                vec![1]
            })
            .unwrap());
        let states: Vec<String> = s
            .changeset_items(card.id)
            .unwrap()
            .into_iter()
            .map(|i| i.state)
            .collect();
        assert_eq!(states, ["pending", "rejected"]);

        let before = s.changeset_items(card.id).unwrap();
        let broken = [item("core", "import", Some(9999), None)];
        assert!(
            s.replace_changeset_items_keeping(card.id, "Broken", &broken, |_| vec![0])
                .is_err(),
            "an item naming no catalog trips the foreign key"
        );
        assert_eq!(s.changeset_items(card.id).unwrap(), before);
        assert_eq!(
            s.get_changeset(card.id).unwrap().unwrap().summary,
            "Adopt 3"
        );
    }

    /// R2: withdrawal only ever closes an open card.
    #[test]
    fn only_an_open_card_is_withdrawn() {
        let (s, _) = store();
        let failed = s.insert_changeset("new", "F", &[]).unwrap();
        s.set_changeset_state(failed.id, "failed", Some("boom"))
            .unwrap();
        assert!(s.withdraw_changeset(failed.id, "gone").unwrap());
        let row = s.get_changeset(failed.id).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.error.as_deref()),
            ("dismissed", Some("gone"))
        );
        let applied = s.insert_changeset("new", "A", &[]).unwrap();
        s.mark_changeset_applied(applied.id, 5, "{}", "[]", None)
            .unwrap();
        assert!(!s.withdraw_changeset(applied.id, "gone").unwrap());
        assert_eq!(
            s.get_changeset(applied.id).unwrap().unwrap().state,
            "applied"
        );
    }

    /// Spec, Testing (store): a verdict holds by content hash; a person's
    /// verdict is never replaced by another decider (Rulings R10).
    #[test]
    fn verdicts_hold_by_content_hash_and_a_persons_verdict_stays() {
        let (s, _) = store();
        let v = |hash: &str, verdict: &str, decider: &str| TriageVerdictRow {
            catalog_id: None,
            kind: "skill".into(),
            name: "w".into(),
            content_hash: hash.into(),
            verdict: verdict.into(),
            decider: decider.into(),
            decided_at: 1,
        };
        s.upsert_triage_verdict(&v("h1", "rejected", "person"))
            .unwrap();
        s.upsert_triage_verdict(&v("h1", "ignored", "rule"))
            .unwrap();
        s.upsert_triage_verdict(&v("h2", "ignored", "rule"))
            .unwrap();
        let all = s.triage_verdicts().unwrap();
        assert_eq!(all.len(), 2, "one row per (kind, name, content_hash)");
        let h1 = all.iter().find(|r| r.content_hash == "h1").unwrap();
        assert_eq!(
            (h1.verdict.as_str(), h1.decider.as_str()),
            ("rejected", "person"),
            "a rule never overturns a person"
        );
        s.upsert_triage_verdict(&v("h2", "rejected", "person"))
            .unwrap();
        let h2 = s
            .triage_verdicts()
            .unwrap()
            .into_iter()
            .find(|r| r.content_hash == "h2")
            .unwrap();
        assert_eq!(h2.decider, "person", "a person may overturn a rule");
    }

    /// Rulings R16: a layer is rolled out once an applied `sync` item names
    /// it — on a failed card too (its other hosts failed, not this one).
    #[test]
    fn a_layer_is_rolled_out_once_an_applied_sync_item_names_it() {
        let (s, p) = store();
        let sync = |layer: &str| NewChangesetItem {
            grp: layer.into(),
            catalog_id: Some(p),
            kind: "host".into(),
            name: "oci".into(),
            action: "sync".into(),
            params: Some(format!(r#"{{"layer":"{layer}","assets":["skill/w"]}}"#)),
            decider: "rule".into(),
        };
        let a = s
            .insert_changeset(
                "rollout",
                "Roll out core to oci",
                &[sync("core"), sync("extra")],
            )
            .unwrap();
        assert!(
            s.rolled_out_layers().unwrap().is_empty(),
            "proposed is not rolled out"
        );
        s.set_changeset_item_states(a.id, &[0], "applied").unwrap();
        s.set_changeset_state(a.id, "failed", Some("trn: unreachable"))
            .unwrap();
        assert_eq!(
            s.rolled_out_layers().unwrap(),
            BTreeSet::from([(p, "core".to_string())])
        );
    }

    /// Rulings R26: removing a catalog withdraws the open cards that name it
    /// and clears its items' catalog; applied history stays.
    #[test]
    fn removing_a_catalog_clears_its_items_and_withdraws_open_cards() {
        let (s, p) = store();
        let org = s.add_org("acme", None, false).unwrap();
        let acme = s
            .upsert_catalog("acme", "/a", None, Some(org.id))
            .unwrap()
            .id;
        let open = s
            .insert_changeset("new", "New", &[item("core", "import", Some(acme), None)])
            .unwrap();
        let done = s
            .insert_changeset(
                "new",
                "Done",
                &[
                    item("core", "import", Some(acme), None),
                    item("core", "import", Some(p), None),
                ],
            )
            .unwrap();
        s.mark_changeset_applied(done.id, 5, "{}", "[]", None)
            .unwrap();

        let gone = s.remove_catalog("acme").unwrap();
        assert_eq!(gone.cards, 1, "one open card withdrawn");
        let open = s.get_changeset(open.id).unwrap().unwrap();
        assert_eq!(open.state, "dismissed");
        assert!(open.error.as_deref().unwrap().contains("acme was removed"));
        assert_eq!(
            s.get_changeset(done.id).unwrap().unwrap().state,
            "applied",
            "history stays"
        );
        let items = s.changeset_items(done.id).unwrap();
        assert_eq!((items[0].catalog_id, items[1].catalog_id), (None, Some(p)));
    }

    /// Task 6 review: an apply's bookkeeping is one transaction — a failing
    /// write leaves the card and its items exactly as they were; a failed
    /// apply puts its items back to pending and fails the card together.
    #[test]
    fn an_apply_is_recorded_whole_or_not_at_all() {
        let (s, p) = store();
        let card = s
            .insert_changeset(
                "new",
                "New",
                &[
                    item("core", "import", Some(p), None),
                    item("hidden", "hide", None, None),
                ],
            )
            .unwrap();
        let bad = TriageVerdictRow {
            catalog_id: Some(9999),
            kind: "hook".into(),
            name: "stop".into(),
            content_hash: "h".into(),
            verdict: "ignored".into(),
            decider: "rule".into(),
            decided_at: 1,
        };
        let r = AppliedRecord {
            applied_at: 1_700_000_000_000,
            commits: "{}",
            layers_snapshot: "[]",
            applied: &[0, 1],
            skipped: &[],
            verdicts: std::slice::from_ref(&bad),
            error: None,
        };
        assert!(s.record_changeset_applied(card.id, &r).is_err());
        assert_eq!(s.get_changeset(card.id).unwrap().unwrap().state, "proposed");
        assert!(s
            .changeset_items(card.id)
            .unwrap()
            .iter()
            .all(|i| i.state == "pending"));
        assert!(s.triage_verdicts().unwrap().is_empty());

        let good = TriageVerdictRow {
            catalog_id: None,
            ..bad.clone()
        };
        let r = AppliedRecord {
            verdicts: &[good],
            applied: &[1],
            skipped: &[0],
            error: Some("skipped: trn (claude): unreachable"),
            ..r
        };
        s.record_changeset_applied(card.id, &r).unwrap();
        let row = s.get_changeset(card.id).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.applied_at),
            ("applied", Some(1_700_000_000_000))
        );
        assert_eq!(
            row.error.as_deref(),
            Some("skipped: trn (claude): unreachable"),
            "the note lands in the same transaction"
        );
        let states: Vec<String> = s
            .changeset_items(card.id)
            .unwrap()
            .into_iter()
            .map(|i| i.state)
            .collect();
        assert_eq!(states, ["skipped", "applied"]);
        assert_eq!(s.triage_verdicts().unwrap().len(), 1);

        let other = s
            .insert_changeset("new", "Other", &[item("core", "import", Some(p), None)])
            .unwrap();
        s.set_changeset_item_states(other.id, &[0], "applied")
            .unwrap();
        s.fail_changeset(other.id, &[0], "core: boom").unwrap();
        let row = s.get_changeset(other.id).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.error.as_deref()),
            ("failed", Some("core: boom"))
        );
        assert_eq!(s.changeset_items(other.id).unwrap()[0].state, "pending");
    }

    /// Assets M5 (R8): an item's decision time is stamped when it leaves
    /// pending, kept when a refresh re-rejects it, cleared when it goes back.
    #[test]
    fn item_decision_times_are_stamped_kept_and_cleared() {
        let (s, p) = store();
        let items = [
            item("core", "set_scope", Some(p), None),
            item("core", "assign_layer", Some(p), None),
        ];
        let card = s.insert_changeset("bootstrap", "Adopt 2", &items).unwrap();
        assert!(s
            .changeset_items(card.id)
            .unwrap()
            .iter()
            .all(|i| i.decided_at.is_none()));

        s.set_changeset_item_states(card.id, &[1], "rejected")
            .unwrap();
        let at = s.changeset_items(card.id).unwrap()[1]
            .decided_at
            .expect("stamped when rejected");
        s.conn
            .execute(
                "UPDATE changeset_items SET decided_at = ?2 WHERE changeset_id = ?1 AND position = 1",
                rusqlite::params![card.id, at - 60_000],
            )
            .unwrap();
        // The refresh moves the rejected item to position 0 (PF14): its
        // time follows what it is, not where it sat.
        let moved = [items[1].clone(), items[0].clone()];
        assert!(s
            .replace_changeset_items_keeping(card.id, "Adopt 2", &moved, |_| vec![0])
            .unwrap());
        let now = s.changeset_items(card.id).unwrap();
        assert_eq!(now[0].state, "rejected");
        assert_eq!(
            now[0].decided_at,
            Some(at - 60_000),
            "a refresh keeps when the person decided"
        );
        assert_eq!(now[1].decided_at, None, "a pending item has no time");

        s.set_changeset_item_states(card.id, &[0], "pending")
            .unwrap();
        assert_eq!(s.changeset_items(card.id).unwrap()[0].decided_at, None);

        s.record_changeset_applied(
            card.id,
            &AppliedRecord {
                applied_at: 5,
                commits: "{}",
                layers_snapshot: "[]",
                applied: &[0],
                skipped: &[1],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            s.changeset_items(card.id)
                .unwrap()
                .iter()
                .all(|i| i.decided_at.is_some()),
            "an apply stamps what it applied and what it skipped"
        );
    }
}
