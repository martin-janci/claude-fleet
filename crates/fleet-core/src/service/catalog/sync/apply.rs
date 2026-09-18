//! The per-host applier: turns one computed `HostPlan` into writes on the
//! host, and records what it wrote in the harness's managed manifest.
//!
//! Everything goes through one of three paths:
//!
//! - **plain files and deletions** — batched into `bash` scripts
//!   (`write_scripts`) that compare-and-swap on the hash the plan was
//!   computed against, back up what they replace
//!   (`<file>.fleet-bak-<epoch>-<pid>`), write through a `.fleet-tmp`
//!   sibling renamed into place, and print one `OK`/`CONFLICT`/`FAIL
//!   <~/path>` line per write. A host that changed under a stale plan is
//!   reported as a conflict instead of being overwritten;
//! - **secret-bearing files** (a rendered body whose content only became
//!   correct through `${NAME}` substitution) and **every config file** —
//!   hash-checked the same way, then written through
//!   `provision::write_host_file_secret`, which never puts the content in an
//!   argv and creates the file 0600. A secret value must never reach a
//!   script body, an error message, a result or a log line: report lines
//!   carry only the `~/` path;
//! - **plugins** — the harness CLI, since fleet cannot install a plugin by
//!   writing files.
//!
//! Config files are never patched blind: the plan's snapshot of the parsed
//! file is re-serialised, run through `Harness::merge_config` (which applies
//! this sync's merges and un-applies the previous manifest entry's), and the
//! whole new text written back under the same compare-and-swap. They take
//! the secret path unconditionally — not because this sync's merge carries a
//! secret, but because the file may ALREADY hold one (an earlier sync's
//! token, or the user's own), and a merge that only touches an unrelated key
//! would otherwise re-embed it, base64'd, in a `bash -lc` command line.
//! **A config file fleet has synced is therefore mode 0600 afterwards.** A
//! config file the scan could not parse is never rewritten at all: every
//! action touching it is `blocked`.
//!
//! Failure is per file, not per asset: within one batch, the non-conflicting
//! files of a multi-file asset are still written even when a sibling
//! conflicts. The action is reported `conflict`/`failed` and gets NO manifest
//! entry, so the next plan sees the half-written state, re-plans it, and the
//! (idempotent) writes converge — nothing is rolled back on the host.
//!
//! `sync::apply_sync` drives this module, wired to the `catalog_apply_sync`
//! Tauri command and the `apply_sync` MCP tool.

use super::super::harness::claude::PLUGINS_PATH;
use super::super::harness::{
    is_hex_hash, value_hash, ConfigMerge, Harness, HostSnapshot, ManifestMerge, MergeMode,
};
use super::super::inventory;
use super::manifest::{Manifest, ManifestEntry};
use super::plan::{Action, ActionOp, HostPlan, PluginTarget};
use crate::service::provision;
use crate::ssh::SshClient;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How long one batch of host writes may take. Generous compared with a
/// scan (60s): a batch can carry several megabytes of base64 and the host
/// may be a slow laptop over a slow link.
const APPLY_WALL_CLOCK: Duration = Duration::from_secs(300);

/// Soft cap on the base64 payload of one script. A chunk is closed as soon
/// as it crosses the cap, so one chunk can exceed it by at most a single
/// write (splitting one file across scripts is not possible — the write has
/// to be atomic).
const CHUNK_B64_BYTES: usize = 512 * 1024;

/// How many `.fleet-bak-*` backups of one file are kept; older ones are
/// pruned right after each new backup is made (see `backup_and_prune`).
const BACKUP_KEEP: usize = 3;

/// Per-action outcome strings (also `ActionResult::outcome`).
const DONE: &str = "done";
const CONFLICT: &str = "conflict";
const FAILED: &str = "failed";
const BLOCKED: &str = "blocked";
const SKIPPED: &str = "skipped";

/// One compare-and-swap write (or deletion) of a `~/`-relative path.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardedWrite {
    pub path: String,
    /// The hash the plan was computed against; `None` = the scan did not see
    /// the file, so it must still be absent.
    pub expected: Option<String>,
    pub bytes: Vec<u8>,
    pub backup: bool,
    pub delete: bool,
}

/// What the host reported for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    Ok,
    Conflict,
    Failed,
}

/// What the applier did about one planned action. `Deserialize` because a
/// whole `SyncRunSummary` is stored as JSON in `sync_runs` and read back by
/// `sync::last_sync`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub kind: String,
    pub name: String,
    pub op: ActionOp,
    /// `done` | `conflict` | `failed` | `blocked` | `skipped`.
    pub outcome: String,
    pub detail: Option<String>,
}

/// What the applier did about one host. `Deserialize` for the same reason
/// as `ActionResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSyncResult {
    pub host_alias: String,
    pub harness: String,
    /// `applied` | `partial` | `skipped` | `failed`.
    pub status: String,
    pub detail: Option<String>,
    /// A hook, MCP server or plugin changed: the harness must be restarted
    /// on the host before it picks the change up.
    pub restart_required: bool,
    pub actions: Vec<ActionResult>,
}

/// Everything `apply_host` needs besides the plan itself.
pub struct ApplyCtx<'a> {
    pub ssh: &'a Arc<SshClient>,
    pub token: CancellationToken,
    pub now: i64,
}

/// Is `p` a `~/`-relative path this module is willing to interpolate into a
/// shell script unquoted? Only `[A-Za-z0-9._/-]`, no empty or `..` segment.
/// Asset names are validated kebab-case and every directory is a constant,
/// so a path that fails this has no business being written at all.
fn is_safe_path(p: &str) -> bool {
    let Some(rest) = p.strip_prefix("~/") else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    if rest
        .split('/')
        .any(|seg| seg.is_empty() || seg == ".." || seg == ".")
    {
        return false;
    }
    rest.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

/// `~/a/b` → `"$HOME"/a/b`. Only the `$HOME` expansion is quoted; the rest
/// is safe unquoted because `is_safe_path` has ruled out every metacharacter.
fn host_path(p: &str) -> String {
    format!("\"$HOME\"/{}", p.trim_start_matches("~/"))
}

/// The `~/`-relative parent directory of `p` (`p` itself if it has none).
fn parent_dir(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => p.to_string(),
    }
}

/// The preamble every write script shares: a hasher, a base64 decoder (GNU
/// `-d` vs BSD `-D`) and `h`, which prints a file's hash or `absent`.
fn script_preamble() -> String {
    let mut s = String::new();
    s.push_str("cd \"$HOME\" || exit 1\n");
    // Epoch seconds alone collide when two batches run inside the same
    // second (a sync's file writes and its config rewrite, say), and the
    // second `cp` would then clobber the first batch's backup; `$$` makes
    // the suffix unique per script run.
    s.push_str("T=$(date +%s)-$$\n");
    s.push_str(
        "if command -v sha256sum >/dev/null 2>&1; then H=sha256sum; else H=\"shasum -a 256\"; fi\n",
    );
    s.push_str(
        "if printf %s Zg== | base64 -d >/dev/null 2>&1; then B=\"base64 -d\"; else B=\"base64 -D\"; fi\n",
    );
    s.push_str(
        "h() { if [ -f \"$1\" ]; then $H \"$1\" | cut -d \" \" -f1; else echo absent; fi; }\n",
    );
    s
}

/// The backup-then-prune pair emitted for the current file `$p`: back it up
/// as `<file>.fleet-bak-<epoch>-<pid>` if it exists, then drop all but the
/// [`BACKUP_KEEP`] newest backups of that exact file. `tail -n +N` with
/// `N = BACKUP_KEEP + 1` is POSIX-portable (no GNU-only `head -n -N`); the
/// glob is unquoted only after `"$p"`, so it matches solely `$p`'s own
/// backup names. Shared by `write_scripts` and `backup_script` so both call
/// sites emit identical text.
fn backup_and_prune() -> String {
    format!(
        "if [ -f \"$p\" ]; then cp -p \"$p\" \"$p.fleet-bak-$T\"; fi\nls -1t \"$p\".fleet-bak-* 2>/dev/null | tail -n +{} | while read -r f; do rm -f \"$f\"; done\n",
        BACKUP_KEEP + 1
    )
}

/// Bash scripts applying `writes` with compare-and-swap, chunked at roughly
/// [`CHUNK_B64_BYTES`] of base64 payload each. The scripts contain no single
/// quote, so the whole thing survives `shell::quote` on its way to a remote
/// host. A write whose path fails [`is_safe_path`] is skipped (the caller
/// must have failed its action already, hence the `debug_assert!`).
pub fn write_scripts(writes: &[GuardedWrite]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut body = String::new();
    let mut payload = 0usize;
    for w in writes {
        debug_assert!(is_safe_path(&w.path), "unsafe path reached write_scripts");
        if !is_safe_path(&w.path) {
            continue;
        }
        let p = host_path(&w.path);
        let report = &w.path;
        let expected = match w.expected.as_deref() {
            None => "absent",
            Some(hash) if is_hex_hash(hash) => hash,
            // The expected hash came off a host's stdout. A value that is
            // not a digest is never interpolated into the test — it is
            // reported as a conflict, which is what a plan computed against
            // something unreadable deserves.
            Some(_) => {
                body.push_str(&format!("echo \"CONFLICT {report}\"\n"));
                continue;
            }
        };
        body.push_str(&format!("p={p}; d=$(dirname \"$p\"); cur=$(h \"$p\")\n"));
        body.push_str(&format!(
            "if [ \"$cur\" != \"{expected}\" ]; then echo \"CONFLICT {report}\"; else\n"
        ));
        if w.backup {
            body.push_str(&backup_and_prune());
        }
        if w.delete {
            body.push_str(&format!(
                "rm -f \"$p\" && {{ rmdir \"$d\" 2>/dev/null; echo \"OK {report}\"; }} || echo \"FAIL {report}\"\n"
            ));
        } else {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&w.bytes);
            payload += b64.len();
            // Decode into a sibling and rename: a link that drops mid
            // transfer (or a full disk) leaves the original file intact
            // rather than truncated, and `mv` within one directory is
            // atomic.
            body.push_str(&format!(
                "mkdir -p \"$d\" && printf %s \"{b64}\" | $B > \"$p.fleet-tmp\" && mv -f \"$p.fleet-tmp\" \"$p\" && echo \"OK {report}\" || {{ rm -f \"$p.fleet-tmp\"; echo \"FAIL {report}\"; }}\n"
            ));
        }
        body.push_str("fi\n");
        if payload >= CHUNK_B64_BYTES {
            out.push(format!("{}{body}", script_preamble()));
            body.clear();
            payload = 0;
        }
    }
    if !body.is_empty() {
        out.push(format!("{}{body}", script_preamble()));
    }
    out
}

