//! Assets M4: changeset cards — proposed by rules (`rules`), built and
//! refreshed by the reconcile pass (`reconcile`), applied (`apply`) and
//! undone (`undo`). Spec: docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md,
//! *Changesets (the cards)*. The rows are `store::changesets`.

pub mod reconcile;
pub mod rules;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::store::{ChangesetItemRow, ChangesetRow, NewChangesetItem, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// One apply at a time (Rulings R27): apply, undo, dismiss, reject_item,
/// on-demand propose, the tick's reconcile and SB6 all take it. A tokio
/// mutex, not the store: it is held across the awaits of an apply, while
/// every store guard inside stays scoped. The tick only ever `try_lock`s it
/// (R19).
pub(crate) static APPLY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Closed cards `list` shows next to every open one.
pub const RECENT_CLOSED: usize = 20;

/// What a card is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Bootstrap,
    New,
    Drift,
    Rollout,
}

impl CardKind {
    pub const ALL: [CardKind; 4] = [
        CardKind::Bootstrap,
        CardKind::New,
        CardKind::Drift,
        CardKind::Rollout,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CardKind::Bootstrap => "bootstrap",
            CardKind::New => "new",
            CardKind::Drift => "drift",
            CardKind::Rollout => "rollout",
        }
    }

    pub fn parse(s: &str) -> Option<CardKind> {
        CardKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What applying one item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemAction {
    Import,
    AssignLayer,
    SetScope,
    Hide,
    TakeHost,
    Restore,
    Sync,
}

impl ItemAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemAction::Import => "import",
            ItemAction::AssignLayer => "assign_layer",
            ItemAction::SetScope => "set_scope",
            ItemAction::Hide => "hide",
            ItemAction::TakeHost => "take_host",
            ItemAction::Restore => "restore",
            ItemAction::Sync => "sync",
        }
    }
}

/// Who decided an item or a verdict. M4 writes `Rule` and `Person` only;
/// `Jev` and `Haiku` are S5's (Rulings R10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decider {
    Rule,
    Jev,
    Haiku,
    Person,
}

impl Decider {
    pub fn as_str(self) -> &'static str {
        match self {
            Decider::Rule => "rule",
            Decider::Jev => "jev",
            Decider::Haiku => "haiku",
            Decider::Person => "person",
        }
    }
}

/// An item's `params` JSON (Rulings R8). Every field optional; each action
/// reads the ones it needs: import `from_host`, `layer`, `member`, `hash`,
/// `reason`; assign_layer `host`, `layer`, `axis`; set_scope `scope`,
/// `member`; hide `hash`, `reason`; take_host/restore `host`, `hash`; sync
/// `layer`, `assets`, `hash`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// `<kind>/<catalog name>` — the layer member key the import becomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The subject's content hash a verdict on this item records (R9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<String>,
}

impl ItemParams {
    /// A stored `params`; anything unreadable is "no params".
    pub fn parse(raw: Option<&str>) -> ItemParams {
        raw.and_then(|r| serde_json::from_str(r).ok())
            .unwrap_or_default()
    }

    /// The stored form: `None` when every field is empty.
    pub(crate) fn to_json(&self) -> Option<String> {
        if *self == ItemParams::default() {
            None
        } else {
            serde_json::to_string(self).ok()
        }
    }
}

/// One item a rule proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedItem {
    pub grp: String,
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    pub action: ItemAction,
    pub params: ItemParams,
    pub decider: Decider,
}

/// What one item is across refreshes of its card (PF14): its catalog, kind,
/// name and action, plus the host it targets (`params.host`) — without the
/// host, one layer's assignments to two hosts would be one item. A refresh
/// keeps a stored item a person rejected when the new proposal has its key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemKey {
    pub catalog_id: Option<i64>,
    pub kind: String,
    pub name: String,
    pub action: String,
    pub host: Option<String>,
}

