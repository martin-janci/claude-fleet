//! Authoring the catalog: kind templates, the static lint, and every write
//! that goes into the catalog repo (create / update / delete / resources /
//! commit-pending / push).
//! Spec: docs/superpowers/specs/2026-09-15-asset-authoring-design.md
//!
//! Every operation resolves the repo root from the stored catalog config,
//! performs the write, stages only the paths it touched, auto-commits with a
//! generated `catalog: …` message, and ends with `catalog::load(false)` so
//! `CATALOG` and the UI refresh through the existing `catalog:loaded` event.

// This module is the service layer for the twelve `catalog_*` authoring
// commands in `commands/assets.rs`.

use super::harness::HARNESS_IDS;
use super::model::{
    find_placeholders, is_valid_name, Asset, AssetSpec, Header, HookAction, Kind, Marketplace,
    Problem,
};
use super::repo::{self, Catalog, RepoStatus};
use super::sync::secrets::{BUILTIN_PORT, BUILTIN_TOKEN};
use super::{CATALOG, E_ASSET_NOT_FOUND, E_CATALOG_GIT, E_LINT};
use crate::ipc_error::codes::{E_INVALID, E_LOCK, E_SERIALIZE};
use crate::ipc_error::IpcError;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SECRETS_EXAMPLE: &str = "secrets.example.yaml";

// ---------------------------------------------------------------- templates

/// A new, lint-clean asset of `kind` named `name`. `plugin_ref` is the one
/// exception: its marketplace coordinates cannot be guessed, so they are
/// left as `TODO` strings that the lint reports as errors until the author
/// fills them in.
pub fn template(kind: Kind, name: &str) -> Asset {
    let header = |description: &str| Header {
        kind,
        name: name.to_string(),
        version: "1".to_string(),
        description: description.to_string(),
        tags: Vec::new(),
        source: None,
        targets: Default::default(),
    };
    match kind {
        Kind::Skill => Asset {
            header: header("Describe when to use this skill."),
            spec: AssetSpec::Skill {
                allowed_tools: Vec::new(),
                user_invocable: true,
                triggers: Vec::new(),
            },
            body: format!("# {name}\n\n## When to use\n\n## Steps\n"),
            resources: Vec::new(),
        },
        Kind::Agent => Asset {
            header: header("Describe what this agent does."),
            spec: AssetSpec::Agent {
                tools: vec!["read".into(), "grep".into(), "glob".into()],
                model: "default".into(),
            },
            body: "You are …\n".to_string(),
            resources: Vec::new(),
        },
        Kind::Hook => Asset {
            header: header("Describe when this hook runs."),
            spec: AssetSpec::Hook {
                event: "stop".into(),
                r#match: None,
                action: HookAction {
                    kind: "command".into(),
                    command: Some("echo hook".into()),
                    url: None,
                    headers: Default::default(),
                    timeout_s: None,
                },
            },
            body: String::new(),
            resources: Vec::new(),
        },
        Kind::McpServer => Asset {
            header: header("Describe what this MCP server provides."),
            spec: AssetSpec::McpServer {
                transport: "http".into(),
                url: Some(format!("http://127.0.0.1:${{{BUILTIN_PORT}}}/mcp")),
                headers: Default::default(),
                command: None,
                args: Vec::new(),
                env: Default::default(),
            },
            body: String::new(),
            resources: Vec::new(),
        },
        Kind::PluginRef => Asset {
            header: header("Describe what this plugin provides."),
            spec: AssetSpec::PluginRef {
                harness: "claude".into(),
                marketplace: Marketplace {
                    name: "TODO".into(),
                    source: "github".into(),
                    repo: "TODO".into(),
                },
                plugin: name.to_string(),
                version: "latest".into(),
            },
            body: String::new(),
            resources: Vec::new(),
        },
    }
}

