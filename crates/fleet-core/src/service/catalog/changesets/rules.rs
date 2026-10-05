//! Assets M4: the rules that turn the last scan into proposed cards (spec,
//! *Changesets (the cards)*). Pure — no store, registry or I/O: `reconcile`
//! gathers the facts into a [`RulesInput`].

use super::{CardKind, Decider, ItemAction, ItemParams, ProposedCard, ProposedItem};
use crate::service::catalog::identity::{AssetIdentity, IdentityClass, IdentityHost};
use crate::service::catalog::import::slugify;
use crate::service::catalog::model::sha256_hex;
use std::collections::{BTreeMap, BTreeSet};

/// Spec: a Bootstrap card once "≥ 20 unmanaged `normal` identities exist".
pub const BOOTSTRAP_MIN: usize = 20;
/// R6: how many members sharing a name prefix make their own layer.
pub const PREFIX_FAMILY_MIN: usize = 3;
pub const NEEDS_A_LOOK: &str = "needs a look";
pub const HIDDEN: &str = "hidden";
pub const DRIFT: &str = "drift";
/// The group of a take_host's follow-up Rollout items (R14).
pub const UPDATE: &str = "update";

const NO_CLAUDE_COPY: &str = "found only outside Claude; import reads Claude config";
const NO_LAYER: &str = "no layer shares its hosts or name prefix";

/// A non-hidden host and its org.
#[derive(Debug, Clone)]
pub struct HostFacts {
    pub alias: String,
    pub org_id: Option<i64>,
}

/// One layer of a catalog and the hosts it is (actively) assigned to.
#[derive(Debug, Clone)]
pub struct LayerFacts {
    pub name: String,
    pub hosts: BTreeSet<String>,
}

/// A configured catalog as the rules see it.
#[derive(Debug, Clone)]
pub struct CatalogFacts {
    pub id: i64,
    pub name: String,
    pub org_id: Option<i64>,
    /// In the registry and not a problem entry.
    pub loaded: bool,
    pub asset_count: usize,
    pub layers: Vec<LayerFacts>,
    /// Assets M6 (R4): `(kind, slug)` of every asset the catalog holds.
    pub slugs: BTreeSet<(String, String)>,
}

/// A managed asset whose copy on a host differs from its catalog's.
#[derive(Debug, Clone)]
pub struct DriftFacts {
    pub catalog_id: i64,
    pub kind: String,
    pub name: String,
    pub host: String,
    pub host_hash: Option<String>,
    /// Assets M5 (R5): the host copy is no longer what fleet wrote
    /// (`drift_side = host`); `false` when the side is unknown.
    pub edited: bool,
}

/// Members a never-rolled-out layer should have put on a host, and has not
/// (R16).
#[derive(Debug, Clone)]
pub struct LayerGap {
    pub catalog_id: i64,
    pub layer: String,
    pub host: String,
    pub assets: Vec<String>,
}

/// Everything the rules read.
pub struct RulesInput<'a> {
    pub identities: &'a [AssetIdentity],
    pub hosts: &'a [HostFacts],
    pub catalogs: &'a [CatalogFacts],
    /// `(kind, name, content_hash)` of every verdict.
    pub verdicts: &'a BTreeSet<(String, String, String)>,
    pub drifted: &'a [DriftFacts],
    pub gaps: &'a [LayerGap],
    /// `(catalog_id, layer)` an open rollout card already names.
    pub rollout_open: &'a BTreeSet<(i64, String)>,
    /// R4: an applied Bootstrap exists, or personal already has assets.
    pub bootstrapped: bool,
    pub bootstrap_open: bool,
    /// `catalog.auto`: the pass hides internals itself, so no `hide` items.
    pub auto: bool,
}

/// One item's share of a card's subject (R2).
pub struct SubjectItem<'a> {
    pub grp: &'a str,
    pub catalog_id: Option<i64>,
    pub kind: &'a str,
    pub name: &'a str,
    pub host: Option<&'a str>,
}

/// A card's subject (R2): what the pass matches an open card by.
pub fn subject_of<'a>(kind: CardKind, items: impl IntoIterator<Item = SubjectItem<'a>>) -> String {
    let mut items = items.into_iter();
    match kind {
        CardKind::Bootstrap => "bootstrap".to_string(),
        CardKind::New => match items.next() {
            Some(i) => format!("new:{}/{}", i.kind, i.name),
            None => "new:".to_string(),
        },
        CardKind::Drift => match items.next() {
            Some(i) => format!(
                "drift:{}:{}/{}@{}",
                i.catalog_id.unwrap_or(0),
                i.kind,
                i.name,
                i.host.unwrap_or("")
            ),
            None => "drift:".to_string(),
        },
        CardKind::Rollout => {
            let groups: BTreeSet<String> = items
                .map(|i| format!("{}/{}", i.catalog_id.unwrap_or(0), i.grp))
                .collect();
            format!(
                "rollout:{}",
                groups.into_iter().collect::<Vec<_>>().join(",")
            )
        }
    }
}

/// The content hash a verdict on this identity holds by (R9).
pub fn identity_hash(id: &AssetIdentity) -> String {
    let hashes: BTreeSet<&str> = id
        .hosts
        .iter()
        .filter_map(|h| h.host_hash.as_deref())
        .collect();
    match hashes.len() {
        0 => "-".to_string(),
        1 => hashes.into_iter().next().unwrap_or("-").to_string(),
        _ => sha256_hex(hashes.into_iter().collect::<Vec<_>>().join("\n").as_bytes()),
    }
}

