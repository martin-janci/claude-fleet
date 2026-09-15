//! Sync plan computation: diff the catalog against one host's snapshot and
//! managed manifest and decide, per asset, exactly what the sync engine
//! would do — plus a short-lived in-process registry so a computed plan can
//! be reviewed (`sync_plan`) and then applied (`sync_apply`) by id without
//! recomputing it against a host that may have changed underneath.
//!
//! Everything here is reserved for the sync engine's command layer (Task 7);
//! the per-item `#[allow(dead_code)]` markers below come off once a Tauri
//! command / MCP tool calls into this module.

use super::super::harness::{json_get, ConfigMerge, Harness, HostSnapshot, MergeMode, RenderPlan};
use super::super::inventory::merge_satisfied;
use super::super::model::{sha256_hex, Asset, AssetSpec, Kind};
use super::super::repo::Catalog;
use super::manifest::{Manifest, ManifestEntry};
use super::secrets::{self, SecretPlan};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What the sync engine would do to one asset on one host.
#[allow(dead_code)]
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
    PluginUpdate,
    /// Nothing to do.
    Noop,
    /// Cannot be planned (unsupported kind, unresolved secret, unpinnable
    /// plugin version); `reason` says why.
    Blocked,
}

#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub kind: String,
    pub name: String,
    pub op: ActionOp,
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
    /// The substituted plan to write. `None` for `Remove`/`Noop`/`Blocked`
    /// and for the plugin ops (which shell out to the harness CLI).
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
    /// The manifest entry a `Remove` undoes.
    #[serde(skip)]
    pub remove_entry: Option<ManifestEntry>,
    #[serde(skip)]
    pub plugin: Option<PluginTarget>,
}

/// Every action planned for one host under one harness. `snapshot` and
/// `manifest` are the exact inputs the actions were computed from, kept so
/// the applier can re-check and rewrite the manifest without re-scanning.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
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
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub struct SyncPlan {
    pub id: String,
    pub computed_at: i64,
    pub hosts: Vec<HostPlan>,
    /// `ActionOp::as_str()` → how many actions carry it (zero ops absent).
    pub counts: BTreeMap<String, usize>,
}

#[allow(dead_code)]
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
/// `kind` and `name` only.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct PlanFilter {
    pub host_alias: Option<String>,
    pub kind: Option<Kind>,
    pub name: Option<String>,
}

