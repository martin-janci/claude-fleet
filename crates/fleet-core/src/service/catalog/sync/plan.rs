//! Sync plan computation: diff the catalog against one host's snapshot and
//! managed manifest and decide, per asset, exactly what the sync engine
//! would do — plus a short-lived in-process registry so a computed plan can
//! be reviewed (`sync_plan`) and then applied (`sync_apply`) by id without
//! recomputing it against a host that may have changed underneath.
//!
//! `sync::plan_sync` drives this module, wired to the `catalog_plan_sync`
//! Tauri command and the `plan_sync` MCP tool.

use super::super::harness::{
    json_get, value_hash, ConfigMerge, Harness, HostSnapshot, MergeMode, RenderPlan,
};
use super::super::inventory::merge_satisfied;
use super::super::model::{sha256_hex, Asset, AssetSpec, Kind};
use super::super::repo::{Catalog, ProblemHolds};
use super::manifest::{Manifest, ManifestEntry};
use super::secrets::{self, SecretPlan};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What the sync engine would do to one asset on one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOp {
    /// Nothing of the asset is on the host yet.
    Create,
    /// Managed by fleet, and the catalog has moved on since the last sync.
    Update,
    /// Present but different, and the difference did not come from the
    /// catalog (host-edited, or never managed) — writing it loses that edit.
    Overwrite,
    /// Already byte-for-byte correct but absent from the manifest: only the
    /// manifest entry is written.
    Adopt,
    /// A manifest entry whose asset is no longer in the catalog.
    Remove,
    PluginInstall,
    /// A pinned plugin ref whose catalog pin changed since fleet last
    /// applied it: `claude plugin update` is scheduled once. A `latest` ref
    /// whose plugin is installed at any version counts as satisfied (see
    /// `plugin_op`), so it never reaches this op.
    PluginUpdate,
    /// Nothing to do.
    Noop,
    /// Cannot be planned (unsupported kind, unresolved secret, unpinnable
    /// plugin version); `reason` says why.
    Blocked,
}

impl ActionOp {
    /// The snake_case name, which is also the key this op tallies under in
    /// `SyncPlan::counts`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionOp::Create => "create",
            ActionOp::Update => "update",
            ActionOp::Overwrite => "overwrite",
            ActionOp::Adopt => "adopt",
            ActionOp::Remove => "remove",
            ActionOp::PluginInstall => "plugin_install",
            ActionOp::PluginUpdate => "plugin_update",
            ActionOp::Noop => "noop",
            ActionOp::Blocked => "blocked",
        }
    }
}

/// The plugin a `PluginInstall`/`PluginUpdate` action refers to, lifted out
/// of `AssetSpec::PluginRef` so the applier never has to re-find the asset.
#[derive(Debug, Clone)]
pub struct PluginTarget {
    pub plugin: String,
    pub marketplace_name: String,
    pub marketplace_repo: String,
    pub version: String,
}

/// One planned change. The `#[serde(skip)]` fields carry what the applier
/// needs and the UI must never see: the substituted plan (secret values
/// inside — hence `SecretPlan`, whose `Debug` is redacted, rather than a
/// bare `RenderPlan`), the host hashes the plan was computed against (for
/// an optimistic-concurrency re-check at apply time), and the manifest
/// entry a `Remove` undoes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub kind: String,
    pub name: String,
    pub op: ActionOp,
    /// Which catalog this asset came from (Assets M2), e.g. `"personal"`.
    /// `None` for an action that names no catalog asset (a `Remove` for a
    /// manifest orphan, or a `Blocked` for a refusal the effective catalog
    /// made before any harness ever saw the asset). `#[serde(default)]`
    /// because `Action` travels the wire (`sync_plan`'s answer): a hub older
    /// than Assets M2 never sends this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// Why, for `Blocked`/`Overwrite`/plugin ops.
    pub reason: Option<String>,
    /// Display: file paths this action would write (or delete).
    pub files: Vec<String>,
    /// Display: `file:json/path` for each config merge.
    pub merges: Vec<String>,
    /// Whether applying replaces or deletes a file that exists right now.
    pub backup: bool,
    /// `${NAME}`s the asset references that resolved to a value.
    pub secrets: Vec<String>,
    /// `${NAME}`s the asset references that did not resolve.
    pub missing_secrets: Vec<String>,
    /// The substituted plan to write. `None` for `Remove`/`Noop`/`Blocked`.
    /// Plugin install/update/adopt actions carry it too, although the
    /// harness CLI does the installing: the applier hashes its rendered
    /// merge into the manifest entry so a later plan can tell whether the
    /// catalog pin changed since fleet last applied it.
    #[serde(skip)]
    pub plan: Option<SecretPlan>,
    /// path → hash the scan saw (`None` = the scan did not see the file).
    #[serde(skip)]
    pub expected: BTreeMap<String, Option<String>>,
    /// Files whose content only became correct through secret substitution;
    /// the union of `Substituted::secret_files` and `secret_merge_files`, so
    /// the applier can tighten permissions on config files too.
    #[serde(skip)]
    pub secret_files: BTreeSet<String>,
    /// The manifest entry this action supersedes: everything a previous sync
    /// wrote for this asset. On a `Remove` it is the whole point — unmerge
    /// and delete it. On a `Create`/`Update`/`Overwrite` it is present
    /// whenever the manifest already named the asset, and means "unmerge
    /// this before applying `plan`", so a changed `AppendUnique` value does
    /// not leave its old element behind in the shared array.
    #[serde(skip)]
    pub remove_entry: Option<ManifestEntry>,
    #[serde(skip)]
    pub plugin: Option<PluginTarget>,
}

/// Every action planned for one host under one harness. `snapshot` and
/// `manifest` are the exact inputs the actions were computed from, kept so
/// the applier can re-check and rewrite the manifest without re-scanning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostPlan {
    pub host_alias: String,
    pub harness: String,
    /// `"planned"` | `"skipped"`.
    pub status: String,
    pub detail: Option<String>,
    pub actions: Vec<Action>,
    #[serde(skip)]
    pub snapshot: HostSnapshot,
    #[serde(skip)]
    pub manifest: Manifest,
}

/// A whole fleet-wide plan, as handed to the UI and stashed in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPlan {
    pub id: String,
    pub computed_at: i64,
    pub hosts: Vec<HostPlan>,
    /// `ActionOp::as_str()` → how many actions carry it (zero ops absent).
    pub counts: BTreeMap<String, usize>,
}

impl SyncPlan {
    /// A plan over `hosts` with `counts` already tallied and an empty `id`
    /// (`registry_put` assigns one).
    pub fn new(hosts: Vec<HostPlan>) -> SyncPlan {
        let mut plan = SyncPlan {
            id: String::new(),
            computed_at: super::super::now_secs(),
            hosts,
            counts: BTreeMap::new(),
        };
        plan.counts = counts(&plan);
        plan
    }
}

/// Narrows a plan to one host / kind / name. `host_alias` is applied by the
/// caller when choosing which hosts to visit; `compute_host_plan` honours
/// `kind`, `name` and `layered` only.
#[derive(Debug, Clone, Default)]
pub struct PlanFilter {
    pub host_alias: Option<String>,
    pub kind: Option<Kind>,
    pub name: Option<String>,
    /// Whether this host has any `host_layers` row (role or context). A host
    /// with none resolves to the whole catalog (backward compat), and a
    /// `plugin_ref` orphan on it must plan exactly what it planned before
    /// layers existed: `Remove`. Only a host that actually has a layer
    /// assignment gets the "reported, not removed" `Noop`.
    pub layered: bool,
}

impl PlanFilter {
    /// `pub(crate)`: `sync::plan_sync` (in the parent `sync` module) uses it
    /// too, to decide which of `EffectiveSet::refused` this host's plan
    /// should turn into a `Blocked` action.
    pub(crate) fn matches(&self, kind: Kind, name: &str) -> bool {
        self.kind.is_none_or(|k| k == kind) && self.name.as_deref().is_none_or(|n| n == name)
    }
}

/// `{ plugin, marketplace_name, marketplace_repo, version }` for a
/// `plugin_ref` asset, `None` for every other kind.
fn plugin_target(asset: &Asset) -> Option<PluginTarget> {
    match &asset.spec {
        AssetSpec::PluginRef {
            marketplace,
            plugin,
            version,
            ..
        } => Some(PluginTarget {
            plugin: plugin.clone(),
            marketplace_name: marketplace.name.clone(),
            marketplace_repo: marketplace.repo.clone(),
            version: version.clone(),
        }),
        _ => None,
    }
}

fn merge_labels<'a>(merges: impl Iterator<Item = (&'a String, &'a Vec<String>)>) -> Vec<String> {
    merges
        .map(|(file, path)| format!("{file}:{}", path.join("/")))
        .collect()
}

/// Rule 5: the scanned hash of every path the action touches — each planned
/// file plus each merged config file — so the applier can detect a host that
/// changed between planning and applying.
fn expected_for<'a>(
    files: impl Iterator<Item = &'a String>,
    merge_files: impl Iterator<Item = &'a String>,
    snap: &HostSnapshot,
) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    for path in files {
        out.insert(path.clone(), snap.files.get(path).cloned());
    }
    for file in merge_files {
        out.entry(file.clone())
            .or_insert_with(|| snap.files.get(file).cloned());
    }
    out
}