/// The hash a Rollout card's verdict holds by (R9).
pub fn gap_hash(gaps: &[&LayerGap]) -> String {
    let lines: BTreeSet<String> = gaps
        .iter()
        .flat_map(|g| g.assets.iter().map(move |a| format!("{}:{a}", g.host)))
        .collect();
    sha256_hex(lines.into_iter().collect::<Vec<_>>().join("\n").as_bytes())
}

/// Every card the facts call for.
pub fn propose(input: &RulesInput<'_>) -> Vec<ProposedCard> {
    let held = |kind: &str, name: &str, hash: &str| {
        input
            .verdicts
            .contains(&(kind.to_string(), name.to_string(), hash.to_string()))
    };
    let eligible: Vec<&AssetIdentity> = input
        .identities
        .iter()
        .filter(|i| !held(&i.kind, &i.name, &identity_hash(i)))
        .collect();
    let normal = eligible
        .iter()
        .filter(|i| i.class == IdentityClass::Normal)
        .count();
    let mut cards = Vec::new();
    if input.bootstrap_open || normal >= BOOTSTRAP_MIN || (!input.bootstrapped && normal > 0) {
        cards.extend(bootstrap_card(&eligible, input));
    } else {
        // Assets M6 (R4, T3 M8): two new names that import as one slug both
        // need a look, as in a Bootstrap card.
        let collides = slug_collisions(&eligible, input);
        for id in &eligible {
            if is_internal(id) && input.auto {
                continue;
            }
            cards.push(new_card(id, input, &collides));
        }
    }
    cards.extend(drift_cards(input, &held));
    cards.extend(rollout_cards(input, &held));
    cards
}

fn is_internal(id: &AssetIdentity) -> bool {
    matches!(
        id.class,
        IdentityClass::FleetInternal | IdentityClass::HarnessInternal
    )
}

fn class_label(c: &IdentityClass) -> &'static str {
    match c {
        IdentityClass::Normal => "normal",
        IdentityClass::FleetInternal => "fleet_internal",
        IdentityClass::HarnessInternal => "harness_internal",
        IdentityClass::NeedsPerson => "needs_person",
    }
}

fn hosts_of(id: &AssetIdentity) -> BTreeSet<String> {
    id.hosts.iter().map(|h| h.host_alias.clone()).collect()
}

fn org_of(input: &RulesInput<'_>, alias: &str) -> Option<i64> {
    input
        .hosts
        .iter()
        .find(|h| h.alias == alias)
        .and_then(|h| h.org_id)
}

fn on_org_host(id: &AssetIdentity, input: &RulesInput<'_>) -> bool {
    id.hosts
        .iter()
        .any(|h| org_of(input, &h.host_alias).is_some())
}

fn catalog<'a>(input: &'a RulesInput<'_>, id: i64) -> Option<&'a CatalogFacts> {
    input.catalogs.iter().find(|c| c.id == id)
}

fn personal_id(input: &RulesInput<'_>) -> Option<i64> {
    input
        .catalogs
        .iter()
        .find(|c| c.org_id.is_none())
        .map(|c| c.id)
}

fn member_of(id: &AssetIdentity) -> String {
    format!("{}/{}", id.kind, slugify(&id.name))
}

/// R6: the slug's part before its first `-`, when there is a rest.
fn prefix_of(name: &str) -> Option<String> {
    let slug = slugify(name);
    let (p, rest) = slug.split_once('-')?;
    (!p.is_empty() && !rest.is_empty()).then(|| p.to_string())
}

/// Org X's catalog when every holder is bound to X and X has one, loaded
/// or not.
fn org_catalog<'a>(id: &AssetIdentity, input: &'a RulesInput<'_>) -> Option<&'a CatalogFacts> {
    let orgs: Vec<Option<i64>> = id
        .hosts
        .iter()
        .map(|h| org_of(input, &h.host_alias))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    match orgs.as_slice() {
        [Some(org)] => input.catalogs.iter().find(|c| c.org_id == Some(*org)),
        _ => None,
    }
}

/// R5: org X's catalog when every holder is bound to X and X has one, else
/// personal; `Err(why)` when it needs a look.
fn destination(id: &AssetIdentity, input: &RulesInput<'_>) -> Result<i64, String> {
    if let Some(c) = org_catalog(id, input) {
        return if c.loaded {
            Ok(c.id)
        } else {
            Err(format!("catalog {} failed to load", c.name))
        };
    }
    personal_id(input).ok_or_else(|| "no personal catalog".to_string())
}

/// R5: the Claude copy most hosts hold (tie: the smaller hash), `local`
/// first among its holders, then alphabetical.
fn source_host(id: &AssetIdentity) -> Option<String> {
    let claude: Vec<&IdentityHost> = id.hosts.iter().filter(|h| h.harness == "claude").collect();
    let mut counts: BTreeMap<Option<&str>, usize> = BTreeMap::new();
    for h in &claude {
        *counts.entry(h.host_hash.as_deref()).or_default() += 1;
    }
    let top = counts.values().copied().max()?;
    let common = counts.iter().find(|(_, n)| **n == top).map(|(h, _)| *h)?;
    let mut holders: Vec<&str> = claude
        .iter()
        .filter(|h| h.host_hash.as_deref() == common)
        .map(|h| h.host_alias.as_str())
        .collect();
    holders.sort_by_key(|a| (*a != "local", *a));
    holders.first().map(|s| s.to_string())
}