// --------------------------------------------------------------------- lint

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LintReport {
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetLint {
    pub kind: String,
    pub name: String,
    pub report: LintReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintAll {
    pub assets: Vec<AssetLint>,
    pub problems: Vec<Problem>,
    pub errors: usize,
    pub warnings: usize,
}

impl LintReport {
    fn error(&mut self, field: &str, message: impl Into<String>) {
        self.errors.push(Finding {
            field: field.to_string(),
            message: message.into(),
        });
    }
    fn warn(&mut self, field: &str, message: impl Into<String>) {
        self.warnings.push(Finding {
            field: field.to_string(),
            message: message.into(),
        });
    }
}

/// The field names `Asset::validate`'s messages start with, so a validation
/// problem can be attached to the form control that caused it. An unknown
/// message yields an empty field (the UI shows it as a general problem)
/// rather than a guess.
const VALIDATE_FIELDS: &[&str] = &[
    "name",
    "description",
    "allowed_tools",
    "tools",
    "model",
    "event",
    "action.command",
    "action.url",
    "action.type",
    "url",
    "command",
    "transport",
    "harness",
    "version",
];

fn validate_field(message: &str) -> String {
    let first = message
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(':');
    if VALIDATE_FIELDS.contains(&first) {
        first.to_string()
    } else {
        String::new()
    }
}

/// Every string value in the asset's YAML form, paired with its dotted field
/// path (`description`, `marketplace.name`, `args[0]`, `body`, …). Resource
/// bytes are excluded: they are base64 blobs, not authored text.
fn string_fields(asset: &Asset) -> Vec<(String, String)> {
    let mut value = match serde_yaml::to_value(asset) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let serde_yaml::Value::Mapping(map) = &mut value {
        map.remove("resources");
    }
    let mut out = Vec::new();
    walk_strings(&value, "", &mut out);
    out
}

fn walk_strings(value: &serde_yaml::Value, path: &str, out: &mut Vec<(String, String)>) {
    match value {
        serde_yaml::Value::String(s) => out.push((path.to_string(), s.clone())),
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default();
                let child = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                walk_strings(v, &child, out);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for (i, v) in seq.iter().enumerate() {
                walk_strings(v, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// The host of a URL: everything between `://` and the first `/`, `?` or
/// `#`, with any `user:password@` prefix dropped and any `:port` suffix
/// stripped (an IPv6 literal keeps its brackets). Lowercased, because host
/// names are case-insensitive.
fn url_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // Userinfo can itself contain '@', so take the *last* one: everything
    // after it is the real host (`http://127.0.0.1@evil.com/` is evil.com).
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    let host = match authority.find(']') {
        // `[::1]` / `[::1]:4180` — the brackets delimit the literal.
        Some(end) if authority.starts_with('[') => &authority[..=end],
        _ => authority.split(':').next().unwrap_or_default(),
    };
    Some(host.to_ascii_lowercase())
}

/// A URL the lint accepts without a warning: TLS, or a plain-http loopback
/// address (which never leaves the machine, so plaintext is fine there).
/// The host is compared exactly — `http://127.0.0.1.evil.com/` and
/// `http://127.0.0.1@evil.com/` are not loopback, and neither is a scheme
/// that is neither http nor https.
fn url_is_safe(url: &str) -> bool {
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("https") => true,
        Some("http") => {
            url_host(url).is_some_and(|h| matches!(h.as_str(), "127.0.0.1" | "localhost" | "[::1]"))
        }
        _ => false,
    }
}

/// Names of the `${NAME}` placeholders declared in `secrets.example.yaml`:
/// the file is either a sequence of names or a mapping whose keys are the
/// names. Missing or unparsable file -> no names.
pub fn secrets_example_names(root: &Path) -> Vec<String> {
    let text = match std::fs::read_to_string(root.join(SECRETS_EXAMPLE)) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value: serde_yaml::Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    match value {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        serde_yaml::Value::Mapping(map) => map
            .keys()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Static lint for one asset. Errors block a save (`E_LINT`), warnings never
/// do. The rule list is the spec's, exactly.
///
/// `catalog` is part of the interface the commands and the UI are built
/// against (and what a cross-asset rule would need); no rule in the spec's
/// list consults it, so it is unused today.
pub fn lint(
    asset: &Asset,
    _catalog: &Catalog,
    secrets_example: &[String],
    secrets_example_exists: bool,
) -> LintReport {
    let mut report = LintReport::default();
    let kind = asset.kind();

    // Errors -------------------------------------------------------------
    for message in asset.validate() {
        let field = validate_field(&message);
        report.error(&field, message);
    }
    let body_empty = asset.body.trim().is_empty();
    if kind.is_folder() && body_empty {
        report.error(
            "body",
            format!(
                "{} must not be empty",
                if kind == Kind::Agent {
                    "prompt.md"
                } else {
                    "body.md"
                }
            ),
        );
    }
    let strings = string_fields(asset);
    for (field, value) in &strings {
        if value.contains("TODO") {
            report.error(field, "replace the TODO placeholder");
        }
    }
    if let AssetSpec::Hook { action, .. } = &asset.spec {
        if action.command.as_deref().is_some_and(|c| c.contains('\n')) {
            report.error("action.command", "command must be a single line");
        }
    }

    // Warnings -----------------------------------------------------------
    let description = asset.header.description.trim();
    if description.chars().count() < 20 {
        report.warn("description", "description is shorter than 20 characters");
    } else if description.chars().count() > 1024 {
        report.warn("description", "description is longer than 1024 characters");
    }

    let mut undeclared: Vec<String> = Vec::new();
    for (_, value) in &strings {
        for name in find_placeholders(value) {
            let known =
                name == BUILTIN_TOKEN || name == BUILTIN_PORT || secrets_example.contains(&name);
            if !known && !undeclared.contains(&name) {
                undeclared.push(name);
            }
        }
    }
    if !undeclared.is_empty() {
        if secrets_example_exists {
            for name in &undeclared {
                report.warn(
                    "secrets",
                    format!("${{{name}}} is not listed in {SECRETS_EXAMPLE}"),
                );
            }
        } else {
            let names: Vec<String> = undeclared.iter().map(|n| format!("${{{n}}}")).collect();
            report.warn(
                "secrets",
                format!(
                    "{SECRETS_EXAMPLE} is missing; it should list {}",
                    names.join(", ")
                ),
            );
        }
    }

    match &asset.spec {
        AssetSpec::Agent { tools, .. } => {
            if tools.is_empty() {
                report.warn("tools", "no tools are allowed; the agent can only think");
            }
        }
        AssetSpec::Hook { action, .. } => {
            if let Some(url) = action.url.as_deref() {
                if !url_is_safe(url) {
                    report.warn(
                        "action.url",
                        "url is neither https:// nor a loopback address",
                    );
                }
            }
        }
        AssetSpec::McpServer { url, .. } => {
            if let Some(url) = url.as_deref() {
                if !url_is_safe(url) {
                    report.warn("url", "url is neither https:// nor a loopback address");
                }
            }
        }
        _ => {}
    }

    for harness in asset.header.targets.keys() {
        if !HARNESS_IDS.contains(&harness.as_str()) {
            report.warn(
                &format!("targets.{harness}"),
                format!("'{harness}' is not a known harness"),
            );
        }
    }

    if kind == Kind::Skill && !body_empty && !asset.body.lines().any(|l| l.starts_with('#')) {
        report.warn("body", "body.md has no markdown heading");
    }

    report
}

/// One report per catalog asset plus the catalog's own load `problems`, with
/// the totals the Lint-all dialog shows.
pub fn lint_all(catalog: &Catalog, root: &Path) -> LintAll {
    let names = secrets_example_names(root);
    let exists = root.join(SECRETS_EXAMPLE).exists();
    let assets: Vec<AssetLint> = catalog
        .assets
        .iter()
        .map(|a| AssetLint {
            kind: a.kind().as_str().to_string(),
            name: a.header.name.clone(),
            report: lint(a, catalog, &names, exists),
        })
        .collect();
    LintAll {
        errors: assets.iter().map(|a| a.report.errors.len()).sum(),
        warnings: assets.iter().map(|a| a.report.warnings.len()).sum(),
        problems: catalog.problems.clone(),
        assets,
    }
}

// --------------------------------------------------------------- operations

#[derive(Debug, Clone, Deserialize)]
pub struct CreateArgs {
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub duplicate_from: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateArgs {
    pub asset: Asset,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetRef {
    pub kind: Kind,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddResourceArgs {
    pub kind: Kind,
    pub name: String,
    pub local_path: String,
    #[serde(default)]
    pub rel_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoveResourceArgs {
    pub kind: Kind,
    pub name: String,
    pub rel_path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommitPendingArgs {
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WriteResult {
    pub commit: String,
    pub lint: LintReport,
}

fn repo_root(store: &Mutex<Store>) -> Result<PathBuf, IpcError> {
    Ok(PathBuf::from(super::require_config(store)?.repo_path))
}

fn check_name(name: &str) -> Result<(), IpcError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID,
            format!("name '{name}' must match [a-z0-9][a-z0-9-]*"),
        ))
    }
}

/// Only skills and agents are folders on disk, so only they can carry
/// `resources/…` files.
fn check_has_resources(kind: Kind) -> Result<(), IpcError> {
    if kind.is_folder() {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID,
            format!("{} assets have no resources", kind.as_str()),
        ))
    }
}

fn check_resource_path(rel_path: &str) -> Result<(), IpcError> {
    if repo::valid_resource_rel_path(rel_path) {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID,
            format!("invalid resource path: {rel_path}"),
        ))
    }
}

/// A clone of the named asset out of the loaded `CATALOG`.
fn catalog_asset(kind: Kind, name: &str) -> Result<Asset, IpcError> {
    let guard = CATALOG
        .read()
        .map_err(|_| IpcError::new(E_LOCK, "catalog lock poisoned"))?;
    guard
        .as_ref()
        .and_then(|c| c.find(kind, name).cloned())
        .ok_or_else(|| {
            IpcError::new(
                E_ASSET_NOT_FOUND,
                format!("{} {name} is not in the catalog", kind.as_str()),
            )
        })
}

/// Lint against the catalog as currently loaded (an unloaded catalog lints
/// against an empty one — no rule consults it).
fn lint_in_repo(asset: &Asset, root: &Path) -> LintReport {
    let names = secrets_example_names(root);
    let exists = root.join(SECRETS_EXAMPLE).exists();
    let guard = CATALOG.read().ok();
    let empty = Catalog::default();
    let catalog = guard.as_ref().and_then(|g| g.as_ref()).unwrap_or(&empty);
    lint(asset, catalog, &names, exists)
}

/// Stage `rel_paths`, commit them under `message`, then reload the catalog
/// and return the resulting HEAD.
///
/// A write that changes nothing (saving an asset whose serialised form is
/// byte-identical to what is on disk) is a no-op, not an error: the commit
/// is skipped and the current HEAD is returned. Only the staged state of
/// `rel_paths` decides that, so an unrelated dirty file elsewhere in the
/// tree neither forces an empty commit nor gets swept into this one. The
/// catalog is reloaded either way, so the caller's view is always fresh.
fn commit_and_reload(
    root: &Path,
    rel_paths: &[String],
    message: &str,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    repo::stage_paths(root, rel_paths)?;
    let commit = if repo::has_staged(root, rel_paths)? {
        repo::commit(root, message)?
    } else {
        repo::head(root)?
    };
    super::load(false, store)?;
    Ok(commit)
}

fn write_commit_reload(
    root: &Path,
    asset: &Asset,
    overwrite: bool,
    message: &str,
    store: &Mutex<Store>,
) -> Result<String, IpcError> {
    repo::write_asset(root, asset, overwrite)?;
    let rel = repo::asset_rel_dir(asset.kind(), &asset.header.name);
    commit_and_reload(root, &[rel], message, store)
}

/// Create an asset from the kind template, or as a copy of an existing asset
/// of the same kind (`duplicate_from`) under the new name with `source`
/// cleared. `E_ASSET_EXISTS` when the name is taken. Lint findings are
/// returned but never block a create: a template is a starting point (the
/// `plugin_ref` template deliberately carries `TODO` errors).
pub fn create(args: CreateArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError> {
    let root = repo_root(store)?;
    check_name(&args.name)?;
    let asset = match args.duplicate_from.as_deref() {
        Some(from) => {
            check_name(from)?;
            let mut a = catalog_asset(args.kind, from)?;
            a.header.name = args.name.clone();
            a.header.source = None;
            a
        }
        None => template(args.kind, &args.name),
    };
    let message = format!("catalog: create {}/{}", args.kind.as_str(), args.name);
    let commit = write_commit_reload(&root, &asset, false, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: lint_in_repo(&asset, &root),
    })
}

/// Save an edited asset. Lints first and writes nothing when there are
/// errors (`E_LINT`, `details` = the report). The write overwrites and
/// prunes `resources/…` files the asset no longer lists.
pub fn update(args: UpdateArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError> {
    let root = repo_root(store)?;
    let asset = args.asset;
    let kind = asset.kind();
    check_name(&asset.header.name)?;
    for r in &asset.resources {
        check_resource_path(&r.rel_path)?;
    }
    if !repo::asset_path(&root, kind, &asset.header.name).exists() {
        return Err(IpcError::new(
            E_ASSET_NOT_FOUND,
            format!(
                "{} {} is not in the catalog repo",
                kind.as_str(),
                asset.header.name
            ),
        ));
    }
    let report = lint_in_repo(&asset, &root);
    if !report.errors.is_empty() {
        let details = serde_json::to_value(&report)
            .map_err(|e| IpcError::new(E_SERIALIZE, format!("lint report: {e}")))?;
        return Err(
            IpcError::new(E_LINT, "the asset has lint errors and was not saved")
                .with_details(details),
        );
    }
    let message = format!("catalog: update {}/{}", kind.as_str(), asset.header.name);
    let commit = write_commit_reload(&root, &asset, true, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: report,
    })
}

/// Delete an asset's folder (skill / agent) or file, commit the removal and
/// reload. Hosts that hold it become `orphan` until the next sync.
pub fn delete_asset(args: AssetRef, store: &Mutex<Store>) -> Result<String, IpcError> {
    let root = repo_root(store)?;
    check_name(&args.name)?;
    repo::remove_asset(&root, args.kind, &args.name)?;
    let rel = repo::asset_rel_dir(args.kind, &args.name);
    let message = format!("catalog: delete {}/{}", args.kind.as_str(), args.name);
    commit_and_reload(&root, &[rel], &message, store)
}

fn resource_message(kind: Kind, name: &str) -> String {
    format!("catalog: update {}/{name} resources", kind.as_str())
}

// Both resource operations rewrite the whole asset from the copy in the
// in-memory `CATALOG` — its `resources` carry the bytes `load_dir` read, so
// an overwrite reproduces the untouched files and prunes the rest. That
// assumes `CATALOG` matches the working tree: it holds what the last
// `catalog::load` saw, every authoring operation ends with one, and the
// sequence here is synchronous, so the window in which someone could edit
// the repo underneath is a single operation. A concurrent outside edit to
// another of *this asset's* files would be reverted by the rewrite (and show
// up in the commit); anything else in the repo is untouched, because only
// this asset's path is ever staged.

/// Copy a local file into the asset's `resources/…` and commit it.
pub fn add_resource(args: AddResourceArgs, store: &Mutex<Store>) -> Result<WriteResult, IpcError> {
    let root = repo_root(store)?;
    check_name(&args.name)?;
    check_has_resources(args.kind)?;
    // `symlink_metadata` does not follow the link, so a symlinked source is
    // rejected rather than silently copied through — the same discipline
    // `repo.rs` applies to everything it reads out of, or removes from, the
    // repo.
    let local = PathBuf::from(&args.local_path);
    let is_regular_file = std::fs::symlink_metadata(&local).is_ok_and(|m| m.is_file());
    if !local.is_absolute() || !is_regular_file {
        return Err(IpcError::new(
            E_INVALID,
            format!(
                "{} is not an absolute path to an existing regular file",
                args.local_path
            ),
        ));
    }
    let rel_path = match args.rel_path {
        Some(p) => p,
        None => format!(
            "resources/{}",
            local.file_name().unwrap_or_default().to_string_lossy()
        ),
    };
    check_resource_path(&rel_path)?;
    let bytes = std::fs::read(&local)?;

    let mut asset = catalog_asset(args.kind, &args.name)?;
    asset.resources.retain(|r| r.rel_path != rel_path);
    asset.resources.push(super::model::Resource {
        rel_path: rel_path.clone(),
        bytes,
    });
    asset.resources.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    let message = resource_message(args.kind, &args.name);
    let commit = write_commit_reload(&root, &asset, true, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: lint_in_repo(&asset, &root),
    })
}

/// Drop one `resources/…` file from the asset and commit the removal (the
/// overwrite write prunes the file and any directory it leaves empty).
pub fn remove_resource(
    args: RemoveResourceArgs,
    store: &Mutex<Store>,
) -> Result<WriteResult, IpcError> {
    let root = repo_root(store)?;
    check_name(&args.name)?;
    check_has_resources(args.kind)?;
    check_resource_path(&args.rel_path)?;

    let mut asset = catalog_asset(args.kind, &args.name)?;
    let before = asset.resources.len();
    asset.resources.retain(|r| r.rel_path != args.rel_path);
    if asset.resources.len() == before {
        return Err(IpcError::new(
            E_ASSET_NOT_FOUND,
            format!("{} has no resource {}", args.name, args.rel_path),
        ));
    }

    let message = resource_message(args.kind, &args.name);
    let commit = write_commit_reload(&root, &asset, true, &message, store)?;
    Ok(WriteResult {
        commit,
        lint: lint_in_repo(&asset, &root),
    })
}

/// Commit whatever is in the working tree (what an import leaves behind).
/// `E_CATALOG_GIT` when the tree is clean.
pub fn commit_pending(args: CommitPendingArgs, store: &Mutex<Store>) -> Result<String, IpcError> {
    let root = repo_root(store)?;
    let message = args
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("catalog: commit pending changes")
        .to_string();
    if repo::git_status(&root)?.dirty == 0 {
        return Err(IpcError::new(E_CATALOG_GIT, "nothing to commit"));
    }
    commit_and_reload(&root, &[], &message, store)
}

/// `git push` the catalog repo, then report the fresh status.
pub fn push(store: &Mutex<Store>) -> Result<RepoStatus, IpcError> {
    let root = repo_root(store)?;
    repo::push(&root)?;
    repo::git_status(&root)
}

pub fn repo_status(store: &Mutex<Store>) -> Result<RepoStatus, IpcError> {
    let root = repo_root(store)?;
    repo::git_status(&root)
}

pub fn lint_asset(args: AssetRef, store: &Mutex<Store>) -> Result<LintReport, IpcError> {
    let root = repo_root(store)?;
    let asset = catalog_asset(args.kind, &args.name)?;
    Ok(lint_in_repo(&asset, &root))
}

pub fn lint_everything(store: &Mutex<Store>) -> Result<LintAll, IpcError> {
    let root = repo_root(store)?;
    let guard = CATALOG
        .read()
        .map_err(|_| IpcError::new(E_LOCK, "catalog lock poisoned"))?;
    let empty = Catalog::default();
    let catalog = guard.as_ref().unwrap_or(&empty);
    Ok(lint_all(catalog, &root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::{Resource, Source, TargetOverride};
    use crate::service::catalog::CATALOG_TEST_LOCK;
    use std::path::PathBuf;

    // ------------------------------------------------------------ helpers

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fleet-author-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn init_repo(tag: &str) -> PathBuf {
        let root = tmp(tag);
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "t@t"]);
        git(&root, &["config", "user.name", "t"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        root
    }

    fn configured_store(root: &Path) -> Mutex<Store> {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store
            .lock()
            .unwrap()
            .set_catalog_config(&root.to_string_lossy(), None)
            .unwrap();
        store
    }

    fn subjects(root: &Path) -> Vec<String> {
        git(root, &["log", "--format=%s"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn clean_skill() -> Asset {
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: demo\ndescription: A reasonably long description here.\n",
        )
        .unwrap();
        a.body = "# demo\n\nSteps.\n".into();
        a
    }

    fn lint_of(asset: &Asset) -> LintReport {
        lint(asset, &Catalog::default(), &[], true)
    }

    fn fields(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.field.as_str()).collect()
    }

    // ---------------------------------------------------------- templates

    #[test]
    fn templates_are_clean_except_the_plugin_ref_todos() {
        for kind in Kind::ALL {
            let asset = template(kind, "demo");
            assert_eq!(asset.kind(), kind);
            assert_eq!(asset.header.name, "demo");
            assert!(
                asset.validate().is_empty(),
                "{}: {:?}",
                kind.as_str(),
                asset.validate()
            );
            let report = lint_of(&asset);
            assert!(
                report.warnings.is_empty(),
                "{}: {:?}",
                kind.as_str(),
                report.warnings
            );
            if kind == Kind::PluginRef {
                assert_eq!(
                    fields(&report.errors),
                    vec!["marketplace.name", "marketplace.repo"],
                    "{report:?}"
                );
            } else {
                assert!(
                    report.errors.is_empty(),
                    "{}: {:?}",
                    kind.as_str(),
                    report.errors
                );
            }
        }
        // The skill template's body is a usable markdown stub, and the MCP
        // template's URL uses the built-in port placeholder.
        assert!(template(Kind::Skill, "demo").body.starts_with("# demo"));
        match template(Kind::McpServer, "demo").spec {
            AssetSpec::McpServer { url, .. } => {
                assert_eq!(
                    url.as_deref(),
                    Some("http://127.0.0.1:${FLEET_MCP_PORT}/mcp")
                );
            }
            other => panic!("wrong spec {other:?}"),
        }
    }

    // --------------------------------------------------------- lint rules

    #[test]
    fn lint_clean_asset_has_no_findings() {
        assert_eq!(lint_of(&clean_skill()), LintReport::default());
    }

    #[test]
    fn lint_reports_validate_errors_with_their_field() {
        let mut a = clean_skill();
        a.header.name = "Bad Name".into();
        a.header.description = String::new();
        let report = lint_of(&a);
        assert_eq!(fields(&report.errors), vec!["name", "description"]);
        assert!(report.errors[0].message.contains("must match"));
    }

    #[test]
    fn lint_reports_an_empty_body_for_folder_kinds() {
        let mut a = clean_skill();
        a.body = "  \n".into();
        let report = lint_of(&a);
        assert_eq!(fields(&report.errors), vec!["body"]);
        assert!(report.errors[0].message.contains("body.md"));

        let mut agent = Asset::from_yaml(
            None,
            "kind: agent\nname: demo\ndescription: A reasonably long description here.\ntools: [read]\n",
        )
        .unwrap();
        agent.body = String::new();
        assert!(lint_of(&agent).errors[0].message.contains("prompt.md"));

        // A single-file kind has no body file, so an empty body is fine.
        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: demo\ndescription: A reasonably long description here.\nevent: stop\naction: { type: command, command: x }\n",
        )
        .unwrap();
        assert!(lint_of(&hook).errors.is_empty());
    }

    #[test]
    fn lint_reports_a_todo_left_in_any_string_field() {
        let mut a = clean_skill();
        a.header.description = "TODO: describe this skill properly.".into();
        a.body = "# demo\n\nTODO\n".into();
        let report = lint_of(&a);
        assert_eq!(fields(&report.errors), vec!["description", "body"]);
        assert_eq!(report.errors[0].message, "replace the TODO placeholder");
    }

    #[test]
    fn lint_reports_a_hook_command_with_a_newline() {
        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: demo\ndescription: A reasonably long description here.\nevent: stop\naction: { type: command, command: \"a\\nb\" }\n",
        )
        .unwrap();
        let report = lint_of(&hook);
        assert_eq!(fields(&report.errors), vec!["action.command"]);
        assert_eq!(report.errors[0].message, "command must be a single line");
    }

    #[test]
    fn lint_warns_on_a_short_or_long_description() {
        let mut a = clean_skill();
        a.header.description = "short".into();
        let report = lint_of(&a);
        assert_eq!(fields(&report.warnings), vec!["description"]);
        assert!(report.warnings[0].message.contains("shorter than 20"));

        a.header.description = "x".repeat(1025);
        let report = lint_of(&a);
        assert_eq!(fields(&report.warnings), vec!["description"]);
        assert!(report.warnings[0].message.contains("longer than 1024"));
    }

    #[test]
    fn lint_warns_on_placeholders_that_secrets_example_does_not_declare() {
        let mut a = clean_skill();
        a.body = "# demo\n\nUse ${API_KEY} and ${FLEET_MCP_TOKEN}.\n".into();
        // Built-ins always count as declared; API_KEY does not.
        let report = lint(&a, &Catalog::default(), &[], true);
        assert_eq!(fields(&report.warnings), vec!["secrets"]);
        assert!(report.warnings[0].message.contains("${API_KEY}"));
        // Declared -> no warning at all.
        let report = lint(&a, &Catalog::default(), &["API_KEY".to_string()], true);
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    #[test]
    fn lint_warns_when_secrets_example_is_missing_and_placeholders_exist() {
        let mut a = clean_skill();
        a.body = "# demo\n\n${API_KEY}\n".into();
        let report = lint(&a, &Catalog::default(), &[], false);
        assert_eq!(fields(&report.warnings), vec!["secrets"]);
        assert!(report.warnings[0].message.contains("is missing"));
        // No placeholders -> the missing file is not worth a warning.
        assert!(lint(&clean_skill(), &Catalog::default(), &[], false)
            .warnings
            .is_empty());
    }

    #[test]
    fn lint_warns_on_a_url_that_is_neither_https_nor_loopback() {
        let mcp = |url: &str| {
            Asset::from_yaml(
                None,
                &format!("kind: mcp_server\nname: demo\ndescription: A reasonably long description here.\ntransport: http\nurl: \"{url}\"\n"),
            )
            .unwrap()
        };
        let report = lint_of(&mcp("http://example.com/mcp"));
        assert_eq!(fields(&report.warnings), vec!["url"]);
        for ok in [
            "https://example.com/mcp",
            "http://127.0.0.1:4180/mcp",
            "http://127.0.0.1/mcp",
            "http://localhost:4180/mcp",
            "http://LocalHost/mcp",
            "http://[::1]:4180/mcp",
            "http://[::1]/mcp",
        ] {
            assert!(lint_of(&mcp(ok)).warnings.is_empty(), "{ok}");
        }
        // A host that merely starts with — or hides behind — a loopback name
        // is not loopback, and a non-http(s) scheme is never trusted.
        for bad in [
            "http://127.0.0.1.evil.com/x",
            "http://localhost.attacker.com/x",
            "http://127.0.0.1@evil.com/x",
            "http://user@127.0.0.1.evil.com/x",
            "ws://127.0.0.1:4180/mcp",
        ] {
            assert_eq!(fields(&lint_of(&mcp(bad)).warnings), vec!["url"], "{bad}");
        }
        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: demo\ndescription: A reasonably long description here.\nevent: stop\naction: { type: http, url: \"http://example.com/h\" }\n",
        )
        .unwrap();
        assert_eq!(fields(&lint_of(&hook).warnings), vec!["action.url"]);
    }

    #[test]
    fn lint_warns_on_an_agent_with_no_tools() {
        let mut agent = Asset::from_yaml(
            None,
            "kind: agent\nname: demo\ndescription: A reasonably long description here.\n",
        )
        .unwrap();
        agent.body = "You are a demo.\n".into();
        assert_eq!(fields(&lint_of(&agent).warnings), vec!["tools"]);
    }

    #[test]
    fn lint_warns_on_an_unknown_target_harness() {
        let mut a = clean_skill();
        a.header
            .targets
            .insert("claude".into(), TargetOverride::default());
        a.header
            .targets
            .insert("nosuch".into(), TargetOverride::default());
        let report = lint_of(&a);
        assert_eq!(fields(&report.warnings), vec!["targets.nosuch"]);
    }

    #[test]
    fn lint_warns_on_a_skill_body_without_a_heading() {
        let mut a = clean_skill();
        a.body = "Just prose, no heading.\n".into();
        let report = lint_of(&a);
        assert_eq!(fields(&report.warnings), vec!["body"]);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn secrets_example_names_reads_a_list_or_a_map() {
        let root = tmp("secrets");
        assert!(secrets_example_names(&root).is_empty());
        std::fs::write(root.join(SECRETS_EXAMPLE), "- API_KEY\n- OTHER\n").unwrap();
        assert_eq!(secrets_example_names(&root), vec!["API_KEY", "OTHER"]);
        std::fs::write(
            root.join(SECRETS_EXAMPLE),
            "API_KEY: your key here\nOTHER: \"\"\n",
        )
        .unwrap();
        assert_eq!(secrets_example_names(&root), vec!["API_KEY", "OTHER"]);
    }

    #[test]
    fn lint_all_counts_every_asset_and_keeps_the_problems() {
        let root = tmp("lintall");
        let mut catalog = Catalog {
            assets: vec![clean_skill(), template(Kind::PluginRef, "p")],
            ..Default::default()
        };
        catalog.problems.push(Problem {
            path: "skills/bad/asset.yaml".into(),
            message: "boom".into(),
        });
        let all = lint_all(&catalog, &root);
        assert_eq!(all.assets.len(), 2);
        assert_eq!(all.assets[0].kind, "skill");
        assert_eq!(all.errors, 2, "{:?}", all.assets);
        assert_eq!(all.warnings, 0);
        assert_eq!(all.problems.len(), 1);
    }

    // ---------------------------------------------------------- operations

    #[test]
    fn operations_require_a_configured_catalog() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let err = create(
            CreateArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_CATALOG_NOT_CONFIGURED);
        assert_eq!(
            repo_status(&store).unwrap_err().code,
            crate::ipc_error::codes::E_CATALOG_NOT_CONFIGURED
        );
    }

    #[test]
    fn create_update_delete_round_trip() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("roundtrip");
        let store = configured_store(&root);

        let created = create(
            CreateArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();
        assert_eq!(created.commit.len(), 40);
        assert!(created.lint.errors.is_empty(), "{:?}", created.lint);
        assert!(root.join("skills/demo/asset.yaml").is_file());
        assert!(root.join("skills/demo/body.md").is_file());
        let loaded = catalog_asset(Kind::Skill, "demo").unwrap();
        assert_eq!(
            loaded.header.description,
            "Describe when to use this skill."
        );

        // A second create of the same name is refused.
        assert_eq!(
            create(
                CreateArgs {
                    kind: Kind::Skill,
                    name: "demo".into(),
                    duplicate_from: None,
                },
                &store,
            )
            .unwrap_err()
            .code,
            crate::ipc_error::codes::E_ASSET_EXISTS
        );

        let mut edited = loaded;
        edited.header.description = "A demo skill used by the authoring tests.".into();
        edited.body = "# demo\n\nEdited body.\n".into();
        let updated = update(UpdateArgs { asset: edited }, &store).unwrap();
        assert_ne!(updated.commit, created.commit);
        assert_eq!(
            std::fs::read_to_string(root.join("skills/demo/body.md")).unwrap(),
            "# demo\n\nEdited body.\n"
        );
        assert_eq!(
            catalog_asset(Kind::Skill, "demo")
                .unwrap()
                .header
                .description,
            "A demo skill used by the authoring tests."
        );

        let deleted = delete_asset(
            AssetRef {
                kind: Kind::Skill,
                name: "demo".into(),
            },
            &store,
        )
        .unwrap();
        assert_eq!(deleted.len(), 40);
        assert!(!root.join("skills/demo").exists());
        assert_eq!(
            catalog_asset(Kind::Skill, "demo").unwrap_err().code,
            E_ASSET_NOT_FOUND
        );

        assert_eq!(
            subjects(&root)[..3].to_vec(),
            vec![
                "catalog: delete skill/demo",
                "catalog: update skill/demo",
                "catalog: create skill/demo",
            ]
        );
        let status = repo_status(&store).unwrap();
        assert_eq!(status.dirty, 0);
        assert!(!status.has_upstream);
        assert_eq!(status.head, deleted);
    }

    #[test]
    fn update_refuses_on_lint_errors_and_writes_nothing() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("lintrefuse");
        let store = configured_store(&root);
        create(
            CreateArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();
        let head_before = repo_status(&store).unwrap().head;

        let mut bad = catalog_asset(Kind::Skill, "demo").unwrap();
        bad.header.description = String::new();
        bad.body = "# demo\n\nEdited.\n".into();
        let err = update(UpdateArgs { asset: bad }, &store).unwrap_err();
        assert_eq!(err.code, E_LINT);
        let details = err.details.unwrap();
        assert_eq!(details["errors"][0]["field"], "description");
        assert!(details["warnings"].is_array());

        assert_eq!(
            std::fs::read_to_string(root.join("skills/demo/body.md")).unwrap(),
            "# demo\n\n## When to use\n\n## Steps\n"
        );
        assert_eq!(repo_status(&store).unwrap().head, head_before);
        assert_eq!(repo_status(&store).unwrap().dirty, 0);

        // An update of an asset that is not in the repo is not a silent create.
        let mut ghost = clean_skill();
        ghost.header.name = "ghost".into();
        assert_eq!(
            update(UpdateArgs { asset: ghost }, &store)
                .unwrap_err()
                .code,
            E_ASSET_NOT_FOUND
        );
        assert!(!root.join("skills/ghost").exists());
    }

    /// Saving an asset nothing changed in must not fail with git's "nothing
    /// to commit", and must not sweep an unrelated dirty file into a commit
    /// of its own (which is what a whole-tree check would have done).
    #[test]
    fn update_with_no_change_is_a_no_op_even_with_the_tree_dirty() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("noop");
        let store = configured_store(&root);
        create(
            CreateArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();
        let head = repo_status(&store).unwrap().head;
        let commits = subjects(&root).len();

        let unchanged = catalog_asset(Kind::Skill, "demo").unwrap();
        let saved = update(
            UpdateArgs {
                asset: unchanged.clone(),
            },
            &store,
        )
        .unwrap();
        assert_eq!(saved.commit, head, "no new commit for an identical save");
        assert_eq!(subjects(&root).len(), commits);
        assert!(saved.lint.errors.is_empty());

        // Same again with something else in the tree dirty: still a no-op,
        // and the unrelated file stays uncommitted.
        std::fs::write(root.join("notes.md"), "scratch\n").unwrap();
        let saved = update(
            UpdateArgs {
                asset: unchanged.clone(),
            },
            &store,
        )
        .unwrap();
        assert_eq!(saved.commit, head);
        assert_eq!(subjects(&root).len(), commits);
        assert_eq!(
            repo_status(&store).unwrap().dirty,
            1,
            "notes.md is untouched"
        );

        // A real edit still commits, and still leaves the stray file alone.
        let mut edited = unchanged;
        edited.header.description = "A demo skill used by the authoring tests.".into();
        let saved = update(UpdateArgs { asset: edited }, &store).unwrap();
        assert_ne!(saved.commit, head);
        assert_eq!(subjects(&root)[0], "catalog: update skill/demo");
        assert_eq!(repo_status(&store).unwrap().dirty, 1);
    }

    #[test]
    fn resources_add_and_remove_commit_and_prune() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("resources");
        let store = configured_store(&root);
        create(
            CreateArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();

        let src = tmp("resources-src").join("run.sh");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"echo hi\n").unwrap();

        let added = add_resource(
            AddResourceArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                local_path: src.to_string_lossy().into(),
                rel_path: None,
            },
            &store,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("skills/demo/resources/run.sh")).unwrap(),
            b"echo hi\n"
        );
        assert_eq!(
            catalog_asset(Kind::Skill, "demo").unwrap().resources[0].rel_path,
            "resources/run.sh"
        );
        assert_eq!(subjects(&root)[0], "catalog: update skill/demo resources");
        assert_eq!(added.commit, repo_status(&store).unwrap().head);

        // A nested explicit rel_path is honoured.
        add_resource(
            AddResourceArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                local_path: src.to_string_lossy().into(),
                rel_path: Some("resources/bin/run.sh".into()),
            },
            &store,
        )
        .unwrap();
        assert!(root.join("skills/demo/resources/bin/run.sh").is_file());

        // Bad inputs never reach the repo.
        for bad in ["../escape", "resources/../../x", "notresources/x"] {
            assert_eq!(
                add_resource(
                    AddResourceArgs {
                        kind: Kind::Skill,
                        name: "demo".into(),
                        local_path: src.to_string_lossy().into(),
                        rel_path: Some(bad.into()),
                    },
                    &store,
                )
                .unwrap_err()
                .code,
                E_INVALID,
                "{bad}"
            );
        }
        assert_eq!(
            add_resource(
                AddResourceArgs {
                    kind: Kind::Skill,
                    name: "demo".into(),
                    local_path: "relative/path.txt".into(),
                    rel_path: None,
                },
                &store,
            )
            .unwrap_err()
            .code,
            E_INVALID
        );
        assert_eq!(
            add_resource(
                AddResourceArgs {
                    kind: Kind::Hook,
                    name: "demo".into(),
                    local_path: src.to_string_lossy().into(),
                    rel_path: None,
                },
                &store,
            )
            .unwrap_err()
            .code,
            E_INVALID
        );
        // A symlinked source is refused rather than followed.
        let link = src.parent().unwrap().join("link.sh");
        std::os::unix::fs::symlink(&src, &link).unwrap();
        assert_eq!(
            add_resource(
                AddResourceArgs {
                    kind: Kind::Skill,
                    name: "demo".into(),
                    local_path: link.to_string_lossy().into(),
                    rel_path: None,
                },
                &store,
            )
            .unwrap_err()
            .code,
            E_INVALID
        );
        assert!(!root.join("skills/demo/resources/link.sh").exists());

        // Removing the nested one prunes the file and the empty directory.
        remove_resource(
            RemoveResourceArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                rel_path: "resources/bin/run.sh".into(),
            },
            &store,
        )
        .unwrap();
        assert!(!root.join("skills/demo/resources/bin").exists());
        assert!(root.join("skills/demo/resources/run.sh").is_file());

        remove_resource(
            RemoveResourceArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                rel_path: "resources/run.sh".into(),
            },
            &store,
        )
        .unwrap();
        assert!(!root.join("skills/demo/resources").exists());
        assert!(catalog_asset(Kind::Skill, "demo")
            .unwrap()
            .resources
            .is_empty());
        assert_eq!(
            remove_resource(
                RemoveResourceArgs {
                    kind: Kind::Skill,
                    name: "demo".into(),
                    rel_path: "resources/run.sh".into(),
                },
                &store,
            )
            .unwrap_err()
            .code,
            E_ASSET_NOT_FOUND
        );
        assert_eq!(repo_status(&store).unwrap().dirty, 0);
    }

    #[test]
    fn duplicate_from_copies_body_and_resources() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("duplicate");
        let store = configured_store(&root);
        create(
            CreateArgs {
                kind: Kind::Skill,
                name: "src".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();
        let mut original = catalog_asset(Kind::Skill, "src").unwrap();
        original.header.description = "The original skill, worth copying twice.".into();
        original.header.tags = vec!["core".into()];
        original.body = "# src\n\nOriginal body.\n".into();
        original.header.source = Some(Source {
            imported_from: Some("local".into()),
            original_path: Some("~/.claude/skills/src".into()),
            symlink_target: None,
        });
        original.resources.push(Resource {
            rel_path: "resources/data.txt".into(),
            bytes: b"payload".to_vec(),
        });
        update(UpdateArgs { asset: original }, &store).unwrap();

        create(
            CreateArgs {
                kind: Kind::Skill,
                name: "copy".into(),
                duplicate_from: Some("src".into()),
            },
            &store,
        )
        .unwrap();

        let copy = catalog_asset(Kind::Skill, "copy").unwrap();
        assert_eq!(copy.header.name, "copy");
        assert_eq!(copy.body, "# src\n\nOriginal body.\n");
        assert_eq!(copy.header.tags, vec!["core".to_string()]);
        assert_eq!(copy.header.source, None, "source is cleared on a duplicate");
        assert_eq!(copy.resources.len(), 1);
        assert_eq!(copy.resources[0].bytes, b"payload".to_vec());
        assert_eq!(
            std::fs::read(root.join("skills/copy/resources/data.txt")).unwrap(),
            b"payload"
        );
        assert_eq!(subjects(&root)[0], "catalog: create skill/copy");
        // The original is untouched.
        assert!(catalog_asset(Kind::Skill, "src")
            .unwrap()
            .header
            .source
            .is_some());
        assert_eq!(
            create(
                CreateArgs {
                    kind: Kind::Skill,
                    name: "other".into(),
                    duplicate_from: Some("nosuch".into()),
                },
                &store,
            )
            .unwrap_err()
            .code,
            E_ASSET_NOT_FOUND
        );
    }

    #[test]
    fn commit_pending_commits_a_dirty_tree_and_errors_when_clean() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("pending");
        let store = configured_store(&root);
        assert_eq!(
            commit_pending(CommitPendingArgs { message: None }, &store)
                .unwrap_err()
                .code,
            E_CATALOG_GIT
        );

        // What an import leaves behind: files written straight into the tree.
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::fs::write(
            root.join("hooks/h.yaml"),
            "kind: hook\nname: h\ndescription: An imported hook from the host.\nevent: stop\naction: { type: command, command: x }\n",
        )
        .unwrap();
        let commit = commit_pending(
            CommitPendingArgs {
                message: Some("  catalog: import from local  ".into()),
            },
            &store,
        )
        .unwrap();
        assert_eq!(subjects(&root)[0], "catalog: import from local");
        assert_eq!(repo_status(&store).unwrap().dirty, 0);
        assert_eq!(repo_status(&store).unwrap().head, commit);
        assert!(catalog_asset(Kind::Hook, "h").is_ok());

        std::fs::write(root.join("notes.md"), "scratch\n").unwrap();
        commit_pending(CommitPendingArgs { message: None }, &store).unwrap();
        assert_eq!(subjects(&root)[0], "catalog: commit pending changes");
    }

    #[test]
    fn push_to_a_bare_remote_updates_ahead_count() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("push");
        let bare = tmp("push-remote");
        git(&bare, &["init", "-q", "--bare", "-b", "main"]);
        git(&root, &["remote", "add", "origin", &bare.to_string_lossy()]);
        git(&root, &["push", "-q", "-u", "origin", "main"]);
        let store = configured_store(&root);

        create(
            CreateArgs {
                kind: Kind::Skill,
                name: "demo".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();
        let before = repo_status(&store).unwrap();
        assert!(before.has_upstream);
        assert_eq!(before.ahead, Some(1));
        assert_eq!(before.behind, Some(0));

        let after = push(&store).unwrap();
        assert_eq!(after.ahead, Some(0));
        assert_eq!(after.behind, Some(0));
        assert_eq!(after.head, before.head);
        assert_eq!(
            git(&bare, &["log", "--format=%s", "-1"]),
            "catalog: create skill/demo"
        );
    }

    #[test]
    fn lint_asset_and_lint_everything_read_the_loaded_catalog() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = init_repo("lintops");
        let store = configured_store(&root);
        create(
            CreateArgs {
                kind: Kind::PluginRef,
                name: "p".into(),
                duplicate_from: None,
            },
            &store,
        )
        .unwrap();
        let report = lint_asset(
            AssetRef {
                kind: Kind::PluginRef,
                name: "p".into(),
            },
            &store,
        )
        .unwrap();
        assert_eq!(
            fields(&report.errors),
            vec!["marketplace.name", "marketplace.repo"]
        );
        assert_eq!(
            lint_asset(
                AssetRef {
                    kind: Kind::Skill,
                    name: "nope".into()
                },
                &store
            )
            .unwrap_err()
            .code,
            E_ASSET_NOT_FOUND
        );

        let all = lint_everything(&store).unwrap();
        assert_eq!(all.assets.len(), 1);
        assert_eq!(all.errors, 2);
        assert!(all.problems.is_empty());
    }
}