/// Compute what `harness` would do on `host_alias` for every catalog asset
/// matching `filter`, plus a `Remove` for every manifest entry the catalog
/// no longer has.
///
/// The decision rules, in order:
///
/// 1. `Harness::render` returning `Unsupported` ⇒ `Blocked("unsupported on
///    <harness>")`. An empty plan (the asset disables this target) ⇒ `Noop`,
///    with the renderer's warning as `reason`.
/// 2. `secrets::substitute` with the host's resolved secrets; any `missing`
///    ⇒ `Blocked("missing secrets: A, B")` carrying `missing_secrets`.
/// 3. Plugin refs never write files — the harness CLI installs them — so
///    they are decided from the rendered `Subset` merge alone: the record is
///    *present* when the merge with its version stripped (`[{}]`) is
///    satisfied, i.e. the `<plugin>@<marketplace>` key holds at least one
///    object. Absent ⇒ `PluginInstall`. Present and *matching* ⇒ `Noop` if
///    the manifest names it, else `Adopt`; a `latest` ref matches any
///    installed version (the host's record says what is installed, never
///    what the marketplace now offers, so a `latest` ref never reaches
///    `PluginUpdate`), and a pinned ref matches when the record carries its
///    version. Present and pinned to a *different* version: the manifest's
///    recorded `value_hash` for the plugin merge (see `plugin_op`) decides
///    whether fleet already tried this exact pin — absent, empty (a legacy
///    entry), or for a *different* pin ⇒ `PluginUpdate` (schedule one
///    `claude plugin update`); already recorded for *this* pin ⇒ `Blocked`:
///    the CLI landed on another version and there is nothing more to try
///    automatically. That record only lands when the whole host apply is
///    clean — the manifest write is skipped when *any* action on the host
///    failed — so an unrelated failure in the same apply means the pin is
///    not recorded and `plugin_update` is issued again on the next apply.
/// 4. Every other kind, against the *substituted* plan: `present` when at
///    least one planned file exists in the snapshot or at least one merge's
///    `json_path` resolves (an `AppendUnique` merge points at a shared
///    per-event array, so it counts as present only once its own value is
///    in it — same rule as `inventory::compute_states`); `matches` when
///    every planned file exists with hash `sha256_hex(bytes)` *and* every
///    merge is `merge_satisfied`. Then: not present ⇒ `Create`; present and
///    matching ⇒ `Noop` if the manifest names it *at the locations this
///    render produces*, `Update` if the entry still lists a path or merge
///    the render has moved away from (a re-pointed `install_as`, or a Codex
///    skill moving from `~/.codex/skills` to `~/.agents/skills` in F3c,
///    whose new location already held an identical copy — the old one has
///    to go), else `Adopt`; present and differing ⇒ `Overwrite` when the
///    asset moved (as above) onto a planned file the entry does not list —
///    a copy fleet never wrote there, so not a catalog update — else
///    `Update` when the manifest names it with a *different*
///    hash (the catalog moved on), `Overwrite("edited on host")` when the
///    manifest names it with the *same* hash (so the difference came from
///    the host), and `Overwrite("present but differs; not managed")` when
///    the manifest does not name it at all. A `Create` whose entry lists
///    locations the render moved away from says so in its reason: rule 8
///    deletes those old files with it, and `files` names only the new ones.
/// 5. `expected` records the scanned hash of every planned file and every
///    merged config file (`None` when the scan did not see it).
/// 6. `manifest.orphans(catalog, keep.speaks_for)` ⇒ `Remove`, with
///    `files`/`merges`/`remove_entry` taken from the manifest entry — only
///    for an entry whose own catalog speaks for this host. A manifest key
///    `Manifest::split_key` cannot parse is skipped (it names no kind, so
///    there is nothing to filter or display it as) and logged. An entry
///    absent from `catalog` whose catalog does NOT speak for this host (not
///    loaded, failed to load, no longer accepted, not configured —
///    `manifest.held`) is kept: a `Noop` naming its catalog and why
///    (Assets M3, Rulings R6; spec: no automatic remove). An entry whose
///    own catalog speaks but whose key that catalog could not read
///    (`keep.problem_held`, Assets M4) is kept the same way.
/// 7. `backup` is true whenever something that exists on the host right now
///    would be replaced (`Update`/`Overwrite`) or deleted (`Remove`) — a
///    planned file that is already there, or, for a merge-only asset, a
///    config file the merge would rewrite.
/// 8. `remove_entry` on a *non*-`Remove` op means: unmerge the previous
///    entry first. Whenever the manifest already names an asset whose op is
///    `Create`/`Update`/`Overwrite`, the previous entry rides along so the
///    applier can `remove_merges(previous)` before `apply_merges(new)`. That
///    matters most for an `AppendUnique` (hook) merge whose value changed:
///    its old element is still in the shared array and nothing else would
///    ever take it out — and because the *new* value is absent from that
///    array, rule 4 reads the asset as `Create`, not `Update`.
/// 9. (F3c) An action that would write, adopt or delete a *file path* under
///    a directory the scan reported as a symlink (`HostSnapshot::links`) ⇒
///    `Blocked(Harness::symlink_reason)`, with no plan and no
///    `remove_entry` left to apply. `Noop`s and earlier refusals stay. File
///    paths only — a config *merge*'s file is not checked against `links`.
/// 10. (F3c) An action whose planned file is absent from the snapshot at its
///     exact path while a path equal to it ignoring ASCII case is present
///     (`skill.md` on disk, `SKILL.md` rendered) ⇒ `Blocked`, naming the
///     on-disk path, with no plan and no `remove_entry`. On a
///     case-insensitive filesystem rule 4 would read it as absent (`Create`)
///     while the applier's compare-and-swap finds the file and conflicts on
///     every sync. Every harness, on every host (conservative where the
///     filesystem is case-sensitive); runs after rule 9, so a symlink
///     refusal keeps its reason, and never touches a `Noop` or a `Remove`.
///
/// A host that could not be reached never reaches this function: the caller
/// pushes a `HostPlan` with `status: "skipped"` and a `detail` instead.
///
/// The hash compared against `ManifestEntry::hash` in rule 4 is the
/// *substituted* plan's hash, matching `Manifest::entry_for`, which hashes
/// already-substituted merge values — so the applier must record
/// `action.plan`'s hash, not the raw `Harness::render` output's. A rotated
/// secret therefore reads as `Update` (the host's bytes really must change),
/// not as a host edit.
#[allow(clippy::too_many_arguments)]
pub fn compute_host_plan(
    catalog: &Catalog,
    harness: &dyn Harness,
    host_alias: &str,
    snap: &HostSnapshot,
    manifest: &Manifest,
    secrets: &BTreeMap<String, String>,
    filter: &PlanFilter,
    // What the orphan pass must leave on the host (see `KeepRules`).
    // `KeepRules::default()` for every caller outside `plan_sync`.
    keep: &KeepRules,
) -> HostPlan {
    let mut actions = Vec::new();
    for asset in &catalog.assets {
        if !filter.matches(asset.kind(), &asset.header.name) {
            continue;
        }
        let mut action = action_for(harness, asset, snap, manifest, secrets);
        // Assets M2: every action for a catalog asset says which catalog it
        // came from. `catalog` here is the host's resolved (`eff.catalog`)
        // catalog, so `origin_of` answers correctly for a union of several
        // catalogs as well as for the single-catalog case (where it falls
        // back to `catalog` itself, e.g. "personal").
        action.catalog = Some(catalog.origin_of(asset.kind(), &asset.header.name).name);
        actions.push(action);
    }
    for (key, entry) in manifest.orphans(catalog, keep.speaks_for.as_ref()) {
        let Some((kind, name)) = Manifest::split_key(key) else {
            tracing::warn!(
                key,
                host = host_alias,
                "manifest key is not <kind>/<name>; skipping its removal"
            );
            continue;
        };
        if !filter.matches(kind, &name) {
            continue;
        }
        if keep.protected.contains(&(kind, name.clone())) {
            // Refused or silently withheld, not dropped from the catalog: a
            // `Blocked` or `Noop` action already says why
            // (`sync::plan_sync` appends it), and removing the host's
            // existing copy here would make either destructive — the one
            // thing neither must ever be.
            continue;
        }
        // Assets M4, carry 2 (Rulings R24): the entry's catalog speaks, but
        // could not read this key's own file (or its whole kind directory)
        // — absence here is "unreadable", not "dropped". Hold it with a
        // `Noop` saying why; never a `Remove`. This is the one deliberate
        // change for a personal-only fleet (ruling PF6 amends the parity
        // constraint: "plans exactly as before, EXCEPT carry 2 — an asset
        // whose catalog file has a Problem is held, never removed"). Checked
        // after `protected`, which already reports its key (one action per
        // key), and before the plugin rule, whose reason would be wrong.
        if let Some(why) = keep
            .problem_held
            .get(&entry.catalog)
            .and_then(|h| h.reason(kind, &name))
        {
            actions.push(held_noop(
                kind,
                &name,
                &entry.catalog,
                format!(
                    "catalog {} could not read it ({why}); its copy is kept, not removed",
                    entry.catalog
                ),
            ));
            continue;
        }
        if kind == Kind::PluginRef && filter.layered {
            actions.push(bare_action(
                kind,
                &name,
                ActionOp::Noop,
                // Not a catalog asset any more — this is exactly the entry
                // the catalog dropped — so it names no catalog.
                None,
                "no longer in this host's effective catalog; plugins are not removed \
                 automatically"
                    .to_string(),
            ));
            continue;
        }
        actions.push(Action {
            kind: kind.as_str().to_string(),
            name,
            op: ActionOp::Remove,
            catalog: None,
            reason: Some("no longer in the catalog".into()),
            files: entry.files.clone(),
            merges: merge_labels(entry.merges.iter().map(|m| (&m.file, &m.json_path))),
            backup: entry.files.iter().any(|p| snap.files.contains_key(p)),
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: None,
            expected: expected_for(
                entry.files.iter(),
                entry.merges.iter().map(|m| &m.file),
                snap,
            ),
            secret_files: BTreeSet::new(),
            remove_entry: Some(entry.clone()),
            plugin: None,
        });
    }
    // Assets M3 (R6): an entry whose catalog does not speak for this host —
    // not loaded, failed to load, no longer accepted, not configured — is
    // kept and reported, never removed (spec: no automatic remove).
    if let Some(speaks) = keep.speaks_for.as_ref() {
        for (key, entry) in manifest.held(catalog, speaks) {
            let Some((kind, name)) = Manifest::split_key(key) else {
                continue;
            };
            if !filter.matches(kind, &name) || keep.protected.contains(&(kind, name.clone())) {
                continue;
            }
            actions.push(held_noop(
                kind,
                &name,
                &entry.catalog,
                keep.held_reason(&entry.catalog),
            ));
        }
    }
    // Rule 9 (F3c): nothing is written or deleted through a symlinked
    // directory.
    block_symlinked(&mut actions, snap, harness);
    // Rule 10 (F3c): nor over a file that differs only in letter case.
    // After rule 9, so a symlink refusal keeps its own reason.
    block_case_variants(&mut actions, snap);
    HostPlan {
        host_alias: host_alias.to_string(),
        harness: harness.id().to_string(),
        status: "planned".into(),
        detail: None,
        actions,
        snapshot: snap.clone(),
        manifest: manifest.clone(),
    }
}

/// What `compute_host_plan`'s orphan pass must leave on the host (Assets
/// M2 + M3). `Default` keeps nothing extra and lets every catalog speak —
/// what every caller outside `plan_sync` wants.
#[derive(Debug, Clone, Default)]
pub struct KeepRules {
    /// `(kind, name)` of every asset the host's existing copy must be left
    /// alone for, though it is absent from `catalog` (M2): the EFFECTIVE
    /// catalog REFUSED it (a scope boundary or a cross-catalog collision) or
    /// the scope boundary WITHHELD it silently (an unlayered org host's
    /// private personal assets) — `effective::EffectiveSet::refused` and
    /// `::withheld`, merged by `sync::plan_sync`, which reports each itself.
    /// Without this such an asset reads to `manifest.orphans` exactly like
    /// one the catalog genuinely dropped, scheduling a `Remove`.
    pub protected: BTreeSet<(Kind, String)>,
    /// The catalogs that speak for this host (`EffectiveSet::speaks_for`);
    /// `None` = all. An orphan `Remove` is planned only for an entry of one.
    pub speaks_for: Option<BTreeSet<String>>,
    /// Catalog → why its entries are kept (`EffectiveSet::held_back_for`).
    pub held_back: BTreeMap<String, String>,
    /// Catalog → the keys its load problems hold (Assets M4, carry 2,
    /// `EffectiveSet::problem_held`): an entry of that catalog whose key is
    /// held gets a `Noop` saying why, never a `Remove`.
    pub problem_held: BTreeMap<String, ProblemHolds>,
}

impl KeepRules {
    fn held_reason(&self, catalog: &str) -> String {
        self.held_back.get(catalog).cloned().unwrap_or_else(|| {
            format!(
                "catalog {catalog} is not configured on this fleet; its assets are kept, not removed"
            )
        })
    }
}

/// An action that carries no files, merges or plan — there is nothing to
/// render for an asset that never reached the resolved catalog. Shared by
/// [`blocked_action`], [`withheld_noop`] and [`held_noop`].
fn bare_action(
    kind: Kind,
    name: &str,
    op: ActionOp,
    catalog: Option<String>,
    reason: String,
) -> Action {
    Action {
        kind: kind.as_str().to_string(),
        name: name.to_string(),
        op,
        catalog,
        reason: Some(reason),
        files: Vec::new(),
        merges: Vec::new(),
        backup: false,
        secrets: Vec::new(),
        missing_secrets: Vec::new(),
        plan: None,
        expected: BTreeMap::new(),
        secret_files: BTreeSet::new(),
        remove_entry: None,
        plugin: None,
    }
}

/// A `Noop` for a manifest entry whose catalog does not speak for this host
/// (Assets M3, R6): kept, never removed, with `reason` saying why.
pub(crate) fn held_noop(kind: Kind, name: &str, catalog: &str, reason: String) -> Action {
    bare_action(
        kind,
        name,
        ActionOp::Noop,
        Some(catalog.to_string()),
        reason,
    )
}

/// A `Blocked` action for an asset the *effective catalog* refused before
/// any harness ever saw it — a scope boundary or a cross-catalog collision
/// (`effective::EffectiveSet::refused`), as opposed to a per-harness render
/// decision. It carries no files, merges or plan: there is nothing to
/// render for an asset that never reached the resolved catalog. `catalog`
/// names the single catalog a scope-boundary refusal is about
/// (`Refusal::catalog`); a collision refuses a member from each of two or
/// more catalogs, so it stays `None` there — unresolved, not irrelevant.
/// `sync::plan_sync` is the only caller.
pub(crate) fn blocked_action(
    kind: Kind,
    name: &str,
    reason: String,
    catalog: Option<String>,
) -> Action {
    bare_action(kind, name, ActionOp::Blocked, catalog, reason)
}

/// A `Noop` action reporting why a private asset the scope boundary dropped
/// SILENTLY (`effective::EffectiveSet::withheld` — an unlayered org host's
/// personal catalog) is nonetheless staying on the host: it is already
/// synced there, and withholding it must never mean removing it. Carries no
/// files, merges or plan, like `blocked_action` — there is nothing to
/// render for an asset that never reached the resolved catalog.
/// `sync::plan_sync` is the only caller, and only when the host's manifest
/// already names the asset (nothing to report otherwise).
pub(crate) fn withheld_noop(kind: Kind, name: &str) -> Action {
    bare_action(
        kind,
        name,
        ActionOp::Noop,
        None,
        "private; withheld from org host, not removed".to_string(),
    )
}

/// Rule 4's reasons for an asset whose manifest entry lists locations the
/// render moved away from (F3c: Codex skills moving to `~/.agents/skills`;
/// also a re-pointed `install_as`).
pub(crate) const MOVED_CREATE_REASON: &str =
    "the last sync's files at its old location are removed";
pub(crate) const MOVED_UPDATE_REASON: &str =
    "installed at a different location; the old copy is removed";
pub(crate) const MOVED_ONTO_FOREIGN_REASON: &str = "moved to a location that already holds a different copy fleet did not write; it is replaced (backed up) and the old copy removed";