impl ItemKey {
    /// A stored item's key.
    pub fn of_row(row: &ChangesetItemRow) -> ItemKey {
        ItemKey {
            catalog_id: row.catalog_id,
            kind: row.kind.clone(),
            name: row.name.clone(),
            action: row.action.clone(),
            host: ItemParams::parse(row.params.as_deref()).host,
        }
    }
}

impl ProposedItem {
    pub fn key(&self) -> ItemKey {
        ItemKey {
            catalog_id: self.catalog_id,
            kind: self.kind.clone(),
            name: self.name.clone(),
            action: self.action.as_str().to_string(),
            host: self.params.host.clone(),
        }
    }

    pub fn to_new(&self) -> NewChangesetItem {
        NewChangesetItem {
            grp: self.grp.clone(),
            catalog_id: self.catalog_id,
            kind: self.kind.clone(),
            name: self.name.clone(),
            action: self.action.as_str().to_string(),
            params: self.params.to_json(),
            decider: self.decider.as_str().to_string(),
        }
    }
}

/// One card a rule proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedCard {
    pub kind: CardKind,
    pub summary: String,
    pub items: Vec<ProposedItem>,
}

impl ProposedCard {
    /// Its subject (Rulings R2).
    pub fn subject(&self) -> String {
        rules::subject_of(
            self.kind,
            self.items.iter().map(|i| rules::SubjectItem {
                grp: &i.grp,
                catalog_id: i.catalog_id,
                kind: &i.kind,
                name: &i.name,
                host: i.params.host.as_deref(),
            }),
        )
    }
}

/// One item as the tool shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemView {
    pub position: i64,
    pub grp: String,
    /// The catalog's name; `None` for an item that names none (hide).
    #[serde(default)]
    pub catalog: Option<String>,
    pub kind: String,
    pub name: String,
    pub action: String,
    #[serde(default)]
    pub params: ItemParams,
    pub decider: String,
    pub state: String,
}

/// One card in full (`changesets { list, id }`, and every mutating action's
/// answer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetView {
    pub id: i64,
    pub kind: String,
    pub summary: String,
    pub state: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix milliseconds (Rulings PF13).
    #[serde(default)]
    pub applied_at: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    /// Catalog name → the commit the apply made there.
    #[serde(default)]
    pub commits: BTreeMap<String, String>,
    #[serde(default)]
    pub undoable: bool,
    pub items: Vec<ItemView>,
}

/// One card in a list (`changesets { list }`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetSummary {
    pub id: i64,
    pub kind: String,
    pub summary: String,
    pub state: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix milliseconds (Rulings PF13).
    #[serde(default)]
    pub applied_at: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    /// Group → item count.
    #[serde(default)]
    pub groups: BTreeMap<String, usize>,
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub undoable: bool,
}

/// R3: a card that can still be applied (and that the pass refreshes).
pub fn is_open(state: &str) -> bool {
    matches!(state, "proposed" | "failed")
}

fn not_found(id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("no changeset card {id}"))
}

/// A card and its items, or `E_NOTFOUND`.
pub fn card(
    id: i64,
    store: &Mutex<Store>,
) -> Result<(ChangesetRow, Vec<ChangesetItemRow>), IpcError> {
    let s = lock(store)?;
    let row = s.get_changeset(id)?.ok_or_else(|| not_found(id))?;
    let items = s.changeset_items(id)?;
    Ok((row, items))
}

/// Whether applying `positions` of this card writes to hosts: every rollout,
/// and a drift card's restore (R15, R25).
pub fn writes_hosts(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    positions: Option<&[i64]>,
) -> bool {
    match card.kind.as_str() {
        "rollout" => true,
        "drift" => positions.unwrap_or(&[]).iter().any(|p| {
            items
                .iter()
                .any(|i| i.position == *p && i.action == ItemAction::Restore.as_str())
        }),
        _ => false,
    }
}

