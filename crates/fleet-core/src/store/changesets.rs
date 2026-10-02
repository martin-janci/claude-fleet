//! Changeset cards, their items and triage verdicts (Assets M4, migration
//! 094). The rules — which cards exist, what applying one does — live in
//! `service::catalog::changesets`; this is the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};
use std::collections::BTreeSet;

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

const CARD_COLS: &str =
    "id, kind, summary, state, created_at, applied_at, commits, layers_snapshot, error";
const ITEM_COLS: &str =
    "changeset_id, position, grp, catalog_id, kind, name, action, params, decider, state";

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

impl Store {
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
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(
            "UPDATE changesets SET summary = ?2 WHERE id = ?1 AND state IN ('proposed', 'failed')",
            rusqlite::params![id, summary],
        )?;
        if n == 0 {
            return Ok(false);
        }
        tx.execute("DELETE FROM changeset_items WHERE changeset_id = ?1", [id])?;
        insert_items(&tx, id, items)?;
        tx.commit()?;
        Ok(true)
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
        self.conn.execute(
            "UPDATE changesets SET state = 'applied', applied_at = ?2, commits = ?3, \
             layers_snapshot = ?4, error = ?5 WHERE id = ?1",
            rusqlite::params![id, applied_at, commits, layers_snapshot, error],
        )?;
        Ok(())
    }

    pub fn set_changeset_item_states(&self, id: i64, positions: &[i64], state: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "UPDATE changeset_items SET state = ?3 WHERE changeset_id = ?1 AND position = ?2",
            )?;
            for p in positions {
                stmt.execute(rusqlite::params![id, p, state])?;
            }
        }
        tx.commit()
    }

    /// Record a verdict on `(kind, name, content_hash)`. A `person` verdict
    /// is never replaced by another decider's (spec: "an agent never
    /// overturns a person's verdict", Rulings R10); a person may replace
    /// anything.
    pub fn upsert_triage_verdict(&self, v: &TriageVerdictRow) -> Result<()> {
        self.conn.execute(
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
}
