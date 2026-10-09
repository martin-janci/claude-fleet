//! Assets M4: changeset cards — proposed by rules (`rules`), built and
//! refreshed by the reconcile pass (`reconcile`), applied (`apply`) and
//! undone (`undo`). Spec: docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md,
//! *Changesets (the cards)*. The rows are `store::changesets`.

pub mod apply;
mod layers;
pub mod reconcile;
pub mod rules;
#[cfg(test)]
pub(crate) mod testkit;
pub mod undo;

pub use layers::{propose_layer, LayerChange};

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::store::{CatalogRow, ChangesetItemRow, ChangesetRow, NewChangesetItem, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// One apply at a time (Rulings R27): apply, undo, dismiss, reject_item,
/// on-demand propose, the tick's reconcile and SB6 all take it. A tokio
/// mutex, not the store: it is held across the awaits of an apply, while
/// every store guard inside stays scoped. The tick only ever `try_lock`s it
/// (R19).
pub(crate) static APPLY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// PF7: what every mutating authoring action (asset and resource edits,
/// layer files, `commit_pending`, `push`, `import_host`, `host_layers`,
/// configure/load/add/remove a catalog) holds around its repo work. It is
/// [`APPLY_LOCK`], so an edit waits for an apply or undo in flight instead
/// of landing mid-apply — where the apply would commit it as the card's, or
/// its failure reset would delete it. Only one process shares it (R27).
pub async fn authoring_lock() -> ApplyGuard {
    APPLY_LOCK.lock().await
}

/// A held [`APPLY_LOCK`]. The `*_held` card actions take one as proof, for
/// a caller that must check the card under the same lock the action runs
/// under — the MCP tool's grant check, so no refresh between the check and
/// the action can add an item in a catalog the caller holds no grant for.
pub type ApplyGuard = tokio::sync::MutexGuard<'static, ()>;

/// Closed cards `list` shows next to every open one.
pub const RECENT_CLOSED: usize = 20;

/// The start of every `error` the system writes when it withdraws a card.
pub const WITHDRAWN_PREFIX: &str = "withdrawn:";

/// What a card is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Bootstrap,
    New,
    Drift,
    Rollout,
    /// Assets M6 (R5): a person's create / rename / move-member change to a
    /// catalog's layers.
    Layer,
}

impl CardKind {
    pub const ALL: [CardKind; 5] = [
        CardKind::Bootstrap,
        CardKind::New,
        CardKind::Drift,
        CardKind::Rollout,
        CardKind::Layer,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CardKind::Bootstrap => "bootstrap",
            CardKind::New => "new",
            CardKind::Drift => "drift",
            CardKind::Rollout => "rollout",
            CardKind::Layer => "layer",
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
    /// Assets M6 (R5): a layer card's three actions.
    CreateLayer,
    RenameLayer,
    MoveMember,
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
            ItemAction::CreateLayer => "create_layer",
            ItemAction::RenameLayer => "rename_layer",
            ItemAction::MoveMember => "move_member",
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
/// `member`; hide `hash`, `reason`; take_host/restore `host`, `hash`,
/// `harness`; sync `layer`, `assets`, `hash`; create_layer `axis`,
/// `description`, `members`; rename_layer `to`; move_member `member`,
/// `layer` (the one it leaves), `to`.
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
    /// The harness a drift was seen on; a restore writes that harness's
    /// copy only. `None` = `claude` (the drift rule reads claude rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Assets M6 (R5): rename_layer / move_member — the target layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// create_layer — its first members (`<kind>/<name>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// create_layer — its description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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

/// Why a host-writing card left one of its assets on a host to a person
/// (Assets M6, R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldWhy {
    /// The host copy is not what fleet wrote.
    Edited,
    /// Its manifest entry predates fleet's file hashes, so an edit cannot
    /// be ruled out.
    Unverified,
    /// The planner had no host-copy verdict, and the action is one a card
    /// never applies (an overwrite).
    Differs,
}

impl HeldWhy {
    /// The words the card's `error` note has used since M5.
    pub fn words(self) -> &'static str {
        match self {
            HeldWhy::Edited => "host copy edited",
            HeldWhy::Unverified => "host copy predates fleet's file hashes",
            HeldWhy::Differs => "host copy differs",
        }
    }
}

