//! Assets M6 (Rulings R5): a person's layer changes as cards — create a
//! layer, rename one, move a member between two layers of one catalog.
//! Proposed here (validated against the loaded catalog), applied by
//! `apply::run_steps` step 2d (the files, `apply_layer_items`) and step 3 (a
//! rename's host_layers rows), undone like every catalog card.

use super::{CardKind, ChangesetView, Decider, ItemAction, ItemParams, APPLY_LOCK};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::catalog::author;
use crate::service::catalog::layer::{split_key, Axis, Layer};
use crate::service::catalog::registry;
use crate::service::catalog::validate::check_layer_name;
use crate::store::{NewChangesetItem, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// A person's change to a catalog's layers (Assets M6, R5): one card of one
/// item, decider `person`. `catalog` is the catalog's name (default
/// `personal`); `members` and layer names follow the catalog's rules
/// (`<kind>/<name>`, `check_layer_name`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LayerChange {
    Create {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        catalog: Option<String>,
        layer: String,
        /// `role` or `context` (default `context`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<String>,
        /// Applies by organisation: the names of the orgs whose hosts take
        /// this layer on top of their own assignment (a context layer only).
        /// Empty: it applies by host, as assigned.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        orgs: Vec<String>,
    },
    Rename {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        catalog: Option<String>,
        layer: String,
        to: String,
    },
    Move {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        catalog: Option<String>,
        member: String,
        /// The layer it leaves.
        layer: String,
        to: String,
    },
}

impl LayerChange {
    /// The catalog's name, `personal` when none is named.
    pub fn catalog(&self) -> &str {
        let c = match self {
            LayerChange::Create { catalog, .. }
            | LayerChange::Rename { catalog, .. }
            | LayerChange::Move { catalog, .. } => catalog,
        };
        c.as_deref().unwrap_or("personal")
    }
}

fn invalid(msg: String) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

/// A create's axis: `role` or `context`, `context` when none is named.
pub(super) fn parse_axis(axis: Option<&str>) -> Result<Axis, IpcError> {
    match axis.unwrap_or("context") {
        "context" => Ok(Axis::Context),
        "role" => Ok(Axis::Role),
        other => Err(invalid(format!(
            "axis must be role or context, not {other}"
        ))),
    }
}

/// The layer a create writes: `name` on `axis` with `description` and
/// `members`, checked (every member a `<kind>/<name>` key). Shared by
/// propose (to refuse early) and apply (which writes it).
pub(super) fn new_layer(
    name: &str,
    axis: Axis,
    description: Option<String>,
    members: Vec<String>,
    orgs: Vec<String>,
) -> Result<Layer, IpcError> {
    let mut l = author::layer_template(name, axis);
    if let Some(d) = description {
        l.description = d;
    }
    l.members = members;
    l.orgs = orgs;
    l.validate().map_err(invalid)?;
    Ok(l)
}

