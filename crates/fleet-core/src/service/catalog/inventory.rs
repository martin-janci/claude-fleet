//! Host inventory: run a harness scan on a host, compare against rendered
//! catalog assets, persist per-asset drift states.

use super::harness::{json_get, ConfigMerge, Harness, HostSnapshot, MergeMode};
use super::harness_set::{gated_catalog, harness_gate, HarnessFacts};
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HostScanResult {
    pub host: String,
    /// "scanned" | "skipped" | "failed"
    pub status: String,
    #[serde(default)]
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
    // Stderr first, and STDOUT's own last word when stderr says nothing: a
    // script that reports on stdout and exits non-zero — which is exactly what
    // `REMOTE_SOURCES_SCRIPT` does at its 64 MiB cap, printing `##TRUNCATED`
    // and exiting 1 — left a message no caller could reach, so the operator
    // got "scan script exited 1: " and nothing else.
    let err = String::from_utf8_lossy(&out.stderr);
    let said: String = match err.trim() {
        "" => last_message_line(&String::from_utf8_lossy(&out.stdout)),
        e => e.chars().take(SCRIPT_MESSAGE_MAX).collect(),
    };
    crate::ipc_error::IpcError::new(
        codes::E_SCAN,
        format!("{host}: scan script exited {code}: {said}"),
    )
}

/// How much of a script's own output an error quotes.
const SCRIPT_MESSAGE_MAX: usize = 200;