/// One asset a card held back on a host (R1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldLine {
    pub kind: String,
    pub name: String,
    pub why: HeldWhy,
}

/// What a host-writing card left undone on one item's host (migration 102).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemOutcome {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<HeldLine>,
    /// A skipped or failed host's line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ItemOutcome {
    pub fn parse(json: Option<&str>) -> Option<ItemOutcome> {
        json.and_then(|j| serde_json::from_str(j).ok())
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
    /// Assets M6 (R1): what the card left undone on this item's host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ItemOutcome>,
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
    /// Assets M6 (R2): the sorted, unique names of the catalogs the items
    /// name.
    #[serde(default)]
    pub catalogs: Vec<String>,
    /// Assets M6 (R3): the system withdrew it (dismissed, error `withdrawn:…`).
    #[serde(default)]
    pub withdrawn: bool,
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
    /// Assets M6 (R2): the sorted, unique names of the catalogs the items
    /// name.
    #[serde(default)]
    pub catalogs: Vec<String>,
    /// Assets M6 (R3): the system withdrew it (dismissed, error `withdrawn:…`).
    #[serde(default)]
    pub withdrawn: bool,
    /// Final review I1: the hosts whose copies its apply held back (an item
    /// outcome with held lines), sorted and unique — the Inbox keeps such a
    /// card under *Recently applied* so its held lines and Sync buttons show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held_hosts: Vec<String>,
    /// Redesign 8.7: SB6 wrote it, not a person (`catalog.auto`'s applied
    /// card, [`apply::AUTO_SYNC_NOTE`]); the Inbox keeps it under *Recently
    /// applied* so an automatic write is seen. Absent from an older hub.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
}

/// The hosts whose copies the items' outcomes held back, sorted and unique.
/// A host item (a Rollout's `sync`) names its host; any other item names it
/// in `params.host`.
fn held_hosts(items: &[ChangesetItemRow]) -> Vec<String> {
    let hosts: BTreeSet<String> = items
        .iter()
        .filter(|i| ItemOutcome::parse(i.outcome.as_deref()).is_some_and(|o| !o.held.is_empty()))
        .map(|i| {
            if i.kind == "host" {
                i.name.clone()
            } else {
                ItemParams::parse(i.params.as_deref())
                    .host
                    .unwrap_or_else(|| i.name.clone())
            }
        })
        .collect();
    hosts.into_iter().collect()
}

/// R3: a card that can still be applied (and that the pass refreshes).
pub fn is_open(state: &str) -> bool {
    matches!(state, "proposed" | "failed")
}

