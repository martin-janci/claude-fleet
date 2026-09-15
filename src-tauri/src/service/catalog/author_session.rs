//! Delegating an asset to an interactive Claude session that works in the
//! catalog repo (spec: "Delegating to a session").
//!
//! `spawn_author_session` makes sure the catalog repo is a fleet project
//! (adopting it via `add_project` the first time), opens a `work` session in
//! it named after the target asset, waits for the REPL, and seeds a prompt
//! describing the asset, the IR rules, and the caller's instructions —
//! mirroring `service::sessions::review::spawn_review`'s soft-fail seeding so
//! a failed seed never discards the freshly spawned session.
//!
//! This module's command is `catalog_spawn_author_session` in
//! `commands/assets.rs`.

use super::model::{is_valid_name, Kind};
use super::require_config;
use crate::cancel::CancellationRegistry;
use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::service::add_project::{add_project, AddProjectArgs, AddProjectSource};
use crate::service::sessions::{new_session, send_prompt, NewSessionArgs, SendPromptArgs};
use crate::service::tasks::wait_for_repl_ready;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

/// Args for `catalog_spawn_author_session`. `kind` + `name` name an existing
/// catalog asset to hand off; leaving either `None` means "create a new
/// asset" (the session decides kind/name itself).
#[derive(Deserialize)]
pub struct SpawnAuthorArgs {
    pub kind: Option<Kind>,
    pub name: Option<String>,
    pub instructions: String,
    pub call_id: Option<u64>,
}

/// The target asset `kind`/`name`, when both are present; `None` means
/// "create a new asset". A blank `name` is rejected upstream as `E_INVALID`
/// (both `spawn_author_session` and the command call `is_valid_name`), so
/// callers never reach here with a blank-but-`Some` name.
fn target_of(kind: Option<Kind>, name: Option<&str>) -> Option<(Kind, &str)> {
    kind.zip(name)
}

/// PURE: the tmux session name and the sidebar friendly name for delegating
/// `target` (or, when `None`, a freshly minted `catalog-new-<hex>` /
/// "author new asset" pair for "create a new asset"). The hex suffix keeps
/// repeated "new asset" delegations from colliding on the same tmux name.
pub fn session_name_for(kind: Option<Kind>, name: Option<&str>) -> (String, String) {
    match target_of(kind, name) {
        Some((k, n)) => (
            // tmux session names are conventionally kebab-case; `Kind::as_str()`
            // uses snake_case for a couple of kinds (`mcp_server`,
            // `plugin_ref`), so kebab-case it here. The friendly name keeps
            // `as_str()` as-is.
            format!("catalog-{}-{n}", k.as_str().replace('_', "-")),
            format!("author {}/{n}", k.as_str()),
        ),
        None => {
            let hex = format!("{:06x}", super::now_secs() as u64 & 0xff_ffff);
            (format!("catalog-new-{hex}"), "author new asset".to_string())
        }
    }
}

/// The IR field rules paragraph, shared by every seeded prompt regardless of
/// target: kebab-case names, required description, the neutral tool / tier /
/// event vocabularies, `${NAME}` secret placeholders, and the per-kind body
/// file. Kept in one place so it can't drift between call sites.
fn ir_rules_paragraph() -> String {
    format!(
        "Asset IR rules: names are kebab-case ([a-z0-9][a-z0-9-]*, e.g. \
         `my-skill`); `description` is required and should be a clear one- or \
         two-sentence summary. Tools are drawn from the neutral vocabulary \
         ({tools}) plus `mcp:<server>` for a specific MCP server's tools. \
         Model tiers are neutral: {tiers}. Hook/agent-loop events are neutral: \
         {events}. Any string field may reference a fleet secret with a \
         `${{NAME}}` placeholder, resolved from the fleet's secrets at render \
         time. A skill's prose body lives in `body.md`; an agent's system \
         prompt lives in `prompt.md`; every other kind is a single \
         `asset.yaml`.",
        tools = super::model::TOOLS.join(" "),
        tiers = super::model::TIERS.join(" "),
        events = super::model::EVENTS.join(" "),
    )
}

