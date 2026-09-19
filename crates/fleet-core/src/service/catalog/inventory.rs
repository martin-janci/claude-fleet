//! Host inventory: run a harness scan on a host, compare against rendered
//! catalog assets, persist per-asset drift states.

use super::harness::{json_get, ConfigMerge, Harness, HostSnapshot, MergeMode};
use super::model::{sha256_hex, Kind};
use super::repo::Catalog;
use super::sync::manifest::Manifest;
use crate::ipc_error::codes;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::AssetInventoryRow;
use crate::store::Store;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const SCAN_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, serde::Serialize)]
pub struct HostScanResult {
    pub host: String,
    /// "scanned" | "skipped" | "failed"
    pub status: String,
    pub detail: Option<String>,
    pub rows: usize,
}

/// Build an `E_SCAN` error from a non-zero exit `Output`. A failed scan
/// script must never be treated as an empty (successful) snapshot — that
/// would read as "every catalog asset is missing" on the host.
fn scan_failed(host: &str, out: &std::process::Output) -> crate::ipc_error::IpcError {
    let code = out
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string());
    crate::ipc_error::IpcError::new(
        codes::E_SCAN,
        format!(
            "{host}: scan script exited {code}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    )
}

/// Run a bash script on a host and return stdout. `local` runs in-process;
/// remote hosts go through the SSH multiplexer with the whole script as one
/// quoted `bash -lc` word (ssh space-joins argv). A non-zero exit status is
/// an error, not an empty snapshot: `tokio::process::Command::output` and
/// `SshClient::run` both return `Ok` on a non-zero exit, so the status must
/// be checked explicitly here or a script that fails partway through would
/// silently read back as "nothing installed".
pub async fn run_host_script(
    ssh: &Arc<SshClient>,
    host: &str,
    script: &str,
) -> Result<String, crate::ipc_error::IpcError> {
    run_host_script_with(
        ssh,
        host,
        script,
        SshClient::default_wall_clock(SCAN_TIMEOUT),
        &CancellationToken::new(),
    )
    .await
}

/// [`run_host_script`] with an explicit wall-clock bound and a cancellation
/// token. The applier needs both: writing a host's assets legitimately takes
/// longer than a scan, and a sync the user cancelled must stop between (and
/// during) its scripts rather than run to completion.
///
/// Local: the child is killed when the future is dropped (`kill_on_drop`,
/// whose reaping tokio's process driver handles) and the timeout is applied
/// around `output()`, which drains both pipes so a chatty script cannot
/// deadlock on a full pipe buffer. Remote: straight to
/// `SshClient::run_bounded_cancellable`, which kills and reaps the ssh child
/// itself.
pub async fn run_host_script_with(
    ssh: &Arc<SshClient>,
    host: &str,
    script: &str,
    wall_clock: Duration,
    token: &CancellationToken,
) -> Result<String, crate::ipc_error::IpcError> {
    if token.is_cancelled() {
        return Err(crate::ipc_error::IpcError::new(
            codes::E_CANCELLED,
            format!("{host}: cancelled"),
        ));
    }
    let out = if host == "local" {
        crate::service::hub::ensure_local_allowed(host)?;
        let mut cmd = tokio::process::Command::new("bash");
        cmd.args(["-lc", script]).kill_on_drop(true);
        tokio::select! {
            res = tokio::time::timeout(wall_clock, cmd.output()) => match res {
                Ok(out) => out.map_err(|e| {
                    crate::ipc_error::IpcError::new(codes::E_IO, format!("spawn bash: {e}"))
                })?,
                Err(_) => {
                    return Err(crate::ipc_error::IpcError::new(
                        codes::E_TIMEOUT,
                        format!("{host}: script did not finish within {}s", wall_clock.as_secs()),
                    ))
                }
            },
            _ = token.cancelled() => {
                return Err(crate::ipc_error::IpcError::new(
                    codes::E_CANCELLED,
                    format!("{host}: cancelled"),
                ))
            }
        }
    } else {
        let quoted = quote(script);
        ssh.run_bounded_cancellable(
            host,
            &["bash", "-lc", &quoted],
            SCAN_TIMEOUT,
            wall_clock,
            token.clone(),
        )
        .await?
    };
    if !out.status.success() {
        return Err(scan_failed(host, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Scan one host with one harness: run its scan script and parse the
/// output into a `HostSnapshot`. A harness with no scan script cannot be
/// inventoried at all, which is `E_ASSET_UNSUPPORTED` rather than an empty
/// snapshot (an empty snapshot would read as "every asset is missing").
pub async fn scan_host_harness(
    ssh: &Arc<SshClient>,
    host: &str,
    harness: &dyn Harness,
) -> Result<HostSnapshot, crate::ipc_error::IpcError> {
    let Some(script) = harness.scan_script() else {
        return Err(crate::ipc_error::IpcError::new(
            codes::E_ASSET_UNSUPPORTED,
            format!("{} cannot scan hosts", harness.id()),
        ));
    };
    let out = run_host_script(ssh, host, &script).await?;
    harness.parse_scan(&out)
}

/// Scan every non-hidden reachable host (or just `only_host`) with every
/// harness that supports scanning, persisting rows per (host, harness).
/// Per-host failures never abort the others (mirrors `provision_hosts`).
pub async fn scan_hosts(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    only_host: Option<&str>,
) -> Result<Vec<HostScanResult>, crate::ipc_error::IpcError> {
    let hosts = {
        let s = store
            .lock()
            .map_err(|_| crate::ipc_error::IpcError::lock())?;
        s.list_hosts()?
    };
    let catalog = {
        let g = super::CATALOG
            .read()
            .map_err(|_| crate::ipc_error::IpcError::new(codes::E_LOCK, "catalog lock poisoned"))?;
        g.clone().ok_or_else(|| {
            crate::ipc_error::IpcError::new(
                super::E_CATALOG_NOT_CONFIGURED,
                "catalog not loaded; call catalog_load",
            )
        })?
    };
    let mut results = Vec::new();
    for h in hosts {
        if h.hidden || only_host.is_some_and(|o| o != h.alias) {
            continue;
        }
        if h.alias != "local" && !h.reachable {
            results.push(HostScanResult {
                host: h.alias,
                status: "skipped".into(),
                detail: Some("unreachable".into()),
                rows: 0,
            });
            continue;
        }
        let mut total = 0usize;
        // Every per-harness failure is kept (not just the last): a host with
        // both a broken claude scan and a broken codex scan must report
        // both reasons, not silently drop the first.
        let mut failures: Vec<String> = Vec::new();
        // Resolved once per host, and BEFORE the scan await: `resolve` takes
        // the store lock internally, so it must never be called with a scan
        // in flight (`await` while holding the guard).
        let secrets = match super::sync::secrets::resolve(store, &h.alias) {
            Ok(v) => v,
            Err(e) => {
                results.push(HostScanResult {
                    host: h.alias,
                    status: "failed".into(),
                    detail: Some(format!("resolve secrets: {}", e.message)),
                    rows: 0,
                });
                continue;
            }
        };
        for harness in super::harness::all() {
            if harness.scan_script().is_none() {
                continue;
            }
            let scanned_at = super::now_secs();
            match scan_host_harness(ssh, &h.alias, harness.as_ref()).await {
                Ok(snap) => {
                    let manifest = Manifest::from_snapshot(&snap, harness.manifest_path());
                    let rows = compute_states(
                        &catalog,
                        harness.as_ref(),
                        &h.alias,
                        &snap,
                        &manifest,
                        &secrets,
                        scanned_at,
                    );
                    total += rows.len();
                    match store.lock() {
                        Ok(s) => {
                            if let Err(e) = s.replace_host_inventory(&h.alias, harness.id(), &rows)
                            {
                                failures.push(format!("persist inventory: {e}"));
                            }
                        }
                        Err(_) => failures.push(format!(
                            "{}: store mutex poisoned while persisting inventory",
                            harness.id()
                        )),
                    }
                }
                Err(e) => {
                    failures.push(format!("{}: {}", harness.id(), e.message));
                }
            }
        }
        results.push(if failures.is_empty() {
            HostScanResult {
                host: h.alias,
                status: "scanned".into(),
                detail: None,
                rows: total,
            }
        } else {
            HostScanResult {
                host: h.alias,
                status: "failed".into(),
                detail: Some(failures.join("; ")),
                rows: total,
            }
        });
    }
    Ok(results)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetState {
    InSync,
    Drifted,
    Missing,
    Unmanaged,
    Unsupported,
    /// The host's fleet manifest names an asset the catalog no longer has;
    /// the next sync would remove it.
    Orphan,
}

impl AssetState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AssetState::InSync => "in_sync",
            AssetState::Drifted => "drifted",
            AssetState::Missing => "missing",
            AssetState::Unmanaged => "unmanaged",
            AssetState::Unsupported => "unsupported",
            AssetState::Orphan => "orphan",
        }
    }
}

fn is_subset(want: &Value, have: &Value) -> bool {
    match (want, have) {
        (Value::Object(w), Value::Object(h)) => w
            .iter()
            .all(|(k, v)| h.get(k).is_some_and(|hv| is_subset(v, hv))),
        (Value::Array(w), Value::Array(h)) => {
            w.iter().all(|wv| h.iter().any(|hv| is_subset(wv, hv)))
        }
        _ => want == have,
    }
}

/// Does the host's config already satisfy this merge?
pub fn merge_satisfied(snap: &HostSnapshot, m: &ConfigMerge) -> bool {
    let Some(root) = snap.configs.get(&m.file) else {
        return false;
    };
    let Some(have) = json_get(root, &m.json_path) else {
        return false;
    };
    match m.mode {
        MergeMode::Set => have == &m.value,
        MergeMode::AppendUnique => have.as_array().is_some_and(|arr| arr.contains(&m.value)),
        MergeMode::Subset => is_subset(&m.value, have),
    }
}

/// Compare every catalog asset against the snapshot, then add `unmanaged`
/// rows for installed assets the catalog does not know and `orphan` rows for
/// assets the host's fleet `manifest` still claims but the catalog has
/// dropped. `managed` on a catalog row says whether the manifest names it —
/// i.e. whether fleet put it there, as opposed to finding it already
/// installed.
///
/// `secrets` are this host's resolved `${NAME}` values, and the comparison
/// runs against the *substituted* plan — the bytes a sync would actually
/// write. Comparing the raw render instead would read every secret-bearing
/// asset as `drifted` forever, since the host holds the real value where
/// the render still says `${NAME}`. `catalog_hash` is likewise the
/// substituted plan's hash, matching `ManifestEntry::hash` and
/// `sync::plan`'s rule 4. A name with no value stays `${NAME}` in the
/// comparison, so the asset reads as drifted/missing — which is what the
/// plan will report as `blocked`.
pub fn compute_states(
    catalog: &Catalog,
    harness: &dyn Harness,
    host_alias: &str,
    snap: &HostSnapshot,
    manifest: &Manifest,
    secrets: &std::collections::BTreeMap<String, String>,
    scanned_at: i64,
) -> Vec<AssetInventoryRow> {
    let mut rows = Vec::new();
    for asset in &catalog.assets {
        let base = AssetInventoryRow {
            host_alias: host_alias.to_string(),
            harness: harness.id().to_string(),
            kind: asset.kind().as_str().to_string(),
            name: asset.header.name.clone(),
            managed: manifest
                .assets
                .contains_key(&Manifest::key(asset.kind(), &asset.header.name)),
            scanned_at,
            ..Default::default()
        };
        let rendered = match harness.render(asset) {
            Ok(p) => p,
            Err(_) => {
                rows.push(AssetInventoryRow {
                    state: AssetState::Unsupported.as_str().into(),
                    ..base
                });
                continue;
            }
        };
        let substituted = super::sync::secrets::substitute(&rendered, secrets);
        let plan = substituted.plan.inner();
        let catalog_hash = plan.hash();
        let mut present = false;
        let mut all_match = true;
        let mut host_parts: Vec<String> = Vec::new();
        for f in &plan.files {
            match snap.files.get(&f.path) {
                Some(h) => {
                    present = true;
                    host_parts.push(format!("{}={h}", f.path));
                    if *h != sha256_hex(&f.bytes) {
                        all_match = false;
                    }
                }
                None => all_match = false,
            }
        }
        for m in &plan.merges {
            let have = snap
                .configs
                .get(&m.file)
                .and_then(|root| json_get(root, &m.json_path));
            let satisfied = merge_satisfied(snap, m);
            // `Set`/`Subset` merges point `json_path` at an asset-specific
            // key (e.g. `mcpServers.<name>`, `plugins.<plugin@marketplace>`),
            // so resolving that path already means *this* asset's entry is
            // present. `AppendUnique` merges (hooks) instead point at a
            // *shared* array keyed only by event (e.g. `hooks.Stop`) that
            // every hook on that event appends into, so resolving the path
            // only proves some hook exists there — not this one. Treat an
            // `AppendUnique` target as present only once its own value is
            // actually found in the array, or a catalog hook whose sibling
            // is installed but who is itself absent would read as `drifted`
            // instead of `missing`.
            let this_present = match m.mode {
                MergeMode::AppendUnique => satisfied,
                MergeMode::Set | MergeMode::Subset => have.is_some(),
            };
            if this_present {
                present = true;
                host_parts.push(format!(
                    "{}:{}={}",
                    m.file,
                    m.json_path.join("/"),
                    have.map(|v| v.to_string()).unwrap_or_default()
                ));
            }
            if !satisfied {
                all_match = false;
            }
        }
        let state = if plan.files.is_empty() && plan.merges.is_empty() {
            AssetState::InSync // disabled target: nothing to install
        } else if !present {
            AssetState::Missing
        } else if all_match {
            AssetState::InSync
        } else {
            AssetState::Drifted
        };
        let host_hash = if present {
            Some(sha256_hex(host_parts.join("\n").as_bytes()))
        } else {
            None
        };
        rows.push(AssetInventoryRow {
            state: state.as_str().into(),
            catalog_hash: Some(catalog_hash),
            host_hash,
            ..base
        });
    }
    // Manifest entries the catalog has dropped. Computed before the
    // `unmanaged` rows because an orphan whose files are still installed is
    // *also* something `installed()` reports, and `(host, harness, kind,
    // name)` is the inventory table's primary key: one row per asset, and
    // `orphan` (fleet put it there, and the next sync removes it) says
    // strictly more than `unmanaged`.
    let mut orphans: Vec<AssetInventoryRow> = Vec::new();
    for (key, _entry) in manifest.orphans(catalog) {
        // A key that names no kind cannot be displayed as an asset row; the
        // sync plan skips it for the same reason.
        let Some((kind, name)) = Manifest::split_key(key) else {
            continue;
        };
        orphans.push(AssetInventoryRow {
            host_alias: host_alias.to_string(),
            harness: harness.id().to_string(),
            kind: kind.as_str().to_string(),
            name,
            state: AssetState::Orphan.as_str().into(),
            catalog_hash: None,
            host_hash: None,
            scanned_at,
            managed: true,
        });
    }
    // Both the catalog name and the install name (when `install_as` is set,
    // they differ) count as "this asset" here: a stray host directory that
    // happens to collide with the catalog name (e.g. both `foo_bar/` and
    // `foo-bar/` present while the catalog asset is `foo-bar` with
    // `install_as: foo_bar`) cannot be surfaced as its own `unmanaged` row,
    // because the inventory table's primary key is (host, harness, kind,
    // name) and that name is always the catalog name — a second row keyed
    // identically to the catalog row would collide in `replace_host_inventory`
    // and silently roll back the whole host refresh.
    let install_names: std::collections::BTreeSet<(Kind, String)> = catalog
        .assets
        .iter()
        .map(|a| (a.kind(), a.install_name().to_string()))
        .collect();
    let catalog_names: std::collections::BTreeSet<(Kind, String)> = catalog
        .assets
        .iter()
        .map(|a| (a.kind(), a.header.name.clone()))
        .collect();
    for (kind, name) in harness.installed(snap) {
        let key = (kind, name.clone());
        if !install_names.contains(&key) && catalog_names.contains(&key) {
            // Suppressed, and not because the host holds what the catalog
            // renders: this identifier is some asset's *catalog* name while
            // the asset installs under a different one, so whatever is on
            // the host here is unrelated to fleet and can never be listed
            // (see the primary-key note above). Say so at least once.
            tracing::debug!(
                host = host_alias,
                kind = kind.as_str(),
                identifier = %name,
                "installed identifier collides with a catalog name; not reported as unmanaged"
            );
        }
        if !install_names.contains(&key)
            && !catalog_names.contains(&key)
            && !orphans
                .iter()
                .any(|o| o.kind == kind.as_str() && o.name == name)
        {
            rows.push(AssetInventoryRow {
                host_alias: host_alias.to_string(),
                harness: harness.id().to_string(),
                kind: kind.as_str().to_string(),
                name,
                state: AssetState::Unmanaged.as_str().into(),
                catalog_hash: None,
                host_hash: None,
                scanned_at,
                managed: false,
            });
        }
    }
    rows.extend(orphans);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::claude::Claude;
    use crate::service::catalog::harness::{ConfigMerge, Harness, HostSnapshot, MergeMode};
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::repo::Catalog;
    use serde_json::json;

    /// No secrets: the default for every test that does not exercise
    /// `${NAME}` substitution.
    fn empty() -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }

    fn snap_with(configs: Vec<(&str, serde_json::Value)>) -> HostSnapshot {
        let mut s = HostSnapshot::default();
        for (k, v) in configs {
            s.configs.insert(k.into(), v);
        }
        s
    }

    #[test]
    fn merge_satisfied_by_mode() {
        let set = ConfigMerge {
            file: "f".into(),
            json_path: vec!["a".into(), "b".into()],
            mode: MergeMode::Set,
            value: json!({"x": 1}),
        };
        assert!(merge_satisfied(
            &snap_with(vec![("f", json!({"a": {"b": {"x": 1}}}))]),
            &set
        ));
        assert!(!merge_satisfied(
            &snap_with(vec![("f", json!({"a": {"b": {"x": 2}}}))]),
            &set
        ));
        assert!(!merge_satisfied(&snap_with(vec![]), &set));

        let append = ConfigMerge {
            file: "f".into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value: json!({"hooks": [{"type": "command", "command": "x"}]}),
        };
        assert!(merge_satisfied(
            &snap_with(vec![(
                "f",
                json!({"hooks": {"Stop": [{"other": 1}, {"hooks": [{"type": "command", "command": "x"}]}]}})
            )]),
            &append
        ));
        assert!(!merge_satisfied(
            &snap_with(vec![("f", json!({"hooks": {"Stop": [{"other": 1}]}}))]),
            &append
        ));

        let subset = ConfigMerge {
            file: "f".into(),
            json_path: vec!["plugins".into(), "p@m".into()],
            mode: MergeMode::Subset,
            value: json!([{"version": "1"}]),
        };
        assert!(merge_satisfied(
            &snap_with(vec![(
                "f",
                json!({"plugins": {"p@m": [{"version": "1", "scope": "user"}]}})
            )]),
            &subset
        ));
        assert!(!merge_satisfied(
            &snap_with(vec![("f", json!({"plugins": {"p@m": [{"version": "2"}]}}))]),
            &subset
        ));
        let latest = ConfigMerge {
            value: json!([{}]),
            ..subset.clone()
        };
        assert!(merge_satisfied(
            &snap_with(vec![("f", json!({"plugins": {"p@m": [{"version": "2"}]}}))]),
            &latest
        ));
    }

    #[test]
    fn compute_states_covers_all_five_states() {
        let mut cat = Catalog::default();
        let mut skill = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        skill.body = "b\n".into();
        cat.assets.push(skill.clone());
        cat.assets.push(
            Asset::from_yaml(
                None,
                "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: u\n",
            )
            .unwrap(),
        );
        cat.assets
            .push(Asset::from_yaml(None, "kind: agent\nname: gone\ndescription: d\n").unwrap());
        cat.assets.push(Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap());

        let claude = Claude;
        let skill_plan = claude.render(&skill).unwrap();
        let skill_hash = crate::service::catalog::model::sha256_hex(&skill_plan.files[0].bytes);
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), skill_hash);
        snap.files
            .insert("~/.claude/skills/extra/SKILL.md".into(), "zzz".into());
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet": {"type": "http", "url": "OTHER"}}}),
        );
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}),
        );

        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            7,
        );
        let state = |kind: &str, name: &str| {
            rows.iter()
                .find(|r| r.kind == kind && r.name == name)
                .map(|r| r.state.clone())
                .unwrap_or_default()
        };
        assert_eq!(state("skill", "s"), "in_sync");
        assert_eq!(state("mcp_server", "fleet"), "drifted");
        assert_eq!(state("agent", "gone"), "missing");
        assert_eq!(state("hook", "h"), "in_sync");
        assert_eq!(state("skill", "extra"), "unmanaged");
        assert!(rows
            .iter()
            .all(|r| r.host_alias == "local" && r.harness == "claude" && r.scanned_at == 7));
        let s = rows.iter().find(|r| r.name == "s").unwrap();
        assert!(s.catalog_hash.is_some() && s.host_hash.is_some());

        let codex = crate::service::catalog::harness::codex::Codex;
        let rows = compute_states(
            &cat,
            &codex,
            "local",
            &HostSnapshot::default(),
            &Manifest::default(),
            &empty(),
            7,
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "gone").unwrap().state,
            "unsupported"
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "s").unwrap().state,
            "missing"
        );
    }

    /// An installed identifier that matches the catalog asset's
    /// `install_as` (not its catalog `name`) must be recognised as *that*
    /// asset rather than reported as a second, `unmanaged` asset next to a
    /// `missing` one.
    #[test]
    fn install_as_matches_the_installed_identifier() {
        let mut cat = Catalog::default();
        let mut skill = Asset::from_yaml(
            None,
            "kind: skill\nname: foo-bar\ndescription: d\ninstall_as: foo_bar\n",
        )
        .unwrap();
        skill.body = "b\n".into();
        cat.assets.push(skill.clone());

        let claude = Claude;
        let skill_plan = claude.render(&skill).unwrap();
        let skill_hash = crate::service::catalog::model::sha256_hex(&skill_plan.files[0].bytes);
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/foo_bar/SKILL.md".into(), skill_hash);

        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            1,
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "foo-bar").unwrap().state,
            "in_sync"
        );
        assert!(
            !rows.iter().any(|r| r.name == "foo_bar"),
            "no unmanaged row for the install-as identifier: {rows:?}"
        );

        // Without `install_as`, the catalog name no longer matches the
        // installed identifier: the catalog asset reads `missing` and the
        // installed `foo_bar` reads `unmanaged`.
        let mut cat_no_install_as = Catalog::default();
        let mut skill2 =
            Asset::from_yaml(None, "kind: skill\nname: foo-bar\ndescription: d\n").unwrap();
        skill2.body = "b\n".into();
        cat_no_install_as.assets.push(skill2);
        let rows = compute_states(
            &cat_no_install_as,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            1,
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "foo-bar").unwrap().state,
            "missing"
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "foo_bar").unwrap().state,
            "unmanaged"
        );
    }

    /// A host with both the install-name directory (`foo_bar/`, what the
    /// catalog actually renders) and a stray directory matching the catalog
    /// name (`foo-bar/`) must still yield exactly one inventory row keyed
    /// `foo-bar` — never a second `unmanaged` row for the same (kind, name),
    /// which would collide with the catalog row on the inventory table's
    /// primary key and silently roll back the whole host refresh (see
    /// `Store::replace_host_inventory`).
    #[test]
    fn name_and_install_as_both_installed_yield_one_row() {
        let mut cat = Catalog::default();
        let mut skill = Asset::from_yaml(
            None,
            "kind: skill\nname: foo-bar\ndescription: d\ninstall_as: foo_bar\n",
        )
        .unwrap();
        skill.body = "b\n".into();
        cat.assets.push(skill.clone());

        let claude = Claude;
        let skill_plan = claude.render(&skill).unwrap();
        let skill_hash = crate::service::catalog::model::sha256_hex(&skill_plan.files[0].bytes);
        let mut snap = HostSnapshot::default();
        snap.files.insert(
            "~/.claude/skills/foo_bar/SKILL.md".into(),
            skill_hash.clone(),
        );
        snap.files
            .insert("~/.claude/skills/foo-bar/SKILL.md".into(), skill_hash);

        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            1,
        );
        let foo_bar_rows: Vec<_> = rows.iter().filter(|r| r.name == "foo-bar").collect();
        assert_eq!(foo_bar_rows.len(), 1, "{rows:?}");
        assert_eq!(foo_bar_rows[0].state, "in_sync");
        assert!(!rows.iter().any(|r| r.name == "foo_bar"), "{rows:?}");

        let mut seen = std::collections::BTreeSet::new();
        for r in &rows {
            assert!(
                seen.insert((r.kind.clone(), r.name.clone())),
                "duplicate (kind, name) in inventory rows: {:?} / {rows:?}",
                (r.kind.clone(), r.name.clone())
            );
        }
    }

    /// Regression test: an `AppendUnique` (hook) merge's `json_path` points
    /// at the *shared* per-event array (`hooks.Stop`), not an asset-specific
    /// key, so a sibling hook occupying that array must not make an absent
    /// catalog hook read as `drifted` — it must read as `missing`. Once the
    /// catalog hook's own entry is actually present alongside the sibling,
    /// it must read as `in_sync`.
    #[test]
    fn hook_presence_requires_its_own_entry_not_just_a_shared_sibling() {
        use crate::service::catalog::harness::claude::SETTINGS_PATH;

        let mut cat = Catalog::default();
        cat.assets.push(
            Asset::from_yaml(
                None,
                "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n",
            )
            .unwrap(),
        );
        let claude = Claude;

        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            SETTINGS_PATH.into(),
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}}),
        );
        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            1,
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "h").unwrap().state,
            "missing"
        );

        snap.configs.insert(
            SETTINGS_PATH.into(),
            json!({"hooks": {"Stop": [
                {"hooks": [{"type": "command", "command": "other"}]},
                {"hooks": [{"type": "command", "command": "x"}]},
            ]}}),
        );
        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            1,
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "h").unwrap().state,
            "in_sync"
        );
    }

    #[tokio::test]
    async fn run_host_script_local_executes_bash() {
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let out = run_host_script(&ssh, "local", "echo \"hi $((1+1))\"")
            .await
            .unwrap();
        assert_eq!(out.trim(), "hi 2");
    }

    /// Regression test: a scan script that exits non-zero must surface as an
    /// error, never as an empty-but-successful snapshot (which previously
    /// made every catalog asset read back as `missing`).
    #[tokio::test]
    async fn run_host_script_local_fails_on_nonzero_exit() {
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let err = run_host_script(&ssh, "local", "echo boom >&2; exit 3")
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_SCAN");
        assert!(err.message.contains("exited 3"), "{}", err.message);
        assert!(err.message.contains("boom"), "{}", err.message);
    }

    /// A cancelled token stops a host script instead of waiting it out, and
    /// an over-budget script fails on the wall clock rather than hanging.
    #[tokio::test]
    async fn run_host_script_with_honours_cancellation_and_the_wall_clock() {
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());

        let token = CancellationToken::new();
        let waiter = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            waiter.cancel();
        });
        let err = run_host_script_with(&ssh, "local", "sleep 30", Duration::from_secs(30), &token)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_CANCELLED");

        // An already-cancelled token never spawns anything at all.
        let dead = CancellationToken::new();
        dead.cancel();
        let err = run_host_script_with(&ssh, "local", "echo hi", Duration::from_secs(5), &dead)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_CANCELLED");

        let err = run_host_script_with(
            &ssh,
            "local",
            "sleep 30",
            Duration::from_millis(100),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_TIMEOUT");
    }

    // `CATALOG_TEST_LOCK` only serialises tests against the process-global
    // `CATALOG`; it guards no resource the async runtime itself needs, so
    // holding it across `scan_hosts`'s awaits is safe despite the lint.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn scan_hosts_scans_local_and_persists_rows() {
        let _g = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("fleet-catalog-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        let cat = crate::service::catalog::repo::load_dir(&root).unwrap();
        *crate::service::catalog::CATALOG.write().unwrap() = Some(cat);

        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("local", None).unwrap();
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let results = scan_hosts(&store, &ssh, Some("local")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "scanned", "{:?}", results[0].detail);
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(rows.iter().any(|r| r.name == "s" && r.harness == "claude"));
    }

    /// `managed` mirrors the host manifest, and a manifest entry the catalog
    /// has dropped becomes its own `orphan` row — exactly one row, even when
    /// the asset's files are still installed and `installed()` would
    /// otherwise report it as `unmanaged` too.
    #[test]
    fn compute_states_marks_managed_rows_and_emits_orphans() {
        use crate::service::catalog::sync::manifest::ManifestEntry;

        let mut cat = Catalog::default();
        let mut skill = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        skill.body = "b\n".into();
        cat.assets.push(skill.clone());

        let claude = Claude;
        let mut snap = HostSnapshot::default();
        let skill_plan = claude.render(&skill).unwrap();
        snap.files.insert(
            "~/.claude/skills/s/SKILL.md".into(),
            crate::service::catalog::model::sha256_hex(&skill_plan.files[0].bytes),
        );
        // Still installed on the host, and still in the manifest, but gone
        // from the catalog.
        snap.files
            .insert("~/.claude/skills/gone/SKILL.md".into(), "abc".into());

        let mut manifest = Manifest::default();
        manifest
            .assets
            .insert("skill/s".into(), ManifestEntry::default());
        manifest
            .assets
            .insert("skill/gone".into(), ManifestEntry::default());
        manifest
            .assets
            .insert("not-a-kind/x".into(), ManifestEntry::default());

        let rows = compute_states(&cat, &claude, "local", &snap, &manifest, &empty(), 9);
        let row = |name: &str| rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(row("s").state, "in_sync");
        assert!(row("s").managed, "the manifest names it");
        assert_eq!(row("gone").state, "orphan");
        assert!(row("gone").managed);
        assert_eq!(row("gone").catalog_hash, None);
        assert_eq!(row("gone").host_hash, None);
        assert_eq!(
            rows.iter().filter(|r| r.name == "gone").count(),
            1,
            "an orphan is never also listed as unmanaged"
        );
        assert!(
            !rows.iter().any(|r| r.name == "x"),
            "an unparseable manifest key names no asset"
        );

        // Without the manifest, the same host reads as unmanaged and
        // nothing is managed.
        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            9,
        );
        let row = |name: &str| rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(row("gone").state, "unmanaged");
        assert!(!row("gone").managed);
        assert!(!row("s").managed);
    }

    /// A secret-bearing asset is compared against the *substituted* bytes:
    /// a host holding exactly what a sync wrote reads `in_sync`, not
    /// `drifted` forever (the rendered plan still says `${TOK}`).
    #[test]
    fn compute_states_compares_against_the_substituted_plan() {
        let mut cat = Catalog::default();
        let mut skill = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        skill.body = "token ${TOK}\n".into();
        cat.assets.push(skill.clone());

        let claude = Claude;
        let secrets: std::collections::BTreeMap<String, String> =
            [("TOK".to_string(), "s3cr3t".to_string())]
                .into_iter()
                .collect();
        let rendered = claude.render(&skill).unwrap();
        let substituted = crate::service::catalog::sync::secrets::substitute(&rendered, &secrets);
        let on_host = &substituted.plan.inner().files[0];
        let mut snap = HostSnapshot::default();
        snap.files.insert(
            on_host.path.clone(),
            crate::service::catalog::model::sha256_hex(&on_host.bytes),
        );

        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &secrets,
            5,
        );
        assert_eq!(rows[0].state, "in_sync", "{rows:?}");
        assert_eq!(
            rows[0].catalog_hash.as_deref(),
            Some(substituted.plan.inner().hash().as_str()),
            "the row records the substituted plan's hash"
        );

        // Without the value the host's bytes cannot match: still drifted.
        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &std::collections::BTreeMap::new(),
            5,
        );
        assert_eq!(rows[0].state, "drifted");
    }
}