fn action_for(
    harness: &dyn Harness,
    asset: &Asset,
    snap: &HostSnapshot,
    manifest: &Manifest,
    secrets: &BTreeMap<String, String>,
) -> Action {
    let kind = asset.kind();
    let name = asset.header.name.clone();
    let plugin = plugin_target(asset);
    let blank = || Action {
        kind: kind.as_str().to_string(),
        name: name.clone(),
        op: ActionOp::Noop,
        // The caller (`compute_host_plan`) stamps the real value once this
        // action comes back — it alone knows which resolved catalog is
        // being planned.
        catalog: None,
        reason: None,
        files: Vec::new(),
        merges: Vec::new(),
        backup: false,
        secrets: Vec::new(),
        missing_secrets: Vec::new(),
        plan: None,
        expected: BTreeMap::new(),
        secret_files: BTreeSet::new(),
        remove_entry: None,
        plugin: plugin.clone(),
    };

    // Rule 1.
    let rendered = match harness.render(asset) {
        Ok(p) => p,
        Err(u) => {
            return Action {
                op: ActionOp::Blocked,
                reason: Some(format!("unsupported on {}", u.harness)),
                ..blank()
            }
        }
    };
    if rendered.files.is_empty() && rendered.merges.is_empty() {
        return Action {
            op: ActionOp::Noop,
            reason: rendered.warnings.first().cloned(),
            ..blank()
        };
    }

    // Rule 2.
    let sub = secrets::substitute(&rendered, secrets);
    let plan = sub.plan.inner();
    let files: Vec<String> = plan.files.iter().map(|f| f.path.clone()).collect();
    let merges = merge_labels(plan.merges.iter().map(|m| (&m.file, &m.json_path)));
    let expected = expected_for(files.iter(), plan.merges.iter().map(|m| &m.file), snap);
    let resolved: Vec<String> = rendered
        .placeholders
        .iter()
        .filter(|p| !sub.missing.contains(p))
        .cloned()
        .collect();
    let mut secret_files = sub.secret_files.clone();
    secret_files.extend(sub.secret_merge_files.iter().cloned());

    if !sub.missing.is_empty() {
        return Action {
            op: ActionOp::Blocked,
            reason: Some(format!("missing secrets: {}", sub.missing.join(", "))),
            files,
            merges,
            secrets: resolved,
            missing_secrets: sub.missing.clone(),
            expected,
            ..blank()
        };
    }

    let manifest_entry = manifest.assets.get(&Manifest::key(kind, &name));

    // Rule 3.
    if let Some(target) = &plugin {
        let (op, reason) = plugin_op(plan, snap, target, manifest_entry);
        return Action {
            op,
            reason,
            files,
            merges,
            secrets: resolved,
            expected,
            secret_files,
            // The applier needs the rendered merge's value to hash into the
            // manifest entry (`plugin_entry` in `apply.rs`); a plugin ref has
            // no files to write, so this carries no secrets, only the value.
            plan: match op {
                ActionOp::PluginInstall | ActionOp::PluginUpdate | ActionOp::Adopt => {
                    Some(sub.plan.clone())
                }
                _ => None,
            },
            ..blank()
        };
    }

    // Rule 4.
    let mut present = false;
    let mut matches = true;
    for f in &plan.files {
        match snap.files.get(&f.path) {
            Some(h) => {
                present = true;
                if *h != sha256_hex(&f.bytes) {
                    matches = false;
                }
            }
            None => matches = false,
        }
    }
    for m in &plan.merges {
        let satisfied = merge_satisfied(snap, m);
        let this_present = match m.mode {
            MergeMode::AppendUnique => satisfied,
            MergeMode::Set | MergeMode::Subset => snap
                .configs
                .get(&m.file)
                .and_then(|root| json_get(root, &m.json_path))
                .is_some(),
        };
        if this_present {
            present = true;
        }
        if !satisfied {
            matches = false;
        }
    }

    let (op, reason) = if !present {
        (
            ActionOp::Create,
            // F3c: the entry lists files this render no longer produces (a
            // Codex skill fleet synced to `~/.codex/skills` before it moved
            // to `~/.agents/skills`). Rule 8's `remove_entry` deletes them;
            // the reason says so, since `files` lists only the new ones.
            manifest_entry
                .filter(|entry| has_stale_locations(entry, plan))
                .map(|_| MOVED_CREATE_REASON.to_string()),
        )
    } else if matches {
        match manifest_entry {
            // Present, identical and managed — but the entry points at a
            // location the render no longer produces. `install_as` can
            // re-point an asset at an identifier that already holds an
            // identical copy (and F3c moves Codex skills to a new
            // directory): the content check then passes at the new
            // location while the old files are still on the host and the
            // entry still claims them. `Update` (whose `remove_entry`,
            // rule 8, carries the previous entry) deletes them and
            // refreshes the entry; a `Noop` would leak them forever.
            Some(entry) if has_stale_locations(entry, plan) => {
                (ActionOp::Update, Some(MOVED_UPDATE_REASON.into()))
            }
            Some(_) => (ActionOp::Noop, None),
            None => (ActionOp::Adopt, None),
        }
    } else {
        match manifest_entry {
            // F3c: the asset moved, and its new location already holds a
            // different copy the entry never listed — fleet did not write
            // it, so replacing it is an overwrite for a person to see, not
            // a catalog update.
            Some(entry)
                if has_stale_locations(entry, plan) && holds_unlisted_copy(entry, plan, snap) =>
            {
                (ActionOp::Overwrite, Some(MOVED_ONTO_FOREIGN_REASON.into()))
            }
            Some(entry) if entry.hash == plan.hash() => (
                ActionOp::Overwrite,
                Some("edited on host; the catalog has not changed".into()),
            ),
            Some(_) => (ActionOp::Update, None),
            None => (
                ActionOp::Overwrite,
                Some("present but differs; not managed".into()),
            ),
        }
    };

    // Rule 7. A merge-only asset (an MCP server, a hook) has no planned
    // files, but rewriting the config file it merges into is just as
    // destructive, so an existing *merge* target counts too.
    let replaces = matches!(op, ActionOp::Update | ActionOp::Overwrite);
    let touches_existing = plan.files.iter().any(|f| snap.files.contains_key(&f.path))
        || plan
            .merges
            .iter()
            .any(|m| snap.files.contains_key(&m.file) || snap.configs.contains_key(&m.file));
    let backup = replaces && touches_existing;

    // Rule 8.
    let remove_entry = match op {
        ActionOp::Create | ActionOp::Update | ActionOp::Overwrite => manifest_entry.cloned(),
        _ => None,
    };

    Action {
        op,
        reason,
        files,
        merges,
        backup,
        secrets: resolved,
        plan: match op {
            ActionOp::Noop => None,
            _ => Some(sub.plan.clone()),
        },
        expected,
        secret_files,
        remove_entry,
        ..blank()
    }
}

/// Does `entry` record a file path or a merge (`file`, `json_path`) that
/// `plan` no longer produces? Non-empty means the asset moved on the host
/// and the old copy is still there.
///
/// Only this direction is checked: `Manifest::entry_for` records *every*
/// location the render it was built from produced, so an entry can lag a
/// render but never lead it.
fn has_stale_locations(entry: &ManifestEntry, plan: &RenderPlan) -> bool {
    let files: BTreeSet<&str> = plan.files.iter().map(|f| f.path.as_str()).collect();
    if entry.files.iter().any(|p| !files.contains(p.as_str())) {
        return true;
    }
    let merges: BTreeSet<(&str, &Vec<String>)> = plan
        .merges
        .iter()
        .map(|m| (m.file.as_str(), &m.json_path))
        .collect();
    entry
        .merges
        .iter()
        .any(|m| !merges.contains(&(m.file.as_str(), &m.json_path)))
}

/// Does the host already hold one of `plan`'s files at a path `entry` does
/// not list — a copy no earlier sync of this asset wrote there? Only asked
/// for an asset that moved (`has_stale_locations`): an entry recorded
/// without files (as some tests build them) never reads as moved.
fn holds_unlisted_copy(entry: &ManifestEntry, plan: &RenderPlan, snap: &HostSnapshot) -> bool {
    plan.files
        .iter()
        .any(|f| snap.files.contains_key(&f.path) && !entry.files.contains(&f.path))
}

/// Every host path `action` writes, adopts or deletes: its own `files` (the
/// planned files, or a `Remove`'s entry files) plus the files of the entry
/// it supersedes, which the applier deletes when the render no longer
/// produces them.
fn touched_paths(action: &Action) -> impl Iterator<Item = &String> {
    action
        .files
        .iter()
        .chain(action.remove_entry.iter().flat_map(|e| e.files.iter()))
}

/// Turn `action` into a refusal: nothing of it is written or deleted.
fn block(action: &mut Action, reason: String) {
    action.op = ActionOp::Blocked;
    action.reason = Some(reason);
    action.backup = false;
    action.plan = None;
    action.remove_entry = None;
}