/// Parse the `OK`/`CONFLICT`/`FAIL <~/path>` lines a write script prints.
/// Anything else on stdout (a login shell's chatter) is ignored.
pub fn parse_write_output(stdout: &str) -> BTreeMap<String, WriteOutcome> {
    let mut out = BTreeMap::new();
    for line in stdout.lines() {
        let Some((tag, path)) = line.trim().split_once(' ') else {
            continue;
        };
        let outcome = match tag {
            "OK" => WriteOutcome::Ok,
            "CONFLICT" => WriteOutcome::Conflict,
            "FAIL" => WriteOutcome::Failed,
            _ => continue,
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        out.insert(path.to_string(), outcome);
    }
    out
}

/// A script printing `HASH <~/path> <hash|absent>` for each of `paths`, used
/// to compare-and-swap a file whose content is a secret (and so cannot be
/// embedded in a script at all).
fn hash_check_script(paths: &[String]) -> String {
    let mut s = script_preamble();
    for path in paths {
        if !is_safe_path(path) {
            continue;
        }
        s.push_str(&format!("p={}\n", host_path(path)));
        s.push_str(&format!("printf %s \"HASH {path} \"; h \"$p\"\n"));
    }
    s
}

/// A script that backs up each of `paths` that exists, using the same
/// `<file>.fleet-bak-<epoch>-<pid>` name a write script would. Needed because
/// a secret file is uploaded out of band (`write_host_file_secret`) rather
/// than by a script that could back it up inline.
fn backup_script(paths: &[String]) -> String {
    let mut s = script_preamble();
    for path in paths {
        if !is_safe_path(path) {
            continue;
        }
        s.push_str(&format!("p={}\n", host_path(path)));
        s.push_str(&backup_and_prune());
    }
    s
}

/// Parse [`hash_check_script`]'s output into `~/path` → hash (`absent`
/// becomes `None`).
fn parse_hash_output(stdout: &str) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    for line in stdout.lines() {
        let Some(rest) = line.trim().strip_prefix("HASH ") else {
            continue;
        };
        let Some((path, hash)) = rest.rsplit_once(' ') else {
            continue;
        };
        let hash = hash.trim();
        out.insert(
            path.trim().to_string(),
            if hash == "absent" {
                None
            } else {
                Some(hash.to_string())
            },
        );
    }
    out
}

/// What the harness CLI said about a plugin operation.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginCliOutcome {
    /// The host has no `claude` on `PATH`.
    NoCli,
    /// The last line that parsed as JSON.
    Json(serde_json::Value),
    /// The CLI ran but said nothing machine-readable.
    Unparsed,
}

/// `NOCLI` beats everything; otherwise the LAST line that parses as JSON
/// wins (the CLI prints progress before its `--json` result).
pub fn parse_plugin_output(stdout: &str) -> PluginCliOutcome {
    let mut found: Option<serde_json::Value> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line == "NOCLI" {
            return PluginCliOutcome::NoCli;
        }
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            found = Some(v);
        }
    }
    match found {
        Some(v) => PluginCliOutcome::Json(v),
        None => PluginCliOutcome::Unparsed,
    }
}

/// Is `t` safe to interpolate unquoted into a plugin CLI script? Plugin,
/// marketplace and repo names come from the catalog, so this is a guard
/// against a malformed asset, not a trust boundary — but it is the only
/// thing between a catalog typo and a shell metacharacter.
fn is_safe_token(t: &str) -> bool {
    !t.is_empty()
        && !t.starts_with('-')
        && !t.contains("..")
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | '@' | ':'))
}

/// The script for one plugin CLI operation (`install` / `update` /
/// `uninstall`) on `spec` (`<plugin>@<marketplace>`), adding `repo` as a
/// marketplace first when given (idempotent, hence `|| true`). `None` when
/// any token is unsafe. Contains no single quote.
fn plugin_script(repo: Option<&str>, spec: &str, op: &str) -> Option<String> {
    if !is_safe_token(spec) || !is_safe_token(op) || repo.is_some_and(|r| !is_safe_token(r)) {
        return None;
    }
    let mut s = String::new();
    s.push_str("command -v claude >/dev/null 2>&1 || { echo NOCLI; exit 0; }\n");
    if let Some(repo) = repo {
        s.push_str(&format!(
            "claude plugin marketplace add {repo} --scope user >/dev/null 2>&1 || true\n"
        ));
    }
    s.push_str(&format!(
        "claude plugin {op} {spec} --scope user --json -y 2>&1 || true\n"
    ));
    Some(s)
}

/// The text `Harness::merge_config` should treat as the file's current
/// content, rebuilt from the plan's parsed snapshot (the raw bytes are not
/// kept — only a hash and the parsed value). `Err` when the snapshot cannot
/// be re-serialised, which must block the merge rather than silently write
/// an empty document over a file we failed to understand.
fn existing_config_text(file: &str, snap: &HostSnapshot) -> Result<String, String> {
    let Some(value) = snap.configs.get(file) else {
        // Hashed by the scan but not parsed by it: the file is there and
        // fleet does not understand it. Treating that as "missing" would
        // rewrite a malformed-but-real config from an empty document,
        // throwing away whatever the user has in it.
        if snap.files.contains_key(file) {
            return Err(format!(
                "{file} could not be parsed on the host; not written"
            ));
        }
        return Ok(String::new());
    };
    if file.ends_with(".toml") {
        let toml_value = toml::Value::try_from(value.clone())
            .map_err(|e| format!("{file} cannot be re-serialised as TOML: {e}"))?;
        toml::to_string_pretty(&toml_value)
            .map_err(|e| format!("{file} cannot be re-serialised as TOML: {e}"))
    } else {
        serde_json::to_string_pretty(value)
            .map_err(|e| format!("{file} cannot be re-serialised as JSON: {e}"))
    }
}

/// Should `rm` (a merge a previous sync applied) still be un-applied, given
/// that this sync applies `adds` to the same file? `merge_config` removes
/// AFTER it adds, so a stale removal at a path this sync also writes would
/// undo the write: an identical `AppendUnique` element would be added and
/// then dropped again, and a `Set`/`Subset` removal drops the whole key.
fn keep_removal(rm: &ManifestMerge, adds: &[ConfigMerge]) -> bool {
    !adds.iter().any(|a| {
        a.json_path == rm.json_path
            && match rm.mode {
                MergeMode::AppendUnique => {
                    a.mode == MergeMode::AppendUnique && value_hash(&a.value) == rm.value_hash
                }
                // A new merge owns this path now; dropping the key after
                // writing it would leave the config without it.
                MergeMode::Set | MergeMode::Subset => true,
            }
    })
}

/// The manifest entry recorded for a plugin action. A plugin ref writes no
/// files (the CLI does the installing), so the entry exists to say "fleet
/// manages this", to carry the `<plugin>@<marketplace>` key a later removal
/// needs in order to uninstall it, and — via `merge_value`'s hash — which
/// catalog pin fleet last applied, so the next plan can tell a pin change
/// from a CLI that landed on the wrong version (see `plan::plugin_op`).
fn plugin_entry(
    target: &PluginTarget,
    merge_value: &serde_json::Value,
    now: i64,
) -> ManifestEntry {
    ManifestEntry {
        hash: String::new(),
        files: Vec::new(),
        merges: vec![ManifestMerge {
            file: PLUGINS_PATH.to_string(),
            json_path: vec![
                "plugins".to_string(),
                format!("{}@{}", target.plugin, target.marketplace_name),
            ],
            mode: MergeMode::Subset,
            value_hash: value_hash(merge_value),
        }],
        synced_at: now,
    }
}

/// The plugin merge value to hash into the manifest entry when the action
/// carries no plan. Must equal exactly what `harness::claude`'s render
/// produces for this target's pin (`[{"version": v}]`) or `latest` (`[{}]`),
/// since a later plan reads the manifest's hash as "the pin fleet last
/// applied" to decide `plugin_update` vs `blocked` (`plan::plugin_op`).
fn plugin_fallback_value(target: &PluginTarget) -> serde_json::Value {
    if target.version == "latest" {
        serde_json::json!([{}])
    } else {
        serde_json::json!([{"version": target.version}])
    }
}

/// `<plugin>@<marketplace>` from a manifest entry written by
/// [`plugin_entry`] (or by the equivalent `plugins.<spec>` merge).
fn plugin_spec_from_entry(entry: &ManifestEntry) -> Option<String> {
    entry
        .merges
        .iter()
        .find(|m| m.file == PLUGINS_PATH)
        .and_then(|m| m.json_path.last().cloned())
}

/// Working state for one action while the applier runs. `outcome: None`
/// means "still on track": every step that cannot finish an action sets it,
/// and whatever is still `None` at the end is `done`.
struct Work {
    outcome: Option<&'static str>,
    detail: Option<String>,
}

impl Work {
    fn set(&mut self, outcome: &'static str, detail: impl Into<String>) {
        self.outcome = Some(outcome);
        self.detail = Some(detail.into());
    }
    fn pending(&self) -> bool {
        self.outcome.is_none()
    }
}

/// One secret-bearing write: content that must never appear in a script.
struct SecretJob {
    actions: Vec<usize>,
    path: String,
    content: String,
    expected: Option<String>,
    backup: bool,
}

fn is_plugin_kind(action: &Action) -> bool {
    action.kind == super::super::model::Kind::PluginRef.as_str()
}

/// `"<kind>/<name>"` — the same key `Manifest::key` builds, without needing
/// to map the action's kind string back to a `Kind`.
fn manifest_key(action: &Action) -> String {
    format!("{}/{}", action.kind, action.name)
}

