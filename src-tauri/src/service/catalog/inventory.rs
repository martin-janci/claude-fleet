//! Host inventory: run a harness scan on a host, compare against rendered
//! catalog assets, persist per-asset drift states.

use super::harness::{json_get, ConfigMerge, Harness, HostSnapshot, MergeMode};
use super::model::sha256_hex;
use super::repo::Catalog;
use crate::ipc_error::codes;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::AssetInventoryRow;
use crate::store::Store;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    if host == "local" {
        let out = tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output()
            .await
            .map_err(|e| {
                crate::ipc_error::IpcError::new(codes::E_IO, format!("spawn bash: {e}"))
            })?;
        if !out.status.success() {
            return Err(scan_failed(host, &out));
        }
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let quoted = quote(script);
    let out = ssh
        .run(host, &["bash", "-lc", &quoted], SCAN_TIMEOUT)
        .await?;
    if !out.status.success() {
        return Err(scan_failed(host, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
        for harness in super::harness::all() {
            let Some(script) = harness.scan_script() else {
                continue;
            };
            let scanned_at = super::now_secs();
            match run_host_script(ssh, &h.alias, &script)
                .await
                .and_then(|out| harness.parse_scan(&out))
            {
                Ok(snap) => {
                    let rows =
                        compute_states(&catalog, harness.as_ref(), &h.alias, &snap, scanned_at);
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
}

impl AssetState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AssetState::InSync => "in_sync",
            AssetState::Drifted => "drifted",
            AssetState::Missing => "missing",
            AssetState::Unmanaged => "unmanaged",
            AssetState::Unsupported => "unsupported",
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
/// rows for installed assets the catalog does not know.
pub fn compute_states(
    catalog: &Catalog,
    harness: &dyn Harness,
    host_alias: &str,
    snap: &HostSnapshot,
    scanned_at: i64,
) -> Vec<AssetInventoryRow> {
    let mut rows = Vec::new();
    for asset in &catalog.assets {
        let base = AssetInventoryRow {
            host_alias: host_alias.to_string(),
            harness: harness.id().to_string(),
            kind: asset.kind().as_str().to_string(),
            name: asset.header.name.clone(),
            scanned_at,
            ..Default::default()
        };
        let plan = match harness.render(asset) {
            Ok(p) => p,
            Err(_) => {
                rows.push(AssetInventoryRow {
                    state: AssetState::Unsupported.as_str().into(),
                    ..base
                });
                continue;
            }
        };
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
    for (kind, name) in harness.installed(snap) {
        if catalog.find(kind, &name).is_none() {
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

        let rows = compute_states(&cat, &claude, "local", &snap, 7);
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
        let rows = compute_states(&cat, &codex, "local", &HostSnapshot::default(), 7);
        assert_eq!(
            rows.iter().find(|r| r.name == "gone").unwrap().state,
            "unsupported"
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "s").unwrap().state,
            "missing"
        );
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
        let rows = compute_states(&cat, &claude, "local", &snap, 1);
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
        let rows = compute_states(&cat, &claude, "local", &snap, 1);
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
}