/// Each configured catalog's id by the label effective sets and sync plans
/// name it by (`personal` for the personal catalog, else its name) — how a
/// provenance or an action's `catalog` maps back to a row (R-b: one helper,
/// for reconcile and SB6 alike).
pub(crate) fn catalog_ids_by_label(rows: &[CatalogRow]) -> BTreeMap<String, i64> {
    rows.iter()
        .map(|r| (super::effective::label_of(r.org_id, &r.name), r.id))
        .collect()
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

/// Whether applying `selected` — exactly the items [`apply::select_items`]
/// picked, which is what `apply` runs — writes to hosts: every rollout,
/// and a drift card's restore (R15, R25). It mirrors `apply`'s dispatch,
/// which sends exactly these to a host sync. Read from the selection, never
/// from the raw `positions`: with none named, `apply` runs every pending
/// item, so a drift card whose take_host was rejected applies its restore
/// (Task 9 fix round 1).
pub fn writes_hosts(card: &ChangesetRow, selected: &[&ChangesetItemRow]) -> bool {
    card.kind == CardKind::Rollout.as_str()
        || selected
            .iter()
            .any(|i| i.action == ItemAction::Restore.as_str())
}

/// A card that changed a catalog, so can be undone (R20): bootstrap, new,
/// layer (Assets M6, R5), and a drift applied as take_host — and only when
/// an applied item names a catalog. A hide-only card committed nothing, so
/// there is nothing to undo (PF12).
pub(crate) fn changes_catalog(card: &ChangesetRow, items: &[ChangesetItemRow]) -> bool {
    if applied_catalogs(items).is_empty() {
        return false;
    }
    match card.kind.as_str() {
        "bootstrap" | "new" | "layer" => true,
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

/// Assets M5 (R9): every card [`is_undoable`] answers true for, in one pass
/// over the cards: an applied catalog-changing card that is the latest —
/// by `(applied_at, id)` — in every catalog it touched. Equal to asking
/// [`later_card`] per card, without re-reading every card's items each time.
pub(crate) fn undoable_ids(cards: &[(ChangesetRow, Vec<ChangesetItemRow>)]) -> BTreeSet<i64> {
    let changing = || {
        cards
            .iter()
            .filter(|(c, items)| c.state == "applied" && changes_catalog(c, items))
    };
    let mut latest: BTreeMap<i64, (Option<i64>, i64)> = BTreeMap::new();
    for (card, items) in changing() {
        let at = (card.applied_at, card.id);
        for cid in applied_catalogs(items) {
            let l = latest.entry(cid).or_insert(at);
            if at > *l {
                *l = at;
            }
        }
    }
    changing()
        .filter(|(c, items)| {
            applied_catalogs(items)
                .iter()
                .all(|cid| latest.get(cid) == Some(&(c.applied_at, c.id)))
        })
        .map(|(c, _)| c.id)
        .collect()
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

/// Assets M6 (R3): the system withdrew the card — dismissed with an error
/// starting [`WITHDRAWN_PREFIX`].
fn is_withdrawn(card: &ChangesetRow) -> bool {
    card.state == "dismissed"
        && card
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with(WITHDRAWN_PREFIX))
}

/// R2: the sorted, unique names of the catalogs a card's items name.
fn catalog_names(items: &[ChangesetItemRow], names: &BTreeMap<i64, String>) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| i.catalog_id.and_then(|c| names.get(&c).cloned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
    let withdrawn = is_withdrawn(&card);
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
        catalogs: catalog_names(&items, &names),
        withdrawn,
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
                outcome: ItemOutcome::parse(i.outcome.as_deref()),
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
    let names: BTreeMap<i64, String> = s
        .list_catalogs()?
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect();
    let mut cards = Vec::new();
    for card in s.list_changesets()? {
        let items = s.changeset_items(card.id)?;
        cards.push((card, items));
    }
    // Assets M5 (R9): undoability once, linear, instead of `later_card`
    // (which reads every card's items again) per card.
    let undoable = undoable_ids(&cards);
    let mut closed = 0;
    let mut out = Vec::new();
    for (card, items) in cards {
        if !is_open(&card.state) {
            if closed >= RECENT_CLOSED {
                continue;
            }
            closed += 1;
        }
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        for i in &items {
            *groups.entry(i.grp.clone()).or_insert(0) += 1;
        }
        let pending = items.iter().filter(|i| i.state == "pending").count();
        let auto = card.state == "applied" && card.error.as_deref() == Some(apply::AUTO_SYNC_NOTE);
        out.push(ChangesetSummary {
            withdrawn: is_withdrawn(&card),
            undoable: undoable.contains(&card.id),
            id: card.id,
            kind: card.kind,
            summary: card.summary,
            state: card.state,
            created_at: card.created_at,
            applied_at: card.applied_at,
            error: card.error,
            groups,
            pending,
            catalogs: catalog_names(&items, &names),
            held_hosts: held_hosts(&items),
            auto,
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
            decided_at: None,
            outcome: None,
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

    /// PF12: an applied New card whose only item is a hide committed to no
    /// catalog, so it is not undoable.
    #[test]
    fn an_applied_hide_only_card_is_not_undoable() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let card = s
            .insert_changeset(
                "new",
                "Hide hook/stop on oci",
                &[NewChangesetItem {
                    grp: "hidden".into(),
                    catalog_id: None,
                    kind: "hook".into(),
                    name: "stop".into(),
                    action: "hide".into(),
                    params: Some(r#"{"hash":"h-stop"}"#.into()),
                    decider: "rule".into(),
                }],
            )
            .unwrap();
        s.set_changeset_item_states(card.id, &[0], "applied")
            .unwrap();
        s.mark_changeset_applied(card.id, 1_000, "{}", "[]", None)
            .unwrap();
        let store = Mutex::new(s);
        let v = get(card.id, &store).unwrap();
        assert_eq!((v.state.as_str(), v.undoable), ("applied", false));
        assert!(!list(&store).unwrap()[0].undoable);
    }

    /// Assets M5 (R9): one linear pass answers what `later_card` answers per
    /// card — the latest applied catalog-changing card of every catalog it
    /// touched is undoable, and nothing else.
    #[test]
    fn undoable_is_the_latest_applied_card_in_each_of_its_catalogs() {
        let card = |id: i64, kind: &str, applied_at: Option<i64>| ChangesetRow {
            id,
            kind: kind.into(),
            summary: "s".into(),
            state: if applied_at.is_some() {
                "applied"
            } else {
                "proposed"
            }
            .into(),
            created_at: 1,
            applied_at,
            commits: None,
            layers_snapshot: None,
            error: None,
            withdrawn_at: None,
        };
        let import = |card: i64, catalog: i64| ChangesetItemRow {
            changeset_id: card,
            position: catalog,
            grp: "core".into(),
            catalog_id: Some(catalog),
            kind: "skill".into(),
            name: "w".into(),
            action: "import".into(),
            params: None,
            decider: "rule".into(),
            state: "applied".into(),
            decided_at: None,
            outcome: None,
        };
        let cards = vec![
            (card(1, "bootstrap", Some(10)), vec![import(1, 1)]),
            (card(2, "new", Some(20)), vec![import(2, 1)]),
            (card(3, "new", Some(15)), vec![import(3, 2)]),
            (card(4, "new", Some(30)), vec![import(4, 1), import(4, 2)]),
            (card(5, "rollout", Some(40)), vec![]),
            (card(6, "new", None), vec![import(6, 1)]),
        ];
        assert_eq!(undoable_ids(&cards), BTreeSet::from([4]));
        assert_eq!(undoable_ids(&cards[..3]), BTreeSet::from([2, 3]));
    }

    /// Assets M6 (R2, carry T7): a card lists the catalogs its items name,
    /// sorted and unique, in the list and in the full view.
    #[test]
    fn a_card_lists_the_catalogs_its_items_name() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        let org = s.add_org("acme", None, false).unwrap();
        let a = s
            .upsert_catalog("acme", "/a", None, Some(org.id))
            .unwrap()
            .id;
        let imp = |catalog: i64, name: &str| {
            testkit::item(
                "core",
                Some(catalog),
                "skill",
                name,
                ItemAction::Import,
                ItemParams::default(),
            )
        };
        let hide = testkit::item(
            "core",
            None,
            "skill",
            "z",
            ItemAction::Hide,
            ItemParams::default(),
        );
        let card = s
            .insert_changeset(
                "bootstrap",
                "Adopt 3 as 1 layers",
                &[imp(a, "x"), imp(p, "y"), hide],
            )
            .unwrap();
        let store = Mutex::new(s);
        let summary = list(&store)
            .unwrap()
            .into_iter()
            .find(|c| c.id == card.id)
            .unwrap();
        assert_eq!(
            summary.catalogs,
            vec!["acme".to_string(), "personal".to_string()]
        );
        assert_eq!(get(card.id, &store).unwrap().catalogs, summary.catalogs);
    }

    /// Final review I1: a listed card names the hosts its apply held back.
    #[test]
    fn a_card_lists_the_hosts_its_apply_held_back() {
        let s = Store::open_in_memory().unwrap();
        let sync = |host: &str| NewChangesetItem {
            grp: "core".into(),
            catalog_id: None,
            kind: "host".into(),
            name: host.into(),
            action: "sync".into(),
            params: None,
            decider: "rule".into(),
        };
        let card = s
            .insert_changeset(
                "rollout",
                "Roll out core",
                &[sync("trn"), sync("oci"), sync("htz")],
            )
            .unwrap();
        let held = r#"{"held":[{"kind":"skill","name":"w","why":"edited"}]}"#;
        s.set_changeset_item_outcomes(
            card.id,
            &[
                (0, Some(held.into())),
                (1, Some(held.into())),
                (2, Some(r#"{"note":"unreachable"}"#.into())),
            ],
        )
        .unwrap();
        let plain = s
            .insert_changeset("rollout", "Roll out", &[sync("oci")])
            .unwrap();
        let store = Mutex::new(s);
        let listed = list(&store).unwrap();
        let of = |id: i64| {
            listed
                .iter()
                .find(|c| c.id == id)
                .unwrap()
                .held_hosts
                .clone()
        };
        assert_eq!(of(card.id), vec!["oci".to_string(), "trn".to_string()]);
        assert!(of(plain.id).is_empty());
        let json =
            serde_json::to_string(listed.iter().find(|c| c.id == plain.id).unwrap()).unwrap();
        assert!(!json.contains("held_hosts"), "absent when empty: {json}");
    }

    /// Assets M6 (R3): a card the system withdrew says so, in a list and in
    /// full; a person's dismissal does not.
    #[test]
    fn a_withdrawn_card_says_so() {
        assert!(reconcile::WITHDRAWN.starts_with(WITHDRAWN_PREFIX));
        let s = Store::open_in_memory().unwrap();
        let item = || NewChangesetItem {
            grp: "core".into(),
            catalog_id: None,
            kind: "host".into(),
            name: "oci".into(),
            action: "sync".into(),
            params: None,
            decider: "rule".into(),
        };
        let withdrawn = s.insert_changeset("new", "New on oci", &[item()]).unwrap();
        let by_person = s.insert_changeset("new", "New on oci", &[item()]).unwrap();
        let open = s.insert_changeset("new", "New on oci", &[item()]).unwrap();
        assert!(s
            .withdraw_changeset(withdrawn.id, reconcile::WITHDRAWN)
            .unwrap());
        assert!(s.withdraw_changeset(by_person.id, "no thanks").unwrap());
        let store = Mutex::new(s);
        let flag = |id: i64| {
            list(&store)
                .unwrap()
                .into_iter()
                .find(|c| c.id == id)
                .unwrap()
                .withdrawn
        };
        assert!(flag(withdrawn.id));
        assert!(
            !flag(by_person.id),
            "a person's dismissal is not a withdrawal"
        );
        assert!(!flag(open.id));
        assert!(get(withdrawn.id, &store).unwrap().withdrawn);
        assert!(!get(open.id, &store).unwrap().withdrawn);
    }

    /// Assets M5 (R9): `list`'s one-pass undoability agrees with
    /// `is_undoable` (what `get` answers) on every stored card.
    #[test]
    fn list_and_get_agree_on_what_is_undoable() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        let import = NewChangesetItem {
            grp: "core".into(),
            catalog_id: Some(p),
            kind: "skill".into(),
            name: "w".into(),
            action: "import".into(),
            params: None,
            decider: "rule".into(),
        };
        let mut ids = Vec::new();
        for (n, applied_at) in [(0, Some(30)), (1, Some(10)), (2, None)] {
            let c = s
                .insert_changeset("new", &format!("New {n}"), std::slice::from_ref(&import))
                .unwrap();
            if let Some(at) = applied_at {
                s.record_changeset_applied(
                    c.id,
                    &crate::store::AppliedRecord {
                        applied_at: at,
                        commits: "{}",
                        layers_snapshot: "[]",
                        applied: &[0],
                        ..Default::default()
                    },
                )
                .unwrap();
            }
            ids.push(c.id);
        }
        let store = Mutex::new(s);
        let listed: BTreeMap<i64, bool> = list(&store)
            .unwrap()
            .into_iter()
            .map(|c| (c.id, c.undoable))
            .collect();
        for id in ids {
            assert_eq!(listed[&id], get(id, &store).unwrap().undoable, "card {id}");
        }
        assert_eq!(
            listed.values().filter(|u| **u).count(),
            1,
            "only the card applied last (the first inserted) undoes"
        );
    }
}
