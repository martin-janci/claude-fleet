//! Assets M4: changeset cards — proposed by rules (`rules`), built and
//! refreshed by the reconcile pass (`reconcile`), applied (`apply`) and
//! undone (`undo`). Spec: docs/superpowers/specs/2026-09-30-assets-s1b-s2-design.md,
//! *Changesets (the cards)*. The rows are `store::changesets`.

pub mod rules;

use crate::store::{ChangesetItemRow, NewChangesetItem};
use serde::{Deserialize, Serialize};

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
}