/// Apply one host's plan. Never returns an error: everything that can go
/// wrong is reported per action (and rolled up into `status`), because one
/// unreachable host or one conflicted file must not abort the fleet-wide
/// sync around it.
pub async fn apply_host(
    ctx: &ApplyCtx<'_>,
    harness: &dyn Harness,
    plan: &HostPlan,
) -> HostSyncResult {
    let host = plan.host_alias.as_str();
    if plan.status != "planned" {
        return HostSyncResult {
            host_alias: plan.host_alias.clone(),
            harness: plan.harness.clone(),
            status: SKIPPED.to_string(),
            detail: plan.detail.clone(),
            restart_required: false,
            actions: plan
                .actions
                .iter()
                .map(|a| result_of(a, SKIPPED, plan.detail.clone()))
                .collect(),
        };
    }

    // Step 1: everything already decided by the planner.
    let mut work: Vec<Work> = plan
        .actions
        .iter()
        .map(|a| match a.op {
            ActionOp::Blocked => Work {
                outcome: Some(BLOCKED),
                detail: a.reason.clone(),
            },
            ActionOp::Noop => Work {
                outcome: Some(SKIPPED),
                detail: None,
            },
            _ => Work {
                outcome: None,
                detail: None,
            },
        })
        .collect();

    let mut host_detail: Option<String> = None;
    let mut script_failed = false;
    let mut cancelled = false;

    // Step 2: plain files (and the deletions a Remove or a shrunken asset
    // implies), all in one batch of compare-and-swap scripts.
    let writes = collect_file_writes(plan, &mut work);
    if !writes.is_empty() && !ctx.token.is_cancelled() {
        let scripts = write_scripts(&writes.iter().map(|(_, w)| w.clone()).collect::<Vec<_>>());
        let mut results: BTreeMap<String, WriteOutcome> = BTreeMap::new();
        for script in &scripts {
            match inventory::run_host_script_with(
                ctx.ssh,
                host,
                script,
                APPLY_WALL_CLOCK,
                &ctx.token,
            )
            .await
            {
                Ok(out) => results.extend(parse_write_output(&out)),
                Err(e) => {
                    script_failed = true;
                    host_detail.get_or_insert(e.message.clone());
                    break;
                }
            }
        }
        apply_write_results(&writes, &results, &mut work, host_detail.as_deref());
    }
    cancelled |= ctx.token.is_cancelled();

    // Step 3: files whose bytes are a secret — hash-checked, then uploaded
    // out of band so the content never reaches a script or an argv.
    if !cancelled {
        let jobs = collect_secret_files(plan, &mut work);
        run_secret_jobs(ctx, host, &jobs, &mut work, &mut host_detail).await;
        cancelled |= ctx.token.is_cancelled();
    }

    // Step 4: config merges, one rewrite per file across every action that
    // touches it.
    if !cancelled {
        apply_config_merges(ctx, harness, plan, &mut work, &mut host_detail).await;
        cancelled |= ctx.token.is_cancelled();
    }

    // Step 5: plugins (the harness CLI, not files).
    if !cancelled {
        apply_plugins(ctx, plan, &mut work).await;
        cancelled |= ctx.token.is_cancelled();
    }

    // Everything still pending finished — unless the sync was cancelled,
    // in which case whatever was not reached never ran.
    for w in work.iter_mut() {
        if w.pending() {
            if cancelled {
                w.set(SKIPPED, "cancelled");
            } else {
                w.outcome = Some(DONE);
            }
        }
    }

    // Step 6: the manifest. Fleet owns this file, so it is written without a
    // compare-and-swap — but only when nothing failed, so a half-applied
    // asset is never recorded as synced.
    let mut manifest_failed = false;
    if !cancelled {
        match build_manifest(plan, &work, ctx.now) {
            Some(mut manifest) if !work.iter().any(|w| w.outcome == Some(FAILED)) => {
                let dropped = manifest.drop_unparseable();
                if !dropped.is_empty() {
                    tracing::warn!(host = %host, dropped = ?dropped, "manifest: dropped unparseable keys");
                }
                let path = harness.manifest_path();
                let dir = parent_dir(path);
                if let Err(e) =
                    provision::write_host_file(&**ctx.ssh, host, &dir, path, &manifest.to_json())
                        .await
                {
                    manifest_failed = true;
                    host_detail.get_or_insert(format!("manifest not written: {}", e.message));
                }
            }
            _ => {}
        }
    }

    // Step 7: roll up.
    let actions: Vec<ActionResult> = plan
        .actions
        .iter()
        .zip(work.iter())
        .map(|(a, w)| result_of(a, w.outcome.unwrap_or(DONE), w.detail.clone()))
        .collect();
    let any_done = actions.iter().any(|r| r.outcome == DONE);
    let any_bad = actions
        .iter()
        .any(|r| r.outcome == FAILED || r.outcome == CONFLICT);
    let status = if cancelled {
        "partial"
    } else if script_failed && !any_done {
        "failed"
    } else if any_bad || manifest_failed {
        "partial"
    } else {
        "applied"
    };
    // An `Adopt` changed nothing on the host — only the manifest — so it
    // never asks the user to restart anything.
    let restart_required = plan.actions.iter().zip(actions.iter()).any(|(a, r)| {
        r.outcome == DONE
            && matches!(a.kind.as_str(), "hook" | "mcp_server" | "plugin_ref")
            && matches!(
                a.op,
                ActionOp::Create
                    | ActionOp::Update
                    | ActionOp::Overwrite
                    | ActionOp::Remove
                    | ActionOp::PluginInstall
                    | ActionOp::PluginUpdate
            )
    });
    tracing::info!(
        host,
        harness = harness.id(),
        status,
        done = actions.iter().filter(|r| r.outcome == DONE).count(),
        conflicts = actions.iter().filter(|r| r.outcome == CONFLICT).count(),
        failed = actions.iter().filter(|r| r.outcome == FAILED).count(),
        "applied catalog sync to host"
    );
    HostSyncResult {
        host_alias: plan.host_alias.clone(),
        harness: plan.harness.clone(),
        status: status.to_string(),
        detail: if cancelled {
            Some("cancelled".to_string())
        } else {
            host_detail
        },
        restart_required,
        actions,
    }
}

fn result_of(action: &Action, outcome: &str, detail: Option<String>) -> ActionResult {
    ActionResult {
        kind: action.kind.clone(),
        name: action.name.clone(),
        op: action.op,
        outcome: outcome.to_string(),
        detail,
    }
}

/// Every plain-file write and deletion this plan implies, tagged with the
/// action that wants it. An action naming a path this module refuses to
/// interpolate fails outright, with none of its writes queued.
fn collect_file_writes(plan: &HostPlan, work: &mut [Work]) -> Vec<(usize, GuardedWrite)> {
    let mut writes: Vec<(usize, GuardedWrite)> = Vec::new();
    for (i, action) in plan.actions.iter().enumerate() {
        if !work[i].pending() {
            continue;
        }
        let mut bad: Option<String> = None;
        match action.op {
            ActionOp::Create | ActionOp::Update | ActionOp::Overwrite => {
                let Some(secret_plan) = &action.plan else {
                    continue;
                };
                let rendered = secret_plan.inner();
                let planned: BTreeSet<&str> =
                    rendered.files.iter().map(|f| f.path.as_str()).collect();
                for f in &rendered.files {
                    if action.secret_files.contains(&f.path) {
                        continue; // step 3 uploads these
                    }
                    if !is_safe_path(&f.path) {
                        bad = Some(f.path.clone());
                        break;
                    }
                    writes.push((
                        i,
                        GuardedWrite {
                            path: f.path.clone(),
                            expected: action.expected.get(&f.path).cloned().flatten(),
                            bytes: f.bytes.clone(),
                            backup: action.backup,
                            delete: false,
                        },
                    ));
                }
                // A file the previous sync wrote that this one no longer
                // renders (a resource dropped from the asset) is deleted in
                // the same batch, backed up first.
                if bad.is_none() {
                    if let Some(prev) = &action.remove_entry {
                        for path in &prev.files {
                            if planned.contains(path.as_str()) {
                                continue;
                            }
                            if !is_safe_path(path) {
                                bad = Some(path.clone());
                                break;
                            }
                            writes.push((i, delete_write(path, plan, action)));
                        }
                    }
                }
            }
            ActionOp::Remove if !is_plugin_kind(action) => {
                let Some(entry) = &action.remove_entry else {
                    continue;
                };
                for path in &entry.files {
                    if !is_safe_path(path) {
                        bad = Some(path.clone());
                        break;
                    }
                    writes.push((i, delete_write(path, plan, action)));
                }
            }
            _ => {}
        }
        if let Some(path) = bad {
            writes.retain(|(idx, _)| *idx != i);
            work[i].set(FAILED, format!("unsafe path {path}"));
        }
    }
    writes
}

fn delete_write(path: &str, plan: &HostPlan, action: &Action) -> GuardedWrite {
    GuardedWrite {
        path: path.to_string(),
        expected: action
            .expected
            .get(path)
            .cloned()
            .unwrap_or_else(|| plan.snapshot.files.get(path).cloned()),
        bytes: Vec::new(),
        backup: true,
        delete: true,
    }
}

/// Fold the host's `OK`/`CONFLICT`/`FAIL` lines back into per-action
/// outcomes: one conflict conflicts the whole action, one failure fails it,
/// and a path with no line at all (a script that never ran) fails it too.
fn apply_write_results(
    writes: &[(usize, GuardedWrite)],
    results: &BTreeMap<String, WriteOutcome>,
    work: &mut [Work],
    host_detail: Option<&str>,
) {
    for (i, w) in writes {
        if !work[*i].pending() {
            continue;
        }
        match results.get(&w.path) {
            Some(WriteOutcome::Ok) => {}
            Some(WriteOutcome::Conflict) => work[*i].set(
                CONFLICT,
                format!("{} changed on the host since the plan was computed", w.path),
            ),
            Some(WriteOutcome::Failed) => {
                work[*i].set(FAILED, format!("could not write {}", w.path))
            }
            None => work[*i].set(
                FAILED,
                match host_detail {
                    Some(d) => d.to_string(),
                    None => format!("no result for {}", w.path),
                },
            ),
        }
    }
}