/// PURE: build the prompt seeded into a freshly spawned author session.
/// States the repo path and layout, the target asset's path (or "create a
/// new asset"), the IR rules paragraph, the caller's `instructions`
/// verbatim, and the commit convention. Golden-tested: every call site
/// (`spawn_author_session`) goes through this so the seeded prompt never
/// drifts from what's tested here.
pub fn build_author_prompt(
    repo_path: &str,
    target: Option<(Kind, &str)>,
    instructions: &str,
) -> String {
    let layout: String = Kind::ALL
        .iter()
        .map(|k| format!("{}/", k.dir()))
        .collect::<Vec<_>>()
        .join(", ");
    let target_line = match target {
        Some((kind, name)) if kind.is_folder() => format!(
            "You are editing an existing asset at `{}/{}/asset.yaml` (with its body in \
             `{}/{}/{}`).",
            kind.dir(),
            name,
            kind.dir(),
            name,
            body_file(kind),
        ),
        Some((kind, name)) => format!(
            "You are editing an existing asset at `{}/{}.yaml`.",
            kind.dir(),
            name,
        ),
        None => "Create a new asset in the catalog (pick the kind, a kebab-case name, and the \
                  right subdirectory for it)."
            .to_string(),
    };
    format!(
        "You are working in the claude-fleet asset catalog repo at `{repo_path}`. \
         Its top-level layout is: {layout}.\n\n\
         {target_line}\n\n\
         {rules}\n\n\
         Instructions from the author:\n{instructions}\n\n\
         When done, commit your changes with a `catalog: …` message (e.g. \
         `catalog: create <kind>/<name>` or `catalog: update <kind>/<name>`).",
        rules = ir_rules_paragraph(),
    )
}

/// `body.md` for a skill, `prompt.md` for an agent; the empty string for a
/// single-file kind (never reached: `target_line` only calls this when
/// `kind.is_folder()`).
fn body_file(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "body.md",
        Kind::Agent => "prompt.md",
        _ => "",
    }
}

/// Does `candidate` name the same directory as `repo_path`? Both sides are
/// canonicalised (`std::fs::canonicalize`) so a relative path, a trailing
/// slash, or a symlink hop compares equal to the form `add_project` recorded;
/// when either side fails to canonicalise (e.g. `repo_path` doesn't exist
/// yet), falls back to plain string equality.
fn same_path(repo_path: &str, candidate: &str) -> bool {
    match (
        std::fs::canonicalize(repo_path),
        std::fs::canonicalize(candidate),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => repo_path == candidate,
    }
}

/// Make sure the catalog repo at `repo_path` is a fleet project, adopting it
/// via `add_project`'s `Folder` source (host `local`; the catalog repo is
/// always a local checkout) the first time it's delegated to. Idempotent:
/// a second call with the same `repo_path` finds the row `add_project`
/// registered and returns its id without touching the store again.
pub async fn ensure_catalog_project(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
    repo_path: &str,
) -> Result<i64, IpcError> {
    let existing = {
        let s = lock(store)?;
        s.list_projects()?
            .into_iter()
            .find(|p| same_path(repo_path, &p.base_path))
    };
    if let Some(p) = existing {
        return Ok(p.id);
    }
    let tree = add_project(
        AddProjectArgs {
            host_alias: "local".to_string(),
            source: AddProjectSource::Folder {
                path: repo_path.to_string(),
            },
            call_id: None,
        },
        store,
        ssh.as_ref(),
        reg,
    )
    .await
    .map_err(|e| {
        // `add_project` reports `E_EXISTS` here when the catalog repo's
        // `add_project` raises E_EXISTS for two different reasons — the
        // catalog's `origin` names a GitHub repo already adopted at some
        // OTHER path (`refuse_existing_project`), or the resolved base path
        // is already registered under a spelling `same_path` did not match
        // (`refuse_existing_base_path`). Its stock messages say which, but
        // not that it was the catalog repo being adopted or what to do about
        // it, so keep the original text and add that context.
        if e.code == codes::E_EXISTS {
            let reason = e.message.clone();
            return IpcError::new(
                codes::E_EXISTS,
                format!(
                    "catalog repo {repo_path} could not be adopted as a fleet project \
                     ({reason}); add the catalog folder as a project first"
                ),
            );
        }
        e
    })?;
    Ok(tree.project.id)
}