fn import_item(
    id: &AssetIdentity,
    grp: &str,
    catalog_id: Option<i64>,
    from_host: Option<String>,
    layer: Option<&str>,
    reason: Option<String>,
) -> ProposedItem {
    ProposedItem {
        grp: grp.to_string(),
        catalog_id,
        kind: id.kind.clone(),
        name: id.name.clone(),
        action: ItemAction::Import,
        params: ItemParams {
            from_host,
            layer: layer.map(String::from),
            member: Some(member_of(id)),
            hash: Some(identity_hash(id)),
            reason,
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

fn share_item(id: &AssetIdentity, grp: &str, catalog_id: i64) -> ProposedItem {
    ProposedItem {
        grp: grp.to_string(),
        catalog_id: Some(catalog_id),
        kind: id.kind.clone(),
        name: id.name.clone(),
        action: ItemAction::SetScope,
        params: ItemParams {
            scope: Some("shared".into()),
            member: Some(member_of(id)),
            hash: Some(identity_hash(id)),
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

fn assign_item(layer: &str, catalog_id: i64, host: &str) -> ProposedItem {
    ProposedItem {
        grp: layer.to_string(),
        catalog_id: Some(catalog_id),
        kind: "layer".into(),
        name: layer.to_string(),
        action: ItemAction::AssignLayer,
        params: ItemParams {
            host: Some(host.to_string()),
            layer: Some(layer.to_string()),
            axis: Some("context".into()),
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

fn hide_item(id: &AssetIdentity) -> ProposedItem {
    ProposedItem {
        grp: HIDDEN.into(),
        catalog_id: None,
        kind: id.kind.clone(),
        name: id.name.clone(),
        action: ItemAction::Hide,
        params: ItemParams {
            hash: Some(identity_hash(id)),
            reason: Some(class_label(&id.class).into()),
            ..Default::default()
        },
        decider: Decider::Rule,
    }
}

/// An import a person must name to apply (R8), with why. Its catalog is
/// where it would go — an org catalog that failed to load included (Task 4
/// review M3), so apply refuses it until that catalog loads (R11) rather
/// than adopting it into personal.
fn look_item(id: &AssetIdentity, input: &RulesInput<'_>) -> ProposedItem {
    let reason = if id.class == IdentityClass::NeedsPerson {
        id.reason.clone().unwrap_or_else(|| "needs a person".into())
    } else {
        match &destination(id, input) {
            Err(e) => e.clone(),
            Ok(_) if source_host(id).is_none() => NO_CLAUDE_COPY.to_string(),
            Ok(_) => NO_LAYER.to_string(),
        }
    };
    look_item_because(id, input, reason)
}

/// [`look_item`] with the reason given.
fn look_item_because(id: &AssetIdentity, input: &RulesInput<'_>, reason: String) -> ProposedItem {
    let catalog_id = org_catalog(id, input)
        .map(|c| c.id)
        .or_else(|| personal_id(input));
    import_item(
        id,
        NEEDS_A_LOOK,
        catalog_id,
        source_host(id),
        None,
        Some(reason),
    )
}

/// An eligible identity a Bootstrap card would group for import: its
/// destination catalog and the host its import reads from.
fn importable(id: &AssetIdentity, input: &RulesInput<'_>) -> Option<(i64, String)> {
    if id.class != IdentityClass::Normal {
        return None;
    }
    Some((destination(id, input).ok()?, source_host(id)?))
}

/// Assets M5 (R7, PF18), M6 (R4): the identities a Bootstrap or New card would import whose
/// slug another of a different name shares in the SAME destination catalog
/// (`My_Skill` and `my-skill` both import as `my-skill`) → why, keyed by
/// `(kind, name)`. One import would create the slug and the other's group
/// would fail the whole card at apply, so both need a look. Names bound for
/// different catalogs never collide.
fn slug_collisions(
    eligible: &[&AssetIdentity],
    input: &RulesInput<'_>,
) -> BTreeMap<(String, String), String> {
    let mut by_slug: BTreeMap<(i64, String, String), BTreeSet<String>> = BTreeMap::new();
    for id in eligible {
        if let Some((dest, _)) = importable(id, input) {
            by_slug
                .entry((dest, id.kind.clone(), slugify(&id.name)))
                .or_default()
                .insert(id.name.clone());
        }
    }
    let mut out = BTreeMap::new();
    for ((_, kind, slug), names) in by_slug.into_iter().filter(|(_, n)| n.len() > 1) {
        for name in &names {
            let others: Vec<&str> = names
                .iter()
                .filter(|n| *n != name)
                .map(String::as_str)
                .collect();
            out.insert(
                (kind.clone(), name.clone()),
                format!("imports as {slug}, as {} does", others.join(", ")),
            );
        }
    }
    out
}

/// A member of a proposed layer and the host its import reads from.
type Member<'a> = (&'a AssetIdentity, String);
/// Members per (destination catalog, host-set signature).
type BySignature<'a> = BTreeMap<(i64, BTreeSet<String>), Vec<Member<'a>>>;

/// One proposed layer.
struct Group<'a> {
    catalog_id: i64,
    hosts: BTreeSet<String>,
    layer: String,
    members: Vec<Member<'a>>,
}

fn bootstrap_card(eligible: &[&AssetIdentity], input: &RulesInput<'_>) -> Option<ProposedCard> {
    let mut looks = Vec::new();
    let mut hides = Vec::new();
    let mut by_sig: BySignature<'_> = BTreeMap::new();
    let collisions = slug_collisions(eligible, input);
    for &id in eligible {
        if is_internal(id) {
            if !input.auto {
                hides.push(hide_item(id));
            }
            continue;
        }
        if let Some(why) = collisions.get(&(id.kind.clone(), id.name.clone())) {
            looks.push(look_item_because(id, input, why.clone()));
            continue;
        }
        match importable(id, input) {
            Some((dest, src)) => by_sig
                .entry((dest, hosts_of(id)))
                .or_default()
                .push((id, src)),
            None => looks.push(look_item(id, input)),
        }
    }
    let groups = name_groups(by_sig, input);
    let mut items = Vec::new();
    let mut imports = 0;
    for g in &groups {
        let personal_dest = catalog(input, g.catalog_id).is_some_and(|c| c.org_id.is_none());
        for (id, src) in &g.members {
            items.push(import_item(
                id,
                &g.layer,
                Some(g.catalog_id),
                Some(src.clone()),
                Some(&g.layer),
                None,
            ));
            imports += 1;
            if personal_dest && on_org_host(id, input) {
                items.push(share_item(id, &g.layer, g.catalog_id));
            }
        }
        // R7: only where layering cannot shrink what the host takes today.
        let cat = catalog(input, g.catalog_id);
        for host in &g.hosts {
            let safe = cat.is_some_and(|c| {
                c.asset_count == 0 || c.layers.iter().any(|l| l.hosts.contains(host))
            });
            if safe {
                items.push(assign_item(&g.layer, g.catalog_id, host));
            }
        }
    }
    let looks_n = looks.len();
    items.extend(looks);
    items.extend(hides);
    if items.is_empty() {
        return None;
    }
    let mut summary = format!("Adopt {imports} as {} layers", groups.len());
    if looks_n > 0 {
        summary.push_str(&format!("; {looks_n} need a look"));
    }
    Some(ProposedCard {
        kind: CardKind::Bootstrap,
        summary,
        items,
    })
}

/// R6: split prefix families out of each signature group, then name every
/// group, largest first.
fn name_groups<'a>(by_sig: BySignature<'a>, input: &RulesInput<'_>) -> Vec<Group<'a>> {
    struct Raw<'a> {
        catalog_id: i64,
        hosts: BTreeSet<String>,
        family: Option<String>,
        members: Vec<Member<'a>>,
    }
    let mut raws: Vec<Raw<'a>> = Vec::new();
    for ((catalog_id, hosts), members) in by_sig {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (id, _) in &members {
            if let Some(p) = prefix_of(&id.name) {
                *counts.entry(p).or_default() += 1;
            }
        }
        let mut families: BTreeMap<String, Vec<Member<'a>>> = BTreeMap::new();
        let mut rest = Vec::new();
        for m in members {
            match prefix_of(&m.0.name) {
                Some(p) if counts.get(&p).copied().unwrap_or(0) >= PREFIX_FAMILY_MIN => {
                    families.entry(p).or_default().push(m)
                }
                _ => rest.push(m),
            }
        }
        for (p, ms) in families {
            raws.push(Raw {
                catalog_id,
                hosts: hosts.clone(),
                family: Some(p),
                members: ms,
            });
        }
        if !rest.is_empty() {
            raws.push(Raw {
                catalog_id,
                hosts,
                family: None,
                members: rest,
            });
        }
    }
    raws.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.catalog_id.cmp(&b.catalog_id))
            .then_with(|| a.hosts.cmp(&b.hosts))
            .then_with(|| a.family.cmp(&b.family))
    });
    let mut used: BTreeMap<i64, BTreeSet<String>> = input
        .catalogs
        .iter()
        .map(|c| (c.id, c.layers.iter().map(|l| l.name.clone()).collect()))
        .collect();
    let mut core_given: BTreeSet<i64> = BTreeSet::new();
    let mut out = Vec::new();
    for r in raws {
        let base = if let Some(p) = &r.family {
            p.clone()
        } else if r.hosts.len() == 1 {
            format!(
                "{}-only",
                slugify(r.hosts.iter().next().map(String::as_str).unwrap_or("host"))
            )
        } else if r.hosts == accepting_hosts(input, r.catalog_id) {
            "everywhere".to_string()
        } else if core_given.insert(r.catalog_id) {
            "core".to_string()
        } else {
            slugify(&r.hosts.iter().cloned().collect::<Vec<_>>().join("-"))
        };
        let taken = used.entry(r.catalog_id).or_default();
        let layer = free_name(&base, taken);
        taken.insert(layer.clone());
        out.push(Group {
            catalog_id: r.catalog_id,
            hosts: r.hosts,
            layer,
            members: r.members,
        });
    }
    out
}

/// Every host that accepts the catalog: all of them for personal, the
/// org's for an org catalog.
fn accepting_hosts(input: &RulesInput<'_>, catalog_id: i64) -> BTreeSet<String> {
    match catalog(input, catalog_id).and_then(|c| c.org_id) {
        None => input.hosts.iter().map(|h| h.alias.clone()).collect(),
        Some(org) => input
            .hosts
            .iter()
            .filter(|h| h.org_id == Some(org))
            .map(|h| h.alias.clone())
            .collect(),
    }
}

fn free_name(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|n| !taken.contains(n))
        .expect("an unbounded range finds a free name")
}

/// New on host: the layer whose assigned hosts are exactly the identity's,
/// else one named after its prefix family covering its hosts.
fn layer_for(id: &AssetIdentity, dest: i64, input: &RulesInput<'_>) -> Option<String> {
    let cat = catalog(input, dest)?;
    let hosts = hosts_of(id);
    if let Some(l) = cat.layers.iter().find(|l| l.hosts == hosts) {
        return Some(l.name.clone());
    }
    let prefix = prefix_of(&id.name)?;
    cat.layers
        .iter()
        .find(|l| l.name == prefix && l.hosts.is_superset(&hosts))
        .map(|l| l.name.clone())
}

fn new_card(
    id: &AssetIdentity,
    input: &RulesInput<'_>,
    collides: &BTreeMap<(String, String), String>,
) -> ProposedCard {
    let on = id.signature.replace(',', ", ");
    if is_internal(id) {
        return ProposedCard {
            kind: CardKind::New,
            summary: format!("Hide {}/{} on {on}", id.kind, id.name),
            items: vec![hide_item(id)],
        };
    }
    if id.class == IdentityClass::Normal {
        // Assets M6 (R4): a slug another candidate or the destination
        // catalog already holds would fail at apply.
        if let Ok(dest) = destination(id, input) {
            let slug = slugify(&id.name);
            let why = collides
                .get(&(id.kind.clone(), id.name.clone()))
                .cloned()
                .or_else(|| {
                    catalog(input, dest)
                        .filter(|c| c.slugs.contains(&(id.kind.clone(), slug.clone())))
                        .filter(|_| slug != id.name)
                        .map(|_| format!("imports as {slug}, which the catalog already holds"))
                });
            if let Some(why) = why {
                return ProposedCard {
                    kind: CardKind::New,
                    summary: format!("New on {on}: {}/{} needs a look", id.kind, id.name),
                    items: vec![look_item_because(id, input, why)],
                };
            }
        }
        if let (Ok(dest), Some(src)) = (destination(id, input), source_host(id)) {
            if let Some(layer) = layer_for(id, dest, input) {
                let mut items = vec![import_item(
                    id,
                    &layer,
                    Some(dest),
                    Some(src),
                    Some(&layer),
                    None,
                )];
                if catalog(input, dest).is_some_and(|c| c.org_id.is_none())
                    && on_org_host(id, input)
                {
                    items.push(share_item(id, &layer, dest));
                }
                return ProposedCard {
                    kind: CardKind::New,
                    summary: format!("New on {on}: {}/{} → {layer}", id.kind, id.name),
                    items,
                };
            }
        }
    }
    ProposedCard {
        kind: CardKind::New,
        summary: format!("New on {on}: {}/{} needs a look", id.kind, id.name),
        items: vec![look_item(id, input)],
    }
}

fn drift_cards(
    input: &RulesInput<'_>,
    held: &dyn Fn(&str, &str, &str) -> bool,
) -> Vec<ProposedCard> {
    input
        .drifted
        .iter()
        .filter(|d| !held(&d.kind, &d.name, d.host_hash.as_deref().unwrap_or("-")))
        .map(|d| {
            let cat = catalog(input, d.catalog_id).map_or("?", |c| c.name.as_str());
            let params = ItemParams {
                host: Some(d.host.clone()),
                hash: Some(d.host_hash.clone().unwrap_or_else(|| "-".into())),
                ..Default::default()
            };
            let item = |action| ProposedItem {
                grp: DRIFT.into(),
                catalog_id: Some(d.catalog_id),
                kind: d.kind.clone(),
                name: d.name.clone(),
                action,
                params: params.clone(),
                decider: Decider::Rule,
            };
            ProposedCard {
                kind: CardKind::Drift,
                summary: if d.edited {
                    format!(
                        "{}/{} was edited on {} (catalog {cat})",
                        d.kind, d.name, d.host
                    )
                } else {
                    format!(
                        "{}/{} differs on {} from catalog {cat}",
                        d.kind, d.name, d.host
                    )
                },
                items: vec![item(ItemAction::TakeHost), item(ItemAction::Restore)],
            }
        })
        .collect()
}

fn rollout_cards(
    input: &RulesInput<'_>,
    held: &dyn Fn(&str, &str, &str) -> bool,
) -> Vec<ProposedCard> {
    let mut by_layer: BTreeMap<(i64, String), Vec<&LayerGap>> = BTreeMap::new();
    for g in input.gaps {
        if !input
            .rollout_open
            .contains(&(g.catalog_id, g.layer.clone()))
        {
            by_layer
                .entry((g.catalog_id, g.layer.clone()))
                .or_default()
                .push(g);
        }
    }
    by_layer
        .into_iter()
        .filter_map(|((cid, layer), gaps)| {
            let hash = gap_hash(&gaps);
            if held("layer", &format!("{cid}/{layer}"), &hash) {
                return None;
            }
            let hosts: Vec<&str> = gaps.iter().map(|g| g.host.as_str()).collect();
            let items = gaps
                .iter()
                .map(|g| ProposedItem {
                    grp: layer.clone(),
                    catalog_id: Some(cid),
                    kind: "host".into(),
                    name: g.host.clone(),
                    action: ItemAction::Sync,
                    params: ItemParams {
                        layer: Some(layer.clone()),
                        assets: g.assets.clone(),
                        hash: Some(hash.clone()),
                        ..Default::default()
                    },
                    decider: Decider::Rule,
                })
                .collect();
            Some(ProposedCard {
                kind: CardKind::Rollout,
                summary: format!("Roll out {layer} to {}", hosts.join(", ")),
                items,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::identity::fixtures::{live_shape, r};
    use crate::service::catalog::identity::group_identities;
    use crate::store::AssetInventoryRow;

    const PERSONAL: i64 = 1;
    const PAPAYA: i64 = 2;
    const ORG: i64 = 7;

    /// Owns everything a `RulesInput` borrows.
    struct Facts {
        identities: Vec<AssetIdentity>,
        hosts: Vec<HostFacts>,
        catalogs: Vec<CatalogFacts>,
        verdicts: BTreeSet<(String, String, String)>,
        drifted: Vec<DriftFacts>,
        gaps: Vec<LayerGap>,
        rollout_open: BTreeSet<(i64, String)>,
        bootstrapped: bool,
        bootstrap_open: bool,
        auto: bool,
    }

    impl Facts {
        fn input(&self) -> RulesInput<'_> {
            RulesInput {
                identities: &self.identities,
                hosts: &self.hosts,
                catalogs: &self.catalogs,
                verdicts: &self.verdicts,
                drifted: &self.drifted,
                gaps: &self.gaps,
                rollout_open: &self.rollout_open,
                bootstrapped: self.bootstrapped,
                bootstrap_open: self.bootstrap_open,
                auto: self.auto,
            }
        }
    }

    fn row(host: &str, kind: &str, name: &str, hash: &str) -> AssetInventoryRow {
        r(host, kind, name, Some(hash))
    }

    /// The live fleet's hosts (trn bound to org 7), an empty personal
    /// catalog and, optionally, org 7's empty `papayapos` catalog.
    fn fleet(with_org_catalog: bool, rows: Vec<AssetInventoryRow>) -> Facts {
        let hosts = ["local", "mefistos", "oci", "trn", "htz"]
            .iter()
            .map(|h| HostFacts {
                alias: h.to_string(),
                org_id: (*h == "trn").then_some(ORG),
            })
            .collect();
        let mut catalogs = vec![CatalogFacts {
            id: PERSONAL,
            name: "personal".into(),
            org_id: None,
            loaded: true,
            asset_count: 0,
            layers: vec![],
            slugs: BTreeSet::new(),
        }];
        if with_org_catalog {
            catalogs.push(CatalogFacts {
                id: PAPAYA,
                name: "papayapos".into(),
                org_id: Some(ORG),
                loaded: true,
                asset_count: 0,
                layers: vec![],
                slugs: BTreeSet::new(),
            });
        }
        Facts {
            identities: group_identities(&rows),
            hosts,
            catalogs,
            verdicts: BTreeSet::new(),
            drifted: vec![],
            gaps: vec![],
            rollout_open: BTreeSet::new(),
            bootstrapped: false,
            bootstrap_open: false,
            auto: true,
        }
    }

    /// Import items per (catalog, group), "needs a look" left out.
    fn imports(card: &ProposedCard) -> BTreeMap<(Option<i64>, String), usize> {
        let mut out = BTreeMap::new();
        for i in card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::Import && i.grp != NEEDS_A_LOOK)
        {
            *out.entry((i.catalog_id, i.grp.clone())).or_insert(0) += 1;
        }
        out
    }

    /// Spec, Testing: "bootstrap on the live fixture shape (164 identities,
    /// 8 signatures) produces the expected groups and the `shared`
    /// proposals for assets present on an org host".
    #[test]
    fn bootstrap_on_the_live_fixture_shape_groups_by_signature_and_shares_what_trn_has() {
        let f = fleet(true, live_shape());
        assert_eq!(f.identities.len(), 164);
        let cards = propose(&f.input());
        assert_eq!(
            cards.len(),
            1,
            "{:?}",
            cards.iter().map(|c| &c.summary).collect::<Vec<_>>()
        );
        let card = &cards[0];
        assert_eq!(card.kind, CardKind::Bootstrap);
        assert_eq!(card.summary, "Adopt 164 as 8 layers");
        let expected: BTreeMap<(Option<i64>, String), usize> = [
            (PERSONAL, "core", 82),
            (PERSONAL, "local-only", 30),
            (PERSONAL, "everywhere", 17),
            (PERSONAL, "local-oci-trn", 11),
            (PERSONAL, "local-mefistos", 9),
            (PERSONAL, "htz-oci", 5),
            (PERSONAL, "htz-only", 1),
            (PAPAYA, "trn-only", 9),
        ]
        .into_iter()
        .map(|(c, l, n)| ((Some(c), l.to_string()), n))
        .collect();
        assert_eq!(imports(card), expected);
        let shared: Vec<&ProposedItem> = card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::SetScope)
            .collect();
        assert_eq!(
            shared.len(),
            82 + 17 + 11,
            "every personal asset trn already has"
        );
        assert!(
            shared
                .iter()
                .all(|i| i.catalog_id == Some(PERSONAL)
                    && i.params.scope.as_deref() == Some("shared"))
        );
        let assigned: BTreeSet<(String, String)> = card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::AssignLayer)
            .map(|i| (i.name.clone(), i.params.host.clone().unwrap()))
            .collect();
        assert_eq!(assigned.len(), 4 + 1 + 5 + 3 + 2 + 2 + 1 + 1);
        assert!(assigned.contains(&("trn-only".to_string(), "trn".to_string())));
        assert!(card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::AssignLayer)
            .all(|i| i.params.axis.as_deref() == Some("context")));
        for i in card.items.iter().filter(|i| i.action == ItemAction::Import) {
            assert!(i.params.from_host.is_some(), "{i:?}");
            assert_eq!(
                i.params.member.as_deref(),
                Some(format!("skill/{}", i.name).as_str())
            );
            assert_eq!(i.params.layer.as_deref(), Some(i.grp.as_str()));
        }
    }

    /// R5: with no catalog for trn's org, its assets go to personal — and,
    /// being on an org host, are proposed shared.
    #[test]
    fn without_an_org_catalog_trn_only_assets_go_to_personal_and_are_shared() {
        let f = fleet(false, live_shape());
        let card = &propose(&f.input())[0];
        assert_eq!(
            imports(card).get(&(Some(PERSONAL), "trn-only".to_string())),
            Some(&9)
        );
        assert_eq!(
            card.items
                .iter()
                .filter(|i| i.action == ItemAction::SetScope)
                .count(),
            82 + 17 + 11 + 9
        );
    }

    /// R6, R8: a prefix family is its own layer; differing copies need a
    /// look; internals are `hide` items only with `catalog.auto` off.
    #[test]
    fn prefix_families_get_their_own_layer_and_odd_ones_need_a_look() {
        let mut rows = Vec::new();
        for n in [
            "author-draft",
            "author-beta",
            "author-critic",
            "jira",
            "worklog",
        ] {
            for h in ["local", "oci"] {
                rows.push(row(h, "skill", n, "x"));
            }
        }
        rows.push(row("local", "skill", "copy", "a"));
        rows.push(row("oci", "skill", "copy", "a"));
        rows.push(row("mefistos", "skill", "copy", "b"));
        let mut fleet_hook = row("local", "hook", "stop", "f");
        fleet_hook.fleet_owned = true;
        rows.push(fleet_hook);
        let mut f = fleet(false, rows);
        f.auto = false;
        let card = &propose(&f.input())[0];
        let by_layer = imports(card);
        assert_eq!(
            by_layer.get(&(Some(PERSONAL), "author".to_string())),
            Some(&3)
        );
        assert_eq!(
            by_layer.get(&(Some(PERSONAL), "core".to_string())),
            Some(&2)
        );
        let look = card.items.iter().find(|i| i.grp == NEEDS_A_LOOK).unwrap();
        assert_eq!(
            (look.name.as_str(), look.params.reason.as_deref()),
            ("copy", Some("copies differ on mefistos"))
        );
        let hide = card
            .items
            .iter()
            .find(|i| i.action == ItemAction::Hide)
            .unwrap();
        assert_eq!((hide.name.as_str(), hide.grp.as_str()), ("stop", HIDDEN));
        assert!(
            card.summary.ends_with("; 1 need a look"),
            "{}",
            card.summary
        );
        f.auto = true;
        assert!(
            propose(&f.input())[0]
                .items
                .iter()
                .all(|i| i.action != ItemAction::Hide),
            "with catalog.auto the pass hides internals itself"
        );
    }

    /// Spec, New on host: after bootstrap a new identity joins the layer
    /// whose hosts it shares; else it needs a look.
    #[test]
    fn after_bootstrap_a_new_identity_joins_the_layer_sharing_its_hosts_or_needs_a_look() {
        let rows = vec![
            row("local", "skill", "fresh", "h"),
            row("oci", "skill", "fresh", "h"),
            row("htz", "skill", "lonely", "h"),
        ];
        let mut f = fleet(false, rows);
        f.bootstrapped = true;
        f.catalogs[0].asset_count = 10;
        f.catalogs[0].layers = vec![LayerFacts {
            name: "core".into(),
            hosts: BTreeSet::from(["local".to_string(), "oci".to_string()]),
        }];
        let cards = propose(&f.input());
        assert_eq!(cards.len(), 2);
        let fresh = cards.iter().find(|c| c.summary.contains("fresh")).unwrap();
        assert_eq!(fresh.kind, CardKind::New);
        assert_eq!(fresh.summary, "New on local, oci: skill/fresh → core");
        assert_eq!(fresh.items[0].params.layer.as_deref(), Some("core"));
        assert_eq!(fresh.subject(), "new:skill/fresh");
        let lonely = cards.iter().find(|c| c.summary.contains("lonely")).unwrap();
        assert_eq!(lonely.items[0].grp, NEEDS_A_LOOK);
        assert!(lonely.summary.ends_with("needs a look"));
    }

    /// Assets M6 (R4, carry T3 M8): two new identities that import as the
    /// same slug into the same catalog both need a look — never two New
    /// cards, the second of which would fail at apply.
    #[test]
    fn two_new_identities_sharing_a_slug_both_need_a_look() {
        let mut f = fleet(
            false,
            vec![
                row("oci", "skill", "My_Skill", "h1"),
                row("oci", "skill", "my-skill", "h2"),
            ],
        );
        f.bootstrapped = true;
        f.catalogs[0].asset_count = 10;
        f.catalogs[0].layers = vec![LayerFacts {
            name: "core".into(),
            hosts: BTreeSet::from(["oci".to_string()]),
        }];
        let cards = propose(&f.input());
        let news: Vec<_> = cards.iter().filter(|c| c.kind == CardKind::New).collect();
        assert_eq!(news.len(), 2);
        for c in news {
            assert!(c.summary.ends_with("needs a look"), "{}", c.summary);
            assert_eq!(c.items[0].grp, NEEDS_A_LOOK);
            assert!(c.items[0]
                .params
                .reason
                .as_deref()
                .unwrap()
                .starts_with("imports as my-skill, as "));
        }
    }

    /// R4: a new identity whose slug the destination catalog already holds
    /// under another name needs a look.
    #[test]
    fn a_new_identity_whose_slug_the_catalog_holds_needs_a_look() {
        let mut f = fleet(false, vec![row("oci", "skill", "My_Skill", "h1")]);
        f.bootstrapped = true;
        f.catalogs[0].asset_count = 10;
        f.catalogs[0].layers = vec![LayerFacts {
            name: "core".into(),
            hosts: BTreeSet::from(["oci".to_string()]),
        }];
        f.catalogs[0]
            .slugs
            .insert(("skill".into(), "my-skill".into()));
        let cards = propose(&f.input());
        let c = cards.iter().find(|c| c.kind == CardKind::New).unwrap();
        assert!(c.summary.ends_with("needs a look"), "{}", c.summary);
        assert_eq!(
            c.items[0].params.reason.as_deref(),
            Some("imports as my-skill, which the catalog already holds")
        );
    }

    /// Spec: "Rules never re-propose a subject with a matching verdict until
    /// its content hash changes."
    #[test]
    fn a_verdict_holds_a_subject_until_its_content_changes() {
        let mut f = fleet(false, vec![row("local", "skill", "w", "h1")]);
        f.bootstrapped = true;
        f.verdicts.insert(("skill".into(), "w".into(), "h1".into()));
        assert!(propose(&f.input()).is_empty());
        f.identities = group_identities(&[row("local", "skill", "w", "h2")]);
        assert_eq!(propose(&f.input()).len(), 1, "a new copy is a new subject");
    }

    /// Assets M5 (R5): a copy a person edited says so in its card.
    #[test]
    fn a_drift_card_says_the_copy_was_edited_on_its_host() {
        let mut f = fleet(false, vec![]);
        f.bootstrapped = true;
        f.drifted = vec![DriftFacts {
            catalog_id: PERSONAL,
            kind: "skill".into(),
            name: "w".into(),
            host: "trn".into(),
            host_hash: Some("e".into()),
            edited: true,
        }];
        let cards = propose(&f.input());
        let drift = cards.iter().find(|c| c.kind == CardKind::Drift).unwrap();
        assert_eq!(
            drift.summary,
            "skill/w was edited on trn (catalog personal)"
        );
        assert_eq!(drift.subject(), format!("drift:{PERSONAL}:skill/w@trn"));
    }

    /// Drift offers take or restore; a never-rolled-out layer's gaps become
    /// one Rollout card; an open rollout and a verdict hold each.
    #[test]
    fn drift_offers_take_or_restore_and_rollout_covers_a_new_layers_gaps() {
        let mut f = fleet(false, vec![]);
        f.bootstrapped = true;
        f.drifted = vec![DriftFacts {
            catalog_id: PERSONAL,
            kind: "skill".into(),
            name: "w".into(),
            host: "trn".into(),
            host_hash: Some("e".into()),
            edited: false,
        }];
        f.gaps = ["oci", "htz"]
            .iter()
            .map(|h| LayerGap {
                catalog_id: PERSONAL,
                layer: "core".into(),
                host: h.to_string(),
                assets: vec!["skill/w".into()],
            })
            .collect();
        let cards = propose(&f.input());
        let drift = cards.iter().find(|c| c.kind == CardKind::Drift).unwrap();
        assert_eq!(
            drift.items.iter().map(|i| i.action).collect::<Vec<_>>(),
            [ItemAction::TakeHost, ItemAction::Restore]
        );
        assert_eq!(drift.subject(), format!("drift:{PERSONAL}:skill/w@trn"));
        let rollout = cards.iter().find(|c| c.kind == CardKind::Rollout).unwrap();
        assert_eq!(rollout.items.len(), 2);
        assert_eq!(rollout.subject(), format!("rollout:{PERSONAL}/core"));
        assert!(rollout
            .items
            .iter()
            .all(|i| i.params.layer.as_deref() == Some("core")));
        let gaps: Vec<&LayerGap> = f.gaps.iter().collect();
        assert!(
            rollout
                .items
                .iter()
                .all(|i| i.params.hash.as_deref() == Some(gap_hash(&gaps).as_str())),
            "every rollout item carries the layer's gap hash, so a verdict can hold it"
        );
        f.rollout_open.insert((PERSONAL, "core".into()));
        f.verdicts.insert(("skill".into(), "w".into(), "e".into()));
        assert!(
            propose(&f.input()).is_empty(),
            "an open rollout and a verdict hold both"
        );
    }

    /// Task 4 review M3 / R11: a look item whose org catalog failed to load
    /// keeps that catalog's id, so apply refuses it until the catalog loads
    /// instead of adopting it into personal.
    #[test]
    fn a_look_item_for_a_failed_org_catalog_keeps_that_catalog() {
        let mut f = fleet(true, vec![row("trn", "skill", "x", "h1")]);
        f.catalogs[1].loaded = false;
        let cards = propose(&f.input());
        let look = cards
            .iter()
            .flat_map(|c| &c.items)
            .find(|i| i.grp == NEEDS_A_LOOK && i.name == "x")
            .expect("a look item");
        assert_eq!(look.catalog_id, Some(PAPAYA));
        assert!(
            look.params
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("failed to load"),
            "{:?}",
            look.params.reason
        );
    }

    /// Assets M5 (R7): two names that import as one slug both need a look —
    /// the card still applies, instead of failing on the second import.
    #[test]
    fn names_that_import_as_one_slug_need_a_look_instead_of_failing_the_card() {
        let f = fleet(
            false,
            vec![
                row("local", "skill", "My_Skill", "h1"),
                row("local", "skill", "my-skill", "h2"),
                row("local", "skill", "other", "h3"),
            ],
        );
        let card = &propose(&f.input())[0];
        assert_eq!(card.kind, CardKind::Bootstrap);
        let looks: Vec<(&str, &str)> = card
            .items
            .iter()
            .filter(|i| i.grp == NEEDS_A_LOOK)
            .map(|i| (i.name.as_str(), i.params.reason.as_deref().unwrap_or("")))
            .collect();
        assert_eq!(
            looks,
            [
                ("My_Skill", "imports as my-skill, as my-skill does"),
                ("my-skill", "imports as my-skill, as My_Skill does"),
            ]
        );
        assert_eq!(
            imports(card).values().sum::<usize>(),
            1,
            "`other` still imports"
        );
    }

    /// Assets M5 (PF18): a slug is one per destination catalog — two names
    /// bound for different catalogs never collide, and both import.
    #[test]
    fn names_bound_for_different_catalogs_do_not_collide() {
        let f = fleet(
            true,
            vec![
                row("trn", "skill", "My_Skill", "h1"),
                row("local", "skill", "my-skill", "h2"),
            ],
        );
        let card = &propose(&f.input())[0];
        assert_eq!(card.kind, CardKind::Bootstrap);
        assert!(
            card.items.iter().all(|i| i.grp != NEEDS_A_LOOK),
            "{:?}",
            card.items
        );
        let by_catalog: BTreeSet<(Option<i64>, &str)> = card
            .items
            .iter()
            .filter(|i| i.action == ItemAction::Import)
            .map(|i| (i.catalog_id, i.name.as_str()))
            .collect();
        assert_eq!(
            by_catalog,
            BTreeSet::from([(Some(PAPAYA), "My_Skill"), (Some(PERSONAL), "my-skill")])
        );
    }
}