impl PlanFilter {
    fn matches(&self, kind: Kind, name: &str) -> bool {
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
///    object. Present and *matching* (a pinned ref whose installed version
///    equals the pin, or a `latest` ref whose record carries no version at
///    all) ⇒ `Noop` if the manifest names it, else `Adopt`. Present but not
///    matching ⇒ `PluginUpdate` for a `latest` ref (the host is pinned to a
///    concrete version; re-installing tracks latest again) and `Blocked` for
///    a pinned ref (the CLI cannot install a specific version). Absent ⇒
///    `PluginInstall`.
/// 4. Every other kind, against the *substituted* plan: `present` when at
///    least one planned file exists in the snapshot or at least one merge's
///    `json_path` resolves (an `AppendUnique` merge points at a shared
///    per-event array, so it counts as present only once its own value is
///    in it — same rule as `inventory::compute_states`); `matches` when
///    every planned file exists with hash `sha256_hex(bytes)` *and* every
///    merge is `merge_satisfied`. Then: not present ⇒ `Create`; present and
///    matching ⇒ `Noop` if the manifest names it, else `Adopt`; present and
///    differing ⇒ `Update` when the manifest names it with a *different*
///    hash (the catalog moved on), `Overwrite("edited on host")` when the
///    manifest names it with the *same* hash (so the difference came from
///    the host), and `Overwrite("present but differs; not managed")` when
///    the manifest does not name it at all.
/// 5. `expected` records the scanned hash of every planned file and every
///    merged config file (`None` when the scan did not see it).
/// 6. `manifest.orphans(catalog)` ⇒ `Remove`, with `files`/`merges`/
///    `remove_entry` taken from the manifest entry. A manifest key
///    `Manifest::split_key` cannot parse is skipped (it names no kind, so
///    there is nothing to filter or display it as) and logged.
/// 7. `backup` is true whenever a file that exists on the host right now
///    would be replaced (`Update`/`Overwrite`) or deleted (`Remove`).
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
#[allow(dead_code)]
pub fn compute_host_plan(
    catalog: &Catalog,
    harness: &dyn Harness,
    host_alias: &str,
    snap: &HostSnapshot,
    manifest: &Manifest,
    secrets: &BTreeMap<String, String>,
    filter: &PlanFilter,
) -> HostPlan {
    let mut actions = Vec::new();
    for asset in &catalog.assets {
        if !filter.matches(asset.kind(), &asset.header.name) {
            continue;
        }
        actions.push(action_for(harness, asset, snap, manifest, secrets));
    }
    for (key, entry) in manifest.orphans(catalog) {
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
        actions.push(Action {
            kind: kind.as_str().to_string(),
            name,
            op: ActionOp::Remove,
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

    let in_manifest = manifest.assets.contains_key(&Manifest::key(kind, &name));

    // Rule 3.
    if let Some(target) = &plugin {
        let (op, reason) = plugin_op(plan, snap, target, in_manifest);
        return Action {
            op,
            reason,
            files,
            merges,
            secrets: resolved,
            expected,
            secret_files,
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
        (ActionOp::Create, None)
    } else if matches {
        (
            if in_manifest {
                ActionOp::Noop
            } else {
                ActionOp::Adopt
            },
            None,
        )
    } else {
        match manifest.assets.get(&Manifest::key(kind, &name)) {
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

    // Rule 7.
    let replaces = matches!(op, ActionOp::Update | ActionOp::Overwrite);
    let backup = replaces && plan.files.iter().any(|f| snap.files.contains_key(&f.path));

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
        ..blank()
    }
}

/// Rule 3's decision, split out to keep `action_for` readable. `plan` is the
/// substituted plan; a plugin ref renders exactly one `Subset` merge.
fn plugin_op(
    plan: &RenderPlan,
    snap: &HostSnapshot,
    target: &PluginTarget,
    in_manifest: bool,
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
    let installed: Option<String> = snap
        .configs
        .get(&merge.file)
        .and_then(|root| json_get(root, &merge.json_path))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|rec| rec.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let tracks_latest = target.version == "latest";
    let matches = if tracks_latest {
        installed.is_none()
    } else {
        installed.as_deref() == Some(target.version.as_str())
    };
    if matches {
        return (
            if in_manifest {
                ActionOp::Noop
            } else {
                ActionOp::Adopt
            },
            None,
        );
    }
    let have = installed.as_deref().unwrap_or("an unknown version");
    if tracks_latest {
        (
            ActionOp::PluginUpdate,
            Some(format!("installed {have}; the catalog tracks latest")),
        )
    } else {
        (
            ActionOp::Blocked,
            Some(format!(
                "installed {have}, catalog pins {}; the CLI cannot pin versions",
                target.version
            )),
        )
    }
}

/// Tally every host's actions by `ActionOp::as_str()`. Ops with no actions
/// are absent rather than zero.
#[allow(dead_code)]
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
static PLANS: Lazy<Mutex<HashMap<String, (Instant, SyncPlan)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn plans() -> std::sync::MutexGuard<'static, HashMap<String, (Instant, SyncPlan)>> {
    // A poisoned registry is not worth failing a sync over: the map holds
    // plain data, and a panic mid-insert leaves it consistent.
    PLANS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stash `plan` under a fresh uuid (also written into `plan.id`) for
/// `PLAN_TTL`, dropping any already-expired plans on the way in. Returns the
/// id to hand back to the caller.
#[allow(dead_code)]
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
/// such plan or it has expired.
#[allow(dead_code)]
pub fn registry_take(id: &str) -> Option<SyncPlan> {
    let (expires_at, plan) = plans().remove(id)?;
    (expires_at > Instant::now()).then_some(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::claude::Claude;
    use crate::service::catalog::harness::codex::Codex;
    use crate::service::catalog::harness::{apply_merges, ManifestMerge};
    use crate::service::catalog::model::Asset;

    const SKILL: &str = "kind: skill\nname: s\ndescription: d\n";
    const HOOK: &str =
        "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n";
    const MCP: &str = "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: https://example/mcp\nheaders:\n  Authorization: \"Bearer ${FLEET_MCP_TOKEN}\"\n";
    const PLUGIN: &str = "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: mk, source: github, repo: o/r }\nplugin: sp\nversion: \"6.3.0\"\n";
    const PLUGIN_LATEST: &str = "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: mk, source: github, repo: o/r }\nplugin: sp\nversion: latest\n";

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
            .into_inner()
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
        // The plugin action names its target, never a file.
        let sp = act(&hp, "sp");
        assert!(sp.files.is_empty());
        assert!(sp.plan.is_none(), "plugin ops shell out; no plan to write");
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

    #[test]
    fn plugin_ops_install_update_and_block_on_a_pin() {
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

        // Installed at another version, catalog pins one: unpinnable.
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            crate::service::catalog::harness::claude::PLUGINS_PATH.into(),
            json!({"plugins": {"sp@mk": [{"version": "5.0.0"}]}}),
        );
        let hp = plan_for(
            &catalog,
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "sp");
        assert_eq!(a.op, ActionOp::Blocked);
        let reason = a.reason.as_deref().unwrap();
        assert!(reason.contains("installed 5.0.0"), "{reason}");
        assert!(reason.contains("cannot pin versions"), "{reason}");

        // Same host, but the catalog tracks latest: re-install to catch up.
        let hp = plan_for(
            &catalog_of(&[PLUGIN_LATEST]),
            &Claude,
            &snap,
            &Manifest::default(),
            &secrets_map(),
        );
        let a = act(&hp, "sp");
        assert_eq!(a.op, ActionOp::PluginUpdate);
        assert!(a.reason.as_deref().unwrap().contains("tracks latest"));

        // A record with no version at all is what "latest" looks like.
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
        );
        assert_eq!(hp.actions.len(), 1);
        assert_eq!(hp.actions[0].name, "fleet");
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

    #[test]
    fn an_expired_plan_is_not_handed_back() {
        let plan = SyncPlan::new(Vec::new());
        let id = registry_put_with_ttl(plan, Duration::ZERO);
        assert!(registry_take(&id).is_none());
    }
}