/// The outermost symlinked directory in `links` that `path` lies under (or
/// is), with its target. `BTreeMap` order puts `~/.agents` before
/// `~/.agents/skills`, so the first match is the outermost; the `/` check
/// keeps `~/.agent` from matching `~/.agents/…`. Compared with ASCII
/// case-folding (review fix round 1, M3): hosts, and therefore the scan's
/// `readlink` output and the catalog's rendered paths, can differ only in
/// case on a case-insensitive filesystem (common on macOS; the default on
/// Windows, if fleet ever grows a harness there). Over-blocking on a
/// case-sensitive filesystem (Linux) where `FOO` and `foo` are genuinely
/// different paths is harmless — it just refuses an action that was never
/// actually under the link.
fn linked_dir<'a>(links: &'a BTreeMap<String, String>, path: &str) -> Option<(&'a str, &'a str)> {
    let path_lower = path.to_ascii_lowercase();
    links
        .iter()
        .find(|(link, _)| {
            path_lower
                .strip_prefix(link.to_ascii_lowercase().as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        .map(|(link, target)| (link.as_str(), target.as_str()))
}

/// Rule 9 (multi-harness F3c): refuse every action that would write, adopt
/// or delete anything under a symlinked directory. Some setups point
/// `~/.agents/skills` at `~/.claude/skills`: writing Codex's render through
/// it would replace Claude's copies, adopting would make two manifests
/// claim one file, and the two harnesses would undo each other on every
/// sync. File paths only (`touched_paths`) — a config *merge*'s file is not
/// checked, so a symlinked `~/.codex/config.toml` itself is out of scope
/// for this rule (M7; no harness symlinks a config file today).
fn block_symlinked(actions: &mut [Action], snap: &HostSnapshot, harness: &dyn Harness) {
    if snap.links.is_empty() {
        return;
    }
    for action in actions.iter_mut() {
        if matches!(action.op, ActionOp::Noop | ActionOp::Blocked) {
            continue;
        }
        let hit = touched_paths(action).find_map(|p| linked_dir(&snap.links, p));
        if let Some((link, target)) = hit {
            let reason = harness.symlink_reason(link, target, action.op);
            block(action, reason);
        }
    }
}

/// Rule 10 (F3c final review, I1): refuse every action whose planned file is
/// absent from the snapshot at its exact path while a path differing from
/// it only in ASCII letter case is present. On a case-insensitive
/// filesystem (common on macOS) `~/.agents/skills/x/skill.md` *is* the
/// rendered `SKILL.md`: rule 4 reads it as absent and plans a `Create`
/// expecting nothing there, and the applier's compare-and-swap then finds
/// the file and reports a conflict on every sync, never converging. A
/// person decides instead — rename it or remove it. Applied on every host:
/// on a case-sensitive filesystem the two really are different files, and
/// refusing there is merely conservative. Only `files` (the planned paths)
/// are checked; `Noop`, `Remove` and already-`Blocked` actions are left
/// alone. `block` drops `remove_entry`, so a moved asset (rule 8) deletes
/// nothing either.
fn block_case_variants(actions: &mut [Action], snap: &HostSnapshot) {
    // Built once per plan: lowercased path ⇒ an on-disk path with it.
    let lower: BTreeMap<String, &str> = snap
        .files
        .keys()
        .map(|p| (p.to_ascii_lowercase(), p.as_str()))
        .collect();
    for action in actions.iter_mut() {
        if matches!(
            action.op,
            ActionOp::Noop | ActionOp::Blocked | ActionOp::Remove
        ) {
            continue;
        }
        let variant = action.files.iter().find_map(|rendered| {
            if snap.files.contains_key(rendered) {
                return None;
            }
            lower
                .get(&rendered.to_ascii_lowercase())
                .map(|on_disk| (rendered.clone(), on_disk.to_string()))
        });
        if let Some((rendered, on_disk)) = variant {
            // Name just the file when only its last component differs.
            let shown = match (rendered.rsplit_once('/'), on_disk.rsplit_once('/')) {
                (Some((dir, file)), Some((disk_dir, _))) if dir == disk_dir => file.to_string(),
                _ => rendered,
            };
            block(
                action,
                format!(
                    "{on_disk} differs from the rendered {shown} only in letter case; rename it to {shown} or remove it"
                ),
            );
        }
    }
}

/// Multi-harness F3c: no two harnesses on one host may manage one file.
/// `plans` are one host's plans, one per harness. A path is claimed by
/// every action that is not `Blocked` and names it in `files` or in its
/// `remove_entry` (so a `Noop` claims what it already manages); every
/// action that would write, adopt or delete a path another harness also
/// claims becomes `Blocked`, naming that harness. Config merges are not
/// compared: each harness merges into its own config files.
///
/// Today Claude (`~/.claude/…`) and Codex (`~/.agents/…`, `~/.codex/…`)
/// never share a path, so this never fires; it is the guard for later
/// harnesses that also render into `~/.agents/skills`.
pub(crate) fn block_cross_harness_collisions(plans: &mut [HostPlan]) {
    let mut claims: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for hp in plans.iter() {
        for action in hp.actions.iter().filter(|a| a.op != ActionOp::Blocked) {
            for path in touched_paths(action) {
                claims
                    .entry(path.clone())
                    .or_default()
                    .insert(hp.harness.clone());
            }
        }
    }
    for hp in plans.iter_mut() {
        let harness = hp.harness.clone();
        for action in hp.actions.iter_mut() {
            if matches!(action.op, ActionOp::Noop | ActionOp::Blocked) {
                continue;
            }
            let clash = touched_paths(action).find_map(|path| {
                claims
                    .get(path)?
                    .iter()
                    .find(|other| **other != harness)
                    .map(|other| (path.clone(), other.clone()))
            });
            if let Some((path, other)) = clash {
                block(
                    action,
                    format!(
                        "{path} is also managed by the {other} plan on this host; fleet won't let two harnesses write one file"
                    ),
                );
            }
        }
    }
}

/// Rule 3's decision, split out to keep `action_for` readable. `plan` is the
/// substituted plan; a plugin ref renders exactly one `Subset` merge.
fn plugin_op(
    plan: &RenderPlan,
    snap: &HostSnapshot,
    target: &PluginTarget,
    entry: Option<&ManifestEntry>,
) -> (ActionOp, Option<String>) {
    let Some(merge) = plan.merges.first() else {
        return (ActionOp::Noop, None);
    };
    // "Is there a record at all?" — the rendered merge with its version
    // stripped, since `[{}]` is a subset of any installed record.
    let any_record = ConfigMerge {
        value: json!([{}]),
        ..merge.clone()
    };
    if !merge_satisfied(snap, &any_record) {
        return (
            ActionOp::PluginInstall,
            Some(format!(
                "install {}@{} from {}",
                target.plugin, target.marketplace_name, target.marketplace_repo
            )),
        );
    }
    // Does the record satisfy what the catalog asks for? A pinned ref
    // renders `[{"version": v}]`, so this is "installed at v"; a `latest`
    // ref renders `[{}]`, which any record satisfies — deliberately, since
    // the host's record says what is installed, never what the marketplace
    // now offers, so fleet cannot tell a stale copy from a current one and
    // never re-installs speculatively.
    if merge_satisfied(snap, merge) {
        return (
            if entry.is_some() {
                ActionOp::Noop
            } else {
                ActionOp::Adopt
            },
            None,
        );
    }
    let installed = snap
        .configs
        .get(&merge.file)
        .and_then(|root| json_get(root, &merge.json_path))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|rec| rec.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("an unknown version");

    // Pinned but not satisfied: has fleet already tried this exact pin? The
    // manifest's recorded hash for the plugin merge says so — absent, an
    // empty legacy hash (written before this hash existed), or a hash for a
    // *different* pin all mean "not yet", so retry once via `claude plugin
    // update`. A hash that already matches this render means an earlier
    // sync tried this same pin and the CLI landed elsewhere; give up rather
    // than loop forever.
    let recorded_hash = entry
        .and_then(|e| e.merges.iter().find(|m| m.file == merge.file))
        .map(|m| m.value_hash.as_str());
    let current_hash = value_hash(&merge.value);
    let already_tried = matches!(recorded_hash, Some(h) if !h.is_empty() && h == current_hash);
    if already_tried {
        (
            ActionOp::Blocked,
            Some(format!(
                "installed {installed}, catalog pins {}; the CLI cannot pin versions",
                target.version
            )),
        )
    } else {
        (
            ActionOp::PluginUpdate,
            Some(format!(
                "catalog pin changed to {} (installed {installed})",
                target.version
            )),
        )
    }
}

/// Tally every host's actions by `ActionOp::as_str()`. Ops with no actions
/// are absent rather than zero.
pub fn counts(plan: &SyncPlan) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for host in &plan.hosts {
        for action in &host.actions {
            *out.entry(action.op.as_str().to_string()).or_insert(0) += 1;
        }
    }
    out
}

/// How long a computed plan stays applicable. Long enough for a human to
/// read a fleet-wide diff, short enough that the host has probably not
/// drifted underneath it (the applier re-checks `Action::expected` anyway).
const PLAN_TTL: Duration = Duration::from_secs(600);

/// Computed plans awaiting `sync_apply`, keyed by id and stamped with the
/// instant they stop being applicable.
static PLANS: LazyLock<Mutex<HashMap<String, (Instant, SyncPlan)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn plans() -> std::sync::MutexGuard<'static, HashMap<String, (Instant, SyncPlan)>> {
    // A poisoned registry is not worth failing a sync over: the map holds
    // plain data, and a panic mid-insert leaves it consistent.
    PLANS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stash `plan` under a fresh uuid (also written into `plan.id`) for
/// `PLAN_TTL`, dropping any already-expired plans on the way in. Returns the
/// id to hand back to the caller.
pub fn registry_put(plan: SyncPlan) -> String {
    registry_put_with_ttl(plan, PLAN_TTL)
}

/// `registry_put` with an explicit lifetime, so a test can prove expiry
/// without sleeping.
pub(crate) fn registry_put_with_ttl(mut plan: SyncPlan, ttl: Duration) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    plan.id = id.clone();
    let now = Instant::now();
    let mut map = plans();
    map.retain(|_, (expires_at, _)| *expires_at > now);
    map.insert(id.clone(), (now + ttl, plan));
    id
}

/// Remove and return the plan stashed under `id`, or `None` if there is no
/// such plan or it has expired. Expired plans are dropped on the way past,
/// like `registry_put` does: each one pins a `HostSnapshot` per host, and a
/// session that computes plans but never applies them would otherwise hold
/// every one of them until the next `registry_put`.
///
/// Test-only: production code needs the original deadline too, so it calls
/// [`registry_take_with_expiry`] directly.
#[cfg(test)]
pub(crate) fn registry_take(id: &str) -> Option<SyncPlan> {
    registry_take_with_expiry(id).map(|(_, plan)| plan)
}

/// Which catalogs the plan parked under `id` would write from: the
/// `catalog` of every action that changes a host (not `Noop`/`Blocked`), and
/// the catalog of the entry it undoes — a `Remove`'s, or the previous entry a
/// `Create`/`Update`/`Overwrite` unmerges first (rule 8). Peeks: the plan
/// stays parked. `None` when no unexpired plan has that id (`apply_sync`
/// reports that itself). For the per-catalog `apply_sync` gate (Assets M3,
/// Rulings R11).
pub(crate) fn registry_catalogs_written(id: &str) -> Option<BTreeSet<String>> {
    let now = Instant::now();
    let map = plans();
    let (expires_at, plan) = map.get(id)?;
    if *expires_at <= now {
        return None;
    }
    Some(
        plan.hosts
            .iter()
            .flat_map(|h| &h.actions)
            .filter(|a| !matches!(a.op, ActionOp::Noop | ActionOp::Blocked))
            .flat_map(|a| {
                let undone = a.remove_entry.as_ref().map(|e| &e.catalog);
                a.catalog.iter().chain(undone).cloned()
            })
            .collect(),
    )
}

/// [`registry_take`], also handing back the plan's original deadline so a
/// caller that puts it back (`registry_put_existing`) can preserve it
/// instead of minting a fresh one.
pub(crate) fn registry_take_with_expiry(id: &str) -> Option<(Instant, SyncPlan)> {
    let now = Instant::now();
    let mut map = plans();
    map.retain(|_, (expires_at, _)| *expires_at > now);
    map.remove(id)
}

/// Put a plan the caller took with [`registry_take_with_expiry`] back under
/// the SAME id and its ORIGINAL `expires_at`, for a `sync_apply` that
/// refused to run rather than one that ran: a plan rejected for missing
/// secrets must still be there when the user sets the secret (or decides to
/// force), addressed by the id they already hold.
///
/// The deadline does NOT restart: the clock measures how long the
/// underlying host snapshot has gone unchecked, and a refused apply did not
/// touch the host, so extending it here would let a plan quietly outlive
/// how stale its snapshot actually is.
pub fn registry_put_existing(id: &str, expires_at: Instant, mut plan: SyncPlan) {
    plan.id = id.to_string();
    let now = Instant::now();
    let mut map = plans();
    map.retain(|_, (e, _)| *e > now);
    map.insert(id.to_string(), (expires_at, plan));
}

/// Drop every parked plan that covers `host_alias` (any harness, any
/// status), returning how many went. A host's harness choice changing
/// (`harness_set::set_host_harnesses`) makes such a plan wrong — one
/// computed before Codex was turned off would still write Codex there — so
/// its id then reads as stale (`E_SYNC_PLAN_STALE`) and the caller re-plans.
pub fn registry_drop_host(host_alias: &str) -> usize {
    let now = Instant::now();
    let mut map = plans();
    map.retain(|_, (e, _)| *e > now);
    let before = map.len();
    map.retain(|_, (_, plan)| !plan.hosts.iter().any(|h| h.host_alias == host_alias));
    before - map.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::claude::Claude;
    use crate::service::catalog::harness::codex::Codex;
    use crate::service::catalog::harness::{apply_merges, ManifestMerge};
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::model::Problem;

    const SKILL: &str = "kind: skill\nname: s\ndescription: d\n";
    const HOOK: &str =
        "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n";
    const MCP: &str = "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: https://example/mcp\nheaders:\n  Authorization: \"Bearer ${FLEET_MCP_TOKEN}\"\n";
    const PLUGIN: &str = "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: mk, source: github, repo: o/r }\nplugin: sp\nversion: \"6.3.0\"\n";
    const PLUGIN_LATEST: &str = "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: mk, source: github, repo: o/r }\nplugin: sp\nversion: latest\n";

    /// A `plugin_ref` catalog YAML pinned to `version`.
    fn plugin_yaml(version: &str) -> String {
        format!(
            "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: {{ name: mk, source: github, repo: o/r }}\nplugin: sp\nversion: \"{version}\"\n"
        )
    }

    /// A manifest naming `plugin_ref/sp` with `value_hash` recorded for its
    /// `PLUGINS_PATH` merge — what `plugin_entry` (apply.rs) would have
    /// written after an earlier sync.
    fn plugin_manifest(hash: &str) -> Manifest {
        let mut m = Manifest::default();
        m.assets.insert(
            "plugin_ref/sp".into(),
            ManifestEntry {
                merges: vec![ManifestMerge {
                    file: crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
                    json_path: vec!["plugins".into(), "sp@mk".into()],
                    mode: MergeMode::Subset,
                    value_hash: hash.to_string(),
                }],
                ..Default::default()
            },
        );
        m
    }

    fn asset(yaml: &str) -> Asset {
        let mut a = Asset::from_yaml(None, yaml).unwrap();
        a.body = "body\n".into();
        a
    }

    fn catalog_of(yamls: &[&str]) -> Catalog {
        Catalog {
            assets: yamls.iter().map(|y| asset(y)).collect(),
            ..Default::default()
        }
    }

    fn cat() -> Catalog {
        catalog_of(&[SKILL, HOOK, MCP, PLUGIN])
    }

    fn secrets_map() -> BTreeMap<String, String> {
        BTreeMap::from([("FLEET_MCP_TOKEN".to_string(), "tok".to_string())])
    }

    /// The substituted plan for one asset — what the host is compared to.
    fn substituted(
        harness: &dyn Harness,
        a: &Asset,
        values: &BTreeMap<String, String>,
    ) -> RenderPlan {
        secrets::substitute(&harness.render(a).unwrap(), values)
            .plan
            .inner()
            .clone()
    }

    /// Make `snap` satisfy `plan` exactly: every file at its real hash,
    /// every merge applied into the config it targets.
    fn satisfy(snap: &mut HostSnapshot, plan: &RenderPlan) {
        for f in &plan.files {
            snap.files.insert(f.path.clone(), sha256_hex(&f.bytes));
        }
        for m in &plan.merges {
            let root = snap
                .configs
                .entry(m.file.clone())
                .or_insert_with(|| json!({}));
            apply_merges(root, std::slice::from_ref(m));
        }
    }

    fn host_with(assets: &[&str]) -> HostSnapshot {
        let mut snap = HostSnapshot::default();
        for y in assets {
            satisfy(&mut snap, &substituted(&Claude, &asset(y), &secrets_map()));
        }
        snap
    }

    fn manifest_with(entries: &[(&str, &str)]) -> Manifest {
        let mut m = Manifest {
            version: 1,
            ..Default::default()
        };
        for (key, hash) in entries {
            m.assets.insert(
                (*key).to_string(),
                ManifestEntry {
                    hash: (*hash).to_string(),
                    ..Default::default()
                },
            );
        }
        m
    }

    fn plan_for(
        catalog: &Catalog,
        harness: &dyn Harness,
        snap: &HostSnapshot,
        manifest: &Manifest,
        values: &BTreeMap<String, String>,
    ) -> HostPlan {
        compute_host_plan(
            catalog,
            harness,
            "local",
            snap,
            manifest,
            values,
            &PlanFilter::default(),
            &KeepRules::default(),
        )
    }

    fn act<'a>(hp: &'a HostPlan, name: &str) -> &'a Action {
        hp.actions
            .iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("no action for {name}: {:?}", hp.actions))
    }

    #[test]
    fn empty_host_creates_everything_and_installs_the_plugin() {
        let hp = plan_for(
            &cat(),
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(hp.status, "planned");
        assert_eq!(hp.harness, "claude");
        assert_eq!(act(&hp, "s").op, ActionOp::Create);
        assert_eq!(act(&hp, "h").op, ActionOp::Create);
        assert_eq!(act(&hp, "fleet").op, ActionOp::Create);
        assert_eq!(act(&hp, "sp").op, ActionOp::PluginInstall);

        let s = act(&hp, "s");
        assert!(!s.backup, "nothing to back up on an empty host");
        assert!(s.plan.is_some(), "a create carries the plan to write");
        assert_eq!(s.files, vec!["~/.claude/skills/s/SKILL.md".to_string()]);
        assert_eq!(s.expected["~/.claude/skills/s/SKILL.md"], None);
        // The plugin action names its target, never a file, but still
        // carries the rendered merge so the applier can hash it into the
        // manifest entry.
        let sp = act(&hp, "sp");
        assert!(sp.files.is_empty());
        assert!(
            sp.plan.is_some(),
            "plugin ops shell out, but the render plan rides along for the manifest write"
        );
        let t = sp.plugin.as_ref().unwrap();
        assert_eq!(
            (t.plugin.as_str(), t.marketplace_repo.as_str()),
            ("sp", "o/r")
        );
        // A secret that resolved is reported, and the file that carries it.
        let fleet = act(&hp, "fleet");
        assert_eq!(fleet.secrets, vec!["FLEET_MCP_TOKEN".to_string()]);
        assert!(fleet.missing_secrets.is_empty());
        assert!(fleet.secret_files.contains("~/.claude.json"));
    }

    #[test]
    fn matching_host_adopts_when_unmanaged_and_noops_when_managed() {
        let snap = host_with(&[SKILL, HOOK, MCP, PLUGIN]);
        let hp = plan_for(&cat(), &Claude, &snap, &Manifest::default(), &secrets_map());
        assert_eq!(act(&hp, "s").op, ActionOp::Adopt);
        assert_eq!(act(&hp, "h").op, ActionOp::Adopt);
        assert_eq!(act(&hp, "fleet").op, ActionOp::Adopt);
        assert_eq!(act(&hp, "sp").op, ActionOp::Adopt);
        assert!(
            act(&hp, "s").plan.is_some(),
            "adopt still writes a manifest entry"
        );
        assert!(!act(&hp, "s").backup);

        let manifest = manifest_with(&[
            ("skill/s", "x"),
            ("hook/h", "x"),
            ("mcp_server/fleet", "x"),
            ("plugin_ref/sp", "x"),
        ]);
        let hp = plan_for(&cat(), &Claude, &snap, &manifest, &secrets_map());
        assert_eq!(act(&hp, "s").op, ActionOp::Noop);
        assert_eq!(act(&hp, "h").op, ActionOp::Noop);
        assert_eq!(act(&hp, "fleet").op, ActionOp::Noop);
        assert_eq!(act(&hp, "sp").op, ActionOp::Noop);
        assert!(act(&hp, "s").plan.is_none(), "a noop writes nothing");
    }

    #[test]
    fn a_re_pointed_install_name_updates_instead_of_noop() {
        const RENAMED: &str = "kind: skill\nname: foo-bar\ndescription: d\ninstall_as: foo_bar\n";
        const PLAIN: &str = "kind: skill\nname: foo-bar\ndescription: d\n";

        let old_plan = substituted(&Claude, &asset(RENAMED), &secrets_map());
        let new_plan = substituted(&Claude, &asset(PLAIN), &secrets_map());
        let old_path = old_plan.files[0].path.clone();
        let new_path = new_plan.files[0].path.clone();
        assert_ne!(old_path, new_path);
        assert!(old_path.ends_with("foo_bar/SKILL.md"), "{old_path}");

        // The host already holds identical content at BOTH identifiers and
        // the manifest still records the old one; the catalog now renders
        // to the new one.
        let mut snap = HostSnapshot::default();
        satisfy(&mut snap, &old_plan);
        satisfy(&mut snap, &new_plan);
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/foo-bar".into(),
            Manifest::entry_for(&old_plan.hash(), &old_plan, 0, "personal"),
        );

        let hp = plan_for(
            &catalog_of(&[PLAIN]),
            &Claude,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "foo-bar");
        assert_eq!(a.op, ActionOp::Update, "{:?}", a.reason);
        assert_eq!(
            a.remove_entry.as_ref().map(|e| e.files.clone()),
            Some(vec![old_path]),
            "the old paths ride along so the applier deletes them"
        );

        // The ordinary in-sync case — the entry records exactly what the
        // render produces — is still a Noop.
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/foo-bar".into(),
            Manifest::entry_for(&new_plan.hash(), &new_plan, 0, "personal"),
        );
        let mut snap = HostSnapshot::default();
        satisfy(&mut snap, &new_plan);
        let hp = plan_for(
            &catalog_of(&[PLAIN]),
            &Claude,
            &snap,
            &manifest,
            &secrets_map(),
        );
        assert_eq!(act(&hp, "foo-bar").op, ActionOp::Noop);
    }

    #[test]
    fn managed_and_stale_is_an_update_managed_and_edited_is_an_overwrite() {
        let mut snap = host_with(&[SKILL]);
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), "edited".into());
        let catalog = catalog_of(&[SKILL]);
        let render_hash = substituted(&Claude, &asset(SKILL), &secrets_map()).hash();

        // The manifest remembers a different render: the catalog moved on.
        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &manifest_with(&[("skill/s", "an-older-render")]),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Update);
        assert!(a.backup, "an existing file is replaced");
        assert_eq!(
            a.expected["~/.claude/skills/s/SKILL.md"],
            Some("edited".into())
        );

        // The manifest remembers *this* render: the host was edited.
        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &manifest_with(&[("skill/s", &render_hash)]),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert!(a.reason.as_deref().unwrap().contains("edited on host"));
        assert!(a.backup);

        // Not managed at all: also an overwrite, with a different reason.
        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(
            a.reason.as_deref(),
            Some("present but differs; not managed")
        );
        assert!(a.backup);
    }

    /// A merge-only asset has no planned files, but overwriting the config
    /// file it merges into is just as destructive — it must still back up.
    #[test]
    fn a_merge_only_asset_that_differs_is_backed_up_too() {
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::CLAUDE_JSON_PATH.into(),
            json!({"mcpServers": {"fleet": {"type": "http", "url": "https://somewhere/else"}}}),
        );
        let hp = plan_for(
            &catalog_of(&[MCP]),
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "fleet");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert!(a.files.is_empty(), "an mcp server writes no files");
        assert!(
            a.backup,
            "the config file it rewrites already exists on the host"
        );
    }

    /// An `AppendUnique` hook whose catalog value changed reads as `Create`
    /// (its *new* element is absent from the shared array), so the stale
    /// element from the last sync would be left behind unless the previous
    /// manifest entry rides along for the applier to unmerge first.
    #[test]
    fn a_changed_hook_carries_the_previous_manifest_entry_to_unmerge() {
        use crate::service::catalog::harness::claude::SETTINGS_PATH;

        // The host still holds what the *old* catalog value merged in.
        let mut snap = HostSnapshot::default();
        satisfy(
            &mut snap,
            &substituted(&Claude, &asset(HOOK), &secrets_map()),
        );

        // …while the catalog now renders a different command.
        let changed = "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: y }\n";
        let mut manifest = manifest_with(&[("hook/h", "the-old-render")]);
        let old = ManifestMerge {
            file: SETTINGS_PATH.into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value_hash: "the-old-value".into(),
        };
        manifest.assets.get_mut("hook/h").unwrap().merges = vec![old.clone()];

        let hp = plan_for(
            &catalog_of(&[changed]),
            &Claude,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "h");
        assert_eq!(
            a.op,
            ActionOp::Create,
            "the new element is not in the array"
        );
        let entry = a
            .remove_entry
            .as_ref()
            .expect("the previous entry must ride along to be unmerged");
        assert_eq!(entry.hash, "the-old-render");
        assert_eq!(entry.merges, vec![old]);
        assert!(a.plan.is_some(), "and the new plan is still applied after");

        // Nothing to unmerge when the manifest never named the asset.
        let hp = plan_for(
            &catalog_of(&[changed]),
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert!(act(&hp, "h").remove_entry.is_none());

        // …and a noop supersedes nothing.
        let hp = plan_for(
            &catalog_of(&[HOOK]),
            &Claude,
            &snap,
            &manifest_with(&[("hook/h", "x")]),
            &secrets_map(),
        );
        let a = act(&hp, "h");
        assert_eq!(a.op, ActionOp::Noop);
        assert!(a.remove_entry.is_none());
    }

    #[test]
    fn a_missing_secret_blocks_the_action() {
        let hp = plan_for(
            &catalog_of(&[MCP]),
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &BTreeMap::new(),
        );
        let a = act(&hp, "fleet");
        assert_eq!(a.op, ActionOp::Blocked);
        assert_eq!(a.missing_secrets, vec!["FLEET_MCP_TOKEN".to_string()]);
        assert_eq!(
            a.reason.as_deref(),
            Some("missing secrets: FLEET_MCP_TOKEN")
        );
        assert!(
            a.plan.is_none(),
            "a blocked action carries nothing to write"
        );
        assert!(a.secrets.is_empty());
    }

    #[test]
    fn an_unsupported_kind_blocks_and_a_disabled_target_noops() {
        let hp = plan_for(
            &catalog_of(&[HOOK]),
            &Codex,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "h");
        assert_eq!(a.op, ActionOp::Blocked);
        assert_eq!(a.reason.as_deref(), Some("unsupported on codex"));

        let disabled =
            "kind: skill\nname: s\ndescription: d\ntargets:\n  claude:\n    enabled: false\n";
        let hp = plan_for(
            &catalog_of(&[disabled]),
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Noop);
        assert_eq!(
            a.reason.as_deref(),
            Some("disabled for claude by targets.claude.enabled")
        );
    }

    /// A `latest` ref is satisfied by whatever version is installed: fleet
    /// has no way to tell a stale copy from a current one, so it never
    /// schedules a re-install — `ActionOp::PluginUpdate` never fires for it.
    #[test]
    fn a_latest_plugin_ref_matches_any_installed_version() {
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "5.0.0"}]}}),
        );
        let catalog = catalog_of(&[PLUGIN_LATEST]);

        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "sp");
        assert_eq!(a.op, ActionOp::Adopt, "{:?}", a.reason);
        assert_eq!(a.reason, None);

        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &manifest_with(&[("plugin_ref/sp", "x")]),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::Noop);

        assert!(
            !hp.actions.iter().any(|a| a.op == ActionOp::PluginUpdate),
            "latest never schedules an automatic plugin update"
        );
    }

    #[test]
    fn plugin_ops_install_adopt_and_block_on_a_pin() {
        let catalog = catalog_of(&[PLUGIN]);
        // Absent.
        let hp = plan_for(
            &catalog,
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::PluginInstall);

        // Installed at another version, catalog pins one, and fleet already
        // tried this exact pin (the manifest's recorded hash matches the
        // current render): unpinnable, stays blocked.
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "5.0.0"}]}}),
        );
        let manifest = plugin_manifest(&value_hash(&json!([{"version": "6.3.0"}])));
        let hp = plan_for(&catalog, &Claude, &snap, &manifest, &secrets_map());
        let a = act(&hp, "sp");
        assert_eq!(a.op, ActionOp::Blocked);
        let reason = a.reason.as_deref().unwrap();
        assert!(reason.contains("installed 5.0.0"), "{reason}");
        assert!(reason.contains("cannot pin versions"), "{reason}");

        // Installed at the pinned version: nothing to do.
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "6.3.0", "scope": "user"}]}}),
        );
        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::Adopt);
        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &manifest_with(&[("plugin_ref/sp", "x")]),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::Noop);

        // A record with no version at all still counts as installed.
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{}]}}),
        );
        let hp = plan_for(
            &catalog_of(&[PLUGIN_LATEST]),
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::Adopt);
        let hp = plan_for(
            &catalog_of(&[PLUGIN_LATEST]),
            &Claude,
            &snap,
            &manifest_with(&[("plugin_ref/sp", "x")]),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::Noop);
    }

    /// The catalog's pin moved on since the last sync (the manifest still
    /// carries the old pin's hash): schedule one `claude plugin update`.
    #[test]
    fn pin_change_schedules_a_plugin_update() {
        let catalog = catalog_of(&[&plugin_yaml("6.0.0")]);
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "5.0.0"}]}}),
        );
        let manifest = plugin_manifest(&value_hash(&json!([{"version": "5.0.0"}])));
        let hp = plan_for(&catalog, &Claude, &snap, &manifest, &secrets_map());
        let a = act(&hp, "sp");
        assert_eq!(a.op, ActionOp::PluginUpdate, "{:?}", a.reason);
        let reason = a.reason.as_deref().unwrap();
        assert!(reason.contains("catalog pin changed to 6.0.0"), "{reason}");
    }

    /// The manifest already recorded *this* pin's hash and the host still
    /// differs: fleet already tried once, so it gives up rather than retry
    /// forever.
    #[test]
    fn unchanged_pin_stays_blocked() {
        let catalog = catalog_of(&[&plugin_yaml("6.0.0")]);
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "5.0.0"}]}}),
        );
        let manifest = plugin_manifest(&value_hash(&json!([{"version": "6.0.0"}])));
        let hp = plan_for(&catalog, &Claude, &snap, &manifest, &secrets_map());
        let a = act(&hp, "sp");
        assert_eq!(a.op, ActionOp::Blocked, "{:?}", a.reason);
    }

    /// A manifest entry written before `value_hash` existed carries an
    /// empty hash. Treated the same as "never tried this pin": update once.
    #[test]
    fn legacy_empty_hash_updates_once() {
        let catalog = catalog_of(&[&plugin_yaml("6.0.0")]);
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "5.0.0"}]}}),
        );
        let manifest = plugin_manifest("");
        let hp = plan_for(&catalog, &Claude, &snap, &manifest, &secrets_map());
        assert_eq!(act(&hp, "sp").op, ActionOp::PluginUpdate);
    }

    /// A `latest` ref is always satisfied by whatever is installed, so it
    /// never reaches the pin-change decision at all.
    #[test]
    fn latest_never_updates() {
        let catalog = catalog_of(&[PLUGIN_LATEST]);
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "1.2.3"}]}}),
        );
        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::Adopt);

        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &manifest_with(&[("plugin_ref/sp", "x")]),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "sp").op, ActionOp::Noop);
    }

    #[test]
    fn a_manifest_entry_the_catalog_lost_becomes_a_remove() {
        let mut manifest = manifest_with(&[]);
        manifest.assets.insert(
            "skill/gone".into(),
            ManifestEntry {
                hash: "h".into(),
                files: vec!["~/.claude/skills/gone/SKILL.md".into()],
                merges: vec![ManifestMerge {
                    file: "~/.claude/settings.json".into(),
                    json_path: vec!["hooks".into(), "Stop".into()],
                    mode: MergeMode::AppendUnique,
                    value_hash: "vh".into(),
                }],
                synced_at: 1,
                catalog: "personal".into(),
            },
        );
        // A key nothing can parse is skipped rather than half-planned.
        manifest
            .assets
            .insert("garbage".into(), ManifestEntry::default());

        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/gone/SKILL.md".into(), "abc".into());
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Claude,
            &snap,
            &manifest,
            &secrets_map(),
        );
        assert_eq!(hp.actions.len(), 2, "{:?}", hp.actions);
        let a = act(&hp, "gone");
        assert_eq!(a.op, ActionOp::Remove);
        assert_eq!(a.kind, "skill");
        assert!(a.backup, "a file that still exists is backed up first");
        assert_eq!(a.files, vec!["~/.claude/skills/gone/SKILL.md".to_string()]);
        assert_eq!(
            a.merges,
            vec!["~/.claude/settings.json:hooks/Stop".to_string()]
        );
        assert_eq!(
            a.expected["~/.claude/skills/gone/SKILL.md"],
            Some("abc".into())
        );
        assert!(a.remove_entry.is_some());
        assert!(a.plan.is_none());
    }

    #[test]
    fn a_dropped_plugin_ref_is_reported_but_never_removed_on_a_layered_host() {
        // The manifest remembers a plugin the (resolved) catalog no longer
        // has. Uninstalling is slow and network-bound, so a context switch
        // must not depend on it: report, never remove. This is conditional
        // on the host actually having a layer assignment (Fix 3b) — the
        // filter's `layered` flag stands in for that here.
        let manifest = manifest_with(&[("plugin_ref/graphify", "h")]);
        let hp = compute_host_plan(
            &catalog_of(&[]),
            &Claude,
            "local",
            &host_with(&[]),
            &manifest,
            &BTreeMap::new(),
            &PlanFilter {
                layered: true,
                ..Default::default()
            },
            &KeepRules::default(),
        );
        let a = act(&hp, "graphify");
        assert_eq!(a.op, ActionOp::Noop);
        assert_eq!(a.kind, "plugin_ref");
        assert!(
            a.reason
                .as_deref()
                .unwrap_or_default()
                .contains("not removed automatically"),
            "{:?}",
            a.reason
        );
        // The reason must be true regardless of whether the host has any
        // layers at all — it must not assert "no longer in this host's
        // layers" for a host that has none (Fix 3a).
        assert!(
            !a.reason.as_deref().unwrap_or_default().contains("layers"),
            "{:?}",
            a.reason
        );
    }

    #[test]
    fn a_dropped_plugin_ref_on_an_unassigned_host_still_plans_remove() {
        // A host with no `host_layers` row must plan a dropped `plugin_ref`
        // exactly as it did before layers existed: `Remove`, restoring the
        // load-bearing backward-compat promise (Fix 3b).
        let manifest = manifest_with(&[("plugin_ref/graphify", "h")]);
        let hp = plan_for(
            &catalog_of(&[]),
            &Claude,
            &host_with(&[]),
            &manifest,
            &BTreeMap::new(),
        );
        let a = act(&hp, "graphify");
        assert_eq!(a.op, ActionOp::Remove);
        assert_eq!(a.kind, "plugin_ref");
    }

    #[test]
    fn a_removed_asset_whose_files_are_already_gone_needs_no_backup() {
        let mut manifest = manifest_with(&[]);
        manifest.assets.insert(
            "skill/gone".into(),
            ManifestEntry {
                files: vec!["~/.claude/skills/gone/SKILL.md".into()],
                ..Default::default()
            },
        );
        let hp = plan_for(
            &Catalog::default(),
            &Claude,
            &HostSnapshot::default(),
            &manifest,
            &secrets_map(),
        );
        assert!(!act(&hp, "gone").backup);
    }

    #[test]
    fn a_name_filter_narrows_the_plan_including_orphans() {
        let mut manifest = manifest_with(&[]);
        manifest
            .assets
            .insert("skill/gone".into(), ManifestEntry::default());
        let filter = PlanFilter {
            name: Some("s".into()),
            ..Default::default()
        };
        let hp = compute_host_plan(
            &cat(),
            &Claude,
            "local",
            &HostSnapshot::default(),
            &manifest,
            &secrets_map(),
            &filter,
            &KeepRules::default(),
        );
        assert_eq!(hp.actions.len(), 1);
        assert_eq!(hp.actions[0].name, "s");

        let filter = PlanFilter {
            kind: Some(Kind::McpServer),
            ..Default::default()
        };
        let hp = compute_host_plan(
            &cat(),
            &Claude,
            "local",
            &HostSnapshot::default(),
            &manifest,
            &secrets_map(),
            &filter,
            &KeepRules::default(),
        );
        assert_eq!(hp.actions.len(), 1);
        assert_eq!(hp.actions[0].name, "fleet");
    }

    /// R6: personal still speaks — its dropped asset is removed as before —
    /// while an entry of a held-back catalog, or of one that is not
    /// configured at all, is kept with a `Noop` naming its catalog and why.
    #[test]
    fn an_orphan_whose_catalog_does_not_speak_is_kept_with_a_noop() {
        let mut manifest = manifest_with(&[]);
        manifest.assets.insert(
            "skill/gone".into(),
            ManifestEntry {
                files: vec!["~/.claude/skills/gone/SKILL.md".into()],
                ..Default::default()
            },
        );
        for (name, catalog) in [
            ("theirs", "acme"),
            ("lost", "removed-one"),
            ("other", "acme"),
        ] {
            manifest.assets.insert(
                format!("skill/{name}"),
                ManifestEntry {
                    catalog: catalog.into(),
                    ..Default::default()
                },
            );
        }
        let keep = KeepRules {
            // A protected key is reported by `plan_sync` itself: no second
            // action here.
            protected: BTreeSet::from([(Kind::Skill, "other".to_string())]),
            speaks_for: Some(BTreeSet::from(["personal".to_string()])),
            held_back: BTreeMap::from([(
                "acme".to_string(),
                "catalog acme is not accepted by this host; its assets are kept, not removed"
                    .to_string(),
            )]),
            ..Default::default()
        };
        let hp = compute_host_plan(
            &Catalog::default(),
            &Claude,
            "local",
            &HostSnapshot::default(),
            &manifest,
            &secrets_map(),
            &PlanFilter::default(),
            &keep,
        );
        assert_eq!(act(&hp, "gone").op, ActionOp::Remove);
        let theirs = act(&hp, "theirs");
        assert_eq!(theirs.op, ActionOp::Noop);
        assert_eq!(theirs.catalog.as_deref(), Some("acme"));
        assert!(theirs.reason.as_deref().unwrap().contains("not accepted"));
        assert!(theirs.remove_entry.is_none() && theirs.files.is_empty());
        let lost = act(&hp, "lost");
        assert_eq!(lost.op, ActionOp::Noop);
        assert_eq!(lost.catalog.as_deref(), Some("removed-one"));
        assert!(lost
            .reason
            .as_deref()
            .unwrap()
            .contains("is not configured"));
        assert!(
            hp.actions.iter().all(|a| a.name != "other"),
            "{:?}",
            hp.actions
        );
        assert_eq!(
            hp.actions
                .iter()
                .filter(|a| a.op == ActionOp::Remove)
                .count(),
            1
        );

        // A name filter narrows the held entries too.
        let filter = PlanFilter {
            name: Some("theirs".into()),
            ..Default::default()
        };
        let hp = compute_host_plan(
            &Catalog::default(),
            &Claude,
            "local",
            &HostSnapshot::default(),
            &manifest,
            &secrets_map(),
            &filter,
            &keep,
        );
        assert_eq!(
            hp.actions
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["theirs"]
        );
    }

    #[test]
    fn counts_tally_every_host() {
        let empty = plan_for(
            &cat(),
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        );
        let matched = plan_for(
            &cat(),
            &Claude,
            &host_with(&[SKILL, HOOK, MCP, PLUGIN]),
            &Manifest::default(),
            &secrets_map(),
        );
        let plan = SyncPlan::new(vec![empty, matched]);
        assert_eq!(plan.counts["create"], 3);
        assert_eq!(plan.counts["plugin_install"], 1);
        assert_eq!(plan.counts["adopt"], 4);
        assert!(!plan.counts.contains_key("blocked"));
        assert_eq!(counts(&plan), plan.counts);
        assert!(plan.id.is_empty(), "the registry assigns the id");
    }

    #[test]
    fn the_registry_hands_a_plan_back_exactly_once() {
        let plan = SyncPlan::new(vec![plan_for(
            &cat(),
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        )]);
        let id = registry_put(plan);
        let back = registry_take(&id).expect("stored plan");
        assert_eq!(back.id, id, "the stored plan knows its own id");
        assert_eq!(back.hosts.len(), 1);
        assert!(registry_take(&id).is_none(), "taking a plan consumes it");
        assert!(registry_take("no-such-plan").is_none());
    }

    /// The `apply_sync` gate peeks which catalogs a parked plan writes from:
    /// every action that changes a host names its own, a `Remove` names the
    /// entry it undoes; `Noop`/`Blocked` name nothing; the plan stays parked.
    #[test]
    fn registry_catalogs_written_peeks_without_taking() {
        let mut hp = plan_for(
            &cat(),
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        );
        for a in &mut hp.actions {
            a.catalog = Some("acme".into());
        }
        let first = hp.actions[0].clone();
        hp.actions.push(Action {
            op: ActionOp::Remove,
            catalog: None,
            remove_entry: Some(ManifestEntry {
                catalog: "old".into(),
                ..Default::default()
            }),
            ..first.clone()
        });
        hp.actions.push(Action {
            op: ActionOp::Noop,
            catalog: Some("ignored".into()),
            ..first
        });
        let id = registry_put(SyncPlan::new(vec![hp]));
        assert_eq!(
            registry_catalogs_written(&id),
            Some(BTreeSet::from(["acme".to_string(), "old".to_string()]))
        );
        assert!(
            registry_take(&id).is_some(),
            "peeking leaves the plan parked"
        );
        assert_eq!(registry_catalogs_written("no-such-plan"), None);
    }

    /// Rule 8: an `Update` that unmerges a previous entry from another
    /// catalog (the asset moved catalogs) writes on that catalog's behalf
    /// too, so the gate names both.
    #[test]
    fn registry_catalogs_written_names_the_entry_an_update_unmerges() {
        let mut hp = plan_for(
            &cat(),
            &Claude,
            &HostSnapshot::default(),
            &Manifest::default(),
            &secrets_map(),
        );
        let first = hp.actions[0].clone();
        hp.actions = vec![Action {
            op: ActionOp::Update,
            catalog: Some("acme".into()),
            remove_entry: Some(ManifestEntry {
                catalog: "prev".into(),
                ..Default::default()
            }),
            ..first
        }];
        let id = registry_put(SyncPlan::new(vec![hp]));
        assert_eq!(
            registry_catalogs_written(&id),
            Some(BTreeSet::from(["acme".to_string(), "prev".to_string()]))
        );
        assert!(registry_take(&id).is_some());
    }

    fn host_plan(alias: &str) -> HostPlan {
        HostPlan {
            host_alias: alias.into(),
            harness: "codex".into(),
            status: "planned".into(),
            detail: None,
            actions: Vec::new(),
            snapshot: HostSnapshot::default(),
            manifest: Manifest::default(),
        }
    }

    #[test]
    fn registry_drop_host_drops_only_the_plans_covering_that_host() {
        let gone = format!("drop-me-{}", uuid::Uuid::new_v4());
        let kept = format!("keep-me-{}", uuid::Uuid::new_v4());
        let both = registry_put(SyncPlan::new(vec![host_plan(&kept), host_plan(&gone)]));
        let only = registry_put(SyncPlan::new(vec![host_plan(&gone)]));
        let other = registry_put(SyncPlan::new(vec![host_plan(&kept)]));
        assert_eq!(registry_drop_host(&gone), 2);
        assert!(registry_take(&both).is_none());
        assert!(registry_take(&only).is_none());
        assert!(registry_take(&other).is_some(), "another host's plan stays");
        assert_eq!(registry_drop_host(&gone), 0);
    }

    #[test]
    fn registry_put_existing_keeps_the_original_deadline() {
        // A short TTL so the test can prove the deadline was NOT restarted
        // without sleeping for the real 10-minute `PLAN_TTL`.
        let id = registry_put_with_ttl(
            SyncPlan::new(vec![plan_for(
                &cat(),
                &Claude,
                &HostSnapshot::default(),
                &Manifest::default(),
                &secrets_map(),
            )]),
            Duration::from_millis(50),
        );
        let (expires_at, plan) = registry_take_with_expiry(&id).expect("plan still live");
        // Put it back, as `sync_apply` does on a refused (missing-secret)
        // apply — the plan was not touched, so its deadline must be
        // unchanged, not pushed out to `now + PLAN_TTL`.
        registry_put_existing(&id, expires_at, plan);
        std::thread::sleep(Duration::from_millis(80));
        assert!(
            registry_take(&id).is_none(),
            "the original (short) deadline must still apply after registry_put_existing"
        );
    }

    #[test]
    fn an_expired_plan_is_not_handed_back() {
        let id = registry_put_with_ttl(SyncPlan::new(Vec::new()), Duration::ZERO);
        // A second expired plan nobody ever asks for: taking *any* id must
        // sweep it out too, or a session that computes plans and never
        // applies them pins every host snapshot it ever rendered.
        let abandoned = registry_put_with_ttl(SyncPlan::new(Vec::new()), Duration::ZERO);
        assert!(registry_take(&id).is_none());
        assert!(
            !plans().contains_key(&abandoned),
            "an expired plan is dropped, not left pinning its host snapshots"
        );
    }

    /// Carry 2 (Rulings R24): an entry whose catalog speaks but whose own
    /// file is a load problem is kept with a `Noop`, never removed; so is
    /// every entry of a kind whose directory could not be read. A sibling
    /// the catalog really dropped is still removed.
    #[test]
    fn an_asset_the_catalog_could_not_read_is_kept_not_removed() {
        let mut manifest = Manifest::default();
        for name in ["broken", "gone"] {
            manifest.assets.insert(
                format!("skill/{name}"),
                ManifestEntry {
                    files: vec![format!("~/.claude/skills/{name}/SKILL.md")],
                    ..Default::default()
                },
            );
        }
        manifest
            .assets
            .insert("agent/x".into(), ManifestEntry::default());
        let holds = ProblemHolds::from_problems(&[
            Problem {
                path: "skills/broken/asset.yaml".into(),
                message: "bad yaml".into(),
            },
            Problem {
                path: "agents".into(),
                message: "permission denied".into(),
            },
        ]);
        let keep = KeepRules {
            speaks_for: Some(BTreeSet::from(["personal".to_string()])),
            problem_held: BTreeMap::from([("personal".to_string(), holds)]),
            ..Default::default()
        };
        let hp = compute_host_plan(
            &Catalog::default(),
            &Claude,
            "local",
            &HostSnapshot::default(),
            &manifest,
            &secrets_map(),
            &PlanFilter::default(),
            &keep,
        );
        assert_eq!(act(&hp, "gone").op, ActionOp::Remove);
        let broken = act(&hp, "broken");
        assert_eq!(broken.op, ActionOp::Noop);
        assert!(
            broken.reason.as_deref().unwrap().contains("bad yaml"),
            "{:?}",
            broken.reason
        );
        assert!(broken.remove_entry.is_none());
        assert_eq!(
            act(&hp, "x").op,
            ActionOp::Noop,
            "an unreadable kind dir holds its entries"
        );
    }

    /// F3c: with `~/.agents/skills` a symlink, every Codex action that would
    /// write or adopt under it is refused with the reason; Codex's MCP merge
    /// (`~/.codex/config.toml`) and Claude's plan are unaffected.
    #[test]
    fn writes_through_a_symlinked_skills_dir_are_blocked() {
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.agents/skills".into(), "/home/u/.claude/skills".into());
        let hp = plan_for(
            &catalog_of(&[SKILL, MCP]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let s = act(&hp, "s");
        assert_eq!(s.op, ActionOp::Blocked);
        assert_eq!(
            s.reason.as_deref(),
            Some("~/.agents/skills is a symlink (to /home/u/.claude/skills); fleet won't write Codex skills through it — replace it with a real directory or turn Codex off for this host")
        );
        assert!(s.plan.is_none() && s.remove_entry.is_none() && !s.backup);
        assert_eq!(
            act(&hp, "fleet").op,
            ActionOp::Create,
            "config.toml is not under the link"
        );

        // Adopting is refused too: Claude's copy of a plain skill is
        // byte-identical to Codex's render, and both manifests would claim it.
        let mut identical = snap.clone();
        satisfy(
            &mut identical,
            &substituted(&Codex, &asset(SKILL), &secrets_map()),
        );
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &identical,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Blocked);

        // Claude never writes under ~/.agents: its plan is unchanged.
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Create);
    }

    /// The outermost link names the reason; a linked single skill blocks
    /// only that skill; a sibling path that merely shares a prefix is not
    /// under the link.
    #[test]
    fn the_outermost_link_names_the_reason_and_unlinked_skills_still_plan() {
        const T: &str = "kind: skill\nname: t\ndescription: d\n";
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.agents/skills/s".into(), "/x/s".into());
        snap.links.insert("~/.agent".into(), "/not/a/parent".into());
        let hp = plan_for(
            &catalog_of(&[SKILL, T]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Blocked);
        assert!(act(&hp, "s")
            .reason
            .as_deref()
            .unwrap()
            .starts_with("~/.agents/skills/s is a symlink (to /x/s);"));
        assert_eq!(act(&hp, "t").op, ActionOp::Create);

        snap.links
            .insert("~/.agents".into(), "/dotfiles/agents".into());
        let hp = plan_for(
            &catalog_of(&[SKILL, T]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        for name in ["s", "t"] {
            assert!(
                act(&hp, name)
                    .reason
                    .as_deref()
                    .unwrap()
                    .starts_with("~/.agents is a symlink (to /dotfiles/agents);"),
                "{name}"
            );
        }
    }

    /// F3c: a symlinked legacy `~/.codex/skills` blocks the removal of a
    /// manifest-listed old copy — it may be Claude's own file — with advice
    /// that does not suggest turning Codex off (a retiring host still
    /// removes).
    #[test]
    fn removing_an_old_copy_through_a_symlinked_legacy_dir_is_blocked() {
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.codex/skills".into(), "/home/u/.claude/skills".into());
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/gone".into(),
            ManifestEntry {
                hash: "h".into(),
                files: vec!["~/.codex/skills/gone/SKILL.md".into()],
                ..Default::default()
            },
        );
        let hp = plan_for(
            &Catalog::default(),
            &Codex,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "gone");
        assert_eq!(a.op, ActionOp::Blocked);
        assert_eq!(
            a.reason.as_deref(),
            Some("~/.codex/skills is a symlink (to /home/u/.claude/skills); fleet won't remove old Codex skill copies through it — replace it with a real directory")
        );
    }

    /// F3c: a Codex skill fleet synced to `~/.codex/skills` before F3c,
    /// planned by what the new `~/.agents/skills` location already holds:
    /// nothing ⇒ `Create` that deletes the old copy; an identical copy ⇒
    /// `Update` that deletes the old copy; a different copy fleet never
    /// wrote ⇒ `Overwrite` (backed up), never a silent `Update`. Without a
    /// manifest entry the ordinary rules hold: identical ⇒ `Adopt`,
    /// different ⇒ `Overwrite("present but differs; not managed")`.
    #[test]
    fn a_codex_skill_moving_to_agents_skills_plans_by_what_the_new_location_holds() {
        let new_plan = substituted(&Codex, &asset(SKILL), &secrets_map());
        let new_path = new_plan.files[0].path.clone();
        assert_eq!(new_path, "~/.agents/skills/s/SKILL.md");
        let old_path = "~/.codex/skills/s/SKILL.md".to_string();
        let mut old_plan = new_plan.clone();
        old_plan.files[0].path = old_path.clone();
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old_plan.hash(), &old_plan, 0, "personal"),
        );
        let mut old_only = HostSnapshot::default();
        satisfy(&mut old_only, &old_plan);
        let catalog = catalog_of(&[SKILL]);
        let old_files = |a: &Action| a.remove_entry.as_ref().map(|e| e.files.clone());

        // Nothing at the new location yet: the normal migration.
        let hp = plan_for(&catalog, &Codex, &old_only, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Create);
        assert_eq!(a.reason.as_deref(), Some(MOVED_CREATE_REASON));
        assert_eq!(a.files, vec![new_path.clone()]);
        assert_eq!(old_files(a), Some(vec![old_path.clone()]));
        assert_eq!(a.expected[&new_path], None);

        // An identical copy is already there.
        let mut identical = old_only.clone();
        satisfy(&mut identical, &new_plan);
        let hp = plan_for(&catalog, &Codex, &identical, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Update);
        assert_eq!(a.reason.as_deref(), Some(MOVED_UPDATE_REASON));
        assert_eq!(old_files(a), Some(vec![old_path.clone()]));

        // A diverged hand-made copy is there: an overwrite, backed up.
        let mut diverged = old_only.clone();
        diverged.files.insert(new_path.clone(), "edited".into());
        let hp = plan_for(&catalog, &Codex, &diverged, &manifest, &secrets_map());
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(a.reason.as_deref(), Some(MOVED_ONTO_FOREIGN_REASON));
        assert!(a.backup);
        assert_eq!(old_files(a), Some(vec![old_path.clone()]));

        // Never managed: the ordinary unmanaged rules.
        let mut fresh_identical = HostSnapshot::default();
        satisfy(&mut fresh_identical, &new_plan);
        let hp = plan_for(
            &catalog,
            &Codex,
            &fresh_identical,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Adopt);
        let mut fresh_diverged = HostSnapshot::default();
        fresh_diverged
            .files
            .insert(new_path.clone(), "edited".into());
        let hp = plan_for(
            &catalog,
            &Codex,
            &fresh_diverged,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(
            a.reason.as_deref(),
            Some("present but differs; not managed")
        );
    }

    /// I1 (F3c review fix round 1): a *per-entry* symlink in the legacy
    /// dir — not the whole `~/.codex/skills`, just one skill's old
    /// location — blocks the migration's removal exactly like the whole
    /// directory would: the compare-and-swap hash would otherwise pass
    /// right through the link and delete whatever it really points at
    /// (possibly Claude's own file).
    #[test]
    fn a_per_entry_symlink_in_the_legacy_dir_blocks_the_move_with_the_legacy_reason() {
        let mut snap = HostSnapshot::default();
        snap.links.insert(
            "~/.codex/skills/s".into(),
            "/home/u/.claude/skills/s".into(),
        );
        let old_plan = {
            let mut p = substituted(&Codex, &asset(SKILL), &secrets_map());
            p.files[0].path = "~/.codex/skills/s/SKILL.md".into();
            p
        };
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old_plan.hash(), &old_plan, 0, "personal"),
        );
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Blocked, "{:?}", a.reason);
        assert_eq!(
            a.reason.as_deref(),
            Some("~/.codex/skills/s is a symlink (to /home/u/.claude/skills/s); fleet won't remove old Codex skill copies through it — replace it with a real directory")
        );
    }

    /// I2 (F3c review fix round 1): `touched_paths`' `remove_entry` half —
    /// untested until this round — must itself be checked against `links`:
    /// a `Create` whose `remove_entry` names the OLD (`~/.codex/skills`)
    /// path is blocked when that whole directory is linked, even though
    /// the NEW path it is about to write is not (nothing is at the new
    /// location). Fails if the `.chain(action.remove_entry…)` half of
    /// `touched_paths` is ever deleted.
    #[test]
    fn a_create_is_blocked_through_its_remove_entrys_old_path() {
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.codex/skills".into(), "/home/u/.claude/skills".into());
        let old_plan = {
            let mut p = substituted(&Codex, &asset(SKILL), &secrets_map());
            p.files[0].path = "~/.codex/skills/s/SKILL.md".into();
            p
        };
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old_plan.hash(), &old_plan, 0, "personal"),
        );
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Blocked, "{:?}", a.reason);
        assert_eq!(
            a.reason.as_deref(),
            Some("~/.codex/skills is a symlink (to /home/u/.claude/skills); fleet won't remove old Codex skill copies through it — replace it with a real directory")
        );
        assert!(a.remove_entry.is_none());
        assert!(a.plan.is_none());
    }

    /// M3 (F3c review fix round 1): `linked_dir` compares with ASCII
    /// case-folding, since a case-insensitive filesystem (common on macOS)
    /// can report `readlink`'s path in different case than the catalog's
    /// rendered path.
    #[test]
    fn linked_dir_matches_case_insensitively() {
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.AGENTS/SKILLS".into(), "/home/u/.claude/skills".into());
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Blocked, "{:?}", a.reason);
        assert!(a
            .reason
            .as_deref()
            .unwrap()
            .starts_with("~/.AGENTS/SKILLS is a symlink"));
    }

    /// M5 (F3c review fix round 1): a `Remove` (an orphaned manifest entry
    /// at the CURRENT `~/.agents/skills` location) reads differently from
    /// every other blocked op — nothing is being written, so "remove"
    /// replaces "write", and there is no "or turn Codex off" (turning
    /// Codex off would not un-block a removal that has to happen anyway).
    #[test]
    fn a_blocked_removal_through_the_current_location_uses_remove_wording() {
        let mut snap = HostSnapshot::default();
        snap.links
            .insert("~/.agents/skills".into(), "/home/u/.claude/skills".into());
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/gone".into(),
            ManifestEntry {
                hash: "h".into(),
                files: vec!["~/.agents/skills/gone/SKILL.md".into()],
                ..Default::default()
            },
        );
        let hp = plan_for(
            &Catalog::default(),
            &Codex,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "gone");
        assert_eq!(a.op, ActionOp::Blocked, "{:?}", a.reason);
        assert_eq!(
            a.reason.as_deref(),
            Some("~/.agents/skills is a symlink (to /home/u/.claude/skills); fleet won't remove Codex skills through it — replace it with a real directory")
        );
    }

    fn planned(harness: &str, actions: Vec<Action>) -> HostPlan {
        HostPlan {
            host_alias: "h".into(),
            harness: harness.into(),
            status: "planned".into(),
            detail: None,
            actions,
            snapshot: HostSnapshot::default(),
            manifest: Manifest::default(),
        }
    }

    fn file_action(name: &str, op: ActionOp, path: &str) -> Action {
        Action {
            kind: "skill".into(),
            name: name.into(),
            op,
            catalog: None,
            reason: None,
            files: vec![path.into()],
            merges: Vec::new(),
            backup: false,
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: None,
            expected: BTreeMap::new(),
            secret_files: BTreeSet::new(),
            remove_entry: None,
            plugin: None,
        }
    }

    /// F3c (future-proofing for harnesses sharing `~/.agents/skills`): two
    /// harnesses on one host writing one file are both blocked, each naming
    /// the other; actions on paths nobody else touches are left alone.
    #[test]
    fn two_harnesses_writing_one_path_are_both_blocked() {
        let shared = "~/.agents/skills/s/SKILL.md";
        let mut plans = vec![
            planned(
                "codex",
                vec![
                    file_action("s", ActionOp::Create, shared),
                    file_action("t", ActionOp::Create, "~/.agents/skills/t/SKILL.md"),
                ],
            ),
            planned("gemini", vec![file_action("s", ActionOp::Update, shared)]),
            planned(
                "claude",
                vec![file_action(
                    "s",
                    ActionOp::Create,
                    "~/.claude/skills/s/SKILL.md",
                )],
            ),
        ];
        block_cross_harness_collisions(&mut plans);
        assert_eq!(plans[0].actions[0].op, ActionOp::Blocked);
        assert_eq!(
            plans[0].actions[0].reason.as_deref(),
            Some("~/.agents/skills/s/SKILL.md is also managed by the gemini plan on this host; fleet won't let two harnesses write one file")
        );
        assert_eq!(plans[1].actions[0].op, ActionOp::Blocked);
        assert!(plans[1].actions[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("by the codex plan"));
        assert_eq!(plans[0].actions[1].op, ActionOp::Create);
        assert_eq!(plans[2].actions[0].op, ActionOp::Create);
    }

    /// A `Noop` claims the file it manages (it stays a `Noop`); a removal
    /// claims its entry's files through `remove_entry`; an already-blocked
    /// action claims nothing.
    #[test]
    fn noops_and_removals_claim_their_files_and_blocked_actions_do_not() {
        let managed = "~/.agents/skills/s/SKILL.md";
        let mut moving = file_action("s", ActionOp::Create, "~/.agents/skills/s2/SKILL.md");
        moving.remove_entry = Some(ManifestEntry {
            files: vec!["~/.agents/skills/old/SKILL.md".into()],
            ..Default::default()
        });
        let mut plans = vec![
            planned(
                "codex",
                vec![
                    file_action("s", ActionOp::Noop, managed),
                    file_action("b", ActionOp::Blocked, "~/.agents/skills/b/SKILL.md"),
                ],
            ),
            planned(
                "gemini",
                vec![
                    file_action("s", ActionOp::Create, managed),
                    moving,
                    file_action("b", ActionOp::Create, "~/.agents/skills/b/SKILL.md"),
                ],
            ),
            planned(
                "agy",
                vec![file_action(
                    "old",
                    ActionOp::Remove,
                    "~/.agents/skills/old/SKILL.md",
                )],
            ),
        ];
        block_cross_harness_collisions(&mut plans);
        assert_eq!(plans[0].actions[0].op, ActionOp::Noop);
        assert_eq!(plans[1].actions[0].op, ActionOp::Blocked);
        assert_eq!(
            plans[1].actions[1].op,
            ActionOp::Blocked,
            "its old file is agy's removal"
        );
        assert_eq!(plans[2].actions[0].op, ActionOp::Blocked);
        assert_eq!(
            plans[1].actions[2].op,
            ActionOp::Create,
            "a blocked action claims nothing"
        );
    }

    /// F3c final review (I1): on a case-insensitive filesystem a host can
    /// hold `~/.agents/skills/x/skill.md` where fleet renders `SKILL.md`.
    /// The exact-case lookup reads the file as absent, so a `Create` would
    /// expect nothing there while the applier's compare-and-swap finds the
    /// file and conflicts on every sync. Blocked instead, naming the
    /// on-disk path — for every harness, and a symlink block still wins.
    #[test]
    fn a_file_differing_only_in_letter_case_is_blocked_not_created() {
        const X: &str = "kind: skill\nname: x\ndescription: d\n";
        for (harness, dir) in [
            (&Codex as &dyn Harness, "~/.agents/skills"),
            (&Claude as &dyn Harness, "~/.claude/skills"),
        ] {
            let rendered = substituted(harness, &asset(X), &secrets_map());
            assert_eq!(rendered.files[0].path, format!("{dir}/x/SKILL.md"));
            let on_disk = format!("{dir}/x/skill.md");
            let mut snap = HostSnapshot::default();
            snap.files.insert(on_disk.clone(), "h".into());
            let hp = plan_for(
                &catalog_of(&[X]),
                harness,
                &snap,
                &Manifest::default(),
                &secrets_map(),
            );
            let a = act(&hp, "x");
            assert_eq!(a.op, ActionOp::Blocked, "{}: {:?}", harness.id(), a.reason);
            assert_eq!(
                a.reason.clone().unwrap(),
                format!(
                    "{on_disk} differs from the rendered SKILL.md only in letter case; rename it to SKILL.md or remove it"
                )
            );
            assert!(a.plan.is_none() && a.remove_entry.is_none() && !a.backup);
        }

        // A case variant in a directory component names the whole path.
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.agents/skills/X/SKILL.md".into(), "h".into());
        let hp = plan_for(
            &catalog_of(&[X]),
            &Codex,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(
            act(&hp, "x").reason.as_deref(),
            Some("~/.agents/skills/X/SKILL.md differs from the rendered ~/.agents/skills/x/SKILL.md only in letter case; rename it to ~/.agents/skills/x/SKILL.md or remove it")
        );

        // A symlink block still wins.
        let mut linked = HostSnapshot::default();
        linked
            .files
            .insert("~/.agents/skills/x/skill.md".into(), "h".into());
        linked
            .links
            .insert("~/.agents/skills".into(), "/home/u/.claude/skills".into());
        let hp = plan_for(
            &catalog_of(&[X]),
            &Codex,
            &linked,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "x");
        assert_eq!(a.op, ActionOp::Blocked);
        assert!(a
            .reason
            .as_deref()
            .unwrap()
            .starts_with("~/.agents/skills is a symlink"));
    }

    /// The case check only fires when the exact path is absent: an
    /// exact-case file plans as before (`Adopt` when identical, `Overwrite`
    /// when different), even beside a case variant (a case-sensitive
    /// filesystem holding both), and a `Noop` is never touched.
    #[test]
    fn an_exact_case_file_still_adopts_or_overwrites() {
        let rendered = substituted(&Codex, &asset(SKILL), &secrets_map());
        let path = rendered.files[0].path.clone();
        let mut identical = HostSnapshot::default();
        satisfy(&mut identical, &rendered);
        identical
            .files
            .insert(path.to_ascii_lowercase(), "other".into());
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &identical,
            &Manifest::default(),
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Adopt);

        let mut diverged = HostSnapshot::default();
        diverged.files.insert(path.clone(), "edited".into());
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &diverged,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Overwrite);
        assert_eq!(
            a.reason.as_deref(),
            Some("present but differs; not managed")
        );

        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&rendered.hash(), &rendered, 0, "personal"),
        );
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &identical,
            &manifest,
            &secrets_map(),
        );
        assert_eq!(act(&hp, "s").op, ActionOp::Noop);
    }

    /// A moved asset (F3c migration) whose new location holds a case
    /// variant is Blocked, not a `Create` whose `remove_entry` deletes the
    /// old copy: nothing is written or deleted until a person resolves it.
    #[test]
    fn a_moved_skill_onto_a_case_variant_is_blocked_and_deletes_nothing() {
        let new_plan = substituted(&Codex, &asset(SKILL), &secrets_map());
        let mut old_plan = new_plan.clone();
        old_plan.files[0].path = "~/.codex/skills/s/SKILL.md".into();
        let mut manifest = Manifest {
            version: 1,
            ..Default::default()
        };
        manifest.assets.insert(
            "skill/s".into(),
            Manifest::entry_for(&old_plan.hash(), &old_plan, 0, "personal"),
        );
        let mut snap = HostSnapshot::default();
        satisfy(&mut snap, &old_plan);
        snap.files
            .insert("~/.agents/skills/s/skill.md".into(), "h".into());
        let hp = plan_for(
            &catalog_of(&[SKILL]),
            &Codex,
            &snap,
            &manifest,
            &secrets_map(),
        );
        let a = act(&hp, "s");
        assert_eq!(a.op, ActionOp::Blocked, "{:?}", a.reason);
        assert_eq!(
            a.reason.as_deref(),
            Some("~/.agents/skills/s/skill.md differs from the rendered SKILL.md only in letter case; rename it to SKILL.md or remove it")
        );
        assert!(a.remove_entry.is_none() && a.plan.is_none() && !a.backup);
    }
}