/// The last line of `stdout` that reads as a MESSAGE rather than payload.
///
/// A scan script's stdout is mostly data — a snapshot, or an import dump whose
/// every other line is base64 — and an error message is no place for it. A
/// message either starts with the scripts' own `##` marker or has a space in
/// it; base64 has neither. Bounded, because one such line can be a whole file.
fn last_message_line(stdout: &str) -> String {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty() && (l.starts_with("##") || l.contains(char::is_whitespace)))
        .map(|l| l.chars().take(SCRIPT_MESSAGE_MAX).collect())
        .unwrap_or_default()
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
/// What a host's script may print before fleet refuses its output.
///
/// A backstop on the CONTROLLER's side. `REMOTE_SOURCES_SCRIPT` caps an import
/// dump at 64 MiB of file bytes, but that cap runs ON THE HOST being imported
/// from — the side `parse_remote_dump` spends its length scrubbing paths
/// against, and the side that decides whether to honour its own cap at all.
/// Before this, both transports read the host's output unbounded:
/// `cmd.output()` locally, `run_with_mux_retry(.., None)` over ssh.
///
/// 96 MiB sits above any dump the script itself would produce: base64 inflates
/// 64 MiB to about 85 MiB, plus a `##FILE <path>` line per file. Reaching this
/// means the host ignored its own cap.
///
/// Read as `+ 1` so an overrun is DETECTED, never silently truncated:
/// `read_capped` keeps `cap` bytes and drops the rest with no signal, and a
/// dump cut at a line boundary would otherwise parse as a complete — but
/// smaller — set of files, which is worse than no cap at all.
pub const HOST_SCRIPT_MAX_OUTPUT: usize = 96 * 1024 * 1024;

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
    let read_cap = HOST_SCRIPT_MAX_OUTPUT.saturating_add(1);
    let out = if host == "local" {
        crate::service::hub::ensure_local_allowed(host)?;
        // Piped and read through `read_capped`, not `cmd.output()`, which
        // buffers whatever the script prints with no bound at all.
        let mut child = crate::proc::command("bash")
            .args(["-lc", script])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                crate::ipc_error::IpcError::new(codes::E_IO, format!("spawn bash: {e}"))
            })?;
        let stdout = tokio::spawn(crate::ssh::read_capped(child.stdout.take(), Some(read_cap)));
        let stderr = tokio::spawn(crate::ssh::read_capped(child.stderr.take(), Some(read_cap)));
        tokio::select! {
            res = tokio::time::timeout(wall_clock, child.wait()) => match res {
                Ok(status) => {
                    let status = status.map_err(|e| {
                        crate::ipc_error::IpcError::new(codes::E_IO, format!("wait bash: {e}"))
                    })?;
                    std::process::Output {
                        status,
                        stdout: stdout.await.unwrap_or_default(),
                        stderr: stderr.await.unwrap_or_default(),
                    }
                }
                Err(_) => {
                    let _ = child.start_kill();
                    stdout.abort();
                    stderr.abort();
                    return Err(crate::ipc_error::IpcError::new(
                        codes::E_TIMEOUT,
                        format!("{host}: script did not finish within {}s", wall_clock.as_secs()),
                    ));
                }
            },
            _ = token.cancelled() => {
                let _ = child.start_kill();
                stdout.abort();
                stderr.abort();
                return Err(crate::ipc_error::IpcError::new(
                    codes::E_CANCELLED,
                    format!("{host}: cancelled"),
                ))
            }
        }
    } else {
        let quoted = quote(script);
        ssh.run_bounded_cancellable_capped(
            host,
            &["bash", "-lc", &quoted],
            SCAN_TIMEOUT,
            wall_clock,
            token.clone(),
            read_cap,
        )
        .await?
    };
    if !out.status.success() {
        return Err(scan_failed(host, &out));
    }
    // One byte over the limit means the host printed more than fleet accepts.
    // Refuse the whole thing: what arrived is a prefix, and a prefix of a dump
    // is a smaller dump that looks complete.
    if out.stdout.len() > HOST_SCRIPT_MAX_OUTPUT {
        return Err(crate::ipc_error::IpcError::new(
            codes::E_INVALID,
            format!(
                "{host}: script printed more than {} MiB — refusing a truncated result",
                HOST_SCRIPT_MAX_OUTPUT / (1024 * 1024)
            ),
        ));
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
/// harness that supports scanning, persisting per (host, harness) the rows
/// `harness_set::harness_gate` lets through.
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
    // Assets M2: every loaded catalog's assets, not just personal's — a
    // host that never accepts an org catalog still has its own asset drift
    // reported the same as before; `compute_states` stamps each row with
    // whichever catalog it actually came from.
    let catalog = super::registry::union_all()?.ok_or_else(|| {
        crate::ipc_error::IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        )
    })?;
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
        // Multi-harness F3a: the host's own harness choice (`None` = auto).
        let configured = h.harnesses.clone();
        for harness in super::harness::all() {
            if harness.scan_script().is_none() {
                continue;
            }
            let scanned_at = super::now_secs();
            match scan_host_harness(ssh, &h.alias, harness.as_ref()).await {
                Ok(snap) => {
                    let manifest = Manifest::from_snapshot(&snap, harness.manifest_path());
                    // `Off` persists no rows (clearing stale ones). `Retiring`
                    // plans against an EMPTY catalog, so every manifest entry
                    // reads as an `orphan` — but `compute_states` also reports
                    // what is installed and unmanaged, and an empty catalog
                    // makes everything unmanaged, so a retiring harness's rows
                    // are its orphans PLUS an `unmanaged` row per installed
                    // asset. That is a true statement about the host (the
                    // assets are there and fleet does not manage them), which
                    // is why it stands; it is not "only fleet's own installs",
                    // as this comment used to say.
                    let gate = harness_gate(
                        harness.id(),
                        configured.as_deref(),
                        HarnessFacts::of(&snap, &manifest),
                    );
                    let rows = gated_catalog(gate, &catalog).map_or_else(Vec::new, |c| {
                        compute_states(
                            c,
                            harness.as_ref(),
                            &h.alias,
                            &snap,
                            &manifest,
                            &secrets,
                            scanned_at,
                        )
                    });
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
            // Assets M2: which catalog this asset came from. `origin_of`
            // falls back to `catalog` itself when there is only one loaded
            // catalog (no `origin` entries), so a personal-only fleet still
            // stamps the personal catalog's own id — never `None` for a
            // catalog asset, restoring what the migration 091 backfill set
            // and every rescan since had been silently erasing. `.filter(|&i|
            // i > 0)`: id `0` is the registry's "stands for personal"
            // convention for a hand-built catalog that was never actually
            // installed under the store's real id (see `registry.rs`) — it
            // names no real `catalogs` row, so stamping it would just be a
            // different spelling of the FK hazard `replace_host_inventory`'s
            // own `SELECT`-guarded insert defends against; `None` here is
            // honest about "the real id is not known".
            catalog_id: Some(catalog.origin_of(asset.kind(), &asset.header.name).id)
                .filter(|&id| id > 0),
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
            ..Default::default()
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
    for a in harness.installed_detail(snap) {
        let key = (a.kind, a.name.clone());
        if !install_names.contains(&key) && catalog_names.contains(&key) {
            // Suppressed, and not because the host holds what the catalog
            // renders: this identifier is some asset's *catalog* name while
            // the asset installs under a different one, so whatever is on
            // the host here is unrelated to fleet and can never be listed
            // (see the primary-key note above). Say so at least once.
            tracing::debug!(
                host = host_alias,
                kind = a.kind.as_str(),
                identifier = %a.name,
                "installed identifier collides with a catalog name; not reported as unmanaged"
            );
        }
        if !install_names.contains(&key)
            && !catalog_names.contains(&key)
            && !orphans
                .iter()
                .any(|o| o.kind == a.kind.as_str() && o.name == a.name)
        {
            rows.push(AssetInventoryRow {
                host_alias: host_alias.to_string(),
                harness: harness.id().to_string(),
                kind: a.kind.as_str().to_string(),
                name: a.name,
                state: AssetState::Unmanaged.as_str().into(),
                catalog_hash: None,
                host_hash: a.hash,
                scanned_at,
                managed: false,
                secret_like: a.secret_like,
                fleet_owned: a.fleet_owned,
                catalog_id: None,
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

    /// A script that reports on STDOUT and exits non-zero still says what
    /// happened.
    ///
    /// `REMOTE_SOURCES_SCRIPT` does exactly that at its 64 MiB cap — it prints
    /// `##TRUNCATED` and exits 1 — so the message it was written to deliver
    /// could never reach a caller: the error read "scan script exited 1: " and
    /// stopped there. Only the TAIL of stdout is quoted, because the start of a
    /// dump is base64.
    #[test]
    fn a_scripts_stdout_is_quoted_when_its_stderr_says_nothing() {
        use std::os::unix::process::ExitStatusExt;
        let out = |stdout: &str, stderr: &str| std::process::Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        let e = scan_failed("oci", &out("##FILE a\nQUJD\n##TRUNCATED\n", ""));
        assert!(e.message.contains("##TRUNCATED"), "{}", e.message);
        assert!(
            !e.message.contains("QUJD"),
            "not the dump itself: {}",
            e.message
        );

        // A dump whose LAST line is payload quotes nothing: an error message is
        // no place for a file, and a `.claude.json` line is one.
        let b64 = "e".repeat(5000);
        let e = scan_failed("oci", &out(&format!("##FILE a\n{b64}\n"), ""));
        assert!(!e.message.contains("eeee"), "{}", e.message);
        assert!(e.message.len() < 120, "{}", e.message);

        // And a long message is cut rather than carried whole.
        let long: String = std::iter::repeat_n("word ", 200).collect();
        let e = scan_failed("oci", &out(&long, ""));
        assert!(e.message.len() < 300, "{}", e.message);

        // Stderr still wins when there is any.
        let e = scan_failed("oci", &out("##TRUNCATED\n", "bash: find: not found"));
        assert!(e.message.contains("find: not found"), "{}", e.message);
        assert!(!e.message.contains("##TRUNCATED"), "{}", e.message);

        // Nothing on either: the exit code alone, as before.
        let e = scan_failed("oci", &out("", ""));
        assert!(e.message.contains("exited 1"), "{}", e.message);
    }

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
            rows.iter().find(|r| r.name == "h").unwrap().state,
            "unsupported",
            "codex renders no hooks"
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "gone").unwrap().state,
            "missing",
            "codex renders agents since F3b"
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "s").unwrap().state,
            "missing"
        );
    }

    /// A carry-forward from Task 1's review: `compute_states` used to write
    /// `catalog_id: None` unconditionally on every row (via
    /// `..Default::default()`), silently erasing migration 091's backfill on
    /// the very first rescan. Every catalog-asset row — whatever its state —
    /// must instead carry the id of the catalog it came from, while an
    /// `unmanaged` row (nothing the catalog defines) stays `None`. With a
    /// single catalog loaded, `Catalog::origin_of` has no `origin` entries to
    /// consult and falls back to the catalog itself, so this also proves the
    /// personal-only case stamps the personal catalog's own id rather than
    /// `None`.
    #[test]
    fn compute_states_stamps_the_catalog_id_on_every_catalog_asset_row_and_none_on_unmanaged() {
        let mut cat = Catalog {
            id: 42,
            name: "acme".into(),
            ..Default::default()
        };
        let mut skill = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        skill.body = "b\n".into();
        cat.assets.push(skill.clone());
        cat.assets
            .push(Asset::from_yaml(None, "kind: agent\nname: gone\ndescription: d\n").unwrap());

        let claude = Claude;
        let skill_plan = claude.render(&skill).unwrap();
        let skill_hash = crate::service::catalog::model::sha256_hex(&skill_plan.files[0].bytes);
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), skill_hash);
        // A stray directory the catalog does not define at all.
        snap.files
            .insert("~/.claude/skills/extra/SKILL.md".into(), "zzz".into());

        let rows = compute_states(
            &cat,
            &claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            1,
        );
        let s = rows.iter().find(|r| r.name == "s").unwrap();
        assert_eq!(s.state, "in_sync");
        assert_eq!(s.catalog_id, Some(42), "an in_sync catalog-asset row");
        let gone = rows.iter().find(|r| r.name == "gone").unwrap();
        assert_eq!(gone.state, "missing");
        assert_eq!(gone.catalog_id, Some(42), "a missing catalog-asset row");
        let extra = rows.iter().find(|r| r.name == "extra").unwrap();
        assert_eq!(extra.state, "unmanaged");
        assert_eq!(
            extra.catalog_id, None,
            "an unmanaged row names nothing the catalog defines"
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

    #[test]
    fn unmanaged_rows_carry_hash_and_flags() {
        let cat = Catalog::default();
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/extra/SKILL.md".into(), "aa".into());
        snap.files.insert(
            "~/.claude/skills/claude-fleet-control/SKILL.md".into(),
            "bb".into(),
        );
        let rows = compute_states(
            &cat,
            &Claude,
            "local",
            &snap,
            &Manifest::default(),
            &empty(),
            1,
        );
        let extra = rows.iter().find(|r| r.name == "extra").unwrap();
        assert_eq!(extra.state, "unmanaged");
        assert_eq!(
            extra.host_hash.as_deref(),
            Some(crate::service::catalog::model::sha256_hex(b"SKILL.md=aa").as_str())
        );
        assert!(!extra.fleet_owned);
        assert!(
            rows.iter()
                .find(|r| r.name == "claude-fleet-control")
                .unwrap()
                .fleet_owned
        );
    }

    /// What a `Retiring` harness's inventory rows actually are: its manifest's
    /// entries as `orphan`, PLUS an `unmanaged` row per installed asset.
    ///
    /// `Retiring` was tested only through `plan_sync`; `scan_hosts` and
    /// `rescan_after_apply` — the two call sites F3b added — had none, and two
    /// code comments described the result as "only fleet's own installs (as
    /// orphans)". It is not: an empty catalog manages nothing, so everything
    /// installed is also reported unmanaged. Both are true statements about the
    /// host, and this is the test that says which is which.
    #[test]
    fn a_retiring_harness_reports_its_orphans_and_what_is_installed() {
        use crate::service::catalog::harness_set::{gated_catalog, HarnessGate};
        let cat = Catalog::default();
        // One asset fleet installed (in the manifest) and one it did not.
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/ours/SKILL.md".into(), "aa".into());
        snap.files
            .insert("~/.claude/skills/theirs/SKILL.md".into(), "bb".into());
        let mut manifest = Manifest::default();
        manifest.assets.insert(
            Manifest::key(crate::service::catalog::model::Kind::Skill, "ours"),
            Default::default(),
        );

        // The gate's own catalog for `Retiring` is the empty one.
        let planned = gated_catalog(HarnessGate::Retiring, &cat).expect("retiring still scans");
        assert!(
            planned.assets.is_empty(),
            "an empty catalog, by construction"
        );
        let rows = compute_states(planned, &Claude, "local", &snap, &manifest, &empty(), 1);

        let state = |n: &str| {
            rows.iter()
                .find(|r| r.name == n)
                .map(|r| r.state.as_str())
                .unwrap_or("absent")
        };
        assert_eq!(
            state("ours"),
            "orphan",
            "in the manifest, not in the catalog"
        );
        assert_eq!(
            state("theirs"),
            "unmanaged",
            "installed and unmanaged — the half the comments denied"
        );

        // `Off` is the other half of the same gate: no catalog, no rows, which
        // is how a turned-off harness's stale rows get cleared.
        assert!(gated_catalog(HarnessGate::Off, &cat).is_none());
    }

    #[cfg(unix)]
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
    #[cfg(unix)]
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

    /// A host that prints more than fleet accepts is refused outright.
    ///
    /// The 64 MiB import cap lives in `REMOTE_SOURCES_SCRIPT`, which runs ON
    /// the host being imported from — so it bounds a cooperative host, not a
    /// hostile or broken one, while both transports used to read the result
    /// unbounded. Refusing matters more than capping: `read_capped` truncates
    /// silently, and a dump cut at a line boundary parses as a complete but
    /// smaller set of files.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_host_that_prints_past_the_cap_is_refused_not_truncated() {
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        // `yes` is cheap and unbounded; the cap is what must stop it.
        let err = run_host_script_with(
            &ssh,
            "local",
            &format!(
                "yes 0123456789abcdef | head -c {}",
                HOST_SCRIPT_MAX_OUTPUT + 4096
            ),
            Duration::from_secs(120),
            &CancellationToken::new(),
        )
        .await
        .expect_err("output past the cap must be refused");
        assert_eq!(err.code, codes::E_INVALID, "{}", err.message);
        assert!(
            err.message.contains("refusing a truncated result"),
            "{}",
            err.message
        );

        // Just under it still comes back whole, so the cap is not merely
        // refusing everything large.
        let want = 64 * 1024;
        let out = run_host_script_with(
            &ssh,
            "local",
            &format!("yes 0123456789abcdef | head -c {want}"),
            Duration::from_secs(120),
            &CancellationToken::new(),
        )
        .await
        .expect("under the cap");
        assert_eq!(out.len(), want, "a dump under the cap arrives intact");
    }

    /// A cancelled token stops a host script instead of waiting it out, and
    /// an over-budget script fails on the wall clock rather than hanging.
    #[cfg(unix)]
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
    // catalog registry; it guards no resource the async runtime itself
    // needs, so holding it across `scan_hosts`'s awaits is safe despite the
    // lint.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn scan_hosts_scans_local_and_persists_rows() {
        let _g = crate::service::catalog::lock_registry_for_test();
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

        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("local", None).unwrap();
        // `asset_inventory.catalog_id` (Assets M2) is a real foreign key into
        // `catalogs(id)`: the registered catalog needs the STORE's real
        // personal id, not the registry's "id 0 stands for personal"
        // convention (which only `host_layers` resolution understands) —
        // production's `load` does this via `set_catalog_config` + `load`;
        // mirrored here by hand since this test bypasses both.
        store
            .lock()
            .unwrap()
            .set_catalog_config("/p", None)
            .unwrap();
        let personal_id = store
            .lock()
            .unwrap()
            .personal_catalog()
            .unwrap()
            .unwrap()
            .id;
        let mut cat = crate::service::catalog::repo::load_dir(&root).unwrap();
        cat.id = personal_id;
        crate::service::catalog::registry::install(cat).unwrap();

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

    /// Restores `HOME` when the test (or a panic) ends. Copied from
    /// `sync::apply`'s test module, which cannot export it.
    #[cfg(unix)]
    struct HomeGuard(Option<String>);
    #[cfg(unix)]
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    /// F3a: `scan_hosts` keeps no Codex rows for a host that turned Codex
    /// off, even with Codex detected (`~/.codex/sessions`); back on auto,
    /// the same host is inventoried for Codex again.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn scan_hosts_follows_the_hosts_harness_choice() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".codex/sessions")).unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("skills/s")).unwrap();
        std::fs::write(root.path().join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.path().join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.path().join("skills/s/body.md"), "b\n").unwrap();

        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        let personal_id = {
            let s = store.lock().unwrap();
            s.insert_host("local", None).unwrap();
            s.set_host_harnesses("local", Some(&["claude".to_string()][..]))
                .unwrap();
            // See `scan_hosts_scans_local_and_persists_rows`: `catalog_id`
            // is a real foreign key, so the registered catalog needs the
            // store's real personal id.
            s.set_catalog_config("/p", None).unwrap();
            s.personal_catalog().unwrap().unwrap().id
        };
        let mut cat = crate::service::catalog::repo::load_dir(root.path()).unwrap();
        cat.id = personal_id;
        crate::service::catalog::registry::install(cat).unwrap();
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let results = scan_hosts(&store, &ssh, Some("local")).await.unwrap();
        assert_eq!(results[0].status, "scanned", "{:?}", results[0].detail);
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(rows.iter().any(|r| r.harness == "claude" && r.name == "s"));
        assert!(rows.iter().all(|r| r.harness != "codex"), "{rows:?}");

        store
            .lock()
            .unwrap()
            .set_host_harnesses("local", None)
            .unwrap();
        scan_hosts(&store, &ssh, Some("local")).await.unwrap();
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.harness == "codex" && r.name == "s" && r.state == "missing"),
            "{rows:?}"
        );
    }
}