/// The secret-bearing file writes this plan implies.
fn collect_secret_files(plan: &HostPlan, work: &mut [Work]) -> Vec<SecretJob> {
    let mut jobs: Vec<SecretJob> = Vec::new();
    for (i, action) in plan.actions.iter().enumerate() {
        if !work[i].pending()
            || !matches!(
                action.op,
                ActionOp::Create | ActionOp::Update | ActionOp::Overwrite
            )
        {
            continue;
        }
        let Some(secret_plan) = &action.plan else {
            continue;
        };
        for f in &secret_plan.inner().files {
            if !action.secret_files.contains(&f.path) {
                continue;
            }
            if !is_safe_path(&f.path) {
                work[i].set(FAILED, format!("unsafe path {}", f.path));
                break;
            }
            // Substitution is textual, so a file carrying a secret is text.
            let Ok(content) = String::from_utf8(f.bytes.clone()) else {
                work[i].set(FAILED, format!("{} is not valid UTF-8", f.path));
                break;
            };
            jobs.push(SecretJob {
                actions: vec![i],
                path: f.path.clone(),
                content,
                expected: action.expected.get(&f.path).cloned().flatten(),
                backup: action.backup,
            });
        }
    }
    jobs
}

/// Hash-check every secret path in one script, then upload the ones that
/// still match through `write_host_file_secret`.
async fn run_secret_jobs(
    ctx: &ApplyCtx<'_>,
    host: &str,
    jobs: &[SecretJob],
    work: &mut [Work],
    host_detail: &mut Option<String>,
) {
    if jobs.is_empty() {
        return;
    }
    let paths: Vec<String> = jobs.iter().map(|j| j.path.clone()).collect();
    let current = match inventory::run_host_script_with(
        ctx.ssh,
        host,
        &hash_check_script(&paths),
        APPLY_WALL_CLOCK,
        &ctx.token,
    )
    .await
    {
        Ok(out) => parse_hash_output(&out),
        Err(e) => {
            host_detail.get_or_insert(e.message.clone());
            for job in jobs {
                for i in &job.actions {
                    work[*i].set(FAILED, e.message.clone());
                }
            }
            return;
        }
    };
    // Back up everything that still matches, in one pass, before any
    // upload replaces it.
    let to_back_up: Vec<String> = jobs
        .iter()
        .filter(|j| {
            j.backup
                && j.actions.iter().any(|i| work[*i].pending())
                && current.get(&j.path).cloned().unwrap_or(None) == j.expected
                && current.get(&j.path).cloned().unwrap_or(None).is_some()
        })
        .map(|j| j.path.clone())
        .collect();
    if !to_back_up.is_empty() {
        if let Err(e) = inventory::run_host_script_with(
            ctx.ssh,
            host,
            &backup_script(&to_back_up),
            APPLY_WALL_CLOCK,
            &ctx.token,
        )
        .await
        {
            host_detail.get_or_insert(e.message.clone());
            for job in jobs.iter().filter(|j| to_back_up.contains(&j.path)) {
                for i in &job.actions {
                    work[*i].set(FAILED, format!("could not back up {}", job.path));
                }
            }
        }
    }

    for job in jobs {
        if job.actions.iter().all(|i| !work[*i].pending()) {
            continue;
        }
        let seen = current.get(&job.path).cloned().unwrap_or(None);
        if seen != job.expected {
            for i in &job.actions {
                work[*i].set(
                    CONFLICT,
                    format!(
                        "{} changed on the host since the plan was computed",
                        job.path
                    ),
                );
            }
            continue;
        }
        let dir = parent_dir(&job.path);
        // Never log or report `content`; only the path.
        if let Err(e) =
            provision::write_host_file_secret(&**ctx.ssh, host, &dir, &job.path, &job.content).await
        {
            for i in &job.actions {
                work[*i].set(FAILED, format!("could not write {}: {}", job.path, e.code));
            }
        }
    }
}

/// One config file's worth of merges, gathered across every action.
#[derive(Default)]
struct MergeJob {
    actions: Vec<usize>,
    adds: Vec<ConfigMerge>,
    removes: Vec<ManifestMerge>,
}

/// Step 4: rewrite each touched config file once, applying this sync's
/// merges and un-applying the previous manifest entry's.
async fn apply_config_merges(
    ctx: &ApplyCtx<'_>,
    harness: &dyn Harness,
    plan: &HostPlan,
    work: &mut [Work],
    host_detail: &mut Option<String>,
) {
    let mut jobs: BTreeMap<String, MergeJob> = BTreeMap::new();
    for (i, action) in plan.actions.iter().enumerate() {
        if !work[i].pending() || is_plugin_kind(action) {
            continue;
        }
        // An `Adopt` is manifest-only: its merges are already satisfied on
        // the host, and rewriting the config file to re-apply them would
        // reformat (and back up) a file nothing asked us to change.
        let writes_merges = matches!(
            action.op,
            ActionOp::Create | ActionOp::Update | ActionOp::Overwrite
        );
        if let Some(secret_plan) = action.plan.as_ref().filter(|_| writes_merges) {
            for m in &secret_plan.inner().merges {
                let job = jobs.entry(m.file.clone()).or_default();
                job.adds.push(m.clone());
                if !job.actions.contains(&i) {
                    job.actions.push(i);
                }
            }
        }
        if let Some(prev) = &action.remove_entry {
            for m in &prev.merges {
                let job = jobs.entry(m.file.clone()).or_default();
                job.removes.push(m.clone());
                if !job.actions.contains(&i) {
                    job.actions.push(i);
                }
            }
        }
    }
    if jobs.is_empty() {
        return;
    }

    let mut secret_jobs: Vec<SecretJob> = Vec::new();
    for (file, job) in &jobs {
        if job.actions.iter().all(|i| !work[*i].pending()) {
            continue;
        }
        if !is_safe_path(file) {
            for i in &job.actions {
                work[*i].set(FAILED, format!("unsafe path {file}"));
            }
            continue;
        }
        let existing = match existing_config_text(file, &plan.snapshot) {
            Ok(text) => text,
            Err(msg) => {
                for i in &job.actions {
                    work[*i].set(BLOCKED, msg.clone());
                }
                continue;
            }
        };
        let removes: Vec<ManifestMerge> = job
            .removes
            .iter()
            .filter(|rm| keep_removal(rm, &job.adds))
            .cloned()
            .collect();
        let merged = match harness.merge_config(file, &existing, &job.adds, &removes) {
            Ok(text) => text,
            Err(e) => {
                for i in &job.actions {
                    work[*i].set(BLOCKED, e.message.clone());
                }
                continue;
            }
        };
        let expected = plan.snapshot.files.get(file).cloned();
        // The whole file is rewritten from its parsed form, so an existing
        // one is always backed up first — that rewrite is what a backup is
        // for (TOML comments, unknown-but-valid keys fleet reformats).
        let backup = plan.snapshot.files.contains_key(file);
        // ALWAYS the secret path, regardless of whether THIS sync's merge
        // carries a secret: the file may already hold one (an earlier
        // sync's bearer token, or the user's own API key), and embedding
        // that base64'd in a `bash -lc` command line would expose it in the
        // host's process table. The cost is that a synced config file ends
        // up 0600.
        secret_jobs.push(SecretJob {
            actions: job.actions.clone(),
            path: file.clone(),
            content: merged,
            expected,
            backup,
        });
    }

    run_secret_jobs(ctx, host_of(plan), &secret_jobs, work, host_detail).await;
}

fn host_of(plan: &HostPlan) -> &str {
    plan.host_alias.as_str()
}

/// Step 5: install / update / uninstall plugins through the harness CLI.
async fn apply_plugins(ctx: &ApplyCtx<'_>, plan: &HostPlan, work: &mut [Work]) {
    for (i, action) in plan.actions.iter().enumerate() {
        if !work[i].pending() {
            continue;
        }
        let (script, target) = match action.op {
            ActionOp::PluginInstall | ActionOp::PluginUpdate => {
                let Some(target) = &action.plugin else {
                    work[i].set(FAILED, "no plugin target on a plugin action");
                    continue;
                };
                let spec = format!("{}@{}", target.plugin, target.marketplace_name);
                let op = if action.op == ActionOp::PluginInstall {
                    "install"
                } else {
                    "update"
                };
                match plugin_script(Some(&target.marketplace_repo), &spec, op) {
                    Some(s) => (s, Some(target)),
                    None => {
                        work[i].set(FAILED, "plugin or marketplace name is not safe to install");
                        continue;
                    }
                }
            }
            ActionOp::Remove if is_plugin_kind(action) => {
                let spec = action
                    .remove_entry
                    .as_ref()
                    .and_then(plugin_spec_from_entry);
                let Some(spec) = spec else {
                    work[i].set(
                        FAILED,
                        "the manifest does not say which plugin to uninstall",
                    );
                    continue;
                };
                match plugin_script(None, &spec, "uninstall") {
                    Some(s) => (s, None),
                    None => {
                        work[i].set(FAILED, "plugin name is not safe to uninstall");
                        continue;
                    }
                }
            }
            _ => continue,
        };
        let out = match inventory::run_host_script_with(
            ctx.ssh,
            host_of(plan),
            &script,
            APPLY_WALL_CLOCK,
            &ctx.token,
        )
        .await
        {
            Ok(out) => out,
            Err(e) => {
                work[i].set(FAILED, e.message);
                continue;
            }
        };
        match parse_plugin_output(&out) {
            PluginCliOutcome::NoCli => work[i].set(BLOCKED, "claude CLI not found on host"),
            PluginCliOutcome::Json(v) => {
                if let Some(msg) = plugin_error(&v) {
                    work[i].set(FAILED, msg);
                    continue;
                }
                if let Some(target) = target {
                    match installed_version(ctx, host_of(plan), target).await {
                        Some(v) if v == target.version || target.version == "latest" => {
                            work[i].detail = Some(format!("installed {v}"))
                        }
                        Some(v) => {
                            work[i].detail =
                                Some(format!("installed {v}, catalog pins {}", target.version))
                        }
                        None => work[i].set(
                            FAILED,
                            "the CLI reported success but the plugin is not recorded on the host",
                        ),
                    }
                }
            }
            PluginCliOutcome::Unparsed => {
                work[i].set(FAILED, "the plugin CLI printed no JSON result")
            }
        }
    }
}