/// Delegate an asset to an interactive session: ensure the catalog repo is a
/// fleet project, open a `work` session in it named after the target (or a
/// freshly minted `catalog-new-<hex>` for "create a new asset"), wait for
/// its REPL, and seed the built prompt with `submit: true`. Mirrors
/// `spawn_review`: seeding is soft-failed with `tracing::warn!` so a slow or
/// unready REPL never discards an otherwise-live session — the row is
/// returned either way and the UI selects it.
pub async fn spawn_author_session(
    args: SpawnAuthorArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    if let Some(name) = args.name.as_deref() {
        if !is_valid_name(name) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("name '{name}' must match [a-z0-9][a-z0-9-]*"),
            ));
        }
    }
    let repo_path = require_config(store)?.repo_path;
    let project_id = ensure_catalog_project(store, ssh, reg, &repo_path).await?;

    let (tmux_name, friendly_name) = session_name_for(args.kind, args.name.as_deref());
    let target = target_of(args.kind, args.name.as_deref());
    let prompt = build_author_prompt(&repo_path, target, &args.instructions);

    let row = new_session(
        NewSessionArgs {
            host_alias: "local".to_string(),
            project_id,
            worktree_id: None,
            name: tmux_name,
            call_id: args.call_id,
            new_worktree: None,
            base_branch: None,
            kind: Some("work".to_string()),
            start_command: None,
            friendly_name: Some(friendly_name),
        },
        store,
        ssh,
        reg,
    )
    .await?;

    wait_for_repl_ready(ssh, "local", &row.tmux_name).await;
    // Soft-fail: the session is already spawned and registered. If seeding
    // the prompt fails, DON'T discard it — return it anyway so the user can
    // type the authoring instructions manually. Mirrors `spawn_review`.
    if let Err(e) = send_prompt(
        SendPromptArgs {
            host_alias: "local".to_string(),
            tmux_name: row.tmux_name.clone(),
            prompt,
            submit: true,
        },
        store,
        ssh,
    )
    .await
    {
        tracing::warn!(
            session = %row.tmux_name,
            error = %e,
            "[spawn_author_session] seeding the author prompt failed (the session is live; seed it manually)"
        );
    }

    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancellationRegistry;
    use crate::ssh::SshClient;
    use std::path::{Path, PathBuf};

    fn tmp(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("fleet-author-session-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

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

    // ------------------------------------------------------- prompt golden

    #[test]
    fn build_author_prompt_names_an_existing_target() {
        let prompt = build_author_prompt(
            "/repo",
            Some((Kind::Skill, "my-skill")),
            "Add a retry example.",
        );
        assert!(prompt.contains("/repo"));
        assert!(prompt.contains("skills/my-skill/asset.yaml"));
        assert!(prompt.contains("skills/my-skill/body.md"));
        assert!(prompt.contains("kebab-case"));
        assert!(prompt.contains("mcp:<server>"));
        assert!(prompt.contains("fast"));
        assert!(prompt.contains("session_start"));
        assert!(prompt.contains("${NAME}"));
        assert!(prompt.contains("Add a retry example."));
        assert!(prompt.contains("catalog: …"));
        assert!(prompt.contains("catalog: create <kind>/<name>"));
    }

    #[test]
    fn build_author_prompt_names_a_single_file_target() {
        let prompt = build_author_prompt("/repo", Some((Kind::Hook, "on-stop")), "Tune it.");
        assert!(prompt.contains("hooks/on-stop.yaml"));
        assert!(!prompt.contains("on-stop/body.md"));
    }

    #[test]
    fn build_author_prompt_says_create_a_new_asset_without_a_target() {
        let prompt = build_author_prompt("/repo", None, "Make something useful.");
        assert!(prompt.contains("Create a new asset"));
        assert!(prompt.contains("Make something useful."));
        assert!(prompt.contains("catalog: …"));
    }

    // ------------------------------------------------------ session naming

    #[test]
    fn session_name_for_an_existing_target_names_it() {
        let (tmux, friendly) = session_name_for(Some(Kind::Agent), Some("reviewer"));
        assert_eq!(tmux, "catalog-agent-reviewer");
        assert_eq!(friendly, "author agent/reviewer");
    }

    #[test]
    fn session_name_for_a_new_asset_mints_a_hex_suffix() {
        let (tmux, friendly) = session_name_for(None, None);
        assert!(tmux.starts_with("catalog-new-"), "{tmux}");
        assert_eq!(tmux.len(), "catalog-new-".len() + 6);
        assert!(tmux["catalog-new-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        assert_eq!(friendly, "author new asset");
    }

    #[test]
    fn session_name_for_kebab_cases_the_kind_for_tmux_but_not_the_friendly_name() {
        let (tmux, friendly) = session_name_for(Some(Kind::McpServer), Some("claude-fleet"));
        assert_eq!(tmux, "catalog-mcp-server-claude-fleet");
        assert_eq!(friendly, "author mcp_server/claude-fleet");

        let (tmux, friendly) = session_name_for(Some(Kind::PluginRef), Some("superpowers"));
        assert_eq!(tmux, "catalog-plugin-ref-superpowers");
        assert_eq!(friendly, "author plugin_ref/superpowers");
    }

    // -------------------------------------------------- project adoption

    #[test]
    fn ensure_catalog_project_is_idempotent() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let root = init_repo("adopt");
            let store = Mutex::new(Store::open_in_memory().unwrap());
            let ssh = Arc::new(SshClient::new());
            let reg = CancellationRegistry::new();
            let repo_path = root.to_string_lossy().into_owned();

            let first = ensure_catalog_project(&store, &ssh, &reg, &repo_path)
                .await
                .unwrap();
            let projects_after_first = store.lock().unwrap().list_projects().unwrap();
            assert_eq!(projects_after_first.len(), 1);
            assert!(projects_after_first[0].adopted);

            let second = ensure_catalog_project(&store, &ssh, &reg, &repo_path)
                .await
                .unwrap();
            assert_eq!(first, second);
            let projects_after_second = store.lock().unwrap().list_projects().unwrap();
            assert_eq!(
                projects_after_second.len(),
                1,
                "a second call must not register a duplicate project"
            );
        });
    }

    #[test]
    fn ensure_catalog_project_matches_an_uncanonicalised_path() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let root = init_repo("adopt-uncanon");
            let store = Mutex::new(Store::open_in_memory().unwrap());
            let ssh = Arc::new(SshClient::new());
            let reg = CancellationRegistry::new();
            let repo_path = root.to_string_lossy().into_owned();

            let first = ensure_catalog_project(&store, &ssh, &reg, &repo_path)
                .await
                .unwrap();

            // A path spelled with a redundant "./" segment canonicalises to
            // the same directory; ensure_catalog_project must still treat it
            // as the same project rather than adopting it a second time.
            let with_dot = format!("{repo_path}/./");
            let second = ensure_catalog_project(&store, &ssh, &reg, &with_dot)
                .await
                .unwrap();
            assert_eq!(first, second);
            assert_eq!(store.lock().unwrap().list_projects().unwrap().len(), 1);
        });
    }

    #[test]
    fn ensure_catalog_project_explains_an_origin_already_adopted_elsewhere() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let root = init_repo("adopt-origin-conflict");
            git(
                &root,
                &[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/acme/widget.git",
                ],
            );
            let store = Mutex::new(Store::open_in_memory().unwrap());
            // `acme/widget` is already a fleet project at a different path,
            // so `add_project` refuses to adopt this checkout as a second
            // row for the same origin (`E_EXISTS`).
            store
                .lock()
                .unwrap()
                .upsert_project("acme", "widget", "/somewhere/else")
                .unwrap();
            let ssh = Arc::new(SshClient::new());
            let reg = CancellationRegistry::new();
            let repo_path = root.to_string_lossy().into_owned();

            let err = ensure_catalog_project(&store, &ssh, &reg, &repo_path)
                .await
                .unwrap_err();
            assert_eq!(err.code, codes::E_EXISTS);
            assert!(
                err.message
                    .starts_with(&format!("catalog repo {repo_path} could not be adopted")),
                "{}",
                err.message
            );
            assert!(err.message.contains("acme/widget"), "{}", err.message);
            assert!(
                err.message
                    .ends_with("add the catalog folder as a project first"),
                "{}",
                err.message
            );
        });
    }
}