/// A card that changed a catalog, so can be undone (R20): bootstrap, new,
/// and a drift applied as take_host.
pub(crate) fn changes_catalog(card: &ChangesetRow, items: &[ChangesetItemRow]) -> bool {
    match card.kind.as_str() {
        "bootstrap" | "new" => true,
        "drift" => items
            .iter()
            .any(|i| i.action == ItemAction::TakeHost.as_str() && i.state == "applied"),
        _ => false,
    }
}

/// The catalogs a card's applied items touched.
pub(crate) fn applied_catalogs(items: &[ChangesetItemRow]) -> BTreeSet<i64> {
    items
        .iter()
        .filter(|i| i.state == "applied")
        .filter_map(|i| i.catalog_id)
        .collect()
}

/// R20: a later applied catalog-changing card sharing a catalog with `card`
/// — `(its id, the shared catalog's name)` — which must be undone first.
/// "Later" orders by `(applied_at, id)`; `applied_at` is milliseconds
/// (PF13).
pub(crate) fn later_card(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    s: &Store,
) -> Result<Option<(i64, String)>, IpcError> {
    let mine = applied_catalogs(items);
    for other in s.list_changesets()? {
        if other.id == card.id
            || other.state != "applied"
            || (other.applied_at, other.id) <= (card.applied_at, card.id)
        {
            continue;
        }
        let its = s.changeset_items(other.id)?;
        if !changes_catalog(&other, &its) {
            continue;
        }
        if let Some(cid) = applied_catalogs(&its).intersection(&mine).next() {
            let name = s
                .get_catalog(*cid)?
                .map(|r| r.name)
                .unwrap_or_else(|| cid.to_string());
            return Ok(Some((other.id, name)));
        }
    }
    Ok(None)
}

fn is_undoable(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    s: &Store,
) -> Result<bool, IpcError> {
    Ok(card.state == "applied"
        && changes_catalog(card, items)
        && later_card(card, items, s)?.is_none())
}

fn view(
    card: ChangesetRow,
    items: Vec<ChangesetItemRow>,
    s: &Store,
) -> Result<ChangesetView, IpcError> {
    let names: BTreeMap<i64, String> = s
        .list_catalogs()?
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect();
    let undoable = is_undoable(&card, &items, s)?;
    let commits = card
        .commits
        .as_deref()
        .and_then(|c| serde_json::from_str::<BTreeMap<String, String>>(c).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(id, sha)| {
            let name = id.parse::<i64>().ok().and_then(|i| names.get(&i).cloned());
            (name.unwrap_or(id), sha)
        })
        .collect();
    Ok(ChangesetView {
        id: card.id,
        kind: card.kind,
        summary: card.summary,
        state: card.state,
        created_at: card.created_at,
        applied_at: card.applied_at,
        error: card.error,
        commits,
        undoable,
        items: items
            .into_iter()
            .map(|i| ItemView {
                position: i.position,
                grp: i.grp,
                catalog: i.catalog_id.and_then(|c| names.get(&c).cloned()),
                kind: i.kind,
                name: i.name,
                action: i.action,
                params: ItemParams::parse(i.params.as_deref()),
                decider: i.decider,
                state: i.state,
            })
            .collect(),
    })
}