/// The CLI's error message, if its JSON result reports one.
fn plugin_error(v: &serde_json::Value) -> Option<String> {
    if v.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        return Some(
            v.get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("the plugin CLI reported failure")
                .to_string(),
        );
    }
    v.get("error")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Read the version the host's plugin record now carries.
async fn installed_version(
    ctx: &ApplyCtx<'_>,
    host: &str,
    target: &PluginTarget,
) -> Option<String> {
    let text = provision::read_host_file(&**ctx.ssh, host, PLUGINS_PATH)
        .await
        .ok()?;
    let root: serde_json::Value = serde_json::from_str(&text).ok()?;
    root.get("plugins")?
        .get(format!("{}@{}", target.plugin, target.marketplace_name))?
        .as_array()?
        .first()?
        .get("version")?
        .as_str()
        .map(str::to_string)
}

/// The manifest to write, or `None` when this host's sync changed nothing
/// the manifest records.
fn build_manifest(plan: &HostPlan, work: &[Work], now: i64) -> Option<Manifest> {
    let mut manifest = plan.manifest.clone();
    let mut changed = false;
    for (action, w) in plan.actions.iter().zip(work.iter()) {
        if w.outcome != Some(DONE) {
            continue;
        }
        let key = manifest_key(action);
        match action.op {
            ActionOp::Remove => {
                changed |= manifest.assets.remove(&key).is_some();
            }
            ActionOp::Create
            | ActionOp::Update
            | ActionOp::Overwrite
            | ActionOp::Adopt
            | ActionOp::PluginInstall
            | ActionOp::PluginUpdate => {
                let entry = match (&action.plugin, &action.plan) {
                    (Some(target), plan) => {
                        // The planner attaches the render plan to every
                        // plugin action that reaches here; the fallback
                        // mirrors `harness::claude`'s render in case it ever
                        // does not, so the hash stays correct either way.
                        let merge_value = plan
                            .as_ref()
                            .and_then(|p| p.inner().merges.first())
                            .map(|m| m.value.clone())
                            .unwrap_or_else(|| plugin_fallback_value(target));
                        plugin_entry(target, &merge_value, now)
                    }
                    (None, Some(secret_plan)) => {
                        Manifest::entry_for(&secret_plan.inner().hash(), secret_plan.inner(), now)
                    }
                    (None, None) => continue,
                };
                changed |= manifest.assets.get(&key) != Some(&entry);
                manifest.assets.insert(key, entry);
            }
            ActionOp::Noop | ActionOp::Blocked => {}
        }
    }
    if !changed {
        return None;
    }
    manifest.updated_at = now;
    Some(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::claude::Claude;
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::repo::{self, Catalog};
    use crate::service::catalog::sync::plan::{self, PlanFilter};

    fn w(path: &str, bytes: &[u8]) -> GuardedWrite {
        GuardedWrite {
            path: path.to_string(),
            expected: None,
            bytes: bytes.to_vec(),
            backup: false,
            delete: false,
        }
    }

    #[test]
    fn safe_paths_reject_traversal_and_metacharacters() {
        assert!(is_safe_path("~/.claude/skills/s/SKILL.md"));
        assert!(!is_safe_path("/etc/passwd"));
        assert!(!is_safe_path("~/../../etc/passwd"));
        assert!(!is_safe_path("~/a/../b"));
        assert!(!is_safe_path("~/a b"));
        assert!(!is_safe_path("~/a$(id)"));
        assert!(!is_safe_path("~/a\"b"));
        assert!(!is_safe_path("~/a'b"));
        assert!(!is_safe_path("~/"));
        assert_eq!(host_path("~/.claude/x"), "\"$HOME\"/.claude/x");
        assert_eq!(
            parent_dir("~/.claude/skills/s/SKILL.md"),
            "~/.claude/skills/s"
        );
    }

    #[test]
    fn write_script_for_a_create_has_no_backup_line() {
        let scripts = write_scripts(&[w("~/.claude/skills/s/SKILL.md", b"body\n")]);
        assert_eq!(scripts.len(), 1);
        let s = &scripts[0];
        assert!(s.contains("p=\"$HOME\"/.claude/skills/s/SKILL.md;"));
        assert!(s.contains("if [ \"$cur\" != \"absent\" ]; then echo \"CONFLICT ~/.claude/skills/s/SKILL.md\"; else"));
        assert!(!s.contains("fleet-bak"), "a create replaces nothing");
        assert!(s.contains("mkdir -p \"$d\" && printf %s \"Ym9keQo=\" | $B > \"$p.fleet-tmp\" && mv -f \"$p.fleet-tmp\" \"$p\" && echo \"OK ~/.claude/skills/s/SKILL.md\" || { rm -f \"$p.fleet-tmp\"; echo \"FAIL ~/.claude/skills/s/SKILL.md\"; }"));
        assert!(!s.contains('\''), "no single quotes: {s}");
    }

    #[test]
    fn write_script_for_an_overwrite_backs_up_before_writing() {
        let scripts = write_scripts(&[GuardedWrite {
            expected: Some("deadbeef".into()),
            backup: true,
            ..w("~/.claude/skills/s/SKILL.md", b"new\n")
        }]);
        let s = &scripts[0];
        let backup = s
            .find("cp -p \"$p\" \"$p.fleet-bak-$T\"")
            .expect("backup line");
        let prune = s
            .find("ls -1t \"$p\".fleet-bak-* 2>/dev/null | tail -n +4 | while read -r f; do rm -f \"$f\"; done")
            .expect("prune line");
        let write = s.find("| $B > \"$p.fleet-tmp\"").expect("write line");
        assert!(backup < prune, "prune must follow the backup line");
        assert!(prune < write, "backup+prune must precede the write");
        assert!(s.contains("if [ \"$cur\" != \"deadbeef\" ]"));
        assert!(!s.contains('\''));
    }

    #[test]
    fn write_script_for_a_delete_removes_the_file_and_prunes_the_dir() {
        let scripts = write_scripts(&[GuardedWrite {
            expected: Some("abc".into()),
            backup: true,
            delete: true,
            ..w("~/.claude/skills/s/SKILL.md", b"")
        }]);
        let s = &scripts[0];
        assert!(s.contains("cp -p \"$p\" \"$p.fleet-bak-$T\""));
        assert!(s.contains(
            "ls -1t \"$p\".fleet-bak-* 2>/dev/null | tail -n +4 | while read -r f; do rm -f \"$f\"; done"
        ));
        assert!(s.contains("rm -f \"$p\" && { rmdir \"$d\" 2>/dev/null; echo \"OK ~/.claude/skills/s/SKILL.md\"; } || echo \"FAIL ~/.claude/skills/s/SKILL.md\""));
        assert!(
            !s.contains("| $B > \"$p.fleet-tmp\""),
            "a delete carries no payload"
        );
        assert!(!s.contains('\''));
    }

    #[test]
    fn backup_script_also_prunes_old_backups() {
        let s = backup_script(&["~/.claude/settings.json".to_string()]);
        assert!(s.contains("cp -p \"$p\" \"$p.fleet-bak-$T\""));
        assert!(s.contains(
            "ls -1t \"$p\".fleet-bak-* 2>/dev/null | tail -n +4 | while read -r f; do rm -f \"$f\"; done"
        ));
        assert!(!s.contains('\''));
    }

    #[test]
    fn write_scripts_chunk_on_payload_size() {
        let big = vec![b'x'; 300 * 1024];
        let writes: Vec<GuardedWrite> = (0..3)
            .map(|i| w(&format!("~/.claude/skills/s{i}/SKILL.md"), &big))
            .collect();
        let scripts = write_scripts(&writes);
        assert_eq!(scripts.len(), 2, "three 300 KB payloads split into two");
        assert!(scripts[0].contains("skills/s0/"));
        assert!(scripts[0].contains("skills/s1/"));
        assert!(scripts[1].contains("skills/s2/"));
        for s in &scripts {
            assert!(!s.contains('\''));
            assert!(s.starts_with("cd \"$HOME\" || exit 1\n"));
            assert!(s.contains("T=$(date +%s)-$$"));
        }
        // Deletions carry no payload and never force a split.
        let deletes: Vec<GuardedWrite> = (0..50)
            .map(|i| GuardedWrite {
                delete: true,
                ..w(&format!("~/.claude/skills/d{i}/SKILL.md"), b"")
            })
            .collect();
        assert_eq!(write_scripts(&deletes).len(), 1);
    }

    /// A hash is host-supplied. One that is not a hex digest is never
    /// interpolated into the compare-and-swap test — the write is reported
    /// as a conflict and skipped entirely.
    #[test]
    fn a_non_hex_expected_hash_conflicts_instead_of_being_interpolated() {
        let scripts = write_scripts(&[GuardedWrite {
            expected: Some("\"; rm -rf ~; echo \"".into()),
            ..w("~/.claude/skills/s/SKILL.md", b"body\n")
        }]);
        let s = &scripts[0];
        assert!(s.contains("echo \"CONFLICT ~/.claude/skills/s/SKILL.md\"\n"));
        assert!(!s.contains("rm -rf"), "{s}");
        assert!(!s.contains("Ym9keQo="), "nothing is written: {s}");
        assert!(!s.contains('\''));
        assert_eq!(
            parse_write_output(&format!("CONFLICT {}", "~/.claude/skills/s/SKILL.md"))
                ["~/.claude/skills/s/SKILL.md"],
            WriteOutcome::Conflict
        );
    }

    #[test]
    fn parse_write_output_reads_every_verdict_and_ignores_chatter() {
        let out = "\nmotd from a login shell\nOK ~/.claude/skills/a/SKILL.md\nCONFLICT ~/.claude/skills/b/SKILL.md\nFAIL ~/.claude/settings.json\nWAT ~/x\n";
        let parsed = parse_write_output(out);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed["~/.claude/skills/a/SKILL.md"], WriteOutcome::Ok);
        assert_eq!(
            parsed["~/.claude/skills/b/SKILL.md"],
            WriteOutcome::Conflict
        );
        assert_eq!(parsed["~/.claude/settings.json"], WriteOutcome::Failed);
        assert!(parse_write_output("").is_empty());
    }

    #[test]
    fn hash_check_script_and_its_output_round_trip() {
        let script = hash_check_script(&["~/.claude/settings.json".to_string()]);
        assert!(!script.contains('\''));
        assert!(script.contains("printf %s \"HASH ~/.claude/settings.json \"; h \"$p\""));
        let parsed = parse_hash_output("HASH ~/.claude/settings.json abc123\nHASH ~/x/y absent\n");
        assert_eq!(
            parsed["~/.claude/settings.json"],
            Some("abc123".to_string())
        );
        assert_eq!(parsed["~/x/y"], None);
    }

    #[test]
    fn plugin_scripts_are_quote_free_and_guard_on_the_cli() {
        let s = plugin_script(Some("owner/repo"), "superpowers@sp", "install").unwrap();
        assert!(s.starts_with("command -v claude >/dev/null 2>&1 || { echo NOCLI; exit 0; }\n"));
        assert!(s.contains(
            "claude plugin marketplace add owner/repo --scope user >/dev/null 2>&1 || true"
        ));
        assert!(
            s.contains("claude plugin install superpowers@sp --scope user --json -y 2>&1 || true")
        );
        assert!(!s.contains('\''));

        let u = plugin_script(None, "superpowers@sp", "uninstall").unwrap();
        assert!(!u.contains("marketplace add"));
        assert!(u.contains("claude plugin uninstall superpowers@sp --scope user --json -y"));

        assert!(plugin_script(Some("owner/repo; rm -rf /"), "a@b", "install").is_none());
        assert!(plugin_script(None, "a@b$(id)", "uninstall").is_none());
        assert!(plugin_script(None, "../evil", "uninstall").is_none());
        // A leading `-` would be read by the CLI as a flag, not a name.
        assert!(plugin_script(None, "-rf", "uninstall").is_none());
        assert!(plugin_script(Some("--help"), "a@b", "install").is_none());
    }

    #[test]
    fn plugin_output_parsing_prefers_nocli_then_the_last_json_line() {
        assert_eq!(parse_plugin_output("NOCLI\n"), PluginCliOutcome::NoCli);
        assert_eq!(
            parse_plugin_output("installing...\n{\"success\":true}\n"),
            PluginCliOutcome::Json(serde_json::json!({"success": true}))
        );
        // The LAST JSON line wins.
        assert_eq!(
            parse_plugin_output("{\"success\":false}\n{\"success\":true}\n"),
            PluginCliOutcome::Json(serde_json::json!({"success": true}))
        );
        assert_eq!(parse_plugin_output("boom\n"), PluginCliOutcome::Unparsed);
        assert_eq!(
            plugin_error(&serde_json::json!({"success": false, "error": "nope"})).as_deref(),
            Some("nope")
        );
        assert_eq!(plugin_error(&serde_json::json!({"success": true})), None);
    }

    /// The planner now attaches the rendered merge to every plugin action
    /// (install, adopt, update) so the applier can hash it into the
    /// manifest entry — this is what lets `plan::plugin_op` later tell a
    /// pin change from a CLI that landed on the wrong version.
    #[test]
    fn plugin_manifest_entry_carries_the_rendered_pin_hash() {
        let yaml = "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: mk, source: github, repo: o/r }\nplugin: sp\nversion: \"6.3.0\"\n";
        let catalog = Catalog {
            assets: vec![Asset::from_yaml(None, yaml).unwrap()],
            ..Default::default()
        };
        let work = vec![Work {
            outcome: Some(DONE),
            detail: None,
        }];
        let pinned_hash = value_hash(&serde_json::json!([{"version": "6.3.0"}]));

        // Install: nothing on the host yet.
        let hp = plan::compute_host_plan(
            &catalog,
            &Claude,
            "local",
            &HostSnapshot::default(),
            &Manifest::default(),
            &BTreeMap::new(),
            &PlanFilter::default(),
        );
        assert_eq!(hp.actions[0].op, ActionOp::PluginInstall);
        assert!(
            hp.actions[0].plan.is_some(),
            "the planner attaches the render plan to a plugin action"
        );
        let manifest = build_manifest(&hp, &work, 1_000).expect("install writes an entry");
        assert_eq!(
            manifest.assets.get("plugin_ref/sp").unwrap().merges[0].value_hash,
            pinned_hash
        );

        // Adopt: already installed at the pinned version, not yet managed.
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            PLUGINS_PATH.to_string(),
            serde_json::json!({"plugins": {"sp@mk": [{"version": "6.3.0"}]}}),
        );
        let hp = plan::compute_host_plan(
            &catalog,
            &Claude,
            "local",
            &snap,
            &Manifest::default(),
            &BTreeMap::new(),
            &PlanFilter::default(),
        );
        assert_eq!(hp.actions[0].op, ActionOp::Adopt);
        let manifest = build_manifest(&hp, &work, 1_000).expect("adopt writes an entry");
        assert_eq!(
            manifest.assets.get("plugin_ref/sp").unwrap().merges[0].value_hash,
            pinned_hash
        );

        // Update: installed at another version, never synced before.
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            PLUGINS_PATH.to_string(),
            serde_json::json!({"plugins": {"sp@mk": [{"version": "5.0.0"}]}}),
        );
        let hp = plan::compute_host_plan(
            &catalog,
            &Claude,
            "local",
            &snap,
            &Manifest::default(),
            &BTreeMap::new(),
            &PlanFilter::default(),
        );
        assert_eq!(hp.actions[0].op, ActionOp::PluginUpdate);
        let manifest = build_manifest(&hp, &work, 1_000).expect("update writes an entry");
        assert_eq!(
            manifest.assets.get("plugin_ref/sp").unwrap().merges[0].value_hash,
            pinned_hash,
            "the entry records the catalog's pin, not the version the CLI landed on"
        );
    }

    #[test]
    fn removals_that_a_new_merge_supersedes_are_dropped() {
        let add = ConfigMerge {
            file: "~/.claude/settings.json".into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value: serde_json::json!({"a": 1}),
        };
        let same = ManifestMerge {
            file: add.file.clone(),
            json_path: add.json_path.clone(),
            mode: MergeMode::AppendUnique,
            value_hash: value_hash(&add.value),
        };
        let stale = ManifestMerge {
            value_hash: value_hash(&serde_json::json!({"a": 0})),
            ..same.clone()
        };
        let adds = std::slice::from_ref(&add);
        assert!(!keep_removal(&same, adds), "would undo this sync's write");
        assert!(keep_removal(&stale, adds), "the old element must go");
        let set = ManifestMerge {
            mode: MergeMode::Set,
            ..same.clone()
        };
        assert!(!keep_removal(&set, adds), "the new merge owns the path");
        assert!(keep_removal(&stale, &[]), "nothing re-adds it");
    }

    #[test]
    fn existing_config_text_rebuilds_json_and_toml() {
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            serde_json::json!({"a": 1}),
        );
        snap.configs.insert(
            "~/.codex/config.toml".into(),
            serde_json::json!({"mcp_servers": {"fleet": {"url": "u"}}}),
        );
        let json = existing_config_text("~/.claude/settings.json", &snap).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            serde_json::json!({"a": 1})
        );
        let toml_text = existing_config_text("~/.codex/config.toml", &snap).unwrap();
        let back: toml::Value = toml::from_str(&toml_text).unwrap();
        assert_eq!(back["mcp_servers"]["fleet"]["url"].as_str(), Some("u"));
        assert_eq!(
            existing_config_text("~/.claude/nothing.json", &snap).unwrap(),
            ""
        );
        // Hashed by the scan but absent from `configs`: the file is there
        // and unparseable, which must block rather than read as empty.
        snap.files
            .insert("~/.claude/broken.json".into(), "a".repeat(64));
        let err = existing_config_text("~/.claude/broken.json", &snap).unwrap_err();
        assert!(err.contains("could not be parsed"), "{err}");
    }

    /// Restores `HOME` when the test (or a panic) ends: it is process-wide,
    /// and leaking a temp dir into it breaks every other test that spawns a
    /// child (see `provision::expand_home_local_expands_tilde`).
    struct HomeGuard(Option<String>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn write_catalog(root: &std::path::Path, files: &[(&str, &str)]) -> Catalog {
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        for (rel, body) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        repo::load_dir(root).unwrap()
    }

    async fn plan_for(ssh: &Arc<SshClient>, catalog: &Catalog) -> HostPlan {
        plan_with_secrets(ssh, catalog, &BTreeMap::new()).await
    }

    async fn plan_with_secrets(
        ssh: &Arc<SshClient>,
        catalog: &Catalog,
        secrets: &BTreeMap<String, String>,
    ) -> HostPlan {
        let snap = inventory::scan_host_harness(ssh, "local", &Claude)
            .await
            .expect("scan");
        let manifest = Manifest::from_snapshot(&snap, Claude.manifest_path());
        plan::compute_host_plan(
            catalog,
            &Claude,
            "local",
            &snap,
            &manifest,
            secrets,
            &PlanFilter::default(),
        )
    }

    fn read_json(p: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).expect("valid JSON")
    }

    fn backups(dir: &std::path::Path) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<String> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".fleet-bak-"))
            .collect();
        out.sort();
        out
    }

    /// The whole applier against a real temp `$HOME`, through the real scan
    /// script and the real planner: create → no-op → conflict on a stale
    /// plan → overwrite with a backup → remove an orphan with a backup →
    /// merge a hook into `settings.json` from nothing → no-op again.
    ///
    /// `CATALOG_TEST_LOCK` serialises the process-global `HOME` mutation
    /// against the other catalog tests; it guards nothing the runtime needs,
    /// so holding it across awaits is safe.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn applies_creates_conflicts_overwrites_removals_and_merges_locally() {
        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());

        let skill_repo = tempfile::tempdir().unwrap();
        let catalog = write_catalog(
            skill_repo.path(),
            &[
                (
                    "skills/s/asset.yaml",
                    "kind: skill\nname: s\ndescription: d\n",
                ),
                ("skills/s/body.md", "first body\n"),
            ],
        );
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 1_000,
        };
        let skill_dir = home.path().join(".claude/skills/s");
        let skill_file = skill_dir.join("SKILL.md");
        let manifest_file = home.path().join(".claude/.fleet-assets.json");

        // 1. Create.
        let create = plan_for(&ssh, &catalog).await;
        assert_eq!(create.actions.len(), 1);
        assert_eq!(create.actions[0].op, ActionOp::Create);
        let res = apply_host(&ctx, &Claude, &create).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert_eq!(res.actions[0].outcome, DONE);
        assert!(!res.restart_required, "a skill needs no restart");
        let written = std::fs::read_to_string(&skill_file).expect("skill written");
        assert!(written.contains("first body"));
        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
        assert_eq!(
            manifest.assets["skill/s"].files,
            vec!["~/.claude/skills/s/SKILL.md".to_string()]
        );
        assert_eq!(manifest.updated_at, 1_000);

        // 2. Applying again changes nothing: the plan is all no-ops.
        let again = plan_for(&ssh, &catalog).await;
        assert_eq!(again.actions[0].op, ActionOp::Noop);
        let res = apply_host(&ctx, &Claude, &again).await;
        assert_eq!(res.status, "applied");
        assert_eq!(res.actions[0].outcome, SKIPPED);

        // 3. A stale plan against a host that moved on: conflict, no write.
        std::fs::write(&skill_file, "hand edited\n").unwrap();
        let res = apply_host(&ctx, &Claude, &create).await;
        assert_eq!(res.status, "partial");
        assert_eq!(res.actions[0].outcome, CONFLICT);
        assert_eq!(
            std::fs::read_to_string(&skill_file).unwrap(),
            "hand edited\n"
        );

        // 4. A fresh plan overwrites it, keeping a backup of the edit.
        let overwrite = plan_for(&ssh, &catalog).await;
        assert_eq!(overwrite.actions[0].op, ActionOp::Overwrite);
        assert!(overwrite.actions[0].backup);
        let res = apply_host(&ctx, &Claude, &overwrite).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert!(std::fs::read_to_string(&skill_file)
            .unwrap()
            .contains("first body"));
        let baks = backups(&skill_dir);
        assert_eq!(baks.len(), 1, "{baks:?}");
        assert_eq!(
            std::fs::read_to_string(skill_dir.join(&baks[0])).unwrap(),
            "hand edited\n"
        );

        // 5. The asset leaves the catalog: its manifest entry becomes an
        //    orphan, and applying removes the file (after backing it up).
        let removal = plan_for(&ssh, &Catalog::default()).await;
        assert_eq!(removal.actions[0].op, ActionOp::Remove);
        let res = apply_host(&ctx, &Claude, &removal).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert!(!skill_file.exists(), "the file is gone");
        assert_eq!(backups(&skill_dir).len(), 2, "the removal backed it up too");
        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
        assert!(manifest.assets.is_empty(), "{manifest:?}");

        // 6. A merge-only asset writes a config file that does not exist yet.
        let hook_repo = tempfile::tempdir().unwrap();
        let hooks = write_catalog(
            hook_repo.path(),
            &[(
                "hooks/stop.yaml",
                "kind: hook\nname: stop\ndescription: d\nevent: stop\naction:\n  type: command\n  command: \"echo hi\"\n",
            )],
        );
        let merge = plan_for(&ssh, &hooks).await;
        assert_eq!(merge.actions[0].op, ActionOp::Create);
        let res = apply_host(&ctx, &Claude, &merge).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert!(res.restart_required, "a hook needs a restart");
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(".claude/settings.json")).unwrap(),
        )
        .expect("settings.json is valid JSON");
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo hi"
        );
        assert!(
            backups(&home.path().join(".claude")).is_empty(),
            "nothing existed to back up"
        );
        // EVERY config rewrite takes the 0600 upload path, not just a
        // secret-bearing one: the file may already hold someone else's
        // secret, which must not be re-embedded in a script.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.path().join(".claude/settings.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        // 7. And that too is a no-op the second time around.
        let after = plan_for(&ssh, &hooks).await;
        assert_eq!(after.actions[0].op, ActionOp::Noop);
        let res = apply_host(&ctx, &Claude, &after).await;
        assert_eq!(res.status, "applied");
        assert_eq!(res.actions[0].outcome, SKIPPED);

        // 8. Changing the hook must REPLACE the merged element, not leave
        //    the previous one behind beside it (the manifest entry this
        //    action supersedes is un-merged in the same guarded write).
        let hooks = write_catalog(
            hook_repo.path(),
            &[(
                "hooks/stop.yaml",
                "kind: hook\nname: stop\ndescription: d\nevent: stop\naction:\n  type: command\n  command: \"echo bye\"\n",
            )],
        );
        let update = plan_with_secrets(&ssh, &hooks, &BTreeMap::new()).await;
        // The planner calls this a `Create`, not an `Update`: an
        // `AppendUnique` element that changed is simply not present on the
        // host any more. What matters here is that it carries the previous
        // manifest entry, so the applier un-merges the old element.
        assert_eq!(update.actions[0].op, ActionOp::Create);
        assert!(update.actions[0].remove_entry.is_some());
        let res = apply_host(&ctx, &Claude, &update).await;
        assert_eq!(res.status, "applied", "{res:?}");
        let settings_file = home.path().join(".claude/settings.json");
        let settings = read_json(&settings_file);
        assert_eq!(
            settings["hooks"]["Stop"].as_array().map(Vec::len),
            Some(1),
            "the stale element must be unmerged: {settings}"
        );
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo bye"
        );
        assert_eq!(
            backups(&home.path().join(".claude")).len(),
            1,
            "the rewritten config was backed up"
        );

        // 9. A hook whose command carries a resolved secret: the config file
        //    goes through the 0600 upload path, and the value never reaches
        //    a script — only the merged file on the host has it.
        let hooks = write_catalog(
            hook_repo.path(),
            &[(
                "hooks/stop.yaml",
                "kind: hook\nname: stop\ndescription: d\nevent: stop\naction:\n  type: command\n  command: \"echo ${MYSECRET}\"\n",
            )],
        );
        let secrets = BTreeMap::from([("MYSECRET".to_string(), "s3cr3t".to_string())]);
        let secret_plan = plan_with_secrets(&ssh, &hooks, &secrets).await;
        assert_eq!(secret_plan.actions[0].op, ActionOp::Create);
        assert_eq!(secret_plan.actions[0].secrets, vec!["MYSECRET".to_string()]);
        let res = apply_host(&ctx, &Claude, &secret_plan).await;
        assert_eq!(res.status, "applied", "{res:?}");
        let settings = read_json(&settings_file);
        assert_eq!(
            settings["hooks"]["Stop"].as_array().map(Vec::len),
            Some(1),
            "{settings}"
        );
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo s3cr3t"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&settings_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "a secret-bearing config is 0600");
        }
        assert_eq!(
            backups(&home.path().join(".claude")).len(),
            2,
            "the secret upload backed the old config up first"
        );
        // Nothing the applier reported may carry the value.
        let reported = serde_json::to_string(&res).unwrap();
        assert!(!reported.contains("s3cr3t"), "{reported}");

        // 10. An `Adopt` (the host is already correct, only the manifest
        //     lost its entry) writes the manifest and nothing else — the
        //     config file is not reformatted or backed up again.
        std::fs::remove_file(&manifest_file).unwrap();
        let before = std::fs::read_to_string(&settings_file).unwrap();
        let adopt = plan_with_secrets(&ssh, &hooks, &secrets).await;
        assert_eq!(adopt.actions[0].op, ActionOp::Adopt);
        let res = apply_host(&ctx, &Claude, &adopt).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert_eq!(res.actions[0].outcome, DONE);
        assert!(
            !res.restart_required,
            "an adopt touches only the manifest, never the hook's own config"
        );
        assert_eq!(std::fs::read_to_string(&settings_file).unwrap(), before);
        assert_eq!(backups(&home.path().join(".claude")).len(), 2);
        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
        assert!(manifest.assets.contains_key("hook/stop"), "{manifest:?}");

        // 11. An MCP server merges into `~/.claude.json`, whose parent dir
        //     is a bare `~` — the local secret-write path must expand that
        //     to `$HOME` rather than create a directory called `~`.
        let mcp_repo = tempfile::tempdir().unwrap();
        let mcps = write_catalog(
            mcp_repo.path(),
            &[(
                "mcp/fleet.yaml",
                "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: \"http://127.0.0.1:4180/mcp\"\n",
            )],
        );
        let mcp_plan = plan_for(&ssh, &mcps).await;
        assert_eq!(mcp_plan.actions[0].op, ActionOp::Create);
        let res = apply_host(&ctx, &Claude, &mcp_plan).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert!(
            !home.path().join("~").exists(),
            "a bare ~ parent must expand, not become a directory"
        );
        let claude_json = read_json(&home.path().join(".claude.json"));
        assert_eq!(
            claude_json["mcpServers"]["fleet"]["url"],
            "http://127.0.0.1:4180/mcp"
        );

        // 12. A config file the scan cannot parse is never rewritten: every
        //     action touching it is blocked, and the bytes stay put.
        let broken = b"{ this is not json";
        std::fs::write(home.path().join(".claude/settings.json"), broken).unwrap();
        let before_backups = backups(&home.path().join(".claude")).len();
        let blocked_plan = plan_with_secrets(&ssh, &hooks, &secrets).await;
        let res = apply_host(&ctx, &Claude, &blocked_plan).await;
        assert_eq!(res.actions[0].outcome, BLOCKED, "{res:?}");
        assert!(
            res.actions[0]
                .detail
                .as_deref()
                .unwrap()
                .contains("could not be parsed"),
            "{:?}",
            res.actions[0].detail
        );
        assert_eq!(
            std::fs::read(home.path().join(".claude/settings.json")).unwrap(),
            broken,
            "the unparseable file is left exactly as it was"
        );
        assert_eq!(
            backups(&home.path().join(".claude")).len(),
            before_backups,
            "a blocked action backs nothing up"
        );
    }

    /// Repeated overwrites of the same file prune `.fleet-bak-*` backups
    /// down to the [`BACKUP_KEEP`] newest, dropping the oldest first.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn repeated_overwrites_prune_backups_to_the_newest_three() {
        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());

        let skill_repo = tempfile::tempdir().unwrap();
        let catalog = write_catalog(
            skill_repo.path(),
            &[
                (
                    "skills/s/asset.yaml",
                    "kind: skill\nname: s\ndescription: d\n",
                ),
                ("skills/s/body.md", "v0\n"),
            ],
        );
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 1_000,
        };
        let skill_dir = home.path().join(".claude/skills/s");

        let create = plan_for(&ssh, &catalog).await;
        let res = apply_host(&ctx, &Claude, &create).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert!(backups(&skill_dir).is_empty());

        // Four overwrites, each changing the body so the plan sees a real
        // change to apply. Each backs up the file as it was before that
        // overwrite.
        let mut first_backup_name: Option<String> = None;
        for v in 1..=4 {
            let catalog = write_catalog(
                skill_repo.path(),
                &[
                    (
                        "skills/s/asset.yaml",
                        "kind: skill\nname: s\ndescription: d\n",
                    ),
                    ("skills/s/body.md", &format!("v{v}\n")),
                ],
            );
            let overwrite = plan_for(&ssh, &catalog).await;
            assert_eq!(overwrite.actions[0].op, ActionOp::Update, "v{v}");
            assert!(overwrite.actions[0].backup, "v{v}");
            let res = apply_host(&ctx, &Claude, &overwrite).await;
            assert_eq!(res.status, "applied", "v{v}: {res:?}");
            if v == 1 {
                let baks = backups(&skill_dir);
                assert_eq!(baks.len(), 1, "{baks:?}");
                first_backup_name = Some(baks[0].clone());
            }
        }

        let baks = backups(&skill_dir);
        assert_eq!(
            baks.len(),
            3,
            "kept only the {BACKUP_KEEP} newest backups: {baks:?}"
        );
        assert!(
            !baks.contains(first_backup_name.as_ref().unwrap()),
            "the oldest backup must have been pruned: {baks:?}"
        );
        let mut contents: Vec<String> = baks
            .iter()
            .map(|f| std::fs::read_to_string(skill_dir.join(f)).unwrap())
            .collect();
        contents.sort();
        assert!(contents[0].ends_with("v1\n"), "{contents:?}");
        assert!(contents[1].ends_with("v2\n"), "{contents:?}");
        assert!(contents[2].ends_with("v3\n"), "{contents:?}");
    }

    /// A manifest key `split_key` cannot parse (garbage, or a foreign
    /// schema) is dropped the next time this host's manifest is rewritten,
    /// while the parseable, just-synced entry survives.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn apply_drops_unparseable_manifest_keys_on_rewrite() {
        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());

        let manifest_dir = home.path().join(".claude");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        let manifest_file = manifest_dir.join(".fleet-assets.json");
        std::fs::write(
            &manifest_file,
            serde_json::json!({
                "version": 1,
                "updated_at": 0,
                "assets": {
                    "bogus/x": {"hash": "h", "files": [], "merges": [], "synced_at": 0}
                }
            })
            .to_string(),
        )
        .unwrap();

        let skill_repo = tempfile::tempdir().unwrap();
        let catalog = write_catalog(
            skill_repo.path(),
            &[
                (
                    "skills/s/asset.yaml",
                    "kind: skill\nname: s\ndescription: d\n",
                ),
                ("skills/s/body.md", "body\n"),
            ],
        );
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 1_000,
        };

        let create = plan_for(&ssh, &catalog).await;
        assert_eq!(create.actions[0].op, ActionOp::Create);
        assert!(
            create.manifest.assets.contains_key("bogus/x"),
            "the scanned manifest still carries the garbage key before apply"
        );
        let res = apply_host(&ctx, &Claude, &create).await;
        assert_eq!(res.status, "applied", "{res:?}");

        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
        assert!(
            !manifest.assets.contains_key("bogus/x"),
            "the unparseable key must be dropped on rewrite: {manifest:?}"
        );
        assert!(
            manifest.assets.contains_key("skill/s"),
            "the parseable, just-synced entry must survive: {manifest:?}"
        );
    }

    /// A file the previous sync wrote that the asset no longer renders is
    /// deleted (after a backup) in the same batch that writes the new ones.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_file_dropped_from_an_asset_is_deleted_with_the_update() {
        use crate::service::catalog::harness::{FileWrite, RenderPlan};
        use crate::service::catalog::model::sha256_hex;
        use crate::service::catalog::sync::secrets::SecretPlan;

        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());

        let dir = home.path().join(".claude/skills/s");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            b"old