/// Validate `change` against the loaded catalog and record it as a proposed
/// `layer` card; answers the card. Takes APPLY_LOCK.
pub async fn propose_layer(
    change: LayerChange,
    store: &Mutex<Store>,
) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let name = change.catalog().to_string();
    let row = lock(store)?
        .get_catalog_by_name(&name)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no catalog {name}")))?;
    // Store guard dropped; the registry next (lock rule).
    let layers: Vec<Layer> =
        registry::with_catalog_row(&row, |c| Ok(c.layers.iter().cloned().collect()))?;
    let has = |l: &str| layers.iter().any(|x| x.name == l);
    let (summary, item) = match &change {
        LayerChange::Create {
            layer,
            axis,
            description,
            members,
            orgs,
            ..
        } => {
            check_layer_name(layer)?;
            if has(layer) {
                return Err(invalid(format!(
                    "catalog {name} already has a layer {layer}"
                )));
            }
            let axis = parse_axis(axis.as_deref())?;
            new_layer(
                layer,
                axis,
                description.clone(),
                members.clone(),
                orgs.clone(),
            )?;
            if !orgs.is_empty() {
                let known = lock(store)?.list_orgs()?;
                if let Some(o) = orgs
                    .iter()
                    .find(|o| !known.iter().any(|k| k.name.eq_ignore_ascii_case(o)))
                {
                    return Err(invalid(format!("no organisation named {o}")));
                }
            }
            (
                format!("New layer {layer} in {name}"),
                NewChangesetItem {
                    grp: layer.clone(),
                    catalog_id: Some(row.id),
                    kind: "layer".into(),
                    name: layer.clone(),
                    action: ItemAction::CreateLayer.as_str().into(),
                    params: ItemParams {
                        axis: Some(axis.as_str().into()),
                        description: description.clone(),
                        members: members.clone(),
                        orgs: orgs.clone(),
                        ..Default::default()
                    }
                    .to_json(),
                    decider: Decider::Person.as_str().into(),
                },
            )
        }
        LayerChange::Rename { layer, to, .. } => {
            if !has(layer) {
                return Err(invalid(format!("catalog {name} has no layer {layer}")));
            }
            check_layer_name(to)?;
            if has(to) {
                return Err(invalid(format!("catalog {name} already has a layer {to}")));
            }
            (
                format!("Rename layer {layer} to {to} in {name}"),
                NewChangesetItem {
                    grp: layer.clone(),
                    catalog_id: Some(row.id),
                    kind: "layer".into(),
                    name: layer.clone(),
                    action: ItemAction::RenameLayer.as_str().into(),
                    params: ItemParams {
                        to: Some(to.clone()),
                        ..Default::default()
                    }
                    .to_json(),
                    decider: Decider::Person.as_str().into(),
                },
            )
        }
        LayerChange::Move {
            member, layer, to, ..
        } => {
            let from = layers
                .iter()
                .find(|l| &l.name == layer)
                .ok_or_else(|| invalid(format!("catalog {name} has no layer {layer}")))?;
            if !from.members.contains(member) {
                return Err(invalid(format!("layer {layer} has no member {member}")));
            }
            let dest = layers
                .iter()
                .find(|l| &l.name == to)
                .ok_or_else(|| invalid(format!("catalog {name} has no layer {to}")))?;
            if dest.members.contains(member) {
                return Err(invalid(format!("layer {to} already has {member}")));
            }
            if dest.exclude.contains(member) {
                return Err(invalid(format!("layer {to} excludes {member}")));
            }
            let (kind, asset) = split_key(member)
                .ok_or_else(|| invalid(format!("bad member {member}: use <kind>/<name>")))?;
            (
                format!("Move {member} from {layer} to {to} in {name}"),
                NewChangesetItem {
                    grp: layer.clone(),
                    catalog_id: Some(row.id),
                    kind: kind.as_str().into(),
                    name: asset,
                    action: ItemAction::MoveMember.as_str().into(),
                    params: ItemParams {
                        member: Some(member.clone()),
                        layer: Some(layer.clone()),
                        to: Some(to.clone()),
                        ..Default::default()
                    }
                    .to_json(),
                    decider: Decider::Person.as_str().into(),
                },
            )
        }
    };
    let id = lock(store)?
        .insert_changeset(CardKind::Layer.as_str(), &summary, &[item])?
        .id;
    super::get(id, store)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::testkit::fleet_with_core;
    use crate::service::catalog::lock_registry_for_test;

    fn create(layer: &str) -> LayerChange {
        LayerChange::Create {
            catalog: None,
            layer: layer.into(),
            axis: None,
            description: None,
            members: vec![],
            orgs: vec![],
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn creating_a_layer_that_exists_is_refused_at_propose() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let e = propose_layer(create("core"), &f.store).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(
            e.message
                .contains("catalog personal already has a layer core"),
            "{}",
            e.message
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_layer_applying_by_organisation_names_a_real_org_on_the_context_axis() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let by_org = |axis: Option<&str>, org: &str| LayerChange::Create {
            catalog: None,
            layer: "papaya".into(),
            axis: axis.map(String::from),
            description: None,
            members: vec![],
            orgs: vec![org.into()],
        };
        let e = propose_layer(by_org(None, "Papaya"), &f.store)
            .await
            .unwrap_err();
        assert!(
            e.message.contains("no organisation named Papaya"),
            "{}",
            e.message
        );
        let e = propose_layer(by_org(Some("role"), "Papaya"), &f.store)
            .await
            .unwrap_err();
        assert!(
            e.message.contains("must be a context layer"),
            "{}",
            e.message
        );
        f.store
            .lock()
            .unwrap()
            .add_org("Papaya", None, false)
            .unwrap();
        let v = propose_layer(by_org(None, "papaya"), &f.store)
            .await
            .unwrap();
        assert_eq!(v.state, "proposed");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn renaming_a_missing_layer_or_onto_an_existing_one_is_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let missing = LayerChange::Rename {
            catalog: None,
            layer: "nope".into(),
            to: "x".into(),
        };
        assert!(propose_layer(missing, &f.store)
            .await
            .unwrap_err()
            .message
            .contains("no layer nope"));
        let onto = LayerChange::Rename {
            catalog: None,
            layer: "core".into(),
            to: "core".into(),
        };
        assert!(propose_layer(onto, &f.store)
            .await
            .unwrap_err()
            .message
            .contains("already has a layer core"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn moving_a_member_the_layer_does_not_hold_is_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.add_layer("extra", &[]);
        let m = LayerChange::Move {
            catalog: None,
            member: "skill/other".into(),
            layer: "core".into(),
            to: "extra".into(),
        };
        let e = propose_layer(m, &f.store).await.unwrap_err();
        assert!(
            e.message.contains("layer core has no member skill/other"),
            "{}",
            e.message
        );
    }

    /// A move whose target excludes the member would write a layer the next
    /// load drops (`Layer::validate`), so it is refused at propose.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn moving_a_member_into_a_layer_that_excludes_it_is_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.commit_files(
            &f.personal_root,
            f.personal.id,
            &[(
                "layers/extra.yaml",
                "kind: layer\nname: extra\naxis: context\nexclude:\n- skill/w\n",
            )],
        );
        let m = LayerChange::Move {
            catalog: None,
            member: "skill/w".into(),
            layer: "core".into(),
            to: "extra".into(),
        };
        let e = propose_layer(m, &f.store).await.unwrap_err();
        assert!(
            e.message.contains("layer extra excludes skill/w"),
            "{}",
            e.message
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_valid_change_is_one_proposed_person_card() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let v = propose_layer(create("servers"), &f.store).await.unwrap();
        assert_eq!((v.kind.as_str(), v.state.as_str()), ("layer", "proposed"));
        assert_eq!(v.summary, "New layer servers in personal");
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].action, "create_layer");
        assert_eq!(v.items[0].decider, "person");
        assert_eq!(v.catalogs, vec!["personal".to_string()]);
    }

    /// R-B: the wire form omits what is absent, so a rename reads
    /// `{"op":"rename","layer":"core","to":"base"}`.
    #[test]
    fn the_wire_form_omits_absent_fields() {
        let rename = LayerChange::Rename {
            catalog: None,
            layer: "core".into(),
            to: "base".into(),
        };
        assert_eq!(
            serde_json::to_string(&rename).unwrap(),
            r#"{"op":"rename","layer":"core","to":"base"}"#
        );
        let back: LayerChange = serde_json::from_str(r#"{"op":"create","layer":"x"}"#).unwrap();
        assert_eq!(back, create("x"));
        assert_eq!(back.catalog(), "personal");
    }
}