/// One card in full.
pub fn get(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError> {
    let s = lock(store)?;
    let card = s.get_changeset(id)?.ok_or_else(|| not_found(id))?;
    let items = s.changeset_items(id)?;
    view(card, items, &s)
}

/// Every open card and the [`RECENT_CLOSED`] most recent others, newest first.
pub fn list(store: &Mutex<Store>) -> Result<Vec<ChangesetSummary>, IpcError> {
    let s = lock(store)?;
    let mut closed = 0;
    let mut out = Vec::new();
    for card in s.list_changesets()? {
        if !is_open(&card.state) {
            if closed >= RECENT_CLOSED {
                continue;
            }
            closed += 1;
        }
        let items = s.changeset_items(card.id)?;
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        for i in &items {
            *groups.entry(i.grp.clone()).or_insert(0) += 1;
        }
        let pending = items.iter().filter(|i| i.state == "pending").count();
        let undoable = is_undoable(&card, &items, &s)?;
        out.push(ChangesetSummary {
            id: card.id,
            kind: card.kind,
            summary: card.summary,
            state: card.state,
            created_at: card.created_at,
            applied_at: card.applied_at,
            error: card.error,
            groups,
            pending,
            undoable,
        });
    }
    Ok(out)
}

/// `changesets { propose }`: run the pass now (waiting for an apply in
/// flight), with `hide` items instead of automatic verdicts when
/// `catalog.auto` is off (R18), then list. Unlike the tick (R19) it waits
/// for `APPLY_LOCK`; the store guards inside stay scoped, none is held
/// across the await.
pub async fn propose(store: &Mutex<Store>) -> Result<Vec<ChangesetSummary>, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let auto = settings::get_bool(&*lock(store)?, settings::CATALOG_AUTO);
    reconcile::reconcile(store, auto)?;
    list(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assign(host: &str) -> ProposedItem {
        ProposedItem {
            grp: "core".into(),
            catalog_id: Some(1),
            kind: "layer".into(),
            name: "core".into(),
            action: ItemAction::AssignLayer,
            params: ItemParams {
                host: Some(host.into()),
                layer: Some("core".into()),
                axis: Some("context".into()),
                ..Default::default()
            },
            decider: Decider::Rule,
        }
    }

    fn stored(item: &ProposedItem) -> ChangesetItemRow {
        let n = item.to_new();
        ChangesetItemRow {
            changeset_id: 1,
            position: 0,
            grp: n.grp,
            catalog_id: n.catalog_id,
            kind: n.kind,
            name: n.name,
            action: n.action,
            params: n.params,
            decider: n.decider,
            state: "rejected".into(),
        }
    }

    #[test]
    fn vocabularies_round_trip_and_empty_params_store_as_none() {
        for k in CardKind::ALL {
            assert_eq!(CardKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(CardKind::parse("nope"), None);
        assert_eq!(ItemParams::default().to_json(), None);
        let p = assign("trn").params;
        assert_eq!(ItemParams::parse(p.to_json().as_deref()), p);
        assert_eq!(ItemParams::parse(Some("not json")), ItemParams::default());
        let n = assign("trn").to_new();
        assert_eq!(
            (n.action.as_str(), n.decider.as_str()),
            ("assign_layer", "rule")
        );
    }

    /// PF14: a refresh matches a stored item to a proposed one by its key —
    /// kind, name, action, catalog, and the host an item targets, so one
    /// layer's assignments to two hosts stay two items.
    #[test]
    fn an_item_keeps_its_key_through_the_store_and_assignments_differ_by_host() {
        let trn = assign("trn");
        assert_eq!(ItemKey::of_row(&stored(&trn)), trn.key());
        assert_ne!(trn.key(), assign("oci").key());
        let mut other_catalog = assign("trn");
        other_catalog.catalog_id = Some(2);
        assert_ne!(trn.key(), other_catalog.key());
    }

    /// `list` shows every open card and the most recent closed ones, with
    /// item counts per group; `get` resolves catalog names.
    #[test]
    fn list_and_get_describe_cards() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        let it = |grp: &str| NewChangesetItem {
            grp: grp.into(),
            catalog_id: Some(p),
            kind: "skill".into(),
            name: "w".into(),
            action: "import".into(),
            params: Some(r#"{"from_host":"oci"}"#.into()),
            decider: "rule".into(),
        };
        let card = s
            .insert_changeset(
                "new",
                "New on oci: skill/w → core",
                &[it("core"), it("core")],
            )
            .unwrap();
        let store = Mutex::new(s);
        let all = list(&store).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].pending, all[0].groups.get("core")), (2, Some(&2)));
        assert!(!all[0].undoable);
        let v = get(card.id, &store).unwrap();
        assert_eq!(v.items[0].catalog.as_deref(), Some("personal"));
        assert_eq!(v.items[0].params.from_host.as_deref(), Some("oci"));
        assert_eq!(get(9999, &store).unwrap_err().code, codes::E_NOTFOUND);
    }
}