",
        )
        .unwrap();
        std::fs::write(
            dir.join("extra.md"),
            b"dropped
",
        )
        .unwrap();

        let mut rendered = RenderPlan::default();
        rendered.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"new
"
            .to_vec(),
        });
        let mut snapshot = HostSnapshot::default();
        snapshot
            .files
            .insert("~/.claude/skills/s/SKILL.md".into(), sha256_hex(b"old\n"));
        snapshot.files.insert(
            "~/.claude/skills/s/extra.md".into(),
            sha256_hex(b"dropped\n"),
        );
        let action = Action {
            kind: "skill".into(),
            name: "s".into(),
            op: ActionOp::Update,
            reason: None,
            files: vec!["~/.claude/skills/s/SKILL.md".into()],
            merges: Vec::new(),
            backup: true,
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: Some(SecretPlan::from(rendered)),
            expected: BTreeMap::from([(
                "~/.claude/skills/s/SKILL.md".to_string(),
                Some(sha256_hex(b"old\n")),
            )]),
            secret_files: BTreeSet::new(),
            remove_entry: Some(ManifestEntry {
                hash: "old".into(),
                files: vec![
                    "~/.claude/skills/s/SKILL.md".into(),
                    "~/.claude/skills/s/extra.md".into(),
                ],
                merges: Vec::new(),
                synced_at: 1,
            }),
            plugin: None,
        };
        let plan = HostPlan {
            host_alias: "local".into(),
            harness: "claude".into(),
            status: "planned".into(),
            detail: None,
            actions: vec![action],
            snapshot,
            manifest: Manifest::default(),
        };
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 5,
        };
        let res = apply_host(&ctx, &Claude, &plan).await;
        assert_eq!(res.status, "applied", "{res:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "new\n"
        );
        assert!(!dir.join("extra.md").exists(), "the dropped file is gone");
        assert_eq!(
            backups(&dir).len(),
            2,
            "both the replaced and the deleted file"
        );
        // The manifest now records only the file that is still rendered.
        let manifest: Manifest = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(".claude/.fleet-assets.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest.assets["skill/s"].files,
            vec!["~/.claude/skills/s/SKILL.md".to_string()]
        );
    }

    /// A path the applier refuses to interpolate fails its action outright,
    /// and nothing at all is sent to the host.
    #[tokio::test]
    async fn an_unsafe_path_fails_its_action_without_touching_the_host() {
        use crate::service::catalog::harness::{FileWrite, RenderPlan};
        use crate::service::catalog::sync::secrets::SecretPlan;

        let mut rendered = RenderPlan::default();
        rendered.files.push(FileWrite {
            path: "~/.claude/skills/../../etc/passwd".into(),
            bytes: b"nope".to_vec(),
        });
        let action = Action {
            kind: "skill".into(),
            name: "evil".into(),
            op: ActionOp::Create,
            reason: None,
            files: vec![rendered.files[0].path.clone()],
            merges: Vec::new(),
            backup: false,
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: Some(SecretPlan::from(rendered)),
            expected: BTreeMap::new(),
            secret_files: BTreeSet::new(),
            remove_entry: None,
            plugin: None,
        };
        let plan = HostPlan {
            host_alias: "local".into(),
            harness: "claude".into(),
            status: "planned".into(),
            detail: None,
            actions: vec![action],
            snapshot: HostSnapshot::default(),
            manifest: Manifest::default(),
        };
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 7,
        };
        let res = apply_host(&ctx, &Claude, &plan).await;
        assert_eq!(res.status, "partial");
        assert_eq!(res.actions[0].outcome, FAILED);
        assert!(
            res.actions[0]
                .detail
                .as_deref()
                .unwrap()
                .contains("unsafe path"),
            "{:?}",
            res.actions[0].detail
        );
    }

    /// A skipped host is reported untouched.
    #[tokio::test]
    async fn a_skipped_host_is_returned_as_skipped() {
        let plan = HostPlan {
            host_alias: "box".into(),
            harness: "claude".into(),
            status: "skipped".into(),
            detail: Some("unreachable".into()),
            actions: Vec::new(),
            snapshot: HostSnapshot::default(),
            manifest: Manifest::default(),
        };
        let ssh = Arc::new(SshClient::new());
        let ctx = ApplyCtx {
            ssh: &ssh,
            token: CancellationToken::new(),
            now: 1,
        };
        let res = apply_host(&ctx, &Claude, &plan).await;
        assert_eq!(res.status, SKIPPED);
        assert_eq!(res.detail.as_deref(), Some("unreachable"));
    }

    /// A cancelled token stops the applier before it writes anything.
    #[tokio::test]
    async fn cancellation_stops_before_any_write() {
        use crate::service::catalog::harness::{FileWrite, RenderPlan};
        use crate::service::catalog::sync::secrets::SecretPlan;

        let mut rendered = RenderPlan::default();
        rendered.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"body\n".to_vec(),
        });
        let plan = HostPlan {
            host_alias: "local".into(),
            harness: "claude".into(),
            status: "planned".into(),
            detail: None,
            actions: vec![Action {
                kind: "skill".into(),
                name: "s".into(),
                op: ActionOp::Create,
                reason: None,
                files: vec!["~/.claude/skills/s/SKILL.md".into()],
                merges: Vec::new(),
                backup: false,
                secrets: Vec::new(),
                missing_secrets: Vec::new(),
                plan: Some(SecretPlan::from(rendered)),
                expected: BTreeMap::new(),
                secret_files: BTreeSet::new(),
                remove_entry: None,
                plugin: None,
            }],
            snapshot: HostSnapshot::default(),
            manifest: Manifest::default(),
        };
        let ssh = Arc::new(SshClient::new());
        let token = CancellationToken::new();
        token.cancel();
        let ctx = ApplyCtx {
            ssh: &ssh,
            token,
            now: 1,
        };
        let res = apply_host(&ctx, &Claude, &plan).await;
        assert_eq!(res.status, "partial");
        assert_eq!(res.detail.as_deref(), Some("cancelled"));
    }
}
